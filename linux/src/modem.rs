//! The XARA modem built into the Linux app: its settings (xara.toml next to config.toml),
//! starting and stopping it, and a bar above the GUI to control it.
//!
//! XARA opens the usual VARA ports (command 8300, data 8301, KISS 8100) on this computer, so the
//! GUI connects to it exactly as it would to VARA; nothing in the shared code changes.

use std::collections::VecDeque;
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::thread::JoinHandle;
use std::time::Duration;

use eframe::egui::{self, Color32, RichText};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedSender;

use crate::gui::state::GuiCommand;

const LOG_LINES: usize = 2000;

pub const MODES: [(&str, &str); 5] = [
    ("hf500", "HF 500 Hz"),
    ("hf2300", "HF 2300 Hz"),
    ("hf2750", "HF 2750 Hz"),
    ("fm-narrow", "FM narrow"),
    ("fm-wide", "FM wide"),
];

pub const PTTS: [(&str, &str); 5] = [
    ("vox", "VOX"),
    ("rts", "Serial RTS"),
    ("dtr", "Serial DTR"),
    ("cm108", "CM108 / AIOC (/dev/hidraw)"),
    ("rigctld", "rigctld (hamlib CAT)"),
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct XaraSettings {
    /// Use the built-in modem (started with the app); off = VARA or another modem the user runs.
    pub enabled: bool,
    pub mode: String,
    /// PulseAudio / PipeWire source and sink names.
    pub capture: String,
    pub playback: String,
    /// Transmit through a real radio interface (else playback only into a virtual device).
    pub radio: bool,
    pub ptt: String,
    pub ptt_serial: String,
    pub ptt_cm108: String,
    pub ptt_rigctld: String,
    pub drive_db: f32,
    pub ui_port: u16,
    /// Registered VARA speeds, for a user with a VARA licence.
    pub registered: bool,
}

impl Default for XaraSettings {
    fn default() -> Self {
        Self {
            // XARA works out of the box; VARA is the user's choice in Modem settings
            enabled: true,
            mode: "hf2300".into(),
            capture: String::new(),
            playback: String::new(),
            radio: false,
            ptt: "vox".into(),
            ptt_serial: String::new(),
            ptt_cm108: "/dev/hidraw0".into(),
            ptt_rigctld: "127.0.0.1:4532".into(),
            drive_db: -5.0,
            ui_port: 8080,
            registered: false,
        }
    }
}

fn app_dirs() -> Option<directories::ProjectDirs> {
    crate::config::project_dirs()
}

impl XaraSettings {
    fn path() -> Option<PathBuf> {
        app_dirs().map(|d| d.config_dir().join("xara.toml"))
    }

    pub fn load() -> Self {
        let mut s: Self = Self::path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default();
        // An older /dev/ttyUSBn becomes its /dev/serial/by-id name (kept on the next Save)
        s.ptt_serial = crate::rig::stable_port_name(&s.ptt_serial);
        s
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::path().ok_or("no config directory")?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let text = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, text).map_err(|e| e.to_string())
    }

    fn ptt_option(&self) -> String {
        match self.ptt.as_str() {
            "rts" | "dtr" => format!("{}:{}", self.ptt, self.ptt_serial),
            "cm108" => format!("cm108:{}", self.ptt_cm108),
            "rigctld" => format!("rigctld:{}", self.ptt_rigctld),
            _ => "vox".into(),
        }
    }

    /// xara's command-line options for this station.
    pub fn arguments(&self, call: &str, cmd_port: u16, kiss_port: u16) -> Vec<String> {
        let mut a: Vec<String> = vec!["--call".into(), call.to_uppercase()];
        a.extend(
            match self.mode.as_str() {
                "hf500" => ["--hf", "500"],
                "hf2750" => ["--hf", "2750"],
                "fm-narrow" => ["--fm", "narrow"],
                "fm-wide" => ["--fm", "wide"],
                _ => ["--hf", "2300"],
            }
            .map(String::from),
        );
        a.extend(["--audio", "pulse", "--capture", &self.capture, "--playback", &self.playback].map(String::from));
        if self.radio {
            a.extend(["--radio".into(), "--ptt".into(), self.ptt_option()]);
        }
        a.extend(["--drive-db".into(), format!("{:.1}", self.drive_db)]);
        a.extend(["--addr".into(), "127.0.0.1".into(), "--port".into(), cmd_port.to_string()]);
        a.extend(["--kiss-port".into(), kiss_port.to_string()]);
        a.extend(["--ui-port".into(), self.ui_port.to_string(), "--ui-addr".into(), "127.0.0.1".into()]);
        if let Some(dir) = app_dirs().map(|d| d.data_dir().join("xara")) {
            let _ = std::fs::create_dir_all(&dir);
            // (private: XARA warns about a state folder other users can write to, and a
            // desktop's usual umask leaves new folders group-writable)
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
            a.extend(["--state-dir".into(), dir.to_string_lossy().into_owned()]);
        }
        if self.registered {
            a.push("--registered".into());
        }
        a
    }

    fn mode_label(&self) -> &'static str {
        MODES.iter().find(|(k, _)| *k == self.mode).map_or("HF 2300 Hz", |(_, l)| l)
    }
}

/// True once something accepts connections on the modem's command port.
fn port_open(port: u16) -> bool {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok()
}

static RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The running modem. Global so the exit hook can stop it when the process ends without the
/// GUI returning to stop it itself.
static HANDLE: std::sync::Mutex<Option<xara_ffi::XaraModem>> = std::sync::Mutex::new(None);

fn handle() -> std::sync::MutexGuard<'static, Option<xara_ffi::XaraModem>> {
    HANDLE.lock().unwrap_or_else(|e| e.into_inner())
}

/// For the exit hook: stops a running modem, so it releases PTT and closes the audio
/// cleanly. At most `limit`, so a
/// stuck audio system can't hold up quitting.
pub fn stop_at_exit(limit: Duration) {
    let Some(m) = handle().take() else { return };
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(m.stop());
    });
    let _ = rx.recv_timeout(limit);
    RUNNING.store(false, std::sync::atomic::Ordering::SeqCst);
}


pub struct BuiltInModem {
    pub settings: XaraSettings,
    draft: XaraSettings,
    call: String,
    cmd_port: u16,
    kiss_port: u16,
    stopping: Option<JoinHandle<u8>>,
    status: String,
    log: VecDeque<String>,
    show_settings: bool,
    show_log: bool,
    inputs: Vec<(String, String)>,
    outputs: Vec<(String, String)>,
    serial_ports: Vec<crate::rig::PortInfo>,
    refresh_devices_later: bool,
    /// Start again once the current stop finishes (settings changed while running).
    restart_after_stop: bool,
    /// The GUI's drive level as last seen, to notice the Tune window changing it.
    gui_drive: Option<i32>,
}

impl BuiltInModem {
    pub fn new(call: &str, cmd_port: u16, kiss_port: u16) -> Self {
        let settings = XaraSettings::load();
        Self {
            draft: settings.clone(),
            settings,
            call: call.to_string(),
            cmd_port,
            kiss_port,
            stopping: None,
            status: "Stopped".into(),
            log: VecDeque::new(),
            show_settings: false,
            show_log: false,
            inputs: Vec::new(),
            outputs: Vec::new(),
            serial_ports: Vec::new(),
            refresh_devices_later: false,
            restart_after_stop: false,
            gui_drive: None,
        }
    }

    /// Keep a drive level set in the Tune window (DRIVELEVEL, live in the running modem) for
    /// the next start too, which takes its level from xara.toml. Only a change counts: a level
    /// left over from before a restart mustn't undo one just saved in Modem settings.
    pub fn follow_drive_level(&mut self, gui: Option<i32>) {
        if gui == std::mem::replace(&mut self.gui_drive, gui) {
            return;
        }
        let Some(db) = gui else { return };
        if !self.settings.enabled || !self.running() || db as f32 == self.settings.drive_db {
            return;
        }
        self.settings.drive_db = db as f32;
        self.draft.drive_db = db as f32;
        if let Err(e) = self.settings.save() {
            self.status = format!("Couldn't save the drive level: {e}");
        }
    }

    pub fn running(&self) -> bool {
        handle().is_some()
    }

    /// Starts the modem; Err says why it can't (shown in the bar).
    pub fn start(&mut self) -> Result<(), String> {
        if handle().is_some() {
            return Ok(());
        }
        if self.stopping.is_some() {
            return Err("still stopping".into());
        }
        let s = &self.settings;
        if s.capture.is_empty() || s.playback.is_empty() {
            self.show_settings = true;
            return Err("choose the audio in and out in Modem settings".into());
        }
        if port_open(self.cmd_port) {
            return Err(format!(
                "port {} is already in use (is another modem, such as the XARA app, running?)",
                self.cmd_port
            ));
        }
        let args = s.arguments(&self.call, self.cmd_port, self.kiss_port);
        tracing::info!("starting built-in XARA: xara {}", args.join(" "));
        match xara_ffi::start(args) {
            Some(m) => {
                *handle() = Some(m);
                RUNNING.store(true, std::sync::atomic::Ordering::SeqCst);
                self.status = format!("Running {} as {}", s.mode_label(), self.call.to_uppercase());
                Ok(())
            }
            None => Err("a XARA modem is already running in this app".into()),
        }
    }

    /// Shows in the bar why it didn't start at launch.
    pub fn not_started(&mut self, why: &str) {
        self.status = format!("Not started: {why}");
    }

    /// Starts the modem and has the GUI connect to it (retrying until it listens).
    fn start_and_connect(&mut self, cmd_tx: &UnboundedSender<GuiCommand>) {
        match self.start() {
            Ok(()) => {
                let _ = cmd_tx.send(GuiCommand::ConnectBuiltInModem);
            }
            Err(e) => self.status = format!("Not started: {e}"),
        }
    }

    /// Stops the modem in the background (it releases PTT first).
    fn stop(&mut self, cmd_tx: &UnboundedSender<GuiCommand>) {
        let m = handle().take();
        if let Some(m) = m {
            let _ = cmd_tx.send(GuiCommand::DisconnectModem);
            self.status = "Stopping…".into();
            self.stopping = Some(std::thread::spawn(move || m.stop()));
        }
    }

    /// Stops the modem and waits for it, for quitting.
    pub fn stop_blocking(&mut self) {
        let m = handle().take();
        if let Some(m) = m {
            m.stop();
        }
        if let Some(t) = self.stopping.take() {
            let _ = t.join();
        }
        RUNNING.store(false, std::sync::atomic::Ordering::SeqCst);
    }

    fn last_error(&self) -> String {
        self.log
            .iter()
            .rev()
            .find_map(|l| l.strip_prefix("xara: "))
            .unwrap_or("see the modem log")
            .to_string()
    }

    fn poll(&mut self, cmd_tx: &UnboundedSender<GuiCommand>) {
        while let Some(line) = xara_ffi::next_log() {
            if self.log.len() == LOG_LINES {
                self.log.pop_front();
            }
            self.log.push_back(line);
        }
        // The GUI shows received traffic itself; drop xara's copy so it doesn't pile up.
        while xara_ffi::next_received().is_some() {}

        let ended = handle().as_ref().is_some_and(|m| !m.running());
        if ended {
            let m = handle().take();
            let code = m.map_or(0, |m| m.stop());
            RUNNING.store(false, std::sync::atomic::Ordering::SeqCst);
            self.status = format!("Stopped (exit {code}): {}", self.last_error());
        }
        if self.stopping.as_ref().is_some_and(|t| t.is_finished()) {
            let code = self.stopping.take().and_then(|t| t.join().ok()).unwrap_or(0);
            RUNNING.store(false, std::sync::atomic::Ordering::SeqCst);
            self.status = if code == 0 { "Stopped".into() } else { format!("Stopped (exit {code}): {}", self.last_error()) };
            if std::mem::take(&mut self.restart_after_stop) {
                self.start_and_connect(cmd_tx);
            }
        }
    }

    fn refresh_devices(&mut self) {
        match xara::audio::list(xara::audio::Backend::Pulse) {
            Ok((i, o)) => {
                self.inputs = i;
                self.outputs = o;
            }
            Err(e) => self.status = format!("Can't list audio devices: {e}"),
        }
        // Serial ports by their /dev/serial/by-id names where they have one, so the PTT port
        // survives the radio being power cycled
        self.serial_ports = crate::rig::list_serial_ports();
        self.serial_ports.sort_by(|a, b| a.name.cmp(&b.name));
    }

    /// The modem bar (above the GUI's own menu bar) and the modem's windows.
    pub fn ui(&mut self, ctx: &egui::Context, cmd_tx: &UnboundedSender<GuiCommand>) {
        self.poll(cmd_tx);

        egui::TopBottomPanel::top("xara_modem_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("XARA modem").strong());
                let color = if self.running() { Color32::from_rgb(60, 180, 75) } else { Color32::GRAY };
                let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 5.0, color);
                ui.label(&self.status);
                ui.separator();
                if self.running() {
                    if ui.button("Stop").clicked() {
                        self.stop(cmd_tx);
                    }
                } else if ui.add_enabled(self.stopping.is_none(), egui::Button::new("Start")).clicked() {
                    self.start_and_connect(cmd_tx);
                }
                if ui.button("Modem settings…").clicked() {
                    self.draft = self.settings.clone();
                    self.refresh_devices();
                    self.show_settings = true;
                }
                if ui.add_enabled(self.running(), egui::Button::new("Web UI")).clicked() {
                    let _ = open::that(format!("http://127.0.0.1:{}/", self.settings.ui_port));
                }
                if ui.button("Modem log").clicked() {
                    self.show_log = !self.show_log;
                }
            });
        });

        if self.show_settings {
            self.settings_window(ctx, cmd_tx);
        }
        if self.show_log {
            let mut open = true;
            egui::Window::new("XARA modem log").open(&mut open).default_size([640.0, 360.0]).show(ctx, |ui| {
                if ui.button("Clear").clicked() {
                    self.log.clear();
                }
                egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink(false).show(ui, |ui| {
                    for line in &self.log {
                        ui.monospace(line);
                    }
                });
            });
            self.show_log = open;
        }
    }

    fn settings_window(&mut self, ctx: &egui::Context, cmd_tx: &UnboundedSender<GuiCommand>) {
        let mut open = true;
        let mut save = false;
        egui::Window::new("XARA modem settings").open(&mut open).resizable(false).show(ctx, |ui| {
            let d = &mut self.draft;
            ui.label(format!(
                "Call sign {} (from Settings › Station). The GUI connects to it on port {}.",
                self.call.to_uppercase(),
                self.cmd_port
            ));
            ui.add_space(4.0);
            ui.label(RichText::new("Modem").strong());
            ui.radio_value(&mut d.enabled, true, "Built-in XARA (starts with Openslot)");
            ui.radio_value(&mut d.enabled, false, "VARA, or another modem I run myself");
            if !d.enabled {
                ui.label(
                    RichText::new(format!(
                        "Start VARA HF (or VARA FM) yourself: Openslot connects to it on port {} (Settings › Vara). \
                         The XARA settings below aren't used.",
                        self.cmd_port
                    ))
                    .small()
                    .weak(),
                );
            }
            ui.add_space(6.0);
            egui::Grid::new("xara_settings").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                ui.label("Mode");
                egui::ComboBox::from_id_salt("xara_mode")
                    .selected_text(MODES.iter().find(|(k, _)| *k == d.mode).map_or("", |(_, l)| l))
                    .show_ui(ui, |ui| {
                        for (k, l) in MODES {
                            ui.selectable_value(&mut d.mode, k.to_string(), l);
                        }
                    });
                ui.end_row();

                for (label, value, list, salt) in [
                    ("Audio in", &mut d.capture, &self.inputs, "xara_in"),
                    ("Audio out", &mut d.playback, &self.outputs, "xara_out"),
                ] {
                    ui.label(label);
                    let shown = list.iter().find(|(uid, _)| uid == value).map_or_else(
                        || if value.is_empty() { "Choose…".to_string() } else { value.clone() },
                        |(_, name)| name.clone(),
                    );
                    egui::ComboBox::from_id_salt(salt).width(320.0).selected_text(shown).show_ui(ui, |ui| {
                        for (uid, name) in list {
                            ui.selectable_value(value, uid.clone(), name);
                        }
                    });
                    ui.end_row();
                }

                ui.label("TX drive");
                ui.add(egui::Slider::new(&mut d.drive_db, -30.0..=0.0).suffix(" dB").step_by(0.5));
                ui.end_row();

                ui.label("Radio");
                ui.checkbox(&mut d.radio, "Transmit through a radio interface");
                ui.end_row();

                if d.radio {
                    ui.label("PTT");
                    egui::ComboBox::from_id_salt("xara_ptt")
                        .selected_text(PTTS.iter().find(|(k, _)| *k == d.ptt).map_or("", |(_, l)| l))
                        .show_ui(ui, |ui| {
                            for (k, l) in PTTS {
                                ui.selectable_value(&mut d.ptt, k.to_string(), l);
                            }
                        });
                    ui.end_row();
                    match d.ptt.as_str() {
                        "rts" | "dtr" => {
                            ui.label("Serial port");
                            let shown = match self.serial_ports.iter().find(|p| p.name == d.ptt_serial) {
                                Some(p) => p.label(),
                                None if d.ptt_serial.is_empty() => "Choose…".to_string(),
                                None => d.ptt_serial.clone(),
                            };
                            egui::ComboBox::from_id_salt("xara_serial").width(320.0).selected_text(shown).show_ui(ui, |ui| {
                                for p in &self.serial_ports {
                                    ui.selectable_value(&mut d.ptt_serial, p.name.clone(), p.label());
                                }
                            });
                            ui.end_row();
                            ui.label("");
                            ui.label(
                                RichText::new(
                                    "On a radio with two serial ports (Yaesu FTDX3000, FT-991…) key on the one marked \
                                     (PTT/keying), the Standard port, and set the radio's PC keying menu to RTS or DTR \
                                     to match. Or leave PTT on VOX here and use CAT PTT in Settings › RIG.",
                                )
                                .small()
                                .weak(),
                            );
                            ui.end_row();
                        }
                        "cm108" => {
                            ui.label("Device (/dev/hidrawN)");
                            ui.text_edit_singleline(&mut d.ptt_cm108);
                            ui.end_row();
                        }
                        "rigctld" => {
                            ui.label("rigctld address");
                            ui.text_edit_singleline(&mut d.ptt_rigctld);
                            ui.end_row();
                        }
                        _ => {}
                    }
                }

                ui.label("Web UI port");
                let mut port = d.ui_port.to_string();
                if ui.add(egui::TextEdit::singleline(&mut port).desired_width(80.0)).changed() {
                    if let Ok(p) = port.parse() {
                        d.ui_port = p;
                    }
                }
                ui.end_row();
            });
            ui.checkbox(&mut d.registered, "Registered (I hold a VARA licence)");
            if !d.radio {
                ui.label(
                    RichText::new(
                        "Without a radio interface XARA only plays into a virtual device (a PulseAudio / PipeWire null sink).",
                    )
                    .weak(),
                );
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("Refresh devices").clicked() {
                    self.refresh_devices_later = true;
                }
                if ui.button("Save").clicked() {
                    save = true;
                }
            });
        });
        if std::mem::take(&mut self.refresh_devices_later) {
            self.refresh_devices();
        }
        if save {
            let changed = self.draft != self.settings;
            self.settings = self.draft.clone();
            match self.settings.save() {
                Ok(()) => self.status = "Settings saved".into(),
                Err(e) => self.status = format!("Couldn't save settings: {e}"),
            }
            if !self.settings.enabled {
                // VARA (or another modem) instead: free its port
                if self.running() {
                    self.stop(cmd_tx);
                    self.status = format!("Stopping XARA: Openslot uses the modem on port {}", self.cmd_port);
                }
            } else if self.running() && changed {
                self.restart_after_stop = true;
                self.stop(cmd_tx);
                self.status = "Restarting with the new settings…".into();
            } else if !self.running() && self.stopping.is_none() {
                self.start_and_connect(cmd_tx);
            }
            open = false;
        }
        self.show_settings = open;
    }
}
