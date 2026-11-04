//! Openslot for Linux, with the modem built in. The program is the shared one in ../src (the
//! same code the plain build compiles), included here unchanged; this file adds what only this
//! app does: asking for a call sign on first launch from the desktop, and the built-in XARA
//! modem.

#[path = "../../src/config.rs"]
mod config;
#[allow(dead_code)] // run_gui: this app has its own, with the modem bar
#[path = "../../src/gui/mod.rs"]
mod gui;
#[path = "../../src/remote/mod.rs"]
mod remote;
#[path = "../../src/rig/mod.rs"]
mod rig;
#[path = "../../src/storage/mod.rs"]
mod storage;
#[path = "../../src/tui/mod.rs"]
mod tui;
#[path = "../../src/vara/mod.rs"]
mod vara;
#[path = "../../src/varac/mod.rs"]
mod varac;

// The built-in XARA modem and its tab, shared with the other desktop apps
#[path = "../../src/xara_modem/mod.rs"]
mod modem;

use std::sync::mpsc;
use std::time::Duration;

use config::{AppConfig, Cli, ConfigFile};
use eframe::egui;
use gui::state::{GuiCommand, SessionMsg};
use vara::connection::VaraConnection;
use varac::messages::ServiceFlags;
use varac::session::{StationConfig, VaracSession};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse_with_old_env();
    // Remote stations can listen to the radio through the modem's own audio
    modem::register_remote_audio();
    let log_level = cli.log_level.clone();
    let use_tui = cli.tui;

    // Opened from the desktop there are no arguments and maybe no config yet: ask for the call sign.
    if cli.callsign.is_none() && !use_tui && !config_has_callsign(&cli) {
        if let Some(call) = ask_callsign() {
            save_callsign(&cli, &call);
        }
    }

    let cfg = AppConfig::resolve(cli).map_err(|e| {
        eprintln!("Configuration error: {e}");
        alert(&format!("Configuration error: {e}"));
        e
    })?;

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| format!("openslot_linux={log_level}").parse().unwrap()),
        )
        .init();

    let db_path = cfg.db_path.clone();
    let db = storage::Storage::open(&db_path)?;

    // VarAC always announces VMail (<S:V>); we support it too, so it's always on
    let _ = cfg.vmail;
    let mut services = ServiceFlags(ServiceFlags::VMAIL);
    if cfg.email_gateway {
        services.0 |= ServiceFlags::EMAIL;
    }
    if cfg.bbs {
        services.0 |= ServiceFlags::BBS;
    }

    let callsign = cfg.callsign.clone();

    let station_config = StationConfig {
        callsign: cfg.callsign,
        name: cfg.name,
        qth: cfg.qth,
        locator: cfg.locator,
        services,
        bbs_dir: cfg.bbs_directory.clone(),
        email_gate: Some({
            let g = varac::email_gateway::EmailGate::new(cfg.email_gateway, cfg.email_gateway_allow.clone());
            g.update(|s| s.address = cfg.gateway_email.clone());
            g
        }),
        bbs_gate: Some(varac::email_gateway::EmailGate::new(true, cfg.bbs_allow.clone())),
        policy: {
            let (info, pings, vsnr) = config::station_policy();
            let p = varac::session::StationPolicy::new(info, pings, vsnr);
            let (text, always) = config::welcome();
            p.set_welcome(&text, always);
            p.set_email(&config::profile_email());
            p.set_full_callsign(&config::full_callsign());
            p.set_profile(config::profile_extra());
            let em = config::emcomm();
            p.set_emcomm(em.no_snr, em.last_connections);
            p.emcomm_no_call_id.store(em.no_call_id, std::sync::atomic::Ordering::Relaxed);
            p.set_blocked(config::block_list());
            let f = config::read_config_file_pub();
            p.file_size_limit.store(f.incoming_files_size_limit.unwrap_or(5000), std::sync::atomic::Ordering::Relaxed);
            p.auto_file.store(em.auto_file, std::sync::atomic::Ordering::Relaxed);
            p.allow_parking.store(f.allow_parking.unwrap_or(true), std::sync::atomic::Ordering::Relaxed);
            let ai = config::ai_settings();
            p.ai_gateway.store(ai.enabled && ai.announce, std::sync::atomic::Ordering::Relaxed);
            p
        },
    };

    let modem_config = tui::ModemConfig {
        host: cfg.host,
        cmd_port: cfg.cmd_port,
        data_port: cfg.data_port,
        kiss_port: cfg.kiss_port,
        // VarAC: VARA HF always starts at 500 Hz, so a
        // station on a calling frequency can beacon and CQ at once
        bandwidth: if config::modem_type() == vara::commands::ModemType::Hf { vara::commands::Bandwidth::Bw500 } else { cfg.bandwidth },
    };

    // The built-in modem listens on this computer, so it only applies when the GUI's modem is local.
    let local_modem = matches!(modem_config.host.as_str(), "127.0.0.1" | "localhost" | "::1");
    let mut xara = modem::BuiltInModem::new(&callsign, modem_config.cmd_port, modem_config.kiss_port);
    // If the process ends through exit() without run_gui returning, the stop below is
    // skipped; this still runs then: unkey the radio, then stop the modem cleanly.
    extern "C" fn on_exit() {
        rig::shutdown_all(Duration::from_secs(2));
        modem::stop_at_exit(Duration::from_secs(3));
    }
    unsafe extern "C" {
        fn atexit(f: extern "C" fn()) -> std::ffi::c_int;
    }
    // SAFETY: registers a plain function to run at exit.
    unsafe { atexit(on_exit) };
    let mut built_in = false;
    if local_modem && xara.settings.enabled {
        match xara.start() {
            Ok(()) => built_in = true,
            Err(e) => {
                eprintln!("Built-in XARA modem not started: {e}");
                xara.not_started(&e);
            }
        }
    }

    // The built-in modem refuses connections until it listens, and serves one client at a
    // time: just started, keep trying for a while
    let deadline = std::time::Instant::now() + Duration::from_secs(if built_in { 15 } else { 0 });
    let first = loop {
        match try_connect_modem(&modem_config, &callsign).await {
            Err(_) if std::time::Instant::now() < deadline => tokio::time::sleep(Duration::from_millis(300)).await,
            r => break r,
        }
    };
    let session = match first {
        Ok(vara) => {
            // (initialize sent MYCALL with the ping alias as the policy says: VarAC sends one)
            let mut sess = VaracSession::new(vara, station_config.clone());
            if let Err(e) = sess.attach_kiss(&modem_config.host, modem_config.kiss_port).await {
                eprintln!("VARA KISS port {} not available ({e}) — broadcasts disabled", modem_config.kiss_port);
            }
            Some(sess)
        }
        Err(e) => {
            eprintln!("VARA modem not available ({e}) — starting in offline mode");
            None
        }
    };

    let result = if use_tui {
        tui::run(session, station_config, db, callsign, modem_config).await
    } else {
        let rt = tokio::runtime::Handle::current();
        run_gui(session, station_config, db, callsign, modem_config, rt, &mut xara)
    };
    // Releases PTT before the process exits.
    xara.stop_blocking();
    result
}

async fn try_connect_modem(
    cfg: &tui::ModemConfig,
    callsign: &str,
) -> Result<VaraConnection, Box<dyn std::error::Error>> {
    if let Some(line) = config::launch_vara_if_set(&cfg.host, cfg.cmd_port, cfg.kiss_port).await {
        eprintln!("{line}");
    }
    let mut vara = VaraConnection::connect(&cfg.host, cfg.cmd_port, cfg.data_port).await?;
    vara.initialize_for(callsign, cfg.bandwidth, config::modem_type(), config::station_policy().1).await?;
    Ok(vara)
}

/// The shared GUI with the modem bar on top: gui::run_gui, plus the built-in modem.
fn run_gui(
    session: Option<VaracSession>,
    station_config: StationConfig,
    db: storage::Storage,
    callsign: String,
    modem_config: tui::ModemConfig,
    rt: tokio::runtime::Handle,
    xara: &mut modem::BuiltInModem,
) -> Result<(), Box<dyn std::error::Error>> {
    let (session_tx, session_rx) = mpsc::channel::<SessionMsg>();
    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel::<GuiCommand>();

    let modem_connected = session.is_some();
    let ptt_router = rig::PttRouter::default();

    gui::spawn_session_bridge(
        &rt,
        session,
        station_config.clone(),
        modem_config.clone(),
        callsign.clone(),
        session_tx,
        cmd_rx,
        ptt_router.clone(),
        db.reopen(),
    );

    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1360.0, 880.0])
        // (as gui::run_gui: a 1366×768 laptop leaves ~690 px, and the side column fits that)
        .with_min_inner_size([1000.0, 600.0])
        .with_title(format!("Openslot — {callsign}"))
        // (matches openslot.desktop, where the dock and Alt-Tab find the icon)
        .with_app_id("openslot");
    gui::install_desktop_entry();
    if let Some(icon) = gui::app_icon() {
        viewport = viewport.with_icon(icon);
    }
    let native_options = eframe::NativeOptions {
        viewport,
        // (as gui::run_gui: a covered chat window's vsync'd swap would freeze the app on Wayland)
        vsync: false,
        ..Default::default()
    };

    eframe::run_native(
        "openslot",
        native_options,
        Box::new(move |cc| {
            let inner = gui::VaracApp::new(
                cc,
                callsign,
                modem_connected,
                session_rx,
                cmd_tx.clone(),
                db,
                station_config,
                modem_config,
                rt,
                ptt_router,
            );
            // (keeps working while the window is hidden, as the root app does)
            let inner = gui::GuiShell::start(inner, &cc.egui_ctx);
            Ok(Box::new(LinuxApp { inner, xara, cmd_tx }))
        }),
    )?;
    Ok(())
}

struct LinuxApp<'a> {
    inner: gui::GuiShell,
    xara: &'a mut modem::BuiltInModem,
    cmd_tx: tokio::sync::mpsc::UnboundedSender<GuiCommand>,
}

impl eframe::App for LinuxApp<'_> {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // First, so the tab bar sits above the GUI's menu bar. On the Modem tab the GUI isn't
        // drawn; GuiShell's tick keeps the station working meanwhile.
        if self.xara.ui(ctx, &self.cmd_tx) {
            eframe::App::update(&mut self.inner, ctx, frame);
        }
        self.xara.follow_drive_level(self.inner.drive_level());
    }
}

fn config_path(cli: &Cli) -> Option<std::path::PathBuf> {
    cli.config.clone().or_else(config::default_config_path)
}

fn read_config(cli: &Cli) -> ConfigFile {
    config_path(cli)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}

fn config_has_callsign(cli: &Cli) -> bool {
    read_config(cli).callsign.is_some_and(|c| !c.trim().is_empty())
}

fn save_callsign(cli: &Cli, call: &str) {
    let Some(path) = config_path(cli) else { return };
    let mut cfg = read_config(cli);
    cfg.callsign = Some(call.to_string());
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = toml::to_string_pretty(&cfg) {
        let _ = std::fs::write(path, text);
    }
}

/// Asks for the call sign with a desktop dialog (zenity or kdialog, whichever is installed),
/// until it looks like one; None on Cancel or with neither installed.
fn ask_callsign() -> Option<String> {
    let mut prompt = "Welcome to Openslot. Enter your call sign:".to_string();
    loop {
        let out = std::process::Command::new("zenity")
            .args(["--entry", "--title", "Openslot", "--text", &prompt])
            .output()
            .or_else(|_| std::process::Command::new("kdialog").args(["--title", "Openslot", "--inputbox", &prompt]).output())
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let call = String::from_utf8_lossy(&out.stdout).trim().to_uppercase();
        let base = call.split('-').next().unwrap_or("");
        if (3..=7).contains(&base.len()) && base.chars().all(|c| c.is_ascii_alphanumeric()) && base.chars().any(|c| c.is_ascii_digit()) {
            return Some(call);
        }
        prompt = format!("\"{call}\" doesn't look like a call sign. Enter your call sign:");
    }
}

fn alert(text: &str) {
    rfd::MessageDialog::new()
        .set_title("Openslot")
        .set_description(text)
        .set_level(rfd::MessageLevel::Error)
        .show();
}
