mod config;
mod gui;
mod rig;
mod storage;
mod tui;
mod vara;
mod varac;

use config::{AppConfig, Cli};
use vara::connection::VaraConnection;
use varac::messages::ServiceFlags;
use varac::session::{StationConfig, VaracSession};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse_with_old_env();
    let log_level = cli.log_level.clone();
    let use_tui = cli.tui;

    let cfg = AppConfig::resolve(cli).map_err(|e| {
        eprintln!("Configuration error: {e}");
        e
    })?;

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| format!("openslot={log_level}").parse().unwrap()),
        )
        .init();

    let db_path = cfg.db_path.clone();
    let db = storage::Storage::open(&db_path)?;

    // VarAC always announces VMail (<S:V>); we support it too, so it's always on
    let _ = cfg.vmail;
    let mut services = ServiceFlags(ServiceFlags::VMAIL);
    if cfg.email_gateway {
        // Announce "E": VarAC stations then offer to send email through us
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

    let session = match try_connect_modem(&modem_config, &callsign).await {
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

    if use_tui {
        tui::run(session, station_config, db, callsign, modem_config).await
    } else {
        let rt = tokio::runtime::Handle::current();
        match gui::run_gui(session, station_config.clone(), db, callsign.clone(), modem_config.clone(), rt) {
            Ok(()) => Ok(()),
            Err(e) => {
                eprintln!("GUI failed ({e}) — falling back to terminal UI");
                let db = storage::Storage::open(&db_path)?;
                let session = match try_connect_modem(&modem_config, &callsign).await {
                    Ok(vara) => Some(VaracSession::new(vara, station_config.clone())),
                    Err(_) => None,
                };
                tui::run(session, station_config, db, callsign, modem_config).await
            }
        }
    }
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
