pub mod state;
pub mod theme;
mod layout;
mod features;
mod help;
pub mod sound;

use std::collections::VecDeque;
use std::sync::mpsc;

use chrono::{Local, Utc};
use eframe::egui;
use egui::{
    Align, Color32, Frame, Layout, Margin, RichText, ScrollArea, Stroke, TextEdit, Vec2,
};

use crate::storage::Storage;
use crate::vara::commands::{Bandwidth, ListenMode};
use crate::vara::connection::VaraConnection;
use crate::varac::bbs;
use crate::varac::messages::{self, IncomingMessage, VMail};
use crate::varac::session::{SessionState, StationConfig, VaracSession};

use state::*;

fn send_notification(title: &str, body: &str) {
    let _ = notify_rust::Notification::new()
        .summary(title)
        .body(body)
        .appname("Openslot")
        .timeout(5000)
        .show();
}

fn play_alert_sound() {
    std::thread::spawn(|| {
        #[cfg(target_os = "linux")]
        {
            let _ = std::process::Command::new("paplay")
                .arg("/usr/share/sounds/freedesktop/stereo/message-new-instant.oga")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .or_else(|_| {
                    std::process::Command::new("aplay")
                        .arg("/usr/share/sounds/freedesktop/stereo/bell.oga")
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .spawn()
                })
                .or_else(|_| {
                    std::process::Command::new("printf")
                        .arg(r"\a")
                        .spawn()
                });
        }
        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("afplay")
                .arg("/System/Library/Sounds/Glass.aiff")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
        }
        #[cfg(target_os = "windows")]
        {
            let _ = sound::no_window(&mut std::process::Command::new("powershell"))
                .args(["-NoProfile", "-c", "[System.Media.SystemSounds]::Asterisk.Play()"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
        }
    });
}

const BLUE_HEADER: Color32 = theme::ACCENT;
/// VarAC's help after a cluster's station list or relayed line
const CLUSTER_HELP: &str = ">> Type 'SM XXX:message' to send a message to a specific callsign.\n>> Type 'SM ALL:message' to send a message to a all connected callsigns.\n>> Or send a VMail to any connected station, and it will be forwarded in real time.";
const GREEN: Color32 = theme::GREEN;
const CYAN: Color32 = theme::ACCENT;
const YELLOW: Color32 = theme::YELLOW;
const RED: Color32 = theme::RED;
const GRAY: Color32 = theme::MUTED;
const WHITE: Color32 = theme::TEXT;
const BEACON_PURPLE: Color32 = theme::PURPLE;

const VMAIL_TEMPLATES: &[(&str, &str, &str)] = &[
    ("ICS-213 General Message", "EmComm General Message",
     "1. INCIDENT NAME: [NL][NL]2. TO/Position: [NL]3. FROM/Position: [NL]4. SUBJECT: [NL]5. DATE: [NL]6. TIME: [NL]7. MESSAGE:[NL][NL]8. APPROVED BY[NL]> Name / Position: [NL][NL]9. REPLY:[NL][NL]10. REPLIED BY[NL]> Name/Position: [NL]> Date/Time: "),
    ("ICS-214 Activity Log", "EmComm Activity Log",
     "1. INCIDENT NAME: [NL][NL]2. OPERATIONAL PERIOD (Date/Time)[NL]> FROM:[NL]> TO: [NL]3. NAME:[NL]4. ICS POSITION:[NL]5. HOME AGENCY (Unit):[NL][NL]6. RESOURCES ASSIGNED:[NL](Name / ICS Position / Home Agency (Unit))[NL][NL]7. ACTIVITY LOG:[NL](Date/Time - Noteable Activities)[NL][NL]8. PREPARED BY:[NL]Name: [NL]Position/Title: [NL]Date/Time: "),
    ("ARRL Radiogram", "ARRL RADIOGRAM",
     "NUMBER: [NL]PRECEDENCE: [NL]STATION OF ORIGIN: [NL]CHECK: [NL]TIME FILED: [NL]DATE: [NL][NL]TO: [NL]ADDRESS: [NL]TELEPHONE/EMAIL: [NL][NL]MESSAGE BODY: [NL][NL]SIGNATURE: [NL]NAME: [NL]POSITION: [NL]"),
    ("ICS-208 Safety Message", "EmComm Safety Message/Plan",
     "1. INCIDENT NAME: [NL][NL]2. OPERATIONAL PERIOD (Date/Time)[NL]> From:[NL]> To: [NL][NL]3. SAFETY/EXPANDED MESSAGE:[NL][NL]4. SITE SAFETY PLAN REQUIRED? (YES/NO)[NL][NL]5. PREPARED BY[NL]> Name / Position: [NL]> Date/Time: "),
];

#[allow(dead_code)]
pub struct VaracApp {
    state: GuiState,
    session_rx: mpsc::Receiver<SessionMsg>,
    db: Storage,
    station_config: StationConfig,
    modem_config: crate::tui::ModemConfig,
    current_qso_guid: Option<String>,
    rt: tokio::runtime::Handle,
    rig: Option<crate::rig::RigManager>,
    ptt_router: crate::rig::PttRouter,
    applied_theme: Option<(bool, f32, bool)>,
    rig_poll_at: Option<chrono::DateTime<Utc>>,
    rig_freq_set_at: Option<chrono::DateTime<Utc>>,
    /// VarAC's CAT frequency-change validation: (frequency asked for, retries so far)
    freq_check: Option<(u64, u32)>,
    /// Dial last handed to the protocol task (PttRouter::set_dial)
    dial_pushed: u64,
    /// Checked whether the default window fits the screen
    fitted_to_screen: bool,
    /// "Load last frequency": retune the radio to the saved frequency when it connects
    restore_freq_pending: bool,
    /// PSKReporter (stations heard), and the dial it last heard about
    psk: std::sync::mpsc::Sender<crate::varac::pskreporter::Command>,
    psk_freq: u64,
}

impl VaracApp {
    pub fn new(
        _cc: &eframe::CreationContext<'_>,
        callsign: String,
        modem_connected: bool,
        session_rx: mpsc::Receiver<SessionMsg>,
        cmd_tx: tokio::sync::mpsc::UnboundedSender<GuiCommand>,
        db: Storage,
        station_config: StationConfig,
        modem_config: crate::tui::ModemConfig,
        rt: tokio::runtime::Handle,
        ptt_router: crate::rig::PttRouter,
    ) -> Self {
        // pictures in the gallery and the QSL card preview
        egui_extras::install_image_loaders(&_cc.egui_ctx);
        let mut state = GuiState::new(callsign, modem_connected, cmd_tx);
        // Our profile from the start (tags, the typed-CQ locator check, the status bar), not
        // only once Settings has been opened
        state.settings_form.name = station_config.name.clone().unwrap_or_default();
        state.settings_form.qth = station_config.qth.clone().unwrap_or_default();
        state.settings_form.locator = station_config.locator.clone().unwrap_or_default();
        // The bandwidth the modem runs at (CQs and beacons announce it). It used to stay
        // empty until Settings was opened, so they claimed 2300 Hz.
        state.settings_form.bandwidth = match modem_config.bandwidth {
            Bandwidth::Bw500 => "500".into(),
            Bandwidth::Bw2300 => "2300".into(),
            Bandwidth::Bw2750 => "2750".into(),
        };
        state.unread_vmails = db.unread_vmail_count().unwrap_or(0);
        refresh_all_vmail_lists(&mut state, &db);

        if let Some(canned) = load_canned_messages() {
            state.canned_messages = canned;
        }

        // Load config file values into settings form
        if let Some(cfg) = load_config_file() {
            if let Some(v) = cfg.dark_mode { state.dark_mode = v; }
            state.scifi_theme = cfg.ui_theme.as_deref().is_some_and(|t| t.eq_ignore_ascii_case("SCIFI"));
            if let Some(v) = cfg.qso_alert_minutes { state.qso_alert_minutes = v; }
            if let Some(v) = cfg.dx_alert_enabled { state.dx_alert_enabled = v; }
            if let Some(v) = cfg.connect_timeout_secs { state.connect_timeout_secs = v; }
            if let Some(v) = cfg.auto_cq_interval_secs { state.auto_cq_interval_secs = v; }
            // (VarAC: AllowAutoQSY defaults to on; the <Q> offer itself needs rig control)
            state.auto_qsy_enabled = cfg.auto_qsy_enabled.unwrap_or(true);
            // VarAC starts the beacon with the program only with StartBeaconUponStartup
            state.settings_form.start_beacon_on_startup = cfg.start_beacon_on_startup.unwrap_or(false);
            state.beacon_auto_enabled = state.settings_form.start_beacon_on_startup;
            state.is_typing_enabled = cfg.send_is_typing.unwrap_or(true);
            state.settings_form.suppress_typing_low_snr = cfg.suppress_typing_low_snr.unwrap_or(true);
            state.ptt_disabled = cfg.ptt_disabled.unwrap_or(false);
            state.unattended_links = cfg.unattended_links.unwrap_or(true);
            // (VarAC restores Ignore DCD too, and its 2-hour limit starts again)
            state.ignore_dcd = cfg.ignore_dcd.unwrap_or(false);
            state.ignore_dcd_since = state.ignore_dcd.then(Utc::now);
            state.send_cmd(GuiCommand::SetPttDisabled(state.ptt_disabled));
            state.send_cmd(GuiCommand::SetIgnoreDcd(state.ignore_dcd));
            state.send_cmd(GuiCommand::SetUnattended(state.unattended_links));
            if let Some(v) = cfg.cq_slot.filter(|s| crate::varac::slots::offset_hz(*s, 750).is_some()) { state.cq_slot = v; }
            if let Some(v) = cfg.beacon_interval {
                // (VarAC raises it to the minimum: 15 minutes, 5 on VARA FM or in EmComm)
                let short = cfg.vara_modem_type.as_deref() == Some("VaraFM") || cfg.emcomm_mode.unwrap_or(false);
                state.settings_form.beacon_interval = v.max(if short { 5 } else { 15 }).to_string();
            }
            if let Some(v) = cfg.auto_log_qso { state.settings_form.auto_log_qso = v; state.auto_log_qso = v; }
            if let Some(v) = cfg.auto_away_enabled { state.settings_form.auto_away_enabled = v; }
            if let Some(v) = cfg.auto_away_minutes { state.settings_form.auto_away_minutes = v.to_string(); }
            if let Some(v) = cfg.allow_pings { state.settings_form.allow_pings = v; }
            if let Some(v) = cfg.allow_info_request { state.settings_form.allow_info_request = v; }
            if let Some(v) = cfg.load_history { state.settings_form.load_history = v; }
            if let Some(v) = cfg.font_size {
                state.font_size = v.clamp(10.0, 24.0);
                state.settings_form.font_size = v.to_string();
            }
            if let Some(v) = cfg.notifications_enabled { state.settings_form.notifications_enabled = v; }
            if let Some(v) = cfg.sound_alerts { state.settings_form.sound_alerts = v; }
            if let Some(v) = cfg.use_local_time {
                state.use_local_time = v;
                state.settings_form.use_local_time = v;
            }
            if let Some(v) = cfg.modem_auto_reconnect {
                state.modem_auto_reconnect = v;
                state.settings_form.modem_auto_reconnect = v;
            }
            if let Some(freq) = cfg.frequency {
                state.current_frequency = freq;
            }
            if let Some(ref band) = cfg.band {
                state.current_band = band.clone();
            }
            if let Some(ref dir) = cfg.bbs_directory {
                state.settings_form.bbs_directory = dir.clone();
            }
            {
                let f = &mut state.settings_form;
                f.email_gateway = cfg.email_gateway.unwrap_or(false);
                if let Some(v) = &cfg.smtp_server { f.smtp_server = v.clone(); }
                if let Some(v) = cfg.smtp_port { f.smtp_port = v.to_string(); }
                if let Some(v) = &cfg.smtp_security { f.smtp_security = v.clone(); }
                if let Some(v) = &cfg.smtp_username { f.smtp_username = v.clone(); }
                if let Some(v) = &cfg.smtp_password { f.smtp_password = v.clone(); }
                if let Some(v) = &cfg.gateway_email { f.gateway_email = v.clone(); }
                f.email_gateway_limit = cfg.email_gateway_limit.unwrap_or(false);
                if let Some(v) = &cfg.email_gateway_allow { f.email_gateway_allow = v.clone(); }
                if let Some(v) = cfg.smtp_retries { f.smtp_retries = v.to_string(); }
                f.email_gateway_announce = cfg.email_gateway_announce.unwrap_or(false);
                f.email_gateway_auto_relay = cfg.email_gateway_auto_relay.unwrap_or(false);
                f.verbose_snr_auto_accept = cfg.verbose_snr_auto_accept.unwrap_or(false);
                f.allow_last_heard = cfg.allow_last_heard.unwrap_or(true);
                f.allow_path_finder = cfg.allow_path_finder.unwrap_or(true);
                f.adif_log_path = cfg.adif_log_path.clone().unwrap_or_default();
                if let Some(v) = &cfg.adif_submode { f.adif_submode = v.clone(); }
                if let Some(v) = &cfg.default_log_comment { f.default_log_comment = v.clone(); }
                if let Some(v) = &cfg.logger_app { f.logger_app = v.clone(); }
                if let Some(v) = &cfg.logger_ip { f.logger_ip = v.clone(); }
                f.logger_port = cfg.logger_port.map(|p| p.to_string()).unwrap_or_default();
                if let Some(v) = &cfg.logger_app2 { f.logger_app2 = v.clone(); }
                if let Some(v) = &cfg.logger_ip2 { f.logger_ip2 = v.clone(); }
                f.logger_port2 = cfg.logger_port2.map(|p| p.to_string()).unwrap_or_default();
                f.logger_callsign_lookup = cfg.logger_callsign_lookup.unwrap_or(false);
                f.pskreporter_upload = cfg.pskreporter_upload.unwrap_or(false);
                f.pskreporter_self_report = cfg.pskreporter_self_report.unwrap_or(true);
                f.callsigns_block_list = cfg.callsigns_block_list.clone().unwrap_or_default();
                f.unlimited_beacon_time = cfg.unlimited_beacon_time.unwrap_or(false);
                f.hamplay_enabled = cfg.hamplay_enabled.unwrap_or(true);
                f.ai_enabled = cfg.ai_enabled.unwrap_or(false);
                f.ai_api_key = cfg.ai_api_key.clone().unwrap_or_default();
                f.ai_model = cfg.ai_model.clone().unwrap_or_else(|| crate::varac::ai_gateway::DEFAULT_MODEL.into());
                f.ai_timeout_secs = cfg.ai_timeout_secs.unwrap_or(30).to_string();
                f.ai_limit_chars = cfg.ai_limit_chars.unwrap_or(200).to_string();
                f.ai_chat_context = cfg.ai_chat_context.unwrap_or(true);
                f.ai_limit_per_qso = cfg.ai_limit_per_qso.unwrap_or(0).to_string();
                f.ai_announce = cfg.ai_announce.unwrap_or(false);
                f.ai_customization = cfg.ai_customization.clone().unwrap_or_default();
                f.dxcluster_enabled = cfg.dxcluster_enabled.unwrap_or(false);
                f.dxcluster_host = cfg.dxcluster_host.clone().unwrap_or_else(|| "k1ttt.net".into());
                f.dxcluster_port = cfg.dxcluster_port.unwrap_or(7373).to_string();
                f.dxcluster_username = cfg.dxcluster_username.clone().unwrap_or_default();
                f.dxcluster_password = cfg.dxcluster_password.clone().unwrap_or_default();
                f.schedule_start_mode = cfg.schedule_start_mode.clone().unwrap_or_else(|| "LAST_STATE".into());
                f.scanner_busy_hold_secs = cfg.scanner_busy_hold_secs.unwrap_or(0).to_string();
                f.scanner_announce = cfg.scanner_announce.unwrap_or(false);
                f.vara_path = cfg.vara_path.clone().unwrap_or_default();
                f.vara_launch = cfg.vara_launch.unwrap_or(false);
                f.vara_launch_delay_secs = cfg.vara_launch_delay_secs.unwrap_or(5).to_string();
                f.emcomm_mode = cfg.emcomm_mode.unwrap_or(false);
                f.emcomm_disable_snr = cfg.emcomm_disable_snr.unwrap_or(false);
                f.emcomm_disable_call_id = cfg.emcomm_disable_call_id.unwrap_or(false);
                f.emcomm_allow_last_connections = cfg.emcomm_allow_last_connections.unwrap_or(false);
                f.emcomm_auto_file_approval = cfg.emcomm_auto_file_approval.unwrap_or(false);
                f.emcomm_qsy_notify = cfg.emcomm_qsy_notify.unwrap_or(true);
                f.gps_manual = cfg.gps_manual.clone().unwrap_or_default();
                f.distance_unit = cfg.distance_unit.clone().unwrap_or_else(|| "MI".into());
                f.incoming_files_dir = cfg.incoming_files_dir.clone().unwrap_or_default();
                f.digipeat_via = cfg.digipeat_via.clone().unwrap_or_default();
                f.vara_modem_type = cfg.vara_modem_type.clone().unwrap_or_else(|| "VaraHF".into());
                f.usb_mode = cfg.usb_mode.clone().unwrap_or_else(|| "USB-D".into());
                f.ale_snr_threshold = cfg.ale_snr_threshold.unwrap_or(-18).to_string();
                f.ale_history_days = cfg.ale_history_days.unwrap_or(3).to_string();
                f.ale_recent_minutes = cfg.ale_recent_minutes.unwrap_or(60).to_string();
                f.ale_skip_approval = cfg.ale_skip_approval.unwrap_or(false);
                f.ale_enabled = cfg.ale_enabled.unwrap_or(false);
                f.send_relay_notifications = cfg.send_relay_notifications.unwrap_or(true);
                f.allow_parking = cfg.allow_parking.unwrap_or(true);
                f.incoming_files_size_limit = cfg.incoming_files_size_limit.unwrap_or(5000).to_string();
                f.emcomm_auto_reply_checkin = cfg.emcomm_auto_reply_checkin.unwrap_or(true);
                if let Some(v) = cfg.call_id_minutes { f.call_id_minutes = v.to_string(); }
                if let Some(v) = cfg.idle_disconnect_minutes { f.idle_disconnect_minutes = v.to_string(); }
                if let Some(v) = &cfg.welcome_message { f.welcome_message = v.clone(); }
                f.welcome_always = cfg.welcome_always.unwrap_or(false);
                f.email = cfg.email.clone().unwrap_or_default();
                f.full_callsign = cfg.full_callsign.clone().unwrap_or_default();
                f.my_rig = cfg.my_rig.clone().unwrap_or_default();
                f.my_power = cfg.my_power.clone().unwrap_or_default();
                f.my_antenna = cfg.my_antenna.clone().unwrap_or_default();
                f.my_age = cfg.my_age.clone().unwrap_or_default();
                f.my_profession = cfg.my_profession.clone().unwrap_or_default();
                f.my_hobbies = cfg.my_hobbies.clone().unwrap_or_default();
                f.multi_band_last_heard = cfg.multi_band_last_heard.unwrap_or(false);
                f.bbs_limit = cfg.bbs_limit.unwrap_or(false);
                if let Some(v) = &cfg.bbs_allow { f.bbs_allow = v.clone(); }
                f.bbs_announce = cfg.bbs_announce.unwrap_or(false);
                if let Some(v) = &cfg.beacon_type { f.beacon_type = v.clone(); }
                if let Some(v) = cfg.cq_slot_wait_secs { f.cq_slot_wait_secs = v.to_string(); }
                f.skip_cq_slot_selector = cfg.skip_cq_slot_selector.unwrap_or(false);
                if let Some(v) = cfg.slot_step_hz { f.slot_step_hz = v.to_string(); }
                f.imap_enabled = cfg.imap_enabled.unwrap_or(false);
                if let Some(v) = &cfg.imap_server { f.imap_server = v.clone(); }
                if let Some(v) = cfg.imap_port { f.imap_port = v.to_string(); }
                if let Some(v) = &cfg.imap_security { f.imap_security = v.clone(); }
                if let Some(v) = &cfg.imap_username { f.imap_username = v.clone(); }
                if let Some(v) = &cfg.imap_password { f.imap_password = v.clone(); }
                if let Some(v) = cfg.imap_poll_seconds { f.imap_poll_seconds = v.to_string(); }
            }
            if let Some(ref list) = cfg.my_frequencies {
                state.my_frequencies = list.clone();
            }
            if let Some(ref wl) = cfg.watchlist {
                state.watchlist = wl.clone();
            }
            if let Some(ref tags) = cfg.callsign_tags {
                state.callsign_tags = tags.clone();
            }
            if let Some(ref ranges) = cfg.auto_qsy_ranges {
                state.auto_qsy_ranges = state::parse_qsy_ranges(ranges);
            }
            if let Some(ref be) = cfg.rig_backend {
                state.rig_settings_backend = be.clone();
            }
            if let Some(ref h) = cfg.rig_host {
                state.rig_settings_host = h.clone();
            }
            if let Some(p) = cfg.rig_port {
                state.rig_settings_port = p.to_string();
            }
            if let Some(ref sp) = cfg.rig_serial_port {
                state.rig_settings_serial = sp.clone();
            }
            if let Some(b) = cfg.rig_baud {
                state.rig_settings_baud = b.to_string();
            }
            if let Some(ref m) = cfg.rig_model {
                state.rig_settings_model = m.clone();
            }
            if let Some(p) = cfg.rig_ptt {
                state.rig_settings_ptt = p;
            }
        }

        // Initialize rig control from config (settings were copied into state above)
        state.rig_backend = state.rig_backend_from_settings();
        ptt_router.set_enabled(state.rig_settings_ptt);
        let rig = state.rig_backend.clone().into_manager();
        ptt_router.set_target(rig.as_ref().map(|r| r.command_sender()));
        state.rig_ports = crate::rig::list_serial_ports();

        // VarAC's frequency scheduler at startup (FrequencyScheduleStartMode): the scheduler
        // tunes right away to the rule in force, the scanner takes over the beacons
        let mut sched_tuned = false;
        {
            use crate::varac::scheduler::{self as sch, Mode};
            let cfg = load_config_file().unwrap_or_default();
            let start = cfg.schedule_start_mode.clone().unwrap_or_else(|| "LAST_STATE".into());
            let mode = if start == "LAST_STATE" { Mode::from_key(cfg.schedule_last_state.as_deref().unwrap_or("")) } else { Mode::from_key(&start) };
            if state.rig_backend != crate::rig::RigBackend::None {
                match mode {
                    Mode::Scheduler if state.schedule_rules.is_empty() => state.add_log(
                        "Warning: Unable to enable Frequency Scheduler at startup because no Scheduled frequencies are configured. Disabling scheduler.".into()),
                    Mode::Scheduler => {
                        state.freq_mode = Mode::Scheduler;
                        if let Some(f) = sch::rule_in_force(&state.schedule_rules, Utc::now().time()) {
                            state.tune_for_schedule(f);
                            state.tune_to = None;
                            sched_tuned = true;
                            state.add_log(format!("Changing frequency to {f} according to the frequency scheduler list."));
                        }
                    }
                    Mode::Scanner | Mode::Ale if state.scan_list.iter().any(|e| e.enabled) => {
                        let m = if mode == Mode::Ale && cfg.ale_enabled.unwrap_or(false) { Mode::Ale } else { Mode::Scanner };
                        state.freq_mode = m;
                        state.scanner.reset(state.scan_list.len());
                        state.beacon_auto_enabled = false;
                        state.add_log(if m == Mode::Ale { "ALE enabled" } else { "FREQ scanner enabled" }.into());
                    }
                    _ => {}
                }
            }
        }

        let app = Self {
            state,
            session_rx,
            db,
            station_config,
            modem_config,
            current_qso_guid: None,
            rt,
            rig,
            ptt_router,
            applied_theme: None,
            rig_poll_at: None,
            rig_freq_set_at: None,
            freq_check: None,
            dial_pushed: 0,
            fitted_to_screen: false,
            restore_freq_pending: sched_tuned || load_config_file().and_then(|c| c.frequency).is_some(),
            psk: crate::varac::pskreporter::spawn(|m| tracing::info!(target: "gui", "{m}")),
            psk_freq: 0,
        };
        app.configure_psk();
        app
    }

    fn process_session_messages(&mut self) {
        while let Ok(msg) = self.session_rx.try_recv() {
            match msg {
                SessionMsg::Connected { source, destination, bandwidth } => {
                    if let Some(w) = self.state.ale.as_mut() {
                        let acts = w.connected(&Utc::now().format("%H:%M:%S").to_string());
                        self.ale_do(acts);
                    }
                    // nobody here dialed: the other station called us
                    let incoming_link = self.state.connect_started.is_none() && self.state.last_connect_callsign.is_none();
                    self.state.link_incoming = incoming_link;
                    // Chat numbers start again with every QSO: older lines can't be replied to
                    for l in self.state.chat_lines.iter_mut() {
                        l.chat_id = None;
                    }
                    (self.state.rx_chat_id, self.state.reply_to_chat) = (None, None);
                    (self.state.partner_awq, self.state.leave_message_popup) = (false, None);
                    self.state.psk_partner_spotted = false;
                    self.state.snr_his_series.clear();
                    self.state.snr_my_series.clear();
                    // Per-QSO: AI mode and gateway count, cluster note, a HamPlay game
                    (self.state.ai_mode, self.state.ai_count, self.state.cluster_note_shown) = (false, 0, false);
                    self.state.ai_history.clear();
                    self.state.ai_pending = None;
                    self.state.ai_waiting.clear();
                    self.state.hamplay = None;
                    // A reply on our slot: the QSO stays here
                    if let Some((_, cf, _)) = self.state.slot_wait.take() {
                        self.state.remember_cf(cf);
                    }
                    self.state.pending_slot_qsy = None;
                    self.state.cq_seq = None;
                    self.state.pending_connect = None;
                    // A link to/from a "-T" alias is a ping (VarAC flags these is_ping and
                    // doesn't log them as QSOs): just show the exchange.
                    if source.to_uppercase().ends_with("-T") || destination.to_uppercase().ends_with("-T") {
                        let who = destination.trim_end_matches("-T").trim_end_matches("-t").to_string();
                        self.state.session_state = SessionState::Connected;
                        self.state.connected_to = Some(destination.clone());
                        self.state.connect_started = None;
                        self.state.ping_active = true;
                        let incoming = source.to_uppercase().ends_with("-T");
                        // (VarAC logs pings to its qso table too, is_ping, "PING using …")
                        self.state.qso_row = Some(self.qso_row_draft(&who.to_uppercase(), bandwidth.clone(), true));
                        self.state.add_system_chat(if incoming {
                            format!("INCOMING PING FROM {who}")
                        } else {
                            format!("PINGING {who}")
                        });
                        continue;
                    }
                    let bw_str = bandwidth.as_deref().unwrap_or("?");
                    self.state.session_state = SessionState::Connected;
                    self.state.connected_to = Some(destination.clone());
                    self.state.connection_start = Some(Utc::now());
                    self.state.connect_started = None;
                    self.state.cq_active = false;
                    self.state.last_connect_callsign = None;
                    self.state.qso_alert_fired = false;
                    self.state.status_msg = format!("Connected to {destination} ({bw_str} Hz)");
                    let band = &self.state.current_band;
                    let freq_mhz = self.state.current_frequency as f64 / 1_000_000.0;
                    self.state.add_system_chat(format!("CONNECTED TO {destination} on {band} ({freq_mhz:.3} MHz)"));
                    sound::narrate(sound::Topic::AlertsAndNotifications, format!("Connected to {}", sound::phonetic(&destination)));
                    if self.state.settings_form.notifications_enabled {
                        send_notification("Connected", &format!("{destination} ({bw_str} Hz)"));
                    }
                    {
                        sound::play(if incoming_link { sound::Sound::IncomingConnection } else { sound::Sound::Connected });
                    }
                    self.state.add_log(format!("Connected: {source} <-> {destination} @ {bw_str}"));

                    // Load chat history (if enabled in settings)
                    if self.state.settings_form.load_history {
                        if let Ok(history) = self.db.chat_history_with_callsign(&destination, 20) {
                            if !history.is_empty() {
                                self.state.add_system_chat(format!("--- previous messages with {destination} ---"));
                                for record in &history {
                                    let ts = record.creation_time.format("%m-%d %H:%M").to_string();
                                    let is_outgoing = record.entry_type == 2;
                                    let call = if is_outgoing {
                                        self.state.my_callsign.clone()
                                    } else {
                                        record.callsign.clone()
                                    };
                                    self.state.add_chat(ChatLine {
                                        time: ts,
                                        callsign: call,
                                        text: record.entry.replace("//n", "\n"),
                                        is_outgoing,
                                        is_system: false,
                                        reply_to: None,
                                        ..Default::default()
                                    });
                                }
                                self.state.add_system_chat("--- end of history ---".into());
                            }
                        }
                    }

                    let guid = uuid::Uuid::new_v4().to_string();
                    // VarAC's QSO log row: filled now, the rest as the QSO goes
                    let contact = self.db.get_contact(&destination).ok().flatten();
                    self.state.qso_log = state::QsoLogFields {
                        call: destination.clone(),
                        band: self.state.current_band.clone(),
                        start: Utc::now().format("%Y-%m-%d %H:%M:%S").to_string(),
                        freq_hz: self.state.dial(),
                        power: self.state.settings_form.my_power.clone(),
                        name: contact.as_ref().and_then(|c| c.name.clone()).unwrap_or_default(),
                        qth: contact.as_ref().and_then(|c| c.qth.clone()).unwrap_or_default(),
                        locator: contact.as_ref().and_then(|c| c.locator.clone()).unwrap_or_default(),
                        ..Default::default()
                    };
                    let lg = crate::config::logging();
                    if lg.callsign_lookup {
                        let call = destination.clone();
                        std::thread::spawn(move || {
                            let _ = crate::varac::logbook::log4om_lookup("127.0.0.1", lg.api_port, Some(&call));
                        });
                    }
                    // VarAC writes the qso row when the link ends, logged or not; "Auto log
                    // QSO" is about the ADIF log and loggers (off: the LOG button)
                    let (mut qso, extra) = self.qso_row_draft(&destination.to_uppercase(), bandwidth, false);
                    qso.guid = guid.clone();
                    self.state.unlogged_qso = (!self.state.auto_log_qso).then(|| qso.clone());
                    self.state.qso_row = Some((qso, extra));
                    self.current_qso_guid = Some(guid);

                    // The session runs VarAC's opening handshake (<Q>/<Q:n>/<QSYR>/<TL>) and
                    // sends our opening report itself; queued VMail waits for HandshakeComplete.
                }
                SessionMsg::QsyInvitationRequested => {
                    self.state.qsy_invite_open = Some(Utc::now());
                    (self.state.qsy_invite_mid, self.state.qsy_invite_target) = (false, None);
                    self.state.add_system_chat("Your QSO partner allows auto-QSY. Send a QSY invitation to clear the calling frequency.".into());
                    {
                        sound::play(sound::Sound::QsyInvitation);
                    }
                }
                SessionMsg::QsyInvitation { target, inviter } => {
                    use crate::varac::session::QsyTarget;
                    let dial = self.state.dial() as i64;
                    let step = self.state.slot_step();
                    let target_hz = match target {
                        QsyTarget::Offset(hz) => dial + hz as i64,
                        QsyTarget::Up => dial + step,
                        QsyTarget::Down => dial - step,
                        QsyTarget::Absolute(f) => f as i64,
                    }.max(0) as u64;
                    let who = if inviter { "YOUR" } else { "A" };
                    self.state.add_system_chat(format!("<<{who} QSY TO {:.3} MHz INVITATION>>", target_hz as f64 / 1e6));
                    self.state.open_qsy_popup(target_hz, false);
                    {
                        sound::play(sound::Sound::QsyInvitation);
                    }
                }
                SessionMsg::QsyResolved => {
                    if self.state.qsy_popup.as_ref().is_some_and(|p| !p.recovery) {
                        self.state.qsy_popup = None;
                    }
                }
                SessionMsg::QsyRecovery(back_to) => self.state.open_qsy_popup(back_to, true),
                SessionMsg::QsyBy(hz) => {
                    self.state.qsy_invite_open = None;
                    // (the bridge moved the radio: an earlier change being checked is over)
                    self.freq_check = None;
                    // Away with auto-QSY: once we've moved, offer to leave a message (VarAC)
                    if std::mem::take(&mut self.state.partner_awq) {
                        self.offer_leave_message();
                    }
                    self.state.remember_cf(self.state.current_frequency);
                    // From where the radio is (on a slot that isn't the CF). The rig was already
                    // moved by the protocol task (ptt_router.offset_freq). With rig control a
                    // slot keeps its calling frequency, as in VarAC; without, the frequency we
                    // show is the one we work on.
                    let f = (self.state.dial() as i64 + hz as i64).max(0) as u64;
                    let on_slot = self.state.rig_connected
                        && crate::varac::slots::slot_at(self.state.current_frequency, f, self.state.slot_step()).is_some_and(|s| s != 0);
                    if !on_slot {
                        self.state.current_frequency = f;
                    }
                    if self.state.rig_connected {
                        self.state.rig_freq = Some(f);
                    }
                    if let Some(band) = freq_to_band(f) {
                        self.state.current_band = band.to_string();
                    }
                    self.state.add_system_chat(format!("QSY {hz:+} Hz → {:.3} kHz", f as f64 / 1000.0));
                }
                SessionMsg::HandshakeComplete => {
                    // Queued Outbox / parked VMail is delivered by the background task
                    // (works while this window is hidden); just show the result.
                    refresh_all_vmail_lists(&mut self.state, &self.db);
                }
                SessionMsg::Disconnected if self.state.ping_active => {
                    self.state.ping_active = false;
                    self.state.connected_to = None;
                    self.state.session_state = SessionState::Idle;
                    self.state.status_msg = "Ping finished".into();
                }
                SessionMsg::Disconnected => {
                    // an ALE attempt that never connected: next option
                    if self.state.connected_to.is_none() {
                        if let Some(w) = self.state.ale.as_mut() {
                            let acts = w.failed(&Utc::now().format("%H:%M:%S").to_string());
                            self.ale_do(acts);
                        }
                    }
                    self.state.ai_mode = false;
                    if let Some(g) = self.state.hamplay.as_mut() {
                        g.active = false;
                        g.invited = false;
                    }
                    // (VarAC: the end time only when there was a link; a ping's is its own)
                    let was_qso = self.state.connected_to.is_some() && !self.state.ping_active;
                    if was_qso && !self.state.qso_log.call.is_empty() && !self.state.qso_log.written {
                        self.state.qso_log.end = Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
                    }
                    if self.state.connected_to.is_some() {
                        self.write_qso_row();
                    }
                    if was_qso && !self.state.qso_log.call.is_empty() && !self.state.qso_log.written {
                        if self.state.auto_log_qso {
                            if self.log_qso(false, false) {
                                self.state.qso_log = state::QsoLogFields::default();
                            }
                        } else {
                            self.state.add_system_chat("AUTO QSO LOG IS OFF: check the QSO fields and click LOG to log this QSO".into());
                            // VarAC's self-closing notice, when the QSO looks complete
                            let f = &self.state.qso_log;
                            if [&f.call, &f.snr_sent, &f.snr_rcvd, &f.band, &f.start, &f.end].iter().all(|v| !v.trim().is_empty())
                                && (!f.name.trim().is_empty() || !f.qth.trim().is_empty())
                            {
                                self.state.auto_log_off_notice = Some(Utc::now() + chrono::Duration::seconds(10));
                            }
                        }
                    }
                    let lg = crate::config::logging();
                    if lg.callsign_lookup {
                        std::thread::spawn(move || { let _ = crate::varac::logbook::log4om_lookup("127.0.0.1", lg.api_port, None); });
                    }
                    self.state.offer_return_to_cf();
                    self.state.relay_offer = None;
                    self.state.verbose_snr = false;
                    self.state.verbose_snr_invite = false;
                    let prev = self.state.connected_to.take();
                    let call = prev.as_deref().unwrap_or("?");
                    self.state.status_msg = format!("Disconnected from {call}");
                    self.state.add_system_chat(format!("DISCONNECTED FROM {call}"));
                    sound::narrate(sound::Topic::AlertsAndNotifications, format!("Disconnected from {}.", sound::phonetic(call)));
                    if let Some(guid) = self.current_qso_guid.take() {
                        if self.state.unlogged_qso.as_ref().is_some_and(|q| q.guid == guid) {
                            self.state.add_system_chat("QSO not logged yet: click LOG to log it".into());
                        }
                    }
                    self.state.session_state = SessionState::Idle;
                    for l in self.state.chat_lines.iter_mut() {
                        if matches!(l.tx, Some(state::TxStage::Queued | state::TxStage::OnAir { .. })) {
                            l.tx = Some(state::TxStage::Unsent);
                        }
                    }
                    self.state.tx_speed = None;
                    self.state.reset_snr_tracking();
                    self.state.connection_start = None;
                    self.state.partner_name = None;
                    self.state.partner_qth = None;
                    self.state.partner_locator = None;
                    self.state.partner_services = None;
                    self.state.partner_typing_at = None;
                    self.state.pending_qsy = None;
                }
                SessionMsg::Ptt(on) => {
                    if on {
                        if let Some(w) = self.state.ale.as_mut() {
                            w.ptt_on();
                        }
                    }
                    self.state.ptt = on;
                    // A CQ transmission ended: VarAC's wait, then its second CQ / the slot
                    if !on {
                        self.state.cq_tx_done();
                    }
                }
                SessionMsg::Buffer(n) => {
                    self.state.buffer_bytes = n;
                    self.state.buffer_at = Some(std::time::Instant::now());
                }
                SessionMsg::Bitrate { level, bps, tx } => {
                    if tx {
                        self.state.tx_speed = Some((level, bps));
                    }
                }
                SessionMsg::TxStarted { text, bytes } => {
                    let now = std::time::Instant::now();
                    for i in our_lines_in_frame(&self.state.chat_lines, &text, |l| l.tx == Some(state::TxStage::Queued)) {
                        self.state.chat_lines[i].tx = Some(state::TxStage::OnAir { total: bytes as u32, since: now });
                    }
                }
                SessionMsg::Busy(on) => {
                    if self.state.busy && !on {
                        self.state.busy_cleared_at = Some(Utc::now());
                    }
                    self.state.busy = on;
                }
                SessionMsg::Snr(db) => {
                    self.state.record_snr(db);
                    if self.state.connected_to.is_some() && self.state.qso_log.snr_sent.is_empty() && !self.state.qso_log.call.is_empty() {
                        self.state.qso_log.snr_sent = crate::varac::logbook::rst(db.round() as i32);
                    }
                }
                SessionMsg::TxHeld { cmd, title, text } => {
                    self.state.add_log(format!("{title} {}", text.replace('\n', " ")));
                    self.state.tx_held = Some((cmd, title, text));
                }
                SessionMsg::TxRefused { title, text } => {
                    self.state.add_log(format!("{title} {}", text.replace('\n', " ")));
                    self.state.tx_alert = Some((title, text));
                }
                SessionMsg::FrameId { outgoing: false, id, .. } => self.state.rx_chat_id = Some(id),
                SessionMsg::FrameId { outgoing: true, id, text } => {
                    // Our line that this frame carried (newest first; a reply has its <C:n> prefix)
                    let body = text.split_once('>').filter(|_| text.starts_with("<C:")).map_or(text.as_str(), |(_, b)| b);
                    let body = body.replace("//n", "\n");
                    // (one multi-line message, or up to three queued lines VarAC-style joined
                    // into one frame: one number for all)
                    let whole = body.replace("\r\n", "\n");
                    let parts: Vec<String> = if self.state.chat_lines.iter().any(|l| l.is_outgoing && l.chat_id.is_none() && l.text == whole) {
                        vec![whole]
                    } else {
                        body.split("\r\n").map(str::to_string).collect()
                    };
                    for part in parts {
                        if let Some(line) = self.state.chat_lines.iter_mut().rev()
                            .find(|l| l.is_outgoing && l.chat_id.is_none() && l.text == part)
                        {
                            line.chat_id = Some(id);
                            if line.tx.is_some() {
                                line.tx = Some(state::TxStage::Sent { at: std::time::Instant::now() });
                            }
                        }
                    }
                }
                SessionMsg::Queue(lines) => {
                    self.state.queue_count = lines.len();
                    self.state.queue_lines = lines;
                }
                SessionMsg::QueueReturned(texts) => {
                    if !texts.is_empty() {
                        let mut back = texts.iter().map(|t| t.replace("//n", "\n")).collect::<Vec<_>>().join("\n");
                        back.push('\n');
                        self.state.input_text = back + &self.state.input_text;
                    }
                }
                SessionMsg::SystemChat(m) => {
                    self.state.add_log(m.clone());
                    self.state.add_system_chat(m);
                }
                SessionMsg::EmailQueued { from, to, subject, error } => {
                    // (the attachment, if any, was unpacked by the background task)
                    self.state.last_attachment_dir = None;
                    let line = match error {
                        None => format!("Email gateway: {from}'s email to {to} queued for sending [{subject}]"),
                        Some(e) => format!("Email gateway: could not queue {from}'s email to {to}: {e}"),
                    };
                    self.state.add_log(line.clone());
                    self.state.add_system_chat(line);
                    refresh_all_vmail_lists(&mut self.state, &self.db);
                }
                SessionMsg::EmailRelayOffer { gateway, count, auto, deadline } => {
                    self.state.relay_offer = Some((gateway, count, auto, deadline));
                }
                SessionMsg::VMailRelayAlert(partner) => {
                    self.state.add_system_chat(format!(
                        "You have VMail waiting for {partner}: invite them to QSY; it is relayed automatically after the QSY"
                    ));
                    self.state.vmail_relay_alert = Some((partner, Utc::now() + chrono::Duration::seconds(30)));
                }
                SessionMsg::EmailRelayOfferClosed => {
                    self.state.relay_offer = None;
                }
                SessionMsg::EmailGatewaySuspended(e) => {
                    let line = format!("EMAIL GATEWAY SHUT DOWN after repeated send failures: {e}. Fix the settings, then Resume in Settings → Email.");
                    self.state.add_log(line.clone());
                    self.state.add_system_chat(line.clone());
                    self.state.status_msg = line;
                    if self.state.settings_form.notifications_enabled {
                        send_notification("Email gateway shut down", &e);
                    }
                }
                SessionMsg::TypedCq(cq, _) if self.state.is_blocked(&cq.callsign) => {
                    self.state.add_log(format!("Beacon/CQ callsigns is on the block list. Ignoring: {}", cq.callsign));
                }
                SessionMsg::TypedCq(cq, snr) => {
                    sound::play(sound::Sound::CqHeard);
                    sound::narrate(sound::Topic::Cqs, format!("{} from {}", if cq.cq_type.is_empty() { "CQ".into() } else { cq.cq_type.to_lowercase() }, sound::phonetic(&cq.callsign)));
                    self.psk_heard(&cq.callsign, snr.map(|s| s.round() as i32), Some(cq.locator.clone()));
                    let ts = self.state.format_time();
                    self.state.add_log(format!("CQ {} de {} {} (slot {})", cq.cq_type, cq.callsign, cq.locator, cq.slot));
                    let entry = CqEntry {
                        time: ts,
                        callsign: if cq.slot == 0 { cq.callsign.clone() } else { format!("{}-{}", cq.callsign, cq.slot) },
                        bandwidth: self.state.settings_form.bandwidth.parse().unwrap_or(500),
                        snr: snr.map(|s| format!("{:+}", s.round() as i32)),
                        locator: Some(cq.locator.clone()).filter(|l| !l.is_empty()),
                        is_beacon: false,
                        cq_type: Some(cq.cq_type.clone()).filter(|t| !t.is_empty()),
                        timestamp: Utc::now(),
                        slot: cq.slot,
                        band: self.state.current_band.clone(),
                        alert: self.alert_check(crate::varac::alerts::Source::Cq, &cq.callsign, "", &[&cq.callsign, &cq.locator, &cq.cq_type]),
                    };
                    self.state.cq_entries.push_back(entry);
                    if self.state.cq_entries.len() > 200 {
                        self.state.cq_entries.pop_front();
                    }
                    let _ = self.db.insert_cqframe(&crate::storage::CqFrameRecord {
                        guid: uuid::Uuid::new_v4().to_string(),
                        cqframe_time: Utc::now(),
                        cqframe_type_id: 1,
                        frequency: self.state.current_frequency as i64,
                        bandwidth: String::new(),
                        from_callsign: cq.callsign.clone(),
                        snr: snr.map(|s| s.round() as i32),
                        slot: Some(cq.slot as i32),
                        locator: Some(cq.locator.clone()).filter(|l| !l.is_empty()),
                        band: Some(self.state.current_band.clone()),
                    });
                    let _ = self.db.upsert_heard_station(&cq.callsign);
                    let loc = cq.locator.trim().to_uppercase();
                    if crate::varac::messages::is_grid_locator(&loc) {
                        let _ = self.db.auto_save_contact_from_qso(&cq.callsign, None, None, Some(&loc));
                    }
                    // (an SOS / EMRG / HELP call deserves a sound)
                    if matches!(cq.cq_type.as_str(), "SOS" | "EMRG" | "HELP") && self.state.settings_form.sound_alerts {
                        play_alert_sound();
                    }
                }
                SessionMsg::AdvancedBeacon(b, _) if self.state.is_blocked(&b.callsign) => {
                    self.state.add_log(format!("Beacon/BEACON callsigns is on the block list. Ignoring: {}", b.callsign));
                }
                SessionMsg::AdvancedBeacon(b, snr) => {
                    let services: Vec<&str> = [
                        (crate::vara::kiss::AdvancedBeacon::EMAIL_GATEWAY, "EMAIL GW"),
                        (crate::vara::kiss::AdvancedBeacon::BBS, "BBS"),
                        (crate::vara::kiss::AdvancedBeacon::AI_GATEWAY, "AI GW"),
                        (crate::vara::kiss::AdvancedBeacon::EMCOMM, "EMCOMM"),
                    ].into_iter().filter(|(bit, _)| b.services() & bit != 0).map(|(_, n)| n)
                        // VarAC's ≫: the station scans several frequencies (be patient connecting)
                        .chain(b.is_scanning().then_some("≫ SCANNING"))
                        .collect();
                    let entry = CqEntry {
                        time: self.state.format_time(),
                        callsign: format!("{}-9", b.callsign),
                        bandwidth: self.state.settings_form.bandwidth.parse().unwrap_or(500),
                        snr: snr.map(|s| format!("{:+}", s.round() as i32)),
                        locator: Some(b.locator.clone()).filter(|l| !l.is_empty()),
                        is_beacon: true,
                        cq_type: (!services.is_empty()).then(|| services.join(" ")),
                        timestamp: Utc::now(),
                        slot: 0,
                        band: self.state.current_band.clone(),
                        alert: self.alert_check(crate::varac::alerts::Source::Beacon, &b.callsign, "", &[&b.callsign, &b.locator]),
                    };
                    let dup = self.state.beacon_entries.back().is_some_and(|last| last.callsign == entry.callsign);
                    if !dup {
                        self.state.beacon_entries.push_back(entry);
                        if self.state.beacon_entries.len() > 200 {
                            self.state.beacon_entries.pop_front();
                        }
                    }
                    if b.email_gateway() {
                        self.state.add_log(format!("Beacon: {} is an email gateway", b.callsign));
                    }
                    // Heard list (VarAC's last-heard answers come from it)
                    let _ = self.db.insert_cqframe(&crate::storage::CqFrameRecord {
                        guid: uuid::Uuid::new_v4().to_string(),
                        cqframe_time: Utc::now(),
                        cqframe_type_id: 2,
                        frequency: self.state.current_frequency as i64,
                        // (VarAC: KISS frames carry no bandwidth)
                        bandwidth: String::new(),
                        from_callsign: b.callsign.clone(),
                        snr: snr.map(|s| s.round() as i32),
                        slot: None,
                        locator: Some(b.locator.clone()).filter(|l| !l.is_empty()),
                        band: Some(self.state.current_band.clone()),
                    });
                    let _ = self.db.upsert_heard_station(&b.callsign);
                    // (as in VarAC: the beacon's locator goes to the contact)
                    let loc = b.locator.trim().to_uppercase();
                    if crate::varac::messages::is_grid_locator(&loc) {
                        let _ = self.db.auto_save_contact_from_qso(&b.callsign, None, None, Some(&loc));
                    }
                }
                SessionMsg::EmailIncoming { to_call, from, subject, parked } => {
                    let line = if parked {
                        format!("Email gateway: email from {from} parked for {to_call} [{subject}]")
                    } else {
                        format!("Email gateway: email from {from} [{subject}]")
                    };
                    self.state.add_log(line.clone());
                    self.state.status_msg = line;
                    if self.state.settings_form.notifications_enabled {
                        send_notification(&format!("Email for {to_call}"), &subject);
                    }
                    self.state.unread_vmails = self.db.unread_vmail_count().unwrap_or(0);
                    refresh_all_vmail_lists(&mut self.state, &self.db);
                }
                SessionMsg::CqFrame { source, .. } if self.state.is_blocked(&source) => {
                    let kind = if source.ends_with("-9") || source.ends_with("-8") { "BEACON" } else { "CQ" };
                    self.state.add_log(format!("Beacon/{kind} callsigns is on the block list. Ignoring: {source}"));
                }
                SessionMsg::CqFrame { source, bandwidth, snr } => {
                    // VarAC: on a calling frequency only 500 Hz frames
                    // count, and a CQ must name its slot (SSID 1–5, 11–15)
                    let on_cf = crate::varac::messages::is_calling_frequency(self.state.dial());
                    let beacon = source.ends_with("-9") || source.ends_with("-8");
                    if on_cf && bandwidth != 0 && bandwidth != 500 {
                        let kind = if beacon { "A Beacon" } else { "A CQ" };
                        self.state.add_log(format!("{kind} with {bandwidth}Hz bandwidth was heard. Only 500Hz are allowed on calling frequencies. Ignoring..."));
                        continue;
                    }
                    if on_cf && !beacon && crate::varac::slots::slot_of_cq(&source) == 0
                        && !self.state.settings_form.skip_cq_slot_selector && self.state.settings_form.vara_modem_type != "VaraFM"
                    {
                        self.state.add_log(format!("A CQ on a CF without a SLOT ID was ignored: {source}"));
                        continue;
                    }
                    {
                        let base = source.split('-').next().unwrap_or(&source).to_string();
                        if base.eq_ignore_ascii_case(self.state.my_callsign.split('-').next().unwrap_or("")) {
                            if source.ends_with("-9") || source.ends_with("-8") {
                                sound::narrate(sound::Topic::MyBeacons, "Sending beacon");
                            }
                        } else if source.ends_with("-9") || source.ends_with("-8") {
                            sound::narrate(sound::Topic::Beacons, format!("Beacon from {}", sound::phonetic(&base)));
                        } else {
                            sound::play(sound::Sound::CqHeard);
                            sound::narrate(sound::Topic::Cqs, format!("CQ from {}", sound::phonetic(&base)));
                        }
                    }
                    let ts = self.state.format_time();
                    let is_beacon = source.ends_with("-9") || source.ends_with("-8");
                    let base_call_cq = source.split('-').next().unwrap_or(&source);
                    let known_loc = self.db.get_contact(base_call_cq).ok().flatten()
                        .and_then(|c| c.locator);
                    // VarAC: the SSID is the slot the caller waits on (1–5, 11–15), -9/-8 = beacon
                    let slot = crate::varac::slots::slot_of_cq(&source);
                    let cq_type = match source.rsplit_once('-').map(|(_, ssid)| ssid) {
                        Some("8") => Some("EMCOMM".into()),
                        _ if slot != 0 => Some(format!("SLOT {slot}")),
                        _ => None,
                    };
                    let entry = CqEntry {
                        time: ts,
                        callsign: source.clone(),
                        bandwidth,
                        snr: snr.map(|s| format!("{:+}", s.round() as i32)),
                        locator: known_loc,
                        is_beacon,
                        cq_type,
                        timestamp: Utc::now(),
                        slot,
                        band: self.state.current_band.clone(),
                        alert: None,
                    };
                    let src = if is_beacon { crate::varac::alerts::Source::Beacon } else { crate::varac::alerts::Source::Cq };
                    let loc = entry.locator.clone().unwrap_or_default();
                    let mut entry = entry;
                    entry.alert = self.alert_check(src, base_call_cq, "", &[&source, &loc]);
                    if is_beacon {
                        // Suppress duplicate beacons from same callsign within 60s
                        let dominated = self.state.beacon_entries.back()
                            .is_some_and(|last| last.callsign == entry.callsign);
                        if !dominated {
                            self.state.beacon_entries.push_back(entry);
                            if self.state.beacon_entries.len() > 200 {
                                self.state.beacon_entries.pop_front();
                            }
                        }
                    } else {
                        // Suppress duplicate CQ from same callsign if most recent entry is same
                        let dominated = self.state.cq_entries.back()
                            .is_some_and(|last| last.callsign == entry.callsign);
                        if dominated {
                            if let Some(last) = self.state.cq_entries.back_mut() {
                                last.time = entry.time;
                                last.bandwidth = entry.bandwidth;
                                last.snr = entry.snr;
                                last.timestamp = entry.timestamp;
                            }
                        } else {
                            self.state.cq_entries.push_back(entry);
                            if self.state.cq_entries.len() > 200 {
                                self.state.cq_entries.pop_front();
                            }
                        }
                    }
                    let base_call = source.split('-').next().unwrap_or(&source);
                    self.psk_heard(base_call, snr.map(|s| s.round() as i32), self.db.get_contact(base_call).ok().flatten().and_then(|c| c.locator));
                    // (our own frames are stored when sent: OwnCq)
                    let own = base_call.eq_ignore_ascii_case(self.state.my_callsign.split('-').next().unwrap_or(""));
                    if !own { let _ = self.db.insert_cqframe(&crate::storage::CqFrameRecord {
                        guid: uuid::Uuid::new_v4().to_string(),
                        cqframe_time: Utc::now(),
                        cqframe_type_id: if is_beacon { 2 } else { 1 },
                        frequency: self.state.current_frequency as i64,
                        bandwidth: bandwidth.to_string(),
                        from_callsign: base_call.to_string(),
                        snr: snr.map(|s| s.round() as i32),
                        slot: Some(slot as i32).filter(|s| *s != 0),
                        locator: None,
                        band: Some(self.state.current_band.clone()),
                    }); }
                    let _ = self.db.upsert_heard_station(base_call);
                    self.state.add_log(format!("CQ from {source} @ {bandwidth} Hz"));
                    if self.state.watchlist.iter().any(|w| w.eq_ignore_ascii_case(base_call)) {
                        self.state.add_system_chat(format!("WATCHLIST ALERT: {base_call} heard!"));
                        if self.state.settings_form.notifications_enabled {
                            send_notification("Watchlist Alert", &format!("{base_call} heard on CQ"));
                        }
                        if self.state.settings_form.sound_alerts {
                            play_alert_sound();
                        }
                    }
                    // DX alert: notify on foreign DXCC entities
                    if self.state.dx_alert_enabled && !is_beacon {
                        let my_entity = dxcc_entity(&self.state.my_callsign);
                        let their_entity = dxcc_entity(base_call);
                        if my_entity.is_some() && their_entity.is_some() && my_entity != their_entity {
                            if let Some(entity) = their_entity {
                                if self.state.settings_form.notifications_enabled {
                                    send_notification("DX Alert", &format!("{base_call} ({entity})"));
                                }
                            }
                        }
                    }
                }
                SessionMsg::Message(msg) => {
                    self.handle_incoming_message(&msg);
                }
                SessionMsg::Log(text) => {
                    self.state.add_log(text);
                }
                SessionMsg::DriveLevel(db) => {
                    self.state.drive_level = Some(db);
                }
                SessionMsg::OwnCq { source, bandwidth } => {
                    // VarAC keeps its own CQs and beacons in the heard history too (no SNR),
                    // so its last-heard answers include them
                    let beacon = source.ends_with("-9") || source.ends_with("-8");
                    let base = source.split('-').next().unwrap_or(&source).to_uppercase();
                    let via = crate::config::digipeat_via().unwrap_or_default();
                    let from = if self.state.settings_form.vara_modem_type == "VaraFM" && !via.trim().is_empty() {
                        format!("{base} VIA {}", via.trim().to_uppercase())
                    } else {
                        base
                    };
                    let slot = crate::varac::slots::slot_of_cq(&source);
                    let _ = self.db.insert_cqframe(&crate::storage::CqFrameRecord {
                        guid: uuid::Uuid::new_v4().to_string(),
                        cqframe_time: Utc::now(),
                        cqframe_type_id: if beacon { 2 } else { 1 },
                        frequency: self.state.dial() as i64,
                        bandwidth: bandwidth.map(|b| b.to_string()).unwrap_or_default(),
                        from_callsign: from,
                        snr: None,
                        slot: Some(slot as i32).filter(|s| *s != 0),
                        locator: None,
                        band: Some(self.state.current_band.clone()),
                    });
                }
                SessionMsg::ModemConnected => {
                    self.state.modem_connected = true;
                    // (the modem was set to 500 Hz, VarAC's start; VARA HF only)
                    if self.state.settings_form.vara_modem_type == "VaraHF" {
                        self.state.settings_form.bandwidth = "500".into();
                    }
                    self.state.status_msg = "VARA modem connected".into();
                    self.state.add_log("VARA modem connected".into());
                }
                SessionMsg::ModemDisconnected => {
                    // Modem gone: make sure it can't leave the radio keyed.
                    self.ptt_router.key(false);
                    self.state.modem_connected = false;
                    self.state.session_state = SessionState::Idle;
                    self.state.connected_to = None;
                    self.state.reset_snr_tracking();
                    self.state.connection_start = None;
                    self.state.status_msg = "Modem disconnected".into();
                    self.state.add_log("VARA modem disconnected".into());
                }
                SessionMsg::ModemFailed(e) => {
                    self.state.status_msg = format!("Modem connection failed: {e}");
                    self.state.add_log(format!("Modem error: {e}"));
                }
            }
        }
    }

    /// As in VarAC: the QSO row → ADIF file (and the loggers),
    /// contact saved. `interactive` (the LOG button) shows "Missing QSO fields".
    /// The qso row of a link that just came up (VarAC's columns).
    fn qso_row_draft(&self, call: &str, bandwidth: Option<String>, ping: bool) -> (crate::storage::QsoRecord, crate::storage::QsoExtra) {
        let f = &self.state.settings_form;
        let full = f.full_callsign.trim().to_uppercase();
        let lg = crate::config::logging();
        let qso = crate::storage::QsoRecord {
            guid: uuid::Uuid::new_v4().to_string(),
            starttime: Utc::now(),
            endtime: None,
            frequency: self.state.dial() as i64,
            band: self.state.current_band.clone(),
            bandwidth: bandwidth.unwrap_or_else(|| "2300".into()),
            callsign: call.to_string(),
            my_callsign: if full.is_empty() { self.state.my_callsign.to_uppercase() } else { full },
            snr_received: None,
            snr_sent: None,
            name: None,
            qth: None,
            comments: Some(format!("{} using Openslot", if ping { "PING" } else { "LINK" })),
        };
        let extra = crate::storage::QsoExtra {
            my_power: f.my_power.trim().to_string(),
            my_rig: f.my_rig.trim().to_string(),
            my_antenna: f.my_antenna.trim().to_string(),
            mode: "DYNAMIC".into(),
            submode: lg.submode.clone(),
            me_away: self.state.away_active,
            is_ping: ping,
            version: env!("CARGO_PKG_VERSION").into(),
            ..Default::default()
        };
        (qso, extra)
    }

    /// The link ended: write its qso row (VarAC: every link, from the QSO log fields).
    fn write_qso_row(&mut self) {
        let Some((mut q, mut x)) = self.state.qso_row.take() else { return };
        let now = Utc::now();
        q.endtime = Some(now);
        if !x.is_ping {
            let f = &self.state.qso_log;
            let txt = |v: &str| Some(v.trim().to_string()).filter(|v| !v.is_empty());
            let num = |v: &str| v.trim().trim_start_matches('+').parse::<i32>().ok();
            if let Some(c) = txt(&f.call) { q.callsign = c.to_uppercase(); }
            if let Some(b) = txt(&f.band) { q.band = b; }
            (q.name, q.qth) = (txt(&f.name), txt(&f.qth));
            // RPT_SENT: how we hear them; RPT_RCVD: how they hear us
            (q.snr_sent, q.snr_received) = (num(&f.snr_sent), num(&f.snr_rcvd));
            let loc = f.locator.trim().to_uppercase();
            x.locator = if crate::varac::messages::is_grid_locator(&loc) { loc } else { String::new() };
            x.my_power = f.power.trim().to_string();
        }
        let guid = q.guid.clone();
        if let Err(e) = self.db.insert_qso_full(&q, &x) {
            self.state.add_log(format!("Error: writing to qso table. {e}"));
            return;
        }
        // SNR report rows as in VarAC: direction 1 = the partner, 2 = us
        let _ = self.db.save_snr_series(&guid, &self.state.snr_his_series, &self.state.snr_my_series);
        self.state.qso_row_written = Some(guid);
    }

    fn log_qso(&mut self, interactive: bool, confirmed: bool) -> bool {
        use crate::varac::logbook;
        let f = self.state.qso_log.clone();
        let lg = crate::config::logging();
        let parse = |s: &str| chrono::NaiveDateTime::parse_from_str(s.trim(), "%Y-%m-%d %H:%M:%S").ok();
        let start = parse(&f.start).unwrap_or_default();
        let end = parse(&f.end).unwrap_or_else(|| Utc::now().naive_utc());
        let full = self.state.settings_form.full_callsign.trim();
        let q = logbook::LogQso {
            call: f.call.trim().to_uppercase(),
            rst_sent: f.snr_sent.trim().to_string(),
            rst_rcvd: f.snr_rcvd.trim().to_string(),
            start,
            end,
            band: f.band.trim().to_string(),
            freq_hz: f.freq_hz,
            name: f.name.trim().to_string(),
            qth: f.qth.trim().to_string(),
            grid: f.locator.trim().to_uppercase(),
            station_callsign: if full.is_empty() { self.state.my_callsign.clone() } else { full.to_string() },
            tx_pwr: f.power.trim().to_string(),
            comment: lg.comment.clone(),
            submode: lg.submode.clone(),
        };
        let missing = logbook::missing_fields(&q);
        if !missing.is_empty() {
            if interactive {
                self.state.tx_alert = Some(("Missing QSO fields".into(),
                    format!("Please fill in:\n {}", missing.join("\n "))));
            } else {
                self.state.add_log(format!("Unable to log QSO with {} ,missing information", q.call));
            }
            return false;
        }
        // VarAC's LOG with Auto log off: "Confirm QSO" first
        if interactive && !confirmed && !self.state.auto_log_qso {
            self.state.confirm_qso = Some(format!(
                "These are the QSO parameters that will\nbe written to the ADIF QSO log:\n\nCALLSIGN: {}\nREPORT SENT: {}\nREPORT RECEIVED: {}\nBAND: {}\nFREQUENCY: {}\nNAME: {}\nQTH: {}\nLOCATOR: {}\nMY TX POWER: {}\nSTART TIME: {}\nEND TIME: {}\n\nAre you sure ?",
                q.call, q.rst_sent, q.rst_rcvd, q.band, q.freq_hz, q.name, q.qth, q.grid, q.tx_pwr,
                q.start.format("%Y-%m-%d %H:%M:%S"), q.end.format("%Y-%m-%d %H:%M:%S"),
            ));
            return false;
        }
        let record = logbook::adif_record(&q);
        self.state.add_log(format!("Logging QSO to ADIF and loggers: {}", q.call));
        if let Err(e) = logbook::append_to_file(&lg.adif_path, &record, "Openslot") {
            self.state.add_log(format!("ADIF log {} not written: {e}", lg.adif_path.display()));
        }
        for (app, ip, port) in lg.loggers.clone() {
            let (record, q) = (record.clone(), q.clone());
            std::thread::spawn(move || {
                if let Err(e) = logbook::send_to_logger(&app, &ip, port, &record, &q, "Openslot") {
                    tracing::warn!(target: "gui", "logger {app} at {ip}:{port}: {e}");
                }
            });
        }
        let _ = self.db.auto_save_contact_from_qso(&q.call, Some(&q.name).filter(|v| !v.is_empty()).map(|s| s.as_str()),
            Some(&q.qth).filter(|v| !v.is_empty()).map(|s| s.as_str()), Some(&q.grid).filter(|v| !v.is_empty()).map(|s| s.as_str()));
        self.state.qso_log.written = true;
        self.state.status_msg = format!("QSO with {} logged", q.call);
        true
    }

    /// A station heard, for PSKReporter (VarAC: CQs, beacons, broadcasts, QSO partners).
    fn psk_heard(&self, call: &str, snr: Option<i32>, grid: Option<String>) {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as u32).unwrap_or(0);
        let _ = self.psk.send(crate::varac::pskreporter::Command::Heard(crate::varac::pskreporter::Spot {
            call: call.to_uppercase(),
            freq_hz: self.state.dial(),
            snr,
            grid: grid.unwrap_or_default(),
            time: now,
        }));
    }

    /// Our AI gateway: ask Gemini on a thread; the tick sends the answer (`<ACIE>…`).
    fn ask_ai(&mut self, ai: crate::varac::ai_gateway::AiSettings, question: String) {
        use crate::varac::ai_gateway;
        let f = &self.state.settings_form;
        let extra = crate::config::profile_extra();
        let people = ai_gateway::PromptPeople {
            my_call: if f.full_callsign.trim().is_empty() { self.state.my_callsign.clone() } else { f.full_callsign.trim().to_uppercase() },
            my_name: f.name.clone(),
            my_qth: f.qth.clone(),
            my_locator: f.locator.clone(),
            my_rig: extra.rig,
            my_antenna: extra.antenna,
            my_power: extra.power,
            their_call: self.state.connected_to.clone().unwrap_or_default(),
            their_name: self.state.qso_log.name.clone(),
            their_qth: self.state.qso_log.qth.clone(),
            their_locator: self.state.qso_log.locator.clone(),
        };
        let prompt = ai_gateway::system_prompt(&ai, &people, Utc::now());
        let history = if ai.chat_context { self.state.ai_history.clone() } else { Vec::new() };
        self.state.ai_history.push((true, question.clone()));
        let (tx, rx) = std::sync::mpsc::channel();
        self.state.ai_pending = Some(rx);
        self.state.add_log("AI gateway: asking the AI".into());
        std::thread::spawn(move || {
            let _ = tx.send(ai_gateway::ask(&ai, &prompt, &history, &question));
        });
    }

    /// VarAC's alert tags: every tag the texts contain is recorded in the Alert center
    /// (Incoming), the alarm sounds, and the row takes the first tag's colours.
    fn alert_check(&mut self, source: crate::varac::alerts::Source, from: &str, to: &str, texts: &[&str]) -> Option<state::AlertColors> {
        if from.eq_ignore_ascii_case(&self.state.my_callsign) {
            return None;
        }
        let hits: Vec<crate::varac::alerts::AlertTag> = crate::varac::alerts::matches(&self.state.alert_tags, source, texts).into_iter().cloned().collect();
        let first = hits.first()?.clone();
        let c = |n: &str, d| crate::varac::alerts::color_rgb(n).unwrap_or(d);
        let colors = (c(&first.font_color, (255, 255, 255)), c(&first.back_color, (255, 0, 0)));
        // the same station's repeated CQ/beacon frames: one alert (VarAC lists a station once)
        let key = format!("{}|{}|{}", source as u8, from.to_uppercase(), hits.iter().map(|h| h.tag.as_str()).collect::<Vec<_>>().join(","));
        let now = Utc::now();
        if matches!(source, crate::varac::alerts::Source::Beacon | crate::varac::alerts::Source::Cq)
            && self.state.alert_recent.get(&key).is_some_and(|t| (now - *t).num_minutes() < 10)
        {
            return Some(colors);
        }
        self.state.alert_recent.retain(|_, t| (now - *t).num_minutes() < 10);
        self.state.alert_recent.insert(key, now);
        let text = texts.iter().copied().filter(|t| !t.is_empty()).collect::<Vec<_>>().join(" ");
        for t in &hits {
            let _ = self.db.insert_alert(&crate::storage::AlertRecord {
                guid: uuid::Uuid::new_v4().to_string(),
                source_id: source as i64,
                from_callsign: from.to_string(),
                to_callsign: to.to_string(),
                alert_tag: t.tag.clone(),
                alert_name: t.description.clone(),
                source_text: text.clone(),
                alert_time: Utc::now(),
                read: false,
                folder_id: 1,
                frequency: self.state.dial() as i64,
            });
            self.state.add_log(format!("ALERT: {} ({}) from {from} in {}", t.tag, t.description, source.label()));
            sound::narrate(sound::Topic::AlertsAndNotifications, format!("Alert {} from {}", t.description, sound::phonetic(from)));
        }
        self.state.unread_alerts = self.db.unread_alert_count().unwrap_or(0);
        {
            sound::play(sound::Sound::Alert);
        }
        if self.state.settings_form.notifications_enabled {
            send_notification(&format!("ALERT: {}", first.tag), &format!("{from}: {text}"));
        }
        Some(colors)
    }

    /// VarAC's frequency scheduler (every 20 s, at a rule's exact minute) and scanner
    /// (every second, paused while anything else happens).
    fn freq_schedule_tick(&mut self) {
        use crate::varac::scheduler::{self as sch, Mode, ScanAction};
        if std::mem::take(&mut self.state.fsr_dirty) {
            // (ALE: VarAC sends nothing at all — "-" tells the session so)
            let answer = match self.state.freq_mode {
                Mode::Ale => "-".to_string(),
                m => sch::fsr_answer(m, &self.state.schedule_rules, &self.state.scan_list),
            };
            if let Ok(mut a) = self.station_config.policy.freq_schedule.write() {
                *a = answer;
            }
        }
        if self.state.busy {
            self.state.scan_busy_seen = true;
        }
        // (VarAC offers a QSY to callers only with AllowAutoQSY and rig control)
        self.station_config.policy.auto_qsy.store(
            self.state.auto_qsy_enabled && self.state.rig_backend != crate::rig::RigBackend::None,
            std::sync::atomic::Ordering::Relaxed,
        );
        let em = &self.state.settings_form;
        self.station_config.policy.emcomm_no_call_id.store(em.emcomm_mode && em.emcomm_disable_call_id, std::sync::atomic::Ordering::Relaxed);
        self.station_config.policy.scanning.store(
            self.state.freq_mode.scanning() && self.state.settings_form.scanner_announce,
            std::sync::atomic::Ordering::Relaxed,
        );
        let now = Utc::now();
        match self.state.freq_mode {
            Mode::Inactive => {}
            Mode::Scheduler => {
                if self.state.sched_last_check.is_some_and(|t| (now - t).num_seconds() <= sch::SCHEDULER_CHECK_SECS) {
                    return;
                }
                self.state.sched_last_check = Some(now);
                // a rule of this minute replaces an older owed QSY
                let target = sch::rule_now(&self.state.schedule_rules, now.time()).or(self.state.sched_queued);
                let Some(f) = target else { return };
                if f == self.state.dial() {
                    self.state.sched_queued = None;
                    return;
                }
                if self.state.freq_mode_busy() {
                    self.state.sched_queued = Some(f);
                    self.state.add_log(format!("Frequency schedule: Need to QSY to {f} but CQ/BEACON/QSO/CONNECTION/QSY-NOTIFICATION in progress or BROADCAST window is open. Checking in 20 seconds..."));
                    return;
                }
                // EmComm: tell everyone where we go first (<QF:hz> to ALL), then QSY
                if self.state.settings_form.emcomm_mode && self.state.settings_form.emcomm_qsy_notify {
                    match self.state.sched_notify_until {
                        None => {
                            self.state.add_log(format!("Queueing a QSY notification broadcast to {}", sch::dotted(f)));
                            self.state.send_cmd(GuiCommand::SendBroadcast { to: "ALL".into(), via: Vec::new(), text: format!("<QF:{f}>") });
                            self.state.sched_notify_until = Some(now + chrono::Duration::seconds(30));
                            self.state.sched_queued = Some(f);
                            return;
                        }
                        Some(t) if now < t => {
                            self.state.sched_queued = Some(f);
                            return;
                        }
                        Some(_) => self.state.sched_notify_until = None,
                    }
                }
                self.state.sched_queued = None;
                self.state.add_log(format!("Frequency schedule : Changing frequency to {f}"));
                self.state.add_system_chat(format!("Frequency schedule: QSY to {} MHz", sch::dotted(f)));
                self.state.tune_for_schedule(f);
                self.apply_tune_to();
                // VarAC: a running beacon goes out about 10 s later on the new frequency
                if self.state.beacon_auto_enabled {
                    let interval: i64 = self.state.settings_form.beacon_interval.parse().unwrap_or(15);
                    self.state.last_beacon_sent = Some(now - chrono::Duration::seconds(interval * 60 - 10));
                }
            }
            Mode::Scanner | Mode::Ale => {
                if self.state.freq_mode_busy() {
                    return;
                }
                let interval: i64 = self.state.settings_form.beacon_interval.parse().unwrap_or(15);
                // (ALE runs the scanner without the busy hold)
                let hold: u32 = if self.state.freq_mode == Mode::Ale { 0 } else { self.state.settings_form.scanner_busy_hold_secs.trim().parse().unwrap_or(0) };
                let busy_seen = self.state.scan_busy_seen;
                let dial = self.state.dial();
                let ptt_ok = !self.state.ptt_disabled;
                let list = self.state.scan_list.clone();
                let quiet = self.state.ignore_dcd || !self.state.busy;
                let wide_on_cf = self.state.settings_form.bandwidth.trim() == "2300"
                    && crate::varac::messages::is_calling_frequency(dial);
                let beacon_ok = quiet && self.state.modem_connected && !wide_on_cf;
                let actions = self.state.scanner.step(&list, now.time(), now.timestamp(), dial, busy_seen, hold, interval * 60, ptt_ok, beacon_ok);
                for a in actions {
                    match a {
                        ScanAction::Tune { freq_hz, tune } => {
                            self.state.add_log(format!("Scanner - Changing frequency: {}", sch::dotted(freq_hz)));
                            self.state.tune_for_schedule(freq_hz);
                            self.apply_tune_to();
                            self.state.scan_busy_seen = false;
                            if tune {
                                if let Some(rig) = self.rig.as_ref() {
                                    rig.tune_atu();
                                }
                            }
                        }
                        ScanAction::Beacon => {
                            // a one-time beacon (step() only asks when it may go out)
                            let source = self.state.beacon_source();
                            let bandwidth = self.state.cq_bandwidth();
                            self.state.send_cmd(GuiCommand::Confirmed(Box::new(GuiCommand::SendCq { source, bandwidth })));
                            self.state.last_beacon_sent = Some(now);
                            self.state.add_log("Scanner - one-time beacon".into());
                        }
                        ScanAction::ExtendHold(n) => {
                            self.state.add_log(format!("Scanner - Extending hold time by {n} as the frequency was busy and a busy hold time is activated."));
                        }
                    }
                }
            }
        }
    }

    /// ALE candidates for `call` (as in VarAC), `exclude` = a frequency
    /// to leave out (the current one, for the mid-link suggestion).
    fn ale_candidates(&self, call: &str, exclude: u64) -> Vec<crate::varac::ale::Candidate> {
        let f = &self.state.settings_form;
        let recent: i64 = f.ale_recent_minutes.trim().parse().unwrap_or(60);
        let days: i64 = f.ale_history_days.trim().parse().unwrap_or(3);
        let now = Utc::now();
        let h = chrono::Timelike::hour(&now);
        let hours = [(h + 23) % 24, h, (h + 1) % 24];
        let base = call.split('-').next().unwrap_or(call);
        let passes = vec![
            (format!("Last {recent} min"), self.db.ale_pass(base, now - chrono::Duration::minutes(recent), None, exclude).unwrap_or_default()),
            (format!("Same hour, last {days} days"), self.db.ale_pass(base, now - chrono::Duration::days(days), Some(hours), exclude).unwrap_or_default()),
            (format!("Last {days} days"), self.db.ale_pass(base, now - chrono::Duration::days(days), None, exclude).unwrap_or_default()),
        ];
        let list: Vec<u64> = self.state.scan_list.iter().filter(|e| e.enabled).map(|e| e.freq_hz).collect();
        crate::varac::ale::candidates(&passes, |fr| list.contains(&fr), |fr| self.state.is_qsy_freq_allowed(fr))
    }

    /// VarAC's ALE: open the window for a connect asked for in ALE mode, run its countdown,
    /// carry out what it decides, and watch a QSO's SNR for a better frequency.
    fn ale_tick(&mut self) {
        use crate::varac::ale::Window;
        let now = Utc::now();
        let hms = now.format("%H:%M:%S").to_string();
        let request = self.state.ale_request.borrow_mut().take();
        if let Some(call) = request {
            let cands = self.ale_candidates(&call, 0);
            if let Some(b) = crate::varac::ale::best(&cands) {
                self.state.add_log(format!("ALE: best frequency for {call} is {} (avg SNR {:.1})", b.freq_hz, b.avg_snr));
            }
            self.state.ale = Some(Window::new(&call, self.state.dial(), cands, self.state.settings_form.ale_skip_approval, &hms));
            self.state.ale_ticked = Some(now);
        }
        let mut actions = Vec::new();
        if let Some(w) = self.state.ale.as_mut() {
            let last = self.state.ale_ticked.unwrap_or(now);
            let tenths = ((now - last).num_milliseconds() / 100).clamp(0, 50);
            if tenths > 0 {
                self.state.ale_ticked = Some(last + chrono::Duration::milliseconds(tenths * 100));
                let busy = self.state.ptt;
                for _ in 0..tenths {
                    actions.extend(w.tick(busy, &hms));
                }
            }
        }
        actions.extend(std::mem::take(&mut self.state.ale_actions));
        self.ale_do(actions);
        self.ale_watch_link(now);
    }

    fn ale_do(&mut self, actions: Vec<crate::varac::ale::Action>) {
        use crate::varac::ale::Action;
        let hms = Utc::now().format("%H:%M:%S").to_string();
        for a in actions {
            match a {
                Action::Call(f) => {
                    let Some(call) = self.state.ale.as_ref().map(|w| w.call.clone()) else { continue };
                    if f != self.state.dial() {
                        self.state.tune_for_schedule(f);
                        self.apply_tune_to();
                    }
                    self.state.add_log(format!("ALE: calling {call} on {f}"));
                    self.state.connect_started = Some(Utc::now());
                    self.state.last_connect_callsign = Some(call.clone());
                    self.state.ale_attempt.set(true);
                    self.state.send_cmd(GuiCommand::Connect(call));
                }
                Action::Abort => {
                    if let Some(w) = self.state.ale.as_mut() {
                        w.aborted(&hms);
                    }
                    self.state.send_cmd(GuiCommand::Abort);
                }
                Action::Tune(f) => {
                    self.state.tune_for_schedule(f);
                    self.apply_tune_to();
                }
                Action::Close => self.state.ale = None,
                Action::Ring => sound::play(sound::Sound::Connected),
                Action::Alarm => sound::play(sound::Sound::Alert),
            }
        }
    }

    /// Mid-link: the worse of the two SNRs under the threshold for 20 s → suggest the best
    /// other frequency (not again for 5 minutes after a REJECT).
    fn ale_watch_link(&mut self, now: chrono::DateTime<Utc>) {
        let f = &self.state.settings_form;
        let threshold: f32 = f.ale_snr_threshold.trim().parse().unwrap_or(-18.0);
        let low = self.state.freq_mode == crate::varac::scheduler::Mode::Ale && f.ale_enabled
            && self.state.session_state == SessionState::Connected
            // (VarAC: Math.Min of both, an unknown one counting as 9999)
            && self.state.snr_last.into_iter().chain(self.state.snr_mine).any(|s| s < threshold);
        if !low {
            self.state.ale_low_since = None;
            return;
        }
        let since = *self.state.ale_low_since.get_or_insert(now);
        let declined_long_ago = self.state.ale_declined_at.is_none_or(|t| (now - t).num_seconds() > 300);
        if (now - since).num_seconds() <= 20 || !declined_long_ago || self.state.ale_suggestion.is_some() || self.state.qsy_popup.is_some() {
            return;
        }
        self.state.ale_low_since = None;
        let call = self.state.connected_to.clone().unwrap_or_default();
        let dial = self.state.dial();
        match crate::varac::ale::best(&self.ale_candidates(&call, dial)).map(|c| c.freq_hz) {
            None => self.state.ale_declined_at = Some(now),
            Some(best) => {
                self.state.add_log(format!("ALE: link SNR below {threshold}. Suggesting QSY to {best}"));
                self.state.ale_suggestion = Some((best, now + chrono::Duration::seconds(30)));
            }
        }
    }

    /// VarAC's EMAIL GATE pill: shows the gateway's state, a click switches it on/off.
    fn email_gate_status(&mut self) {
        let Some(gate) = self.station_config.email_gate.clone() else {
            self.state.email_gate_pill = None;
            return;
        };
        if std::mem::take(&mut self.state.email_gate_toggle) {
            if !crate::config::gateway_configured() {
                self.state.add_system_chat("Email gateway is not configured. Go to Settings, navigate to the Email tab, and configure your gateway before enabling it.".into());
            } else {
                let g = gate.state();
                let on = !(g.enabled && !g.suspended);
                gate.update(|g| {
                    g.enabled = on;
                    g.suspended = false;
                });
                crate::config::update_config_file(|c| c.email_gateway = Some(on));
                self.state.settings_form.email_gateway = on;
                self.state.add_log(format!("Email gateway {}", if on { "ACTIVATED" } else { "DEACTIVATED" }));
            }
        }
        let g = gate.state();
        self.state.email_gate_pill = Some((g.enabled, g.suspended));
    }

    /// The AI gateway's answer arrived: send it to the partner.
    fn poll_ai(&mut self) {
        let Some(r) = self.state.ai_pending.as_ref().and_then(|rx| rx.try_recv().ok()) else { return };
        self.state.ai_pending = None;
        if self.state.session_state != SessionState::Connected {
            self.state.ai_waiting.clear();
            return;
        }
        if let Some(q) = self.state.ai_waiting.pop_front() {
            self.ask_ai(crate::config::ai_settings(), q);
        }
        match r {
            Ok(answer) => {
                // (newlines go as VarAC's //n, as typed chat does)
                self.state.send_cmd(GuiCommand::SendInquiry(format!("<ACIE>{}", answer.replace("\r\n", "\n").replace('\n', "\r\n"))));
                self.state.ai_history.push((false, answer.clone()));
                let ts = self.state.format_time();
                self.state.add_chat(ChatLine {
                    time: ts,
                    callsign: format!("{} ({})", self.state.my_callsign, crate::varac::ai_gateway::AGENT_NAME),
                    text: answer,
                    is_outgoing: true,
                    ..Default::default()
                });
            }
            Err(e) => {
                self.state.send_cmd(GuiCommand::SendInquiry("<AIE>".into()));
                self.state.add_log(format!("AI gateway error: {e}"));
                self.state.add_system_chat(format!("AI gateway: the AI didn't answer ({e})"));
            }
        }
    }

    /// Tell PSKReporter who we are and whether to report (startup, Settings saved).
    fn configure_psk(&self) {
        let lg = crate::config::logging();
        let _ = self.psk.send(crate::varac::pskreporter::Command::Configure {
            station: crate::varac::pskreporter::Station {
                // (VarAC reports under its full callsign, e.g. 4X/W1ABC/P, with its antenna)
                call: Some(self.state.settings_form.full_callsign.trim().to_uppercase()).filter(|c| !c.is_empty())
                    .unwrap_or_else(|| self.state.my_callsign.split('-').next().unwrap_or("").to_uppercase()),
                grid: self.state.settings_form.locator.trim().to_uppercase(),
                software: format!("Openslot {}", env!("CARGO_PKG_VERSION")),
                antenna: self.state.settings_form.my_antenna.trim().to_string(),
            },
            enabled: lg.pskreporter,
            self_report: lg.self_report,
        });
    }

    /// VarAC's "Leave a message" offer: the partner is away (or answered <AWQ> and we QSYed).
    fn offer_leave_message(&mut self) {
        if let Some(p) = self.state.connected_to.clone() {
            if crate::config::vmail_popups_disabled().1 {
                self.state.add_system_chat("YOUR QSO PARTNER IS AWAY. YOU CAN LEAVE THEM A VMAIL.".into());
            } else {
                self.state.leave_message_popup = Some(p);
            }
        }
    }

    fn handle_incoming_message(&mut self, msg: &IncomingMessage) {
        let ts = self.state.format_time();
        let partner = self.state.connected_to.clone().unwrap_or_else(|| "?".into());

        match msg {
            IncomingMessage::Opening(report) => {
                if report.has_report {
                    self.state.set_snr_mine(report.signal_report_db as f32);
                    if self.state.qso_log.snr_rcvd.is_empty() && !self.state.qso_log.call.is_empty() {
                        self.state.qso_log.snr_rcvd = crate::varac::logbook::rst(report.signal_report_db as i32);
                    }
                }
                {
                    let l = &mut self.state.qso_log;
                    if let Some(v) = &report.name { l.name = v.clone(); }
                    if let Some(v) = &report.qth { l.qth = v.clone(); }
                    if let Some(v) = &report.locator { l.locator = v.to_uppercase(); }
                    if let Some(fc) = &report.full_callsign {
                        if !l.call.is_empty() { l.call = fc.clone(); }
                    }
                }
                // VarAC spots the QSO partner on PSKReporter at the start of the QSO
                if report.has_report && self.state.connected_to.is_some() && !self.state.psk_partner_spotted {
                    self.state.psk_partner_spotted = true;
                    let loc = report.locator.clone().or_else(|| self.state.partner_locator.clone());
                    self.psk_heard(&report.callsign, self.state.snr_last.map(|s| s.round() as i32), loc);
                }
                // (a Call ID later in the QSO, "de CALL <R+NN>", carries none of these)
                self.state.partner_name = report.name.clone().or(self.state.partner_name.take());
                self.state.partner_qth = report.qth.clone().or(self.state.partner_qth.take());
                self.state.partner_locator = report.locator.clone().or(self.state.partner_locator.take());
                if self.state.partner_services.is_none() || report.services.0 != 0 {
                    self.state.partner_services = Some(report.services);
                }
                if let Some(w) = &report.welcome {
                    self.state.add_chat(ChatLine {
                        time: ts.clone(),
                        callsign: partner.clone(),
                        text: w.replace("//n", "\n"),
                        is_outgoing: false,
                        is_system: false,
                        reply_to: None,
                        ..Default::default()
                    });
                }
                self.state.add_system_chat(format!(
                    "de {}{}{}{}{}",
                    report.callsign,
                    if report.has_report { format!(" R{:+}dB", report.signal_report_db) } else { String::new() },
                    report.name.as_ref().map(|n| format!(" [{n}]")).unwrap_or_default(),
                    report.qth.as_ref().map(|q| format!(" {q}")).unwrap_or_default(),
                    report.locator.as_ref().map(|l| format!(" ({l})")).unwrap_or_default(),
                ));
                let _ = self.db.auto_save_contact_from_qso(
                    &report.callsign,
                    report.name.as_deref(),
                    report.qth.as_deref(),
                    report.locator.as_deref(),
                );
            }
            IncomingMessage::VMail(vmail) => {
                // Collected: that station's relay notifications are done (VarAC: only for
                // mail addressed to us, not mail parked here for others)
                if let Some(p) = self.state.connected_to.as_ref().filter(|_| !vmail.is_parked) {
                    let _ = self.db.clear_relay_notifications(p);
                    self.state.relay_notification_count = self.db.relay_notifications().map(|r| r.len()).unwrap_or(0);
                }
                let folder = if vmail.is_parked {
                    crate::storage::VMailFolder::Parking
                } else {
                    crate::storage::VMailFolder::Inbox
                };
                let label = if vmail.is_parked { "Parked VMail" } else { "VMail" };
                self.state.add_log(format!("{label} from {}: [{}]", vmail.from, vmail.subject));
                self.state.status_msg = format!("{label} received from {}", vmail.from);
                if self.state.settings_form.notifications_enabled {
                    send_notification(
                        &format!("{label} from {}", vmail.from),
                        &vmail.subject,
                    );
                }
                if self.state.settings_form.sound_alerts {
                    play_alert_sound();
                }
                let guid = uuid::Uuid::new_v4().to_string();
                let body = vmail.body.clone();
                // VarAC keeps the sender's time (<TME>, UTC), who it came through when that
                // isn't the sender, and the band / SNR / frequency it arrived on
                let sent = chrono::NaiveDateTime::parse_from_str(vmail.compose_time.trim(), "%Y-%m-%d %H:%M:%S").ok().map(|t| t.and_utc());
                let via = self.state.connected_to.clone()
                    .filter(|p| p.split('-').next() != vmail.from.split(['-', '/']).next());
                let stored = self.db.insert_vmail(&crate::storage::VMailRecord {
                    guid: guid.clone(),
                    creation_time: vmail.timestamp,
                    sent_time: sent,
                    received_time: Some(Utc::now()),
                    folder_id: folder as i32,
                    vmail_to: vmail.to.clone(),
                    vmail_from: vmail.from.clone(),
                    vmail_via: via,
                    subject: vmail.subject.clone(),
                    msg: body,
                    read_status: false,
                    is_deleted: false,
                    urgent: vmail.urgent,
                    delivery_band: Some(self.state.current_band.clone()).filter(|b| !b.is_empty()),
                    delivery_snr: self.state.snr_last.map(|s| s.round() as i32),
                    frequency: Some(self.state.dial() as i64),
                });
                if let Err(e) = stored {
                    // (VarAC's text; it would answer <SMF>, our <SMR> already went out)
                    self.state.add_log(format!("Error storing incoming VMail: {e}"));
                    self.state.add_system_chat("ERROR PROCESSING INCOMING VMAIL".into());
                }
                // Its attachments belong to it (so a parked VMail is relayed with them)
                if vmail.has_attachment {
                    if let Some(dir) = self.state.last_attachment_dir.take() {
                        if let Ok(rd) = std::fs::read_dir(&dir) {
                            for f in rd.flatten() {
                                let _ = self.db.add_vmail_attachment(&guid, &f.path());
                            }
                        }
                    }
                }
                self.state.unread_vmails = self.db.unread_vmail_count().unwrap_or(0);
                refresh_all_vmail_lists(&mut self.state, &self.db);
                // <SMR> was already sent by the session
            }
            IncomingMessage::VMailAck => {
                self.state.add_system_chat("VMAIL DELIVERED SUCCESSFULLY".into());
            }
            IncomingMessage::Reaction { like, id } => {
                sound::play(sound::Sound::Reaction);
                // VarAC: a heart on our own line (only lines of this QSO have numbers)
                if let Some(line) = self.state.chat_lines.iter_mut().find(|l| l.is_outgoing && l.chat_id == Some(*id)) {
                    line.liked = *like;
                    let text: String = line.text.chars().take(40).collect();
                    self.state.add_log(format!("{partner} {} your message \"{text}\"", if *like { "liked" } else { "unliked" }));
                }
            }
            // Email gateway answers, worded as original VarAC shows them
            IncomingMessage::EmailGatewayRejected => {
                let partner = self.state.connected_to.clone().unwrap_or_default();
                let line = format!("VMAIL TO EMAIL GATEWAY WAS REJECTED AS {partner} DOES NOT ALLOW YOU TO SEND EMAILS");
                self.state.add_system_chat(line.clone());
                self.state.add_log(line);
            }
            IncomingMessage::EmailSent => {
                self.state.add_system_chat("EMAIL WAS SENT SUCCESSFULLY".into());
                self.state.add_log("Email gateway reports: email was sent successfully".into());
            }
            IncomingMessage::EmailFailed => {
                let line = "EMAIL FAILED TO SEND. ADDITIONAL ATTEMPTS WILL BE MADE ONCE THE EMAIL GATEWAY OPERATOR RESOLVES THE ISSUE.";
                self.state.add_system_chat(line.into());
                self.state.add_log(line.into());
            }
            IncomingMessage::EmailRefused(sender, vm, why) => {
                let line = format!("BLOCKING INCOMING VMAIL TO EMAIL FOR {} FROM {sender} : {why}.", vm.to);
                self.state.add_system_chat(line.clone());
                self.state.add_log(line);
            }
            IncomingMessage::Disconnect => {
                self.state.add_system_chat("Partner requested disconnect".into());
            }
            IncomingMessage::Away(reason) => {
                let r = reason.as_deref().unwrap_or("(no message)");
                self.state.add_system_chat(format!("Station AWAY: {r}"));
                self.offer_leave_message();
            }
            IncomingMessage::Chat(chat_msg) => {
                let display_text = chat_msg.text.replace("//n", "\n");
                let from_call = if chat_msg.from.is_empty() { partner.clone() } else { chat_msg.from.clone() };
                if self.state.away_active && self.state.settings_form.notifications_enabled {
                    send_notification(
                        &format!("Message from {}", from_call),
                        &display_text,
                    );
                    if self.state.settings_form.sound_alerts {
                        play_alert_sound();
                    }
                }
                self.state.partner_typing_at = None;
                let my = self.state.my_callsign.clone();
                let alert = self.alert_check(crate::varac::alerts::Source::Datastream, &from_call, &my, &[&display_text]);
                sound::play(sound::Sound::IncomingChat);
                sound::narrate(sound::Topic::IncomingMessages, display_text.clone());
                self.state.add_chat(ChatLine {
                    alert,
                    time: ts,
                    callsign: from_call.clone(),
                    text: display_text.clone(),
                    is_outgoing: false,
                    is_system: false,
                    reply_to: chat_msg.reply_to.clone(),
                    chat_id: self.state.rx_chat_id,
                    reply_quote: chat_msg.reply_to.as_deref().and_then(|r| r.parse().ok()).and_then(|id| self.state.chat_quote(id)),
                    ..Default::default()
                });
                if let Some(guid) = &self.current_qso_guid {
                    let reply_id = chat_msg.reply_to.as_ref().and_then(|r| r.parse::<i64>().ok());
                    let _ = self.db.insert_chat(&crate::storage::ChatRecord {
                        guid: uuid::Uuid::new_v4().to_string(),
                        entry_type: 1,
                        qso_guid: guid.clone(),
                        callsign: from_call,
                        entry: display_text,
                        creation_time: Utc::now(),
                        chat_id: None,
                        reply_on_chat_id: reply_id,
                    });
                }
            }
            IncomingMessage::RawText(text) => {
                let display_text = text.replace("//n", "\n");
                if self.state.away_active && self.state.settings_form.notifications_enabled {
                    send_notification(
                        &format!("Message from {}", partner),
                        &display_text,
                    );
                    if self.state.settings_form.sound_alerts {
                        play_alert_sound();
                    }
                }
                self.state.partner_typing_at = None;
                let my = self.state.my_callsign.clone();
                let alert = self.alert_check(crate::varac::alerts::Source::Datastream, &partner, &my, &[&display_text]);
                sound::play(sound::Sound::IncomingChat);
                sound::narrate(sound::Topic::IncomingMessages, display_text.clone());
                self.state.add_chat(ChatLine {
                    alert,
                    time: ts,
                    callsign: partner.clone(),
                    text: display_text.clone(),
                    is_outgoing: false,
                    is_system: false,
                    reply_to: None,
                    chat_id: self.state.rx_chat_id,
                    ..Default::default()
                });
                if let Some(guid) = &self.current_qso_guid {
                    let _ = self.db.insert_chat(&crate::storage::ChatRecord {
                        guid: uuid::Uuid::new_v4().to_string(),
                        entry_type: 1,
                        qso_guid: guid.clone(),
                        callsign: partner,
                        entry: display_text,
                        creation_time: Utc::now(),
                        chat_id: None,
                        reply_on_chat_id: None,
                    });
                }
            }
            IncomingMessage::IsTyping => {
                // (shown above the chat, not in the status bar, where it never went away)
                if self.state.connected_to.is_some() {
                    if self.state.partner_typing_at.is_none_or(|t| (Utc::now() - t).num_seconds() > 10) {
                        sound::play(sound::Sound::IsTyping);
                    }
                    self.state.partner_typing_at = Some(Utc::now());
                }
            }
            // The session answers <SNRR>/<INFO>/<VER> automatically, like VarAC
            IncomingMessage::SnrRequest => self.state.add_log("SNR request answered".into()),
            IncomingMessage::InfoRequest => self.state.add_log("Info request answered".into()),
            IncomingMessage::EmailGatewayAddressRequest => self.state.add_log("Email gateway address request answered".into()),
            IncomingMessage::VersionRequest => self.state.add_log("Version request answered".into()),
            IncomingMessage::VMailAttachmentOffer(offer) => {
                use crate::varac::session::FileOfferDecision;
                match self.station_config.policy.file_offer_decision(offer.compressed_size as u64, self.state.away_active) {
                    // the session answered <SFRD> and will verify/confirm the data
                    FileOfferDecision::Accept => {
                        self.state.add_system_chat("<< RECEIVING VMAIL WITH ATTACHMENT >>".into());
                        self.state.pending_file_offer = Some(offer.clone());
                        self.state.pending_vmail_attachment = true;
                    }
                    FileOfferDecision::Refuse => {
                        self.state.add_system_chat(format!("VMail with a {} bytes attachment refused (over the size limit while away)", offer.compressed_size));
                    }
                    // VarAC asks the operator (rejecting it rejects the whole VMail)
                    FileOfferDecision::Ask => {
                        self.state.pending_file_offer = Some(offer.clone());
                        self.state.pending_vmail_attachment = true;
                        self.state.attachment_ask = Some(offer.compressed_size as u64);
                    }
                }
            }
            IncomingMessage::RelayNotification { relay, urgent } => {
                let _ = self.db.record_relay_notification(relay, self.state.current_frequency as i64, *urgent, &Utc::now());
                self.state.relay_notification_count = self.db.relay_notifications().map(|r| r.len()).unwrap_or(0);
                let msg = format!(" <ASYNC MESSAGE RECEIVED> VMAIL RELAY NOTIFICATION: {relay} HAS {}VMAILS WAITING FOR YOU. CONNECT {relay} TO COLLECT THEM.",
                    if *urgent { "URGENT " } else { "" });
                self.state.add_system_chat(msg.clone());
                self.state.status_msg = msg;
                if self.state.settings_form.notifications_enabled {
                    send_notification("VMail waiting", &format!("Parked for you at {relay}"));
                }
                {
                    sound::play(sound::Sound::RelayNotification);
                }
            }
            IncomingMessage::SnrReport(db) => {
                self.state.set_snr_mine(*db as f32);
                if self.state.ping_active {
                    self.state.add_system_chat(format!("PING RESULT: {db:+} dB"));
                    self.state.status_msg = format!("Ping result: {db:+} dB");
                } else {
                    self.state.add_system_chat(format!("They hear you at {db:+} dB"));
                }
            }
            IncomingMessage::StationInfo { name, qth, locator, email } => {
                let l = &mut self.state.qso_log;
                if let Some(v) = name { l.name = v.clone(); }
                if let Some(v) = qth { l.qth = v.clone(); }
                if let Some(v) = locator { l.locator = v.to_uppercase(); }
                if name.is_some() {
                    self.state.partner_name = name.clone();
                }
                if qth.is_some() {
                    self.state.partner_qth = qth.clone();
                }
                if locator.is_some() {
                    self.state.partner_locator = locator.clone();
                }
                let parts: Vec<String> = [name.as_ref().map(|n| format!("Name: {n}")), qth.as_ref().map(|q| format!("QTH: {q}")), locator.as_ref().map(|l| format!("Locator: {l}")), email.as_ref().map(|e| format!("Email: {e}"))]
                    .into_iter()
                    .flatten()
                    .collect();
                self.state.add_system_chat(format!("Station info: {}", parts.join(", ")));
            }
            IncomingMessage::BbsFileOffer(offer) => {
                self.state.file_accepted = false;
                let original = bbs::strip_gzip_suffix(&offer.name);
                self.state.add_system_chat(format!(
                    "File offered: {} ({} bytes compressed)",
                    original, offer.compressed_size,
                ));
                self.state.status_msg = format!("File offer: {}", original);
                self.state.pending_file_offer = Some(offer.clone());
                // The session answered as VarAC does (small: <SFRD>; big while away: <SFAB>)
                use crate::varac::session::FileOfferDecision;
                match self.station_config.policy.file_offer_decision(offer.compressed_size as u64, self.state.away_active) {
                    FileOfferDecision::Accept => {
                        self.state.file_accepted = true;
                        self.state.add_system_chat(format!("File {original} accepted automatically"));
                    }
                    FileOfferDecision::Refuse => {
                        self.state.pending_file_offer = None;
                        self.state.add_system_chat(format!("File {original} refused: bigger than the incoming files size limit while away"));
                    }
                    FileOfferDecision::Ask => {}
                }
                if self.state.settings_form.notifications_enabled {
                    send_notification("File Offer", &format!("{} ({} bytes)", original, offer.compressed_size));
                }
                if self.state.settings_form.sound_alerts {
                    play_alert_sound();
                }
            }
            IncomingMessage::BbsFilePayload(data) if self.state.pending_vmail_attachment => {
                self.state.pending_vmail_attachment = false;
                let b64_str = String::from_utf8_lossy(data);
                if let Some(offer) = self.state.pending_file_offer.take() {
                    match bbs::receive_file(b64_str.trim(), &offer.md5_hex)
                        .map_err(|e| e.to_string())
                        .and_then(|zip| extract_vmail_attachments(&zip, bbs::strip_gzip_suffix(&offer.name)))
                    {
                        Ok((dir, files)) => {
                            for f in &files {
                                let _ = self.db.log_file(&dir.join(f), &partner, false);
                            }
                            self.state.add_system_chat(format!("VMail attachments saved to {}: {}", dir.display(), files.join(", ")));
                            self.state.last_attachment_dir = Some(dir);
                        }
                        Err(e) => {
                            self.state.add_system_chat(format!("VMail attachment error: {e}"));
                        }
                    }
                }
            }
            IncomingMessage::BbsFilePayload(data) => {
                let b64_str = String::from_utf8_lossy(data);
                self.state.file_accepted = false;
                if let Some(offer) = self.state.pending_file_offer.take() {
                    let original = bbs::strip_gzip_suffix(&offer.name);
                    match bbs::receive_file(b64_str.trim(), &offer.md5_hex) {
                        Ok(file_data) => {
                            // Settings → QSO incoming files folder (VarAC IncomingFilesDir), else
                            // ~/Downloads, else our data folder (never the current directory)
                            let save_dir = Some(std::path::PathBuf::from(self.state.settings_form.incoming_files_dir.trim()))
                                .filter(|d| !d.as_os_str().is_empty())
                                .or_else(|| directories::UserDirs::new().and_then(|d| d.download_dir().map(|p| p.to_path_buf())))
                                .or_else(|| crate::config::project_dirs().map(|d| d.data_dir().join("received")))
                                .unwrap_or_else(std::env::temp_dir);
                            let _ = std::fs::create_dir_all(&save_dir);
                            // The name comes from the other station: keep only a plain file name;
                            // never overwrite (VarAC: name_1.ext) and defuse runnable files
                            let random6 = (Utc::now().timestamp_subsec_nanos() / 1000) % 1_000_000;
                            let save_path = bbs::received_path(&save_dir, &safe_file_name(original), random6);
                            match std::fs::write(&save_path, &file_data) {
                                Ok(()) => {
                                    let _ = self.db.log_file(&save_path, &partner, false);
                                    self.state.add_system_chat(format!(
                                        "File saved: {} ({} bytes)",
                                        save_path.display(), file_data.len(),
                                    ));
                                    self.state.send_cmd(GuiCommand::FileOk);
                                }
                                Err(e) => {
                                    // VarAC: <SFOK> only once it's saved; a failure is <SFFA>
                                    self.state.add_system_chat(format!("File save error: {e}"));
                                    self.state.send_cmd(GuiCommand::FileFailed);
                                }
                            }
                        }
                        Err(e) => {
                            // VarAC answers a broken file with <SFFA>
                            self.state.add_system_chat(format!("File transfer error: {e}"));
                            self.state.send_cmd(GuiCommand::FileFailed);
                        }
                    }
                }
            }
            IncomingMessage::BbsFileOk => {
                self.state.add_system_chat("File transfer completed".into());
            }
            IncomingMessage::BbsFileAborted => {
                self.state.add_system_chat("File transfer aborted".into());
                self.state.pending_file_offer = None;
                self.state.file_accepted = false;
                self.state.attachment_ask = None;
            }
            IncomingMessage::BbsFileFailed => {
                self.state.add_system_chat("FILE TRANSFER FAILED AT DESTINATION! Your partner was given the reason for the failure.".into());
                self.state.pending_file_offer = None;
                self.state.file_accepted = false;
            }
            IncomingMessage::Broadcast(bc) if self.state.is_blocked(&bc.from) => {
                self.state.add_log(format!("Broadcast received from a Blocked station: {}. Ignored.", bc.from));
            }
            IncomingMessage::Broadcast(bc) => {
                if bc.from.split('-').next().unwrap_or("").eq_ignore_ascii_case(self.state.my_callsign.split('-').next().unwrap_or("")) {
                    sound::narrate(sound::Topic::MyBroadcasts, format!("Sending broadcast to {}", if bc.to.eq_ignore_ascii_case("ALL") { "all".to_string() } else { sound::phonetic(&bc.to) }));
                } else {
                    sound::play(sound::Sound::Broadcast);
                    sound::narrate(sound::Topic::Broadcasts, format!("Broadcast from {} to {}. ", sound::phonetic(bc.from.split('-').next().unwrap_or(&bc.from)),
                        if bc.to.eq_ignore_ascii_case("ALL") { "all".to_string() } else { sound::phonetic(&bc.to) }));
                }
                if !bc.from.eq_ignore_ascii_case(&self.state.my_callsign) {
                    let from = bc.from.split('-').next().unwrap_or(&bc.from).to_string();
                    self.psk_heard(&from, bc.snr.map(|s| s.round() as i32), None);
                }
                // VARA FM: the same broadcast again through a digipeater within 10 s is an
                // echo (as in VarAC)
                let freq = self.state.current_frequency as i64;
                if self.state.settings_form.vara_modem_type == "VaraFM" && bc.via.is_some()
                    && self.db.is_duplicate_broadcast(freq, &bc.from, &bc.to, bc.via.as_deref(), &bc.message, 10).unwrap_or(false)
                {
                    self.state.add_log(format!("Broadcast from {} repeated by a digipeater, ignored", bc.from));
                    return;
                }
                // (stored even when not shown, VarAC)
                let _ = self.db.insert_broadcast(&crate::storage::BroadcastRecord {
                    guid: uuid::Uuid::new_v4().to_string(),
                    broadcast_time: bc.timestamp,
                    frequency: freq,
                    from_callsign: bc.from.clone(),
                    to_callsign: bc.to.clone(),
                    via_callsign: bc.via.clone(),
                    broadcast_message: bc.message.clone(),
                    snr: bc.snr.map(|s| s as i32),
                    band: Some(self.state.current_band.clone()),
                });
                // VarAC doesn't show other stations' Path Finder answers at all
                let Some(shown) = crate::varac::messages::broadcast_display(
                    &bc.message, &bc.to, &self.state.my_callsign, self.state.path_finder_target.as_deref(),
                ) else {
                    return;
                };
                let entry = BroadcastEntry {
                    time: bc.timestamp.format("%H:%M").to_string(),
                    from: bc.from.split('-').next().unwrap_or(&bc.from).to_string(),
                    to: bc.to.clone(),
                    via: bc.via.clone(),
                    message: shown.clone(),
                    qsy_hz: crate::varac::messages::qsy_broadcast_freq(&bc.message),
                    alert: self.alert_check(crate::varac::alerts::Source::Broadcast, &bc.from, &bc.to, &[&bc.message, &bc.from, &bc.to]),
                };
                self.state.broadcasts.push_back(entry);
                if self.state.broadcasts.len() > 200 {
                    self.state.broadcasts.pop_front();
                }
                self.state.add_log(format!("Broadcast {} → {}: {}", bc.from, bc.to, shown));
            }
            IncomingMessage::QsyRequest { offset_hz } => {
                self.state.add_system_chat(format!("QSY requested: {offset_hz} Hz offset — Accept or Reject in toolbar"));
                self.state.add_log(format!("QSY request: {offset_hz} Hz"));
                self.state.pending_qsy = Some(state::PendingQsy { offset_hz: Some(*offset_hz), freq_hz: None });
                if self.state.settings_form.notifications_enabled {
                    send_notification("QSY Request", &format!("{offset_hz} Hz offset"));
                }
            }
            IncomingMessage::QsyAbsolute { freq_hz } => {
                if self.state.auto_qsy_enabled && self.state.is_qsy_freq_allowed(*freq_hz) {
                    self.state.add_system_chat(format!("Auto-QSY: accepting {} Hz", freq_hz));
                    self.state.add_log(format!("Auto-QSY accepted: {freq_hz} Hz"));
                    self.state.current_frequency = *freq_hz;
                    if let Some(band) = freq_to_band(*freq_hz) {
                        self.state.current_band = band.to_string();
                    }
                    if let Some(ref rig) = self.rig {
                        rig.set_mode(self.state.rig_usb_mode());
                        rig.set_freq(*freq_hz);
                        self.rig_freq_set_at = Some(Utc::now());
                        self.freq_check = Some(((*freq_hz) as u64, 0));
                        self.rig_poll_at = Some(Utc::now() + chrono::Duration::seconds(3));
                    }
                    self.state.send_cmd(GuiCommand::QsyAccept);
                } else {
                    self.state.add_system_chat(format!("QSY requested: {} Hz — Accept or Reject in toolbar", freq_hz));
                    self.state.add_log(format!("QSY absolute request: {freq_hz} Hz"));
                    self.state.pending_qsy = Some(state::PendingQsy { offset_hz: None, freq_hz: Some(*freq_hz) });
                    if self.state.settings_form.notifications_enabled {
                        send_notification("QSY Request", &format!("{freq_hz} Hz"));
                    }
                }
            }
            IncomingMessage::QsyAccepted => {
                self.state.add_system_chat("QSY accepted by partner".into());
                self.state.add_log("QSY accepted".into());
                if let Some(ref pqsy) = self.state.pending_qsy {
                    let target = if let Some(f) = pqsy.freq_hz {
                        Some(f)
                    } else if let Some(off) = pqsy.offset_hz {
                        Some((self.state.current_frequency as i64 + off as i64) as u64)
                    } else {
                        None
                    };
                    if let Some(f) = target {
                        self.state.current_frequency = f;
                        if let Some(band) = freq_to_band(f) {
                            self.state.current_band = band.to_string();
                        }
                        if let Some(ref rig) = self.rig {
                            rig.set_mode(self.state.rig_usb_mode());
                            rig.set_freq(f);
                            self.rig_freq_set_at = Some(Utc::now());
                            self.freq_check = Some(((f) as u64, 0));
                            self.rig_poll_at = Some(Utc::now() + chrono::Duration::seconds(3));
                        }
                        self.state.status_msg = format!("QSY to {f} Hz");
                    } else {
                        self.state.status_msg = "QSY accepted — change frequency".into();
                    }
                } else {
                    self.state.status_msg = "QSY accepted — change frequency".into();
                }
            }
            IncomingMessage::QsyRejected => {
                self.state.add_system_chat("QSY rejected by partner".into());
                self.state.add_log("QSY rejected".into());
                self.state.status_msg = "QSY rejected".into();
            }
            IncomingMessage::QsyOutOfRange => {
                self.state.add_system_chat("QSY out of allowed range".into());
                self.state.add_log("QSY out of range".into());
            }
            IncomingMessage::TestLink => {
                self.state.add_system_chat("Test link received (QSY confirmed)".into());
            }
            // VarAC cluster requests: a single station, answered <CCE> by the session
            IncomingMessage::ClusterStationsRequest => self.state.add_log("Cluster stations request answered: <CCE>".into()),
            IncomingMessage::ClusterStationsReply(stations) => {
                self.state.add_system_chat(format!("STATIONS CONNECTED TO THIS CLUSTER:\n{}\n{CLUSTER_HELP}", stations.replace('|', " / ")));
            }
            IncomingMessage::ClusterStationsEmpty => {
                self.state.add_system_chat("CLUSTER: No connected stations.".into());
            }
            IncomingMessage::ClusterMessage(msg) => {
                self.state.add_system_chat(format!("Cluster: {msg}"));
                self.state.add_log(format!("Cluster message: {msg}"));
            }
            // Answered by the session (<LHJ>: not available yet)
            IncomingMessage::LastHeardRequest => self.state.add_log("Last-heard request answered: not available".into()),
            IncomingMessage::LastHeardReply(data) => {
                self.state.add_system_chat(format!("Last heard: {data}"));
            }
            // The partner's answers, worded as VarAC shows them
            IncomingMessage::LastHeardEmpty => {
                let p = self.state.connected_to.clone().unwrap_or_default();
                self.state.add_system_chat(format!("{p} LAST HEARD LIST IS EMPTY (OR DID NOT HEAR THIS STATION)"));
            }
            IncomingMessage::LastHeardRejected => {
                let p = self.state.connected_to.clone().unwrap_or_default();
                self.state.add_system_chat(format!("{p} REJECTED YOUR LAST HEARD PEEKING REQUEST"));
            }
            IncomingMessage::LastHeardCallsignRequest(c) => self.state.add_log(format!("Last-heard request for {c} answered")),
            IncomingMessage::LocatorRequest => self.state.add_log("Locator request answered".into()),
            IncomingMessage::GpsRequest => self.state.add_log("GPS request answered (no GPS)".into()),
            IncomingMessage::RelayPeek => self.state.add_log("Parked-VMail peek answered".into()),
            IncomingMessage::RelayPeekRejected => {
                self.state.add_system_chat("Partner won't show its parked VMail".into());
            }
            IncomingMessage::FreqScheduleRequest => self.state.add_log("Frequency schedule request answered: scheduler off".into()),
            IncomingMessage::FreqScheduleResponse(_) => {
                // <FSO>
                let p = self.state.connected_to.clone().unwrap_or_default();
                self.state.add_system_chat(format!("{p} FREQUENCY SCHEDULER IS OFF"));
            }
            // Our AI gateway (VarAC's "AI:" handler): off → <AIJ>, over the per-QSO limit →
            // <AIL>, otherwise ask Gemini and answer <ACIE>… (<AIE> on failure)
            IncomingMessage::AiPrompt(question) => {
                self.state.add_chat(ChatLine {
                    time: ts,
                    callsign: partner.clone(),
                    text: format!("AI:{question}"),
                    chat_id: self.state.rx_chat_id,
                    ..Default::default()
                });
                let ai = crate::config::ai_settings();
                if !ai.enabled {
                    self.state.send_cmd(GuiCommand::SendInquiry("<AIJ>".into()));
                    self.state.add_log("AI request rejected: the AI gateway is off".into());
                } else {
                    self.state.ai_count += 1;
                    if ai.limit_per_qso > 0 && self.state.ai_count > ai.limit_per_qso {
                        self.state.send_cmd(GuiCommand::SendInquiry("<AIL>".into()));
                        self.state.add_log("AI request refused: interactions per QSO limit reached".into());
                    } else if self.state.ai_pending.is_some() {
                        // one at a time: the next question waits for the answer
                        self.state.ai_waiting.push_back(question.clone());
                    } else {
                        self.ask_ai(ai, question.clone());
                    }
                }
            }
            // The partner's AI gateway answered: from now on we talk to the AI (VarAC AI MODE)
            IncomingMessage::AiResponse(answer) => {
                let quit = answer.contains("<DISAI>");
                let answer = answer.replace("<DISAI>", "").trim().to_string();
                if !self.state.ai_mode && !quit {
                    self.state.ai_mode = true;
                    self.state.add_system_chat(format!("AI MODE ACTIVATED. ALL TRAFFIC IS ROUTED TO {}. CLICK 'AI MODE' TO EXIT.", crate::varac::ai_gateway::AGENT_NAME));
                }
                self.state.add_chat(ChatLine {
                    time: ts,
                    callsign: format!("{partner} ({})", crate::varac::ai_gateway::AGENT_NAME),
                    text: answer.replace("//n", "\n"),
                    chat_id: self.state.rx_chat_id,
                    ..Default::default()
                });
                if quit {
                    self.state.ai_mode = false;
                    self.state.add_system_chat("EXITING AI MODE.".into());
                }
            }
            IncomingMessage::AiError => {
                self.state.ai_mode = false;
                self.state.add_system_chat("AI REQUEST FAILED. PLEASE TRY AGAIN LATER.".into());
            }
            IncomingMessage::AiRateLimited => {
                self.state.ai_mode = false;
                self.state.add_system_chat("AI REQUESTS LIMIT EXCEEDED. EXITING AI MODE.".into());
            }
            IncomingMessage::AiRejected => {
                self.state.ai_mode = false;
                self.state.add_system_chat("AI REQUEST REJECTED.".into());
            }
            IncomingMessage::AiDisable => {
                self.state.ai_mode = false;
                self.state.add_system_chat("EXITING AI MODE.".into());
            }
            IncomingMessage::HamPlay(m) => self.state.on_hamplay(m.clone()),
            IncomingMessage::ServiceNotice(t) => self.state.add_system_chat(t.clone()),
            IncomingMessage::ClusterInstancesRequest => self.state.add_log("Cluster instances request answered: NO OTHER INSTANCES".into()),
            // A line relayed by a VarAC cluster (FROM>text <CM>)
            IncomingMessage::ClusterRelayed(text) => {
                if !self.state.cluster_note_shown {
                    self.state.cluster_note_shown = true;
                    self.state.add_system_chat(format!("YOU HAVE RECEIVED A MESSAGE FROM A USER CURRENTLY CONNECTED TO THE {partner} CLUSTER.\n{CLUSTER_HELP}"));
                }
                self.state.add_chat(ChatLine {
                    time: ts,
                    callsign: partner.clone(),
                    text: text.clone(),
                    chat_id: self.state.rx_chat_id,
                    ..Default::default()
                });
            }
            IncomingMessage::PactorLevelRequest(n) => self.state.add_log(format!("SPEED LEVEL CHANGE REQUESTED (LEVEL {n}). APPROVING...")),
            IncomingMessage::PactorLevelAnswer(ok) => self.state.add_system_chat(
                if *ok { "SPEED LEVEL CHANGE APPROVED".into() } else { "SPEED LEVEL CHANGE REQUEST REJECTED".into() }),
            IncomingMessage::AwayQsy => {
                self.state.partner_awq = true;
                self.state.add_system_chat("Partner is away (QSY/VMail capable)".into());
                self.state.add_log("Partner away with QSY capability".into());
            }
            IncomingMessage::AwayTimeExceeded => {
                self.state.add_system_chat("Away time limit exceeded — disconnecting".into());
                self.state.add_log("Away time exceeded".into());
            }
            IncomingMessage::IdleTimeExceeded => {
                self.state.add_system_chat("Idle time limit exceeded — disconnecting".into());
                self.state.add_log("Idle time exceeded".into());
            }
            IncomingMessage::VMailWaiting => {
                // VarAC: "{CALL} HAS VMAILS TO RELAY TO YOU. INVITE {CALL} TO QSY TO RECEIVE THE VMAILS"
                let partner = self.state.connected_to.clone().unwrap_or_default();
                self.state.add_system_chat(format!("{partner} HAS VMAILS TO RELAY TO YOU. INVITE {partner} TO QSY TO RECEIVE THE VMAILS"));
                self.state.add_log("VMail waiting notification received".into());
                if self.state.settings_form.notifications_enabled {
                    send_notification("VMail Waiting", &format!("{partner} has VMail for you"));
                }
            }
            IncomingMessage::VMailRejected => {
                let partner = self.state.connected_to.clone().unwrap_or_default();
                self.state.add_system_chat(format!("VMAIL REJECTED AS {partner} DOES NOT ACCEPT INCOMING VMAILS."));
            }
            IncomingMessage::AttachmentRefused => {
                self.state.add_system_chat("The VMail's attachment was not taken: the VMail stays in your Outbox.".into());
                refresh_all_vmail_lists(&mut self.state, &self.db);
            }
            IncomingMessage::VMailFailed => {
                self.state.add_system_chat("VMAIL DELIVERY FAILED ON THE RECEIVING SIDE. STOPPING VMAIL FORWARDING.".into());
            }
            IncomingMessage::LastConnectionsRequest => self.state.add_log(
                if crate::config::emcomm().last_connections { "Last-connections request answered" } else { "Last-connections request refused (EmComm only)" }.into(),
            ),
            IncomingMessage::LastConnectionsRejected => {
                let partner = self.state.connected_to.clone().unwrap_or_default();
                self.state.add_system_chat(format!("{partner} REJECTED YOUR LAST HEARD CONNECTIONS REQUEST"));
            }
            IncomingMessage::ParkingNotAllowed => {
                let p = self.state.connected_to.clone().unwrap_or_default();
                self.state.add_system_chat(format!("VMAIL DELIVERY FAILED AS {p} DOES NOT ALLOW PARKING VMAILS FOR 3rd PARTIES"));
                self.state.add_log("VMail parking rejected".into());
                self.state.status_msg = "Parking not allowed".into();
            }
            // Verbose SNR (VarAC <VSI>): the session already answered when we're away
            // (reject) or auto-accept is on; otherwise ask the operator
            IncomingMessage::ExtendedSnrRequest => {
                let partner = self.state.connected_to.clone().unwrap_or_default();
                if self.state.away_active {
                    self.state.add_system_chat(format!("{partner} invited you to verbose SNR: rejected (away)"));
                } else if self.state.settings_form.verbose_snr_auto_accept {
                    self.state.verbose_snr = true;
                    self.state.add_system_chat(format!("{partner} invited you to verbose SNR: accepted, SNR reports every minute"));
                } else {
                    self.state.verbose_snr_invite = true;
                }
            }
            IncomingMessage::ExtendedSnrAccepted => {
                let partner = self.state.connected_to.clone().unwrap_or_default();
                self.state.add_system_chat(format!("{partner} ACCEPTED YOUR VERBOSE SNR EXCHANGE INVITATION"));
            }
            IncomingMessage::ExtendedSnrRejected => {
                let partner = self.state.connected_to.clone().unwrap_or_default();
                self.state.verbose_snr = false;
                self.state.add_system_chat(format!("{partner} REJECTED YOUR VERBOSE SNR EXCHANGE INVITATION"));
            }
            IncomingMessage::ExtendedSnrStop => {
                self.state.verbose_snr = false;
                self.state.add_system_chat("Partner stopped verbose SNR".into());
            }
            IncomingMessage::QsyReady => {
                self.state.add_system_chat("Partner ready on new frequency".into());
                self.state.add_log("QSY ready signal received".into());
            }
            IncomingMessage::QsyAuto => {
                self.state.add_system_chat("QSY auto mode active".into());
                self.state.add_log("QSY auto signal received".into());
            }
            IncomingMessage::QsyLock => {
                self.state.add_system_chat("Frequency locked by partner".into());
                self.state.add_log("QSY lock signal received".into());
            }
            IncomingMessage::InfoDenied => {
                let p = self.state.connected_to.clone().unwrap_or_default();
                self.state.add_system_chat(format!("{p} REJECTED YOUR INFO PEEKING REQUEST"));
                self.state.add_log("Info request denied (INFOJ)".into());
            }
            // Answered by the session (works while this window is hidden)
            IncomingMessage::BbsListRequest => self.state.add_log("BBS file list requested".into()),
            IncomingMessage::BbsListing(entries) => {
                self.state.add_log(format!("BBS listing received: {} files", entries.len()));
                self.state.bbs_listing_entries = entries.clone();
                self.state.bbs_listing_open = true;
            }
            IncomingMessage::BbsFileRequest(name) => self.state.add_log(format!("BBS file requested: {name}")),
            IncomingMessage::BbsFileReady => {
                self.state.add_system_chat("Remote station ready to receive file".into());
            }
            IncomingMessage::BbsListRejected => {
                self.state.add_system_chat("BBS listing request rejected".into());
                self.state.status_msg = "BBS listing rejected by remote station".into();
            }
            IncomingMessage::BbsFileRejected => {
                self.state.add_system_chat("BBS file request rejected".into());
                self.state.status_msg = "BBS file rejected by remote station".into();
            }
            IncomingMessage::ServiceInfo(flags) => {
                self.state.partner_services = Some(*flags);
                self.state.add_log(format!("Partner services: {}", flags.to_letters()));
            }
        }
    }
}

impl VaracApp {
    /// Everything that doesn't draw: session messages, rig events, timers (beacons, slots,
    /// auto-CQ, scanner, reconnects). Runs from the frame loop and from a background
    /// thread, so it keeps going while the window is covered or minimized (Wayland stops
    /// drawing those).
    pub fn tick(&mut self) {
        self.process_session_messages();
        for f in std::mem::take(&mut self.state.sent_files) {
            let call = self.state.connected_to.clone().unwrap_or_default();
            let _ = self.db.log_file(&f, &call, true);
        }
        self.poll_ai();
        self.email_gate_status();
        self.ale_tick();
        // The real dial (on a slot the CF stays put), pushed only when our view of it
        // changes: a late frame mustn't undo the protocol task's own QSY update
        let dial = self.state.dial();
        if dial != self.dial_pushed {
            self.dial_pushed = dial;
            self.ptt_router.set_dial(dial);
        }


        self.freq_schedule_tick();

        // Rig control event processing
        if let Some(ref mut rig) = self.rig {
            for ev in rig.poll_events() {
                match ev {
                    crate::rig::RigEvent::Connected(name) => {
                        self.state.rig_connected = true;
                        self.state.rig_error = None;
                        self.state.add_log(format!("Rig connected: {name}"));
                        self.state.status_msg = format!("Radio: {name}");
                        self.rig_poll_at = Some(Utc::now() + chrono::Duration::seconds(2));
                        // Back to where we were last time (it was only shown, never tuned)
                        if std::mem::take(&mut self.restore_freq_pending) {
                            rig.set_mode(self.state.rig_usb_mode());
                            rig.set_freq(self.state.current_frequency);
                            self.rig_freq_set_at = Some(Utc::now());
                            self.freq_check = Some(((self.state.current_frequency) as u64, 0));
                            self.state.add_log(format!("Radio tuned to the last frequency {:.3} MHz", self.state.current_frequency as f64 / 1e6));
                        }
                    }
                    crate::rig::RigEvent::Disconnected => {
                        self.state.rig_connected = false;
                        self.state.rig_freq = None;
                        self.state.rig_mode = None;
                        self.state.add_log("Rig disconnected".into());
                    }
                    crate::rig::RigEvent::FreqUpdate(f) => {
                        self.state.rig_freq = Some(f);
                        // VarAC (CAT): still not there 3 s after the change → ask again, up to
                        // 10 times ("Trying to change frequency again to …")
                        let cat = matches!(self.state.rig_backend, crate::rig::RigBackend::CatSerial { .. });
                        match self.freq_check {
                            Some((want, _)) if want == f || !cat => self.freq_check = None,
                            Some((want, n)) if n < 10 && self.rig_freq_set_at.is_some_and(|t| (Utc::now() - t).num_seconds() >= 3) => {
                                self.state.add_log(format!("Trying to change frequency again to {:.6} as the rig is still reporting it is on {:.6}",
                                    want as f64 / 1e6, f as f64 / 1e6));
                                rig.set_mode(self.state.rig_usb_mode());
                                rig.set_freq(want);
                                self.rig_freq_set_at = Some(Utc::now());
                                self.freq_check = Some((want, n + 1));
                                continue;
                            }
                            Some((_, n)) if n >= 10 => self.freq_check = None,
                            _ => {}
                        }
                        let in_cooldown = self.rig_freq_set_at
                            .map(|t| (Utc::now() - t).num_seconds() < 3)
                            .unwrap_or(false);
                        // The calling frequency follows the dial, except while the radio sits
                        // on one of its slots (VarAC keeps the CF while you're on a slot)
                        let on_a_slot = crate::varac::slots::slot_at(self.state.current_frequency, f, self.state.slot_step())
                            .is_some_and(|s| s != 0);
                        if !self.state.freq_mode.scanning() && !in_cooldown && !on_a_slot {
                            self.state.current_frequency = f;
                            if let Some(band) = freq_to_band(f) {
                                self.state.current_band = band.to_string();
                            }
                        }
                    }
                    crate::rig::RigEvent::ModeUpdate(m) => {
                        self.state.rig_mode = Some(m);
                    }
                    crate::rig::RigEvent::PttUpdate(on) => {
                        self.state.rig_ptt = on;
                    }
                    crate::rig::RigEvent::Error(e) => {
                        self.state.add_log(format!("Rig error: {e}"));
                        self.state.rig_error = Some(e);
                    }
                }
            }

        }

        // PTT test buttons in Settings → RIG
        if let Some(on) = self.state.rig_test_ptt.take() {
            match self.rig {
                Some(ref rig) => rig.set_ptt(on),
                None => self.state.status_msg = "No rig control configured".into(),
            }
        }

        // "Detect radio" in Settings → RIG: close our own CAT port so it can be probed, then
        // look for the radio off the UI thread
        if self.state.rig_detect_requested {
            self.state.rig_detect_requested = false;
            self.ptt_router.set_target(None);
            self.rig = None;
            self.state.rig_connected = false;
            self.state.rig_ptt = false;
            self.state.rig_ports = crate::rig::list_serial_ports();
            let ports = self.state.rig_ports.clone();
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                // (the port the old manager held is released once its worker stops)
                std::thread::sleep(std::time::Duration::from_millis(500));
                let _ = tx.send(crate::rig::detect_cat(&ports));
            });
            self.state.rig_detect_rx = Some(rx);
            self.state.rig_detect_status = Some("Looking for the radio…".into());
        }
        if let Some(r) = self.state.rig_detect_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.state.rig_detect_rx = None;
            match r {
                Ok(d) => {
                    self.state.rig_detect_status =
                        Some(format!("Found {} on {} at {} baud. Press Save to keep it.", d.radio, d.port, d.baud));
                    self.state.rig_settings_backend = "cat".into();
                    self.state.rig_settings_serial = d.port;
                    self.state.rig_settings_baud = d.baud.to_string();
                    self.state.rig_settings_model = d.model.into();
                }
                Err(e) => self.state.rig_detect_status = Some(format!("Not found: {e}")),
            }
            // Connect with what was found, or back to the settings as they were
            self.state.rig_reconnect_requested = true;
        }

        // Rig (re)connect on settings save or the Reconnect button
        if self.state.rig_reconnect_requested {
            self.state.rig_reconnect_requested = false;
            self.ptt_router.set_enabled(self.state.rig_settings_ptt);
            // Dropping the old manager unkeys the radio and stops its worker.
            self.ptt_router.set_target(None);
            self.rig = None;
            self.state.rig_connected = false;
            self.state.rig_ptt = false;
            self.state.rig_freq = None;
            self.state.rig_mode = None;
            self.state.rig_error = None;
            self.state.rig_backend = self.state.rig_backend_from_settings();
            self.rig = self.state.rig_backend.clone().into_manager();
            self.ptt_router.set_target(self.rig.as_ref().map(|r| r.command_sender()));
        }

        // QSO duration alert
        if self.state.qso_alert_minutes > 0 && !self.state.qso_alert_fired {
            if let (Some(_call), Some(start)) = (&self.state.connected_to, self.state.connection_start) {
                let elapsed = (Utc::now() - start).num_minutes();
                if elapsed >= self.state.qso_alert_minutes as i64 {
                    self.state.qso_alert_fired = true;
                    let mins = self.state.qso_alert_minutes;
                    send_notification("QSO Duration Alert", &format!("QSO has reached {mins} minutes"));
                    play_alert_sound();
                    self.state.add_log(format!("QSO duration alert: {mins} minutes reached"));
                }
            }
        }

        // Auto away (as in VarAC): idle for the set minutes with no
        // link, or on a link that came in, sets away; otherwise away is cleared again
        if self.state.activity_bump.take() {
            self.state.last_activity = Utc::now();
        }
        if self.state.settings_form.auto_away_enabled {
            let minutes: i64 = self.state.settings_form.auto_away_minutes.parse().unwrap_or(10);
            let idle = (Utc::now() - self.state.last_activity).num_minutes() >= minutes;
            let linked = self.state.session_state == SessionState::Connected;
            let want = idle && (!linked || self.state.link_incoming);
            if want && !self.state.away_active {
                self.state.away_active = true;
                self.state.auto_away_triggered = true;
                self.state.add_log("Setting away status to true (auto)".into());
                let msg = if self.state.away_message.trim().is_empty() { "Away".to_string() } else { self.state.away_message.clone() };
                self.state.send_cmd(GuiCommand::SetAway(Some(msg)));
            } else if !want && self.state.away_active {
                self.state.away_active = false;
                self.state.auto_away_triggered = false;
                self.state.add_log("Setting away status to false (auto)".into());
                self.state.send_cmd(GuiCommand::SetAway(None));
            }
        }

        // VarAC lets Ignore DCD run for 2 hours at most
        if self.state.ignore_dcd && self.state.ignore_dcd_since.is_some_and(|t| (Utc::now() - t).num_seconds() >= 7200) {
            self.state.ignore_dcd = false;
            self.state.ignore_dcd_since = None;
            self.state.send_cmd(GuiCommand::SetIgnoreDcd(false));
            crate::config::update_config_file(|c| c.ignore_dcd = Some(false));
            self.state.add_log("Resetting Ignore DCD to false. Max allowed time on Ignore DCD reached: 7200 Seconds".into());
        }

        // VarAC stops the periodic beacon 72 hours after it was switched on
        match (self.state.beacon_auto_enabled, self.state.beacon_started_at) {
            (true, None) => self.state.beacon_started_at = Some(Utc::now()),
            (false, Some(_)) => self.state.beacon_started_at = None,
            (true, Some(t)) if !self.state.settings_form.unlimited_beacon_time && (Utc::now() - t).num_minutes() > 4320 => {
                self.state.beacon_auto_enabled = false;
                self.state.beacon_started_at = None;
                self.state.add_log("Beacon limit time exceeded. Stopping.".into());
                self.state.add_system_chat("Beacon limit time (72 hours) exceeded: beacons stopped. Re-enable them to continue beaconing.".into());
            }
            _ => {}
        }
        // VarAC: after any frequency change the next beacon goes out ~10 s later (never
        // sooner), and switching beacons off and on
        // sends one at once
        {
            let dial = self.state.dial();
            let interval: i64 = self.state.settings_form.beacon_interval.parse().unwrap_or(15);
            if dial != self.state.beacon_dial {
                if self.state.beacon_dial != 0 && !self.state.freq_mode.scanning() {
                    self.state.last_beacon_sent = Some(Utc::now() - chrono::Duration::seconds(interval * 60 - 10));
                }
                self.state.beacon_dial = dial;
                self.state.beacon_freq_changed_at = Some(Utc::now());
            }
            if !self.state.beacon_auto_enabled && self.state.beacon_was_on {
                self.state.last_beacon_sent = None;
            }
            self.state.beacon_was_on = self.state.beacon_auto_enabled;
        }
        let settled = self.state.freq_mode.scanning()
            || self.state.beacon_freq_changed_at.is_none_or(|t| (Utc::now() - t).num_seconds() >= 10);
        // Beacon auto-send timer: paused while PTT is disabled, and (VarAC) it waits for the
        // channel to have been quiet a few seconds rather than asking
        let quiet = self.state.ignore_dcd
            || (!self.state.busy
                && self.state.busy_cleared_at.is_none_or(|t| (Utc::now() - t).num_seconds() >= self.state.busy_holdoff_secs));
        if self.state.beacon_auto_enabled
            && !self.state.ptt_disabled
            && self.state.cq_sniff.is_none() // (VarAC pauses beacons while the CQ sniffer runs)
            // (VarAC: not during a CQ, nor while standing by on a slot)
            && self.state.cq_seq.is_none() && self.state.slot_wait.is_none() && self.state.pending_slot_qsy.is_none()
            && settled
            && quiet
            && self.state.modem_connected
            && self.state.session_state == SessionState::Idle
        {
            let interval_min: i64 = self.state.settings_form.beacon_interval
                .parse().unwrap_or(15);
            let should_send = self.state.last_beacon_sent
                .map(|t| (Utc::now() - t).num_minutes() >= interval_min)
                .unwrap_or(true);
            if should_send {
                if self.state.refuse_wide_on_calling_freq() {
                    // skip this beacon (warned once per interval)
                    self.state.last_beacon_sent = Some(Utc::now());
                } else {
                    let source = self.state.beacon_source();
                    let bandwidth = self.state.cq_bandwidth();
                    // (already checked the channel: no "frequency is busy" question)
                    self.state.send_cmd(GuiCommand::Confirmed(Box::new(GuiCommand::SendCq { source, bandwidth })));
                    self.state.last_beacon_sent = Some(Utc::now());
                    self.state.add_log("Auto-beacon sent".into());
                    self.state.busy_holdoff_secs = rand_between(2, 7) as i64;
                }
            }
        }

        // VarAC slots: the CQ went out (fallback if no PTT report came), the wait on
        // the slot ran out, a connect that waits for the retune
        self.state.cq_seq_tick();
        if self.state.slot_wait.is_some_and(|(_, _, until)| Utc::now() >= until) {
            self.state.return_to_cf();
            self.state.add_system_chat("No reply on the slot: back on the calling frequency".into());
        }
        if self.state.pending_connect.as_ref().is_some_and(|(_, at)| Utc::now() >= *at) {
            if let Some((call, _)) = self.state.pending_connect.take() {
                if self.state.modem_connected {
                    self.state.add_log(format!("Connecting to {call}..."));
                    self.state.status_msg = format!("Connecting to {call}...");
                    self.state.connect_started = Some(Utc::now());
                    self.state.last_connect_callsign = Some(call.clone());
                    self.state.send_cmd(GuiCommand::Connect(call));
                }
            }
        }

        // Auto-CQ repeat (from the calling frequency, on the chosen slot again)
        if self.state.auto_cq_enabled
            && self.state.cq_active
            && self.state.modem_connected
            && self.state.session_state == SessionState::Idle
            && self.state.slot_wait.is_none()
            && self.state.pending_slot_qsy.is_none()
        {
            let should_send = self.state.last_cq_sent
                .map(|t| (Utc::now() - t).num_seconds() >= self.state.auto_cq_interval_secs as i64)
                .unwrap_or(true);
            if should_send {
                let slot = if self.state.settings_form.skip_cq_slot_selector { 0 } else { self.state.cq_slot };
                self.state.call_cq_on_slot(slot);
                self.state.add_log("Auto-CQ repeat sent".into());
            }
        }

        // Connection timeout
        if let Some(start) = self.state.connect_started {
            if self.state.session_state == SessionState::Connecting {
                let elapsed = (Utc::now() - start).num_seconds();
                let timeout = self.state.connect_timeout_secs as i64;
                if elapsed >= timeout {
                    self.state.connect_started = None;
                    self.state.status_msg = "Connection timed out — aborting".into();
                    self.state.add_log(format!("Connection timeout after {timeout}s — sending abort"));
                    self.state.send_cmd(GuiCommand::Abort);
                }
            } else {
                self.state.connect_started = None;
            }
        }

        // Modem auto-reconnect
        if self.state.modem_auto_reconnect
            && !self.state.modem_connected
            && self.state.session_state == SessionState::Idle
        {
            if let Some(at) = self.state.modem_reconnect_at {
                if Utc::now() >= at {
                    self.state.modem_reconnect_at = None;
                    self.state.add_log("Auto-reconnect: attempting modem connection...".into());
                    self.state.status_msg = "Auto-reconnecting to modem...".into();
                    self.state.send_cmd(GuiCommand::ConnectModem);
                }
            } else {
                self.state.modem_reconnect_at = Some(Utc::now() + chrono::Duration::seconds(10));
                self.state.add_log("Auto-reconnect: will retry in 10s".into());
            }
        }
        if self.state.modem_connected {
            self.state.modem_reconnect_at = None;
        }

        // PSKReporter: who we are (after Settings), and where we listen
        if std::mem::take(&mut self.state.psk_reconfigure) {
            self.configure_psk();
        }
        let dial = self.state.dial();
        if dial != self.psk_freq && dial > 0 {
            self.psk_freq = dial;
            let _ = self.psk.send(crate::varac::pskreporter::Command::Frequency(dial));
        }
        // VarAC's "QSY NOW" decides by itself when its countdown runs out (also while the
        // window isn't drawn)
        if self.state.qsy_popup.as_ref().is_some_and(|p| Utc::now() > p.until) {
            let accept = self.state.qsy_popup.as_ref().is_some_and(|p| p.default_accept);
            resolve_qsy_popup(&mut self.state, accept);
        }
        // VarAC's "QSY back" popup answers itself when its countdown runs out
        if let Some((cf, until, go)) = self.state.return_cf_prompt {
            if Utc::now() > until || self.state.connected_to.is_some() {
                self.state.return_cf_prompt = None;
                if go && self.state.connected_to.is_none() {
                    self.state.go_to_frequency(cf);
                }
            }
        }
        self.apply_tune_to();
    }

    fn apply_tune_to(&mut self) {
    if let Some(f) = self.state.tune_to.take() {
        sound::play(sound::Sound::FrequencyChange);
        if let Some(ref rig) = self.rig {
            // VarAC: the configured mode first, then the frequency
            rig.set_mode(self.state.rig_usb_mode());
            rig.set_freq(f);
            self.rig_freq_set_at = Some(Utc::now());
            self.freq_check = Some(((f) as u64, 0));
            self.rig_poll_at = Some(Utc::now() + chrono::Duration::seconds(3));
        }
    }
    }
}

/// The app as eframe runs it, shared with the background tick thread.
pub struct GuiShell(std::sync::Arc<std::sync::Mutex<VaracApp>>);

impl GuiShell {
    /// Keep the station working while the window isn't drawn (covered, minimized, another
    /// workspace, App Nap): no frames come then, and update() never runs.
    pub fn start(app: VaracApp, ctx: &egui::Context) -> Self {
        let app = std::sync::Arc::new(std::sync::Mutex::new(app));
        let weak = std::sync::Arc::downgrade(&app);
        let ctx = ctx.clone();
        std::thread::Builder::new()
            .name("gui-tick".into())
            .spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_millis(250));
                let Some(app) = weak.upgrade() else { break };
                let Ok(mut app) = app.lock() else { break };
                app.tick();
                drop(app);
                ctx.request_repaint();
            })
            .ok();
        Self(app)
    }
}

impl eframe::App for GuiShell {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if let Ok(mut app) = self.0.lock() {
            app.update(ctx, frame);
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        crate::vara::launch::close();
    }
}

impl eframe::App for VaracApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.tick();
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
        // VarAC: closing during a QSO, connect, CQ, beacon, broadcast or
        // ping asks first
        if ctx.input(|i| i.viewport().close_requested()) && self.state.close_confirm != Some(true) && self.state.freq_mode_busy() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.state.close_confirm = Some(false);
        }
        if self.state.close_confirm == Some(false) {
            egui::Window::new("Closing application")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label("You are in the middle of an activity (CQ, Beacon, QSO, Broadcast, Ping, etc.).\n\nAre you sure you want to close now?");
                    ui.horizontal(|ui| {
                        if ui.button("Yes").clicked() {
                            self.state.close_confirm = Some(true);
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        if ui.button("No").clicked() {
                            self.state.close_confirm = None;
                        }
                    });
                });
        }
        // The default 1360×880 window is taller than a laptop screen: fill the screen
        // instead of hanging off its bottom (where LOG and SEND are)
        if !self.fitted_to_screen {
            if let Some(mon) = ctx.input(|i| i.viewport().monitor_size) {
                self.fitted_to_screen = true;
                tracing::info!(target: "gui", "screen {}x{}", mon.x, mon.y);
                if mon.x < 1360.0 + 60.0 || mon.y < 880.0 + 100.0 {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
                }
            }
        }

        debug_screenshot(ctx);

        // Apply theme (only when it changes)
        let theme_key = (self.state.dark_mode, self.state.font_size.clamp(10.0, 24.0), self.state.scifi_theme);
        if self.applied_theme != Some(theme_key) {
            theme::apply_with(ctx, theme_key.0, theme_key.1, theme_key.2);
            self.applied_theme = Some(theme_key);
        }

        // Dynamic window title (with unread VMail indicator)
        let mut vmail_badge = if self.state.unread_vmails > 0 {
            format!(" ({} new)", self.state.unread_vmails)
        } else {
            String::new()
        };
        if self.state.settings_form.emcomm_mode {
            vmail_badge.push_str(" — EMCOMM");
        }
        let title = if let Some(ref call) = self.state.connected_to {
            format!("Openslot — {} [{}]{vmail_badge}", self.state.my_callsign, call)
        } else if !self.state.modem_connected {
            format!("Openslot — {} [OFFLINE]{vmail_badge}", self.state.my_callsign)
        } else {
            format!("Openslot — {}{vmail_badge}", self.state.my_callsign)
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));

        // Keyboard shortcuts
        ctx.input(|i| {
            if i.key_pressed(egui::Key::Escape) {
                self.state.viewing_vmail = false;
                self.state.compose_open = false;
                self.state.settings_open = false;
                self.state.log_open = false;
                self.state.qso_log_open = false;
                self.state.broadcast_compose_open = false;
                self.state.path_finder_open = false;
                self.state.file_send_open = false;
                self.state.canned_editor_open = false;
                self.state.heard_stations_open = false;
                self.state.contacts_open = false;
                self.state.chat_search_active = false;
                self.state.freq_ref_open = false;
                self.state.about_open = false;
                self.state.bbs_listing_open = false;
                self.state.qsy_dialog_open = false;
                self.state.watchlist_open = false;
                self.state.callsign_tags_open = false;
                self.state.qso_stats_open = false;
                self.state.bookmarks_open = false;
            }

            // Ctrl+F: toggle chat search
            if i.modifiers.ctrl && i.key_pressed(egui::Key::F) {
                self.state.chat_search_active = !self.state.chat_search_active;
                if !self.state.chat_search_active {
                    self.state.chat_search.clear();
                }
            }
            // Ctrl+L: toggle log window
            if i.modifiers.ctrl && i.key_pressed(egui::Key::L) {
                self.state.log_open = !self.state.log_open;
            }
            // Ctrl+Q: toggle QSO log
            if i.modifiers.ctrl && i.key_pressed(egui::Key::Q) {
                self.state.qso_log_open = !self.state.qso_log_open;
            }
            // Ctrl+M: compose VMail
            if i.modifiers.ctrl && i.key_pressed(egui::Key::M) {
                self.state.compose_open = true;
            }
            // Ctrl+B: compose broadcast
            if i.modifiers.ctrl && i.key_pressed(egui::Key::B) {
                self.state.broadcast_compose_open = true;
            }

            // F1–F12: canned messages (VarAC)
            let fkeys = [
                egui::Key::F1, egui::Key::F2, egui::Key::F3, egui::Key::F4, egui::Key::F5, egui::Key::F6,
                egui::Key::F7, egui::Key::F8, egui::Key::F9, egui::Key::F10, egui::Key::F11, egui::Key::F12,
            ];
            for (idx, key) in fkeys.iter().enumerate() {
                if i.key_pressed(*key) {
                    layout::pick_canned(&mut self.state, idx);
                }
            }
        });

        draw_menu_bar(ctx, &mut self.state);
        let band_changed = layout::draw_main_window(ctx, &mut self.state, &self.db, &self.current_qso_guid);
        let confirmed = std::mem::take(&mut self.state.confirm_qso_yes);
        if std::mem::take(&mut self.state.log_qso_requested) || confirmed {
            // VarAC's LOG: the ADIF log and loggers (and our QSO history)
            if self.log_qso(true, confirmed) {
                self.state.unlogged_qso = None;
                // after the link: its row takes the edited fields (during it, the row is
                // written from them at the end)
                if let (None, Some(guid)) = (&self.state.connected_to, self.state.qso_row_written.clone()) {
                    let f = &self.state.qso_log;
                    let num = |v: &str| v.trim().trim_start_matches('+').parse::<i32>().ok();
                    let end = chrono::NaiveDateTime::parse_from_str(f.end.trim(), "%Y-%m-%d %H:%M:%S").ok().map(|t| t.and_utc());
                    let _ = self.db.update_qso_from_log(&guid, &f.call.trim().to_uppercase(), f.band.trim(), f.name.trim(), f.qth.trim(),
                        &f.locator.trim().to_uppercase(), f.power.trim(), num(&f.snr_sent), num(&f.snr_rcvd), end.as_ref());
                }
                if self.state.connected_to.is_none() {
                    self.state.qso_log = state::QsoLogFields::default();
                }
            }
        }
        self.apply_tune_to();
        if band_changed {
            if let Some(ref rig) = self.rig {
                rig.set_mode(self.state.rig_usb_mode());
                rig.set_freq(self.state.current_frequency);
                self.rig_freq_set_at = Some(Utc::now());
                self.freq_check = Some(((self.state.current_frequency) as u64, 0));
                self.rig_poll_at = Some(Utc::now() + chrono::Duration::seconds(3));
            }
        }

        // Modal windows
        draw_settings_window(ctx, &mut self.state, &self.station_config, &self.modem_config);
        layout::draw_qsy_invitation_window(ctx, &mut self.state);
        layout::draw_cq_slot_selector(ctx, &mut self.state);
        layout::draw_stale_cq_confirm(ctx, &mut self.state);
        draw_log_window(ctx, &mut self.state);
        draw_qso_log_window(ctx, &mut self.state, &self.db);
        draw_qso_stats_window(ctx, &mut self.state, &self.db);
        draw_vmail_viewer_window(ctx, &mut self.state, &self.db);
        draw_broadcast_window(ctx, &mut self.state);
        draw_relay_offer(ctx, &mut self.state);
        draw_verbose_snr_invite(ctx, &mut self.state);
        draw_vmail_relay_alert(ctx, &mut self.state);
        draw_return_to_cf(ctx, &mut self.state);
        draw_tx_held(ctx, &mut self.state);
        draw_vmail_popups(ctx, &mut self.state);
        draw_qsy_popup(ctx, &mut self.state);
        draw_qso_log_pick(ctx, &mut self.state);
        draw_relay_notifications(ctx, &mut self.state, &self.db);
        draw_emcomm_confirm(ctx, &mut self.state);
        layout::draw_tune_window(ctx, &mut self.state);
        help::draw_help(ctx, &mut self.state);
        draw_bw_refused(ctx, &mut self.state);
        draw_path_finder_window(ctx, &mut self.state);
        draw_file_send_window(ctx, &mut self.state);
        draw_canned_editor_window(ctx, &mut self.state);
        draw_heard_stations_window(ctx, &mut self.state, &self.db);
        draw_contacts_window(ctx, &mut self.state, &self.db);
        draw_freq_ref_window(ctx, &mut self.state);
        draw_bbs_listing_window(ctx, &mut self.state);
        draw_qsy_window(ctx, &mut self.state);
        draw_watchlist_window(ctx, &mut self.state);
        draw_callsign_tags_window(ctx, &mut self.state);
        draw_bookmarks_window(ctx, &mut self.state);
        draw_about_window(ctx, &mut self.state);
        features::draw_hamplay_center(ctx, &mut self.state);
        features::draw_hamplay_game(ctx, &mut self.state);
        features::draw_block_confirm(ctx, &mut self.state, &self.station_config);
        features::draw_callsign_history(ctx, &mut self.state, &self.db);
        features::draw_queue_window(ctx, &mut self.state);
        features::draw_alert_center(ctx, &mut self.state, &self.db);
        features::draw_alert_setup(ctx, &mut self.state);
        features::draw_qso_confirm(ctx, &mut self.state);
        features::draw_dx_spot(ctx, &mut self.state);
        features::draw_ale(ctx, &mut self.state);
        features::draw_attachment_ask(ctx, &mut self.state);
        features::draw_sched_editor(ctx, &mut self.state);
        features::draw_shrinker(ctx, &mut self.state);
        features::draw_gallery(ctx, &mut self.state, &self.db);
        features::draw_qsl(ctx, &mut self.state);
        features::draw_map(ctx, &mut self.state, &self.db);
        features::draw_profiles(ctx, &mut self.state);
        features::draw_snr_chart(ctx, &mut self.state, &self.db);
    }
}

/// `OPENSLOT_SCREENSHOT=/path/out.ppm` saves one screenshot of the window ~3s after start
/// (for checking UI changes without a desktop screenshot tool).
fn debug_screenshot(ctx: &egui::Context) {
    let Ok(path) = std::env::var("OPENSLOT_SCREENSHOT").or_else(|_| std::env::var("VARAC_SCREENSHOT")) else { return };
    let requested = ctx.data_mut(|d| *d.get_temp_mut_or_default::<bool>(egui::Id::new("dbg_shot_req")));
    if !requested && ctx.input(|i| i.time) > 3.0 {
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("dbg_shot_req"), true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
    }
    ctx.input(|i| {
        for ev in &i.raw.events {
            if let egui::Event::Screenshot { image, .. } = ev {
                let [w, h] = image.size;
                let mut out = format!("P6\n{w} {h}\n255\n").into_bytes();
                for px in &image.pixels {
                    out.extend_from_slice(&[px.r(), px.g(), px.b()]);
                }
                let _ = std::fs::write(&path, out);
            }
        }
    });
}

/// Reduce a file name received over the air to a safe plain name (no directories,
/// no "..", no drive prefixes), so a crafted offer can't write outside the folder.
fn safe_file_name(name: &str) -> String {
    let last = name.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = last.chars().filter(|c| !c.is_control() && *c != ':').collect();
    let cleaned = cleaned.trim().trim_start_matches('.').to_string();
    if cleaned.is_empty() { "received_file".to_string() } else { cleaned }
}

/// Unpack a VarAC VMail-attachment zip into the data dir's `attachments/<name>/`.
fn extract_vmail_attachments(zip_bytes: &[u8], name: &str) -> Result<(std::path::PathBuf, Vec<String>), String> {
    let base = crate::config::project_dirs()
        .map(|d| d.data_dir().join("attachments"))
        .unwrap_or_else(|| std::path::PathBuf::from("attachments"));
    // `name` comes from the other station: flatten it before using it as a folder
    let dir = base.join(safe_file_name(name.trim_end_matches(".zip")));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(zip_bytes)).map_err(|e| e.to_string())?;
    let mut files = Vec::new();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        // enclosed_name() rejects absolute paths and ".." so a zip can't write outside `dir`
        let Some(rel) = entry.enclosed_name() else { continue };
        if entry.is_dir() {
            continue;
        }
        let out = dir.join(&rel);
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut f = std::fs::File::create(&out).map_err(|e| e.to_string())?;
        std::io::copy(&mut entry, &mut f).map_err(|e| e.to_string())?;
        files.push(rel.display().to_string());
    }
    Ok((dir, files))
}

fn draw_menu_bar(ctx: &egui::Context, state: &mut GuiState) {
    egui::TopBottomPanel::top("menu_bar")
        .frame(Frame::none().fill(theme::SURFACE).inner_margin(Margin::symmetric(12.0, 6.0)).stroke(Stroke::new(1.0_f32, theme::BORDER)))
        .show(ctx, |ui| {
        egui::menu::bar(ui, |ui| {
            // Brand mark
            let (rect, _) = ui.allocate_exact_size(Vec2::new(18.0, 18.0), egui::Sense::hover());
            let c = rect.center();
            ui.painter().circle_filled(c, 3.5, theme::ACCENT);
            ui.painter().circle_stroke(c, 6.5, Stroke::new(1.5_f32, theme::tint(theme::ACCENT, 160)));
            ui.painter().circle_stroke(c, 9.0, Stroke::new(1.0_f32, theme::tint(theme::ACCENT, 70)));
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.label(RichText::new("Open").strong().size(15.0).color(theme::TEXT));
            ui.label(RichText::new("slot").strong().size(15.0).color(theme::ACCENT));
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.add_space(14.0);
            ui.menu_button("Settings", |ui| {
                // Same entries/order as VarAC's Settings menu
                for (label, tab) in [
                    ("My Information", 0),
                    ("RIG Control and VARA Configurations", 5),
                    ("Appearance and Sounds", 4),
                ] {
                    if ui.button(label).clicked() {
                        state.settings_open = true;
                        state.settings_form.settings_tab = tab;
                        ui.close_menu();
                    }
                }
                if ui.button("Switch profile...").clicked() {
                    state.profiles_open = true;
                    ui.close_menu();
                }
                if ui.button("Canned (predefined) Messages").clicked() {
                    state.canned_editor_open = true;
                    ui.close_menu();
                }
                ui.separator();
                if ui.button("Callsign Tags").clicked() {
                    state.callsign_tags_open = true;
                    ui.close_menu();
                }
                ui.separator();
                if !state.modem_connected {
                    if ui.button("Connect Modem").clicked() {
                        state.send_cmd(GuiCommand::ConnectModem);
                        ui.close_menu();
                    }
                } else {
                    if ui.button("Disconnect Modem").clicked() {
                        state.send_cmd(GuiCommand::DisconnectModem);
                        ui.close_menu();
                    }
                }
                ui.separator();
                ui.menu_button("Bandwidth", |ui| {
                    if state.settings_form.vara_modem_type != "VaraHF" {
                        ui.label("Only VARA HF has a bandwidth choice");
                        return;
                    }
                    if ui.button("500 Hz").clicked() {
                        state.send_cmd(GuiCommand::SetBandwidth(Bandwidth::Bw500));
                        state.settings_form.bandwidth = "500".into();
                        ui.close_menu();
                    }
                    if ui.button("2300 Hz").clicked() {
                        state.send_cmd(GuiCommand::SetBandwidth(Bandwidth::Bw2300));
                        state.settings_form.bandwidth = "2300".into();
                        ui.close_menu();
                    }
                    if ui.button("2750 Hz").clicked() {
                        state.send_cmd(GuiCommand::SetBandwidth(Bandwidth::Bw2750));
                        state.settings_form.bandwidth = "2750".into();
                        ui.close_menu();
                    }
                });
                ui.menu_button("Listen Mode", |ui| {
                    if ui.button("ON").clicked() {
                        state.send_cmd(GuiCommand::SetListen(ListenMode::On));
                        ui.close_menu();
                    }
                    if ui.button("CQ only").clicked() {
                        state.send_cmd(GuiCommand::SetListen(ListenMode::Cq));
                        ui.close_menu();
                    }
                    if ui.button("OFF").clicked() {
                        state.send_cmd(GuiCommand::SetListen(ListenMode::Off));
                        ui.close_menu();
                    }
                });
                ui.separator();
                if ui.button("TUNE").clicked() {
                    state.send_cmd(GuiCommand::Tune);
                    ui.close_menu();
                }
                if ui.button("STOP TUNE").clicked() {
                    state.send_cmd(GuiCommand::TuneStop);
                    ui.close_menu();
                }
            });
            ui.menu_button("Tools", |ui| {
                if ui.button("Calling Frequencies...").clicked() {
                    state.freq_ref_open = true;
                    ui.close_menu();
                }
                if ui.button("Callsign Watchlist...").clicked() {
                    state.watchlist_open = true;
                    ui.close_menu();
                }
                if ui.button("Callsign Tags...").clicked() {
                    state.callsign_tags_open = true;
                    ui.close_menu();
                }
                if ui.button("Alert center...").clicked() {
                    state.alert_center_open = true;
                    ui.close_menu();
                }
                if ui.button("Alert tags...").clicked() {
                    state.alert_setup = Some(state.alert_tags.clone());
                    ui.close_menu();
                }
                if ui.button("QSL card (this / last QSO)...").clicked() {
                    let f = state.qso_log.clone();
                    let start = chrono::NaiveDateTime::parse_from_str(f.start.trim(), "%Y-%m-%d %H:%M:%S")
                        .map(|d| d.and_utc()).unwrap_or_else(|_| Utc::now());
                    state.open_qsl(&f.call, &f.snr_sent, &f.snr_rcvd, &f.band, start, &f.name);
                    ui.close_menu();
                }
                if ui.button("Map...").clicked() {
                    state.map_open = true;
                    ui.close_menu();
                }
                if ui.button("SNR chart (this QSO)...").clicked() {
                    state.snr_chart = Some(None);
                    ui.close_menu();
                }
                if ui.button("Image gallery...").clicked() {
                    state.gallery_open = true;
                    ui.close_menu();
                }
                if ui.button("HamPlay (games)...").clicked() {
                    state.hamplay_center_open = true;
                    ui.close_menu();
                }
                if ui.button("Clear Chat Window").clicked() {
                    state.chat_lines.clear();
                    state.status_msg = "Chat cleared".into();
                    ui.close_menu();
                }
            });
            ui.menu_button("Logs", |ui| {
                if ui.button("QSO Log...").clicked() {
                    state.qso_log_open = true;
                    ui.close_menu();
                }
                if ui.button("QSO Statistics...").clicked() {
                    state.qso_stats_open = true;
                    ui.close_menu();
                }
                if ui.button("Heard Stations...").clicked() {
                    state.heard_stations_open = true;
                    ui.close_menu();
                }
                if ui.button("Contacts / Address Book...").clicked() {
                    state.contacts_open = true;
                    ui.close_menu();
                }
                if ui.button("System Log...").clicked() {
                    state.log_open = true;
                    ui.close_menu();
                }
                if ui.button(format!("Bookmarks ({})...", state.bookmarked_messages.len())).clicked() {
                    state.bookmarks_open = true;
                    ui.close_menu();
                }
                ui.separator();
                if ui.button("Export Chat to File...").clicked() {
                    export_chat_log(state);
                    ui.close_menu();
                }
            });
            ui.menu_button("Help", |ui| {
                if ui.button("User guide...").clicked() {
                    state.help_open = true;
                    ui.close_menu();
                }
                if ui.button("About Openslot...").clicked() {
                    state.about_open = true;
                    ui.close_menu();
                }
                ui.separator();
                ui.label(RichText::new("Quick Reference").strong());
                ui.label("F1–F12: Canned messages");
                ui.label("Enter: Send chat message");
                ui.label("Ctrl+Enter: Send (multiline mode)");
                ui.label("Ctrl+F: Search chat");
                ui.label("Escape: Close dialogs");
                ui.separator();
                ui.label(RichText::new("Tips").small().color(GRAY));
                ui.label(RichText::new("Double-click CQ/beacon entry to connect").small());
                ui.label(RichText::new("Right-click entries for context menus").small());
                ui.label(RichText::new("Click QSO in log to add notes").small());
            });

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let clock = if state.use_local_time {
                    Local::now().format("%H:%M:%S LOC").to_string()
                } else {
                    Utc::now().format("%H:%M:%S UTC").to_string()
                };
                ui.label(RichText::new(clock).monospace().color(theme::MUTED));
                ui.add_space(6.0);
                theme::pill(ui, &state.my_callsign, theme::ACCENT);
                if state.unread_vmails > 0 {
                    if theme::pill(ui, format!("{} VMAIL", state.unread_vmails), theme::YELLOW)
                        .interact(egui::Sense::click())
                        .on_hover_text("Unread VMail")
                        .clicked()
                    {
                        state.compose_open = false;
                    }
                }
            });
        });
    });
}

/// The callsign's country, from a prefix table matching VarAC's.
fn dxcc_entity(callsign: &str) -> Option<&'static str> {
    crate::varac::countries::country_of(callsign.split('-').next().unwrap_or(callsign))
}

fn callsign_color(callsign: &str) -> Color32 {
    let hash: u32 = callsign.bytes().fold(5381_u32, |h, b| h.wrapping_mul(33).wrapping_add(b as u32));
    let hue = (hash % 360) as f32;
    let s = 0.55_f32;
    let l = 0.65_f32;
    // HSL to RGB
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((hue / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = match (hue as u32) / 60 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    Color32::from_rgb(
        ((r + m) * 255.0) as u8,
        ((g + m) * 255.0) as u8,
        ((b + m) * 255.0) as u8,
    )
}

fn maidenhead_to_latlon(loc: &str) -> Option<(f64, f64)> {
    let b = loc.as_bytes();
    if b.len() < 4 { return None; }
    let lon = (b[0].wrapping_sub(b'A') as f64) * 20.0
        + (b[2].wrapping_sub(b'0') as f64) * 2.0
        + 1.0 - 180.0;
    let lat = (b[1].wrapping_sub(b'A') as f64) * 10.0
        + (b[3].wrapping_sub(b'0') as f64) * 1.0
        + 0.5 - 90.0;
    if (-180.0..=180.0).contains(&lon) && (-90.0..=90.0).contains(&lat) {
        Some((lat, lon))
    } else {
        None
    }
}


/// The band of a frequency, as VarAC works it out (whole-MHz windows).
pub(crate) fn freq_to_band(hz: u64) -> Option<&'static str> {
    messages::band_name(hz)
}

/// VarAC's distance and bearing (6-character locators only), in the unit chosen in
/// Settings → Appearance (distance unit as in VarAC, miles by default).
fn grid_distance_str(my_loc: &str, their_loc: &str) -> Option<String> {
    static KM: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    KM.store(crate::config::distance_in_km_cached(), std::sync::atomic::Ordering::Relaxed);
    messages::loc_distance_text(my_loc, their_loc, KM.load(std::sync::atomic::Ordering::Relaxed))
}

fn open_qrz_lookup(callsign: &str) {
    let base = callsign.split('-').next().unwrap_or(callsign);
    let url = format!("https://www.qrz.com/db/{}", base);
    let _ = open::that(&url);
}

/// VARA's "BITRATE (4)  88 bps TX" → (level, bps, transmitting).
fn parse_bitrate(s: &str) -> Option<(u8, u32, bool)> {
    let level = s.split_once('(')?.1.split_once(')')?.0.trim().parse().ok()?;
    let mut words = s.split_once(')')?.1.split_whitespace();
    let bps = words.next()?.parse().ok()?;
    let tx = s.trim_end().ends_with("TX");
    Some((level, bps, tx))
}

/// Our lines a frame carries (one message, or up to three joined by CRLF; a reply has its
/// <C:n> prefix), newest matching line for each part.
fn our_lines_in_frame(lines: &std::collections::VecDeque<ChatLine>, text: &str, want: impl Fn(&ChatLine) -> bool) -> Vec<usize> {
    let body = text.split_once('>').filter(|_| text.starts_with("<C:")).map_or(text, |(_, b)| b).replace("//n", "\n");
    let whole = body.replace("\r\n", "\n");
    let parts: Vec<String> = if lines.iter().any(|l| l.is_outgoing && want(l) && l.text == whole) {
        vec![whole]
    } else {
        body.split("\r\n").map(str::to_string).collect()
    };
    parts.iter().filter_map(|p| lines.iter().rposition(|l| l.is_outgoing && want(l) && &l.text == p)).collect()
}

/// Under one of our lines: where it is on its way out. Queued: a slowly drifting dashed
/// track. On the air: a glowing bar that fills as the modem's buffer empties (moving on
/// smoothly between its reports at the current speed), with speed and time left. Sent: the
/// bar settles, fades and leaves a check mark.
fn draw_tx_progress(ui: &mut egui::Ui, stage: state::TxStage, state: &GuiState) {
    use state::TxStage;
    let now = std::time::Instant::now();
    let t = ui.input(|i| i.time) as f32;
    let w = ui.available_width().max(120.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, 5.0), egui::Sense::hover());
    let p = ui.painter_at(rect.expand(3.0));
    let track = egui::Rounding::same(3.0);
    p.rect_filled(rect, track, theme::tint(theme::MUTED, 28));
    let caption = |ui: &mut egui::Ui, text: String, color: Color32| {
        ui.label(RichText::new(text).size(10.5).color(color));
    };
    match stage {
        TxStage::Queued => {
            // dashes drifting to the right: waiting its turn
            let dash = 7.0;
            let off = (t * 14.0) % (dash * 2.0);
            let mut x = rect.left() - dash * 2.0 + off;
            while x < rect.right() {
                let a = x.max(rect.left());
                let b = (x + dash).min(rect.right());
                if b > a {
                    p.rect_filled(egui::Rect::from_min_max(egui::pos2(a, rect.top()), egui::pos2(b, rect.bottom())), track, theme::tint(theme::MUTED, 90));
                }
                x += dash * 2.0;
            }
            caption(ui, "in queue · waiting to send".into(), theme::MUTED);
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(40));
        }
        TxStage::OnAir { total, since } => {
            let total = total.max(1) as f32;
            let bps = state.tx_speed.map(|(_, b)| b).filter(|b| *b > 0);
            // the modem's last report, run on at the current speed (never to 100% before it
            // says the buffer is empty)
            let reported = match state.buffer_at {
                Some(at) if at >= since => state.buffer_bytes as f32,
                _ => total,
            };
            let since_report = state.buffer_at.filter(|at| *at >= since).map_or(now - since, |at| now - at).as_secs_f32();
            let left = match bps {
                Some(b) => (reported - since_report * b as f32 / 8.0).max(reported.min(1.0)),
                None => reported,
            }
            .min(total);
            let frac = (1.0 - left / total).clamp(0.0, 1.0);
            let fill = egui::Rect::from_min_max(rect.min, egui::pos2(rect.left() + rect.width() * frac.max(0.02), rect.bottom()));
            // glow, then a two-colour fill, then a sweep of light across it
            p.rect_filled(fill.expand(2.0), egui::Rounding::same(5.0), theme::tint(theme::ACCENT, 40));
            let mesh = {
                let mut m = egui::Mesh::default();
                let (c0, c1) = (theme::ACCENT, theme::PURPLE);
                m.colored_vertex(fill.left_top(), c0);
                m.colored_vertex(fill.right_top(), c1);
                m.colored_vertex(fill.right_bottom(), c1);
                m.colored_vertex(fill.left_bottom(), c0);
                m.add_triangle(0, 1, 2);
                m.add_triangle(0, 2, 3);
                m
            };
            p.add(egui::Shape::mesh(mesh));
            let sweep_x = fill.left() + (t * 0.9).fract() * fill.width();
            let sweep = egui::Rect::from_center_size(egui::pos2(sweep_x, fill.center().y), egui::vec2(14.0, fill.height()));
            p.rect_filled(sweep.intersect(fill), track, Color32::from_white_alpha(70));
            let mut text = format!("on the air · {:.0}%", frac * 100.0);
            if let Some((level, b)) = state.tx_speed {
                text += &format!(" · {b} bps (level {level})");
            }
            if let Some(b) = bps {
                text += &format!(" · ~{:.0} s", (left * 8.0 / b as f32).ceil());
            }
            caption(ui, text, theme::ACCENT);
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(30));
        }
        TxStage::Sent { at } => {
            // the full bar fades over a second and a half, leaving the check
            let age = (now - at).as_secs_f32();
            if age < 1.5 {
                let a = ((1.0 - age / 1.5) * 200.0) as u8;
                p.rect_filled(rect, track, theme::tint(theme::GREEN, a));
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(30));
            }
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                let c = r.center();
                ui.painter().add(egui::Shape::line(
                    vec![egui::pos2(c.x - 4.0, c.y), egui::pos2(c.x - 1.0, c.y + 3.0), egui::pos2(c.x + 4.5, c.y - 3.5)],
                    Stroke::new(1.6_f32, theme::GREEN),
                ));
                caption(ui, "sent".into(), theme::GREEN);
            });
        }
        TxStage::Unsent => {
            p.rect_filled(rect, track, theme::tint(theme::RED, 60));
            caption(ui, "not sent: the link ended first".into(), theme::RED);
        }
    }
}

fn draw_chat_area(ui: &mut egui::Ui, state: &mut GuiState) {
    let frame = theme::card().inner_margin(Margin::same(10.0));

    frame.show(ui, |ui| {
        ui.set_min_size(ui.available_size().max(Vec2::ZERO));
        // Header
        ui.horizontal(|ui| {
            theme::caption(ui, "Chat");
            if let (Some(t), Some(call)) = (state.partner_typing_at, state.connected_to.as_ref()) {
                // VarAC: up to 60 s after the last &IT& (the partner sends one a minute at most),
                // gone as soon as their message comes
                if (Utc::now() - t).num_seconds() < 60 {
                    ui.label(RichText::new(format!("{call} is typing…")).small().italics().color(theme::MUTED));
                }
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                // Search toggle
                if ui.small_button(if state.chat_search_active { "X" } else { "Search" }).clicked() {
                    state.chat_search_active = !state.chat_search_active;
                    if !state.chat_search_active {
                        state.chat_search.clear();
                    }
                }
                // VarAC's one-click QSY up / down a slot (<QSYU> / <QSYD>)
                if state.session_state == SessionState::Connected {
                    let step = state.slot_step() as i32;
                    if ui.small_button("QSY ▼").on_hover_text(format!("Invite your partner one slot down ({step} Hz, <QSYD>)")).clicked() {
                        state.send_cmd(GuiCommand::QsyStep { up: false, step_hz: step });
                    }
                    if ui.small_button("QSY ▲").on_hover_text(format!("Invite your partner one slot up ({step} Hz, <QSYU>)")).clicked() {
                        state.send_cmd(GuiCommand::QsyStep { up: true, step_hz: step });
                    }
                }
                if state.chat_search_active {
                    ui.add(TextEdit::singleline(&mut state.chat_search)
                        .desired_width(120.0)
                        .hint_text("Filter..."));
                }
                if !state.chat_lines.is_empty() {
                    if ui.small_button(RichText::new("Clear").color(GRAY).small()).clicked() {
                        state.chat_lines.clear();
                    }
                }

                // Connected indicator with duration
                if let Some(ref call) = state.connected_to {
                    let duration_str = state.connection_start
                        .map(|start| {
                            let secs = (Utc::now() - start).num_seconds().max(0);
                            format!("  {}:{:02}", secs / 60, secs % 60)
                        })
                        .unwrap_or_default();
                    let loc_str = state.partner_locator.as_ref().map(|l| format!(" ({l})")).unwrap_or_default();
                    let partner_info = match (&state.partner_name, &state.partner_qth) {
                        (Some(name), Some(qth)) => format!(" [{name}, {qth}{loc_str}]"),
                        (Some(name), None) => format!(" [{name}{loc_str}]"),
                        (None, Some(qth)) => format!(" [{qth}{loc_str}]"),
                        (None, None) if state.partner_locator.is_some() => format!(" [{loc_str}]"),
                        (None, None) => String::new(),
                    };
                    let svc_str = state.partner_services
                        .map(|s| {
                            let letters = s.to_letters();
                            if letters.is_empty() { String::new() } else { format!(" ({letters})") }
                        })
                        .unwrap_or_default();
                    let dist_str = state.partner_locator.as_ref()
                        .and_then(|their| {
                            let my = &state.settings_form.locator;
                            if my.len() >= 4 { grid_distance_str(my, their) } else { None }
                        })
                        .map(|d| format!(" [{d}]"))
                        .unwrap_or_default();
                    let dxcc_str = dxcc_entity(call).map(|e| format!(" [{e}]")).unwrap_or_default();
                    ui.colored_label(GREEN, RichText::new(format!("In QSO with: {call}{partner_info}{svc_str}{dist_str}{dxcc_str}{duration_str}")).strong());
                    if state.snr_samples.len() >= 2 {
                        let spark = snr_sparkline(&state.snr_samples);
                        ui.colored_label(CYAN, RichText::new(format!(" SNR {spark}")).small());
                    }
                } else if let Some(start) = state.connect_started {
                    let secs = (Utc::now() - start).num_seconds().max(0);
                    ui.colored_label(YELLOW, RichText::new(format!("CONNECTING... {secs}s")).strong());
                } else if state.session_state == SessionState::Idle {
                    if let Some(ref last_call) = state.last_connect_callsign.clone() {
                        if state.modem_connected {
                            if ui.small_button(RichText::new(format!("Retry {last_call}")).color(YELLOW)).clicked() {
                                state.connect_started = Some(Utc::now());
                                state.send_cmd(GuiCommand::Connect(last_call.clone()));
                            }
                        }
                    }
                }
            });
        });
        ui.separator();

        let search_lower = state.chat_search.to_lowercase();

        ScrollArea::vertical()
            .auto_shrink([false; 2])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                if state.chat_lines.is_empty() {
                    ui.add_space(ui.available_height() * 0.35);
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new("No messages yet").size(16.0).color(theme::MUTED));
                        ui.label(RichText::new("Connect to a station or answer a CQ to start chatting · /help for commands")
                            .small().color(theme::tint(theme::MUTED, 160)));
                    });
                }
                // VarAC's Reply / Reaction cells: (line index, like?) or (chat number, quote)
                let can_react = state.session_state == SessionState::Connected;
                let mut like_click: Option<(usize, u32, bool)> = None;
                let mut reply_click: Option<(u32, String)> = None;
                for (idx, line) in state.chat_lines.iter().enumerate() {
                    if !search_lower.is_empty() {
                        let matches = line.text.to_lowercase().contains(&search_lower)
                            || line.callsign.to_lowercase().contains(&search_lower);
                        if !matches {
                            continue;
                        }
                    }

                    let resp = if line.is_system {
                        ui.vertical_centered(|ui| {
                            Frame::none()
                                .fill(theme::tint(theme::MUTED, 18))
                                .rounding(egui::Rounding::same(999.0))
                                .inner_margin(Margin::symmetric(12.0, 3.0))
                                .show(ui, |ui| {
                                    ui.label(RichText::new(format!("{}  ·  {}", line.time, line.text)).small().color(theme::MUTED));
                                })
                                .response
                        }).inner
                    } else {
                        let out = line.is_outgoing;
                        let call_color = if out { theme::ACCENT } else { callsign_color(&line.callsign) };
                        let max_w = (ui.available_width() * 0.75).max(220.0);
                        let layout = if out { Layout::right_to_left(Align::TOP) } else { Layout::left_to_right(Align::TOP) };
                        let (tail_l, tail_r) = if out { (12.0, 3.0) } else { (3.0, 12.0) };
                        ui.with_layout(layout, |ui| {
                            let bubble = Frame::none()
                                .fill(if out { theme::tint(theme::ACCENT, 26) } else { theme::SURFACE_2 })
                                .stroke(Stroke::new(1.0_f32, if out { theme::tint(theme::ACCENT, 70) } else { theme::BORDER }))
                                .rounding(egui::Rounding { nw: 12.0, ne: 12.0, sw: tail_l, se: tail_r })
                                .inner_margin(Margin::symmetric(12.0, 7.0))
                                .show(ui, |ui| {
                                    // Size the bubble to its text (right-to-left layouts would otherwise stretch it)
                                    let body = egui::TextStyle::Body.resolve(ui.style());
                                    let text_w = ui.fonts(|f| {
                                        f.layout_no_wrap(line.text.clone(), body, Color32::WHITE).size().x
                                    });
                                    let bubble_w = (text_w + 2.0).clamp(140.0, max_w);
                                    ui.set_min_width(bubble_w);
                                    ui.set_max_width(bubble_w);
                                    ui.vertical(|ui| {
                                        ui.spacing_mut().item_spacing.y = 2.0;
                                        ui.horizontal(|ui| {
                                            if line.reply_to.is_some() {
                                                ui.label(RichText::new("\u{21B3}").small().color(theme::MUTED));
                                            }
                                            ui.label(RichText::new(&line.callsign).strong().color(call_color));
                                            ui.label(RichText::new(&line.time).small().color(theme::MUTED));
                                        });
                                        if let Some(q) = &line.reply_quote {
                                            ui.label(RichText::new(format!(">>>> REPLIED TO: {q}")).small().italics().color(theme::MUTED));
                                        }
                                        // (our line is faint until it's out, then solid)
                                        let pending = matches!(line.tx, Some(state::TxStage::Queued | state::TxStage::OnAir { .. }));
                                        let rt = match line.alert {
                                            Some(((fr, fg, fb), (br, bg, bb))) => RichText::new(&line.text)
                                                .color(Color32::from_rgb(fr, fg, fb)).background_color(Color32::from_rgb(br, bg, bb)),
                                            None if pending => RichText::new(&line.text).color(theme::tint(theme::TEXT, 140)),
                                            None => RichText::new(&line.text).color(theme::TEXT),
                                        };
                                        ui.add(egui::Label::new(rt).wrap());
                                        if let Some(stage) = line.tx {
                                            ui.add_space(3.0);
                                            draw_tx_progress(ui, stage, state);
                                        }
                                    });
                                })
                                .response;
                            // Reply / like beside the bubble (inside it the bubble's own click
                            // area sits on top and swallows them)
                            if let Some(id) = line.chat_id.filter(|_| can_react) {
                                ui.vertical(|ui| {
                                    ui.spacing_mut().item_spacing.y = 2.0;
                                    if ui.small_button("↩").on_hover_text("Reply to this message").clicked() {
                                        let quote: String = line.text.replace('\n', " ").chars().take(50).collect();
                                        reply_click = Some((id, quote));
                                    }
                                    // Likes go on the partner's lines; ours show the partner's like
                                    if !out {
                                        let (heart, tip) = if line.liked { ("❤", "Unlike") } else { ("♡", "Like") };
                                        if ui.small_button(RichText::new(heart).color(theme::RED)).on_hover_text(tip).clicked() {
                                            like_click = Some((idx, id, !line.liked));
                                        }
                                    }
                                });
                            }
                            if line.liked && (out || line.chat_id.is_none() || !can_react) {
                                let tip = if out { "Your partner liked this" } else { "You liked this" };
                                ui.label(RichText::new("❤").color(theme::RED)).on_hover_text(tip);
                            }
                            bubble
                        }).inner
                    }
                    .interact(egui::Sense::click());
                    ui.add_space(2.0);

                    let copy_text = format!("[{}] {}: {}", line.time, line.callsign, line.text);
                    let copy_call = line.callsign.clone();
                    let quote_text = format!("> {}: {}\n", line.callsign, line.text);
                    let bm_time = line.time.clone();
                    let bm_call = line.callsign.clone();
                    let bm_text = line.text.clone();
                    let is_user_msg = !line.callsign.is_empty() && !line.is_system;
                    resp.context_menu(|ui| {
                        if ui.button("Copy message").clicked() {
                            ui.output_mut(|o| o.copied_text = copy_text.clone());
                            ui.close_menu();
                        }
                        if is_user_msg {
                            if ui.button("QSO log: set Name/QTH/LOC…").clicked() {
                                state.qso_log_pick = Some(bm_text.clone());
                                ui.close_menu();
                            }
                            if ui.button("Quote in chat").clicked() {
                                state.input_text.push_str(&quote_text);
                                ui.close_menu();
                            }
                            if ui.button("Bookmark").clicked() {
                                state.bookmarked_messages.push(BookmarkedMessage {
                                    time: bm_time.clone(),
                                    callsign: bm_call.clone(),
                                    text: bm_text.clone(),
                                });
                                ui.close_menu();
                            }
                            if ui.button(format!("Copy callsign ({copy_call})")).clicked() {
                                ui.output_mut(|o| o.copied_text = copy_call.clone());
                                ui.close_menu();
                            }
                            if ui.button(format!("Lookup {copy_call} on QRZ")).clicked() {
                                open_qrz_lookup(&copy_call);
                                ui.close_menu();
                            }
                        }
                    });
                }
                if let Some((idx, id, like)) = like_click {
                    if let Some(l) = state.chat_lines.get_mut(idx) {
                        l.liked = like;
                    }
                    state.send_cmd(GuiCommand::SendReaction { id, like });
                }
                if reply_click.is_some() {
                    state.reply_to_chat = reply_click;
                }
            });
    });
}

fn handle_chat_slash_command(state: &mut GuiState, text: &str) {
    let parts: Vec<&str> = text.splitn(2, ' ').collect();
    let cmd = parts[0].to_lowercase();
    let arg = parts.get(1).map(|s| s.trim()).unwrap_or("");

    match cmd.as_str() {
        "/connect" if !arg.is_empty() => {
            if state.modem_connected {
                state.send_cmd(GuiCommand::Connect(arg.to_uppercase()));
                state.connect_started = Some(Utc::now());
                state.add_log(format!("Connecting to {}", arg.to_uppercase()));
            } else {
                state.status_msg = "Modem not connected".into();
            }
        }
        "/disconnect" | "/disc" => {
            state.send_cmd(GuiCommand::Disconnect);
        }
        "/cq" => {
            // /cq [SLOT]: a plain CQ, or one naming a VarAC slot (then QSY there)
            if state.modem_connected {
                let slot: u8 = arg.trim().parse().unwrap_or(0);
                state.call_cq_on_slot(slot);
            }
        }
        "/clear" => {
            state.chat_lines.clear();
        }
        "/freq" => {
            let band = &state.current_band;
            let freq_mhz = state.current_frequency as f64 / 1_000_000.0;
            state.add_system_chat(format!("Current: {band} — {freq_mhz:.3} MHz"));
        }
        "/away" => {
            if !state.away_active {
                let msg = if arg.is_empty() { "Away from keyboard" } else { arg };
                state.away_active = true;
                state.away_message = msg.to_string();
                state.send_cmd(GuiCommand::SetAway(Some(msg.to_string())));
                state.add_system_chat(format!("Away: {msg}"));
            } else {
                state.away_active = false;
                state.send_cmd(GuiCommand::SetAway(None));
                state.add_system_chat("Away mode off".into());
            }
        }
        "/bookmarks" | "/bm" => {
            state.bookmarks_open = true;
        }
        "/help" => {
            state.add_system_chat("Commands: /connect CALL, /disconnect, /cq, /clear, /freq, /away [msg], /bookmarks, /help. The full guide: Help → User guide.".into());
        }
        _ => {
            state.status_msg = format!("Unknown command: {cmd}");
        }
    }
}

/// What the compose window's buttons do.
#[derive(Clone, Copy, PartialEq)]
enum ComposeAction {
    Send,
    SendAndDisconnect,
    SaveDraft,
}

/// VarAC's compose window: To takes several callsigns separated by commas
/// (one VMail each), subject up to 50 characters, the body up to 500 (10000 at
/// 2300/2750 Hz while linked, or in EmComm); SEND, SEND AND DISCONNECT, SAVE TO DRAFTS,
/// CANCEL ("Are you sure?"). An Outbox VMail being edited keeps its To and says UPDATE.
fn draw_compose_window(ui: &mut egui::Ui, state: &mut GuiState, db: &Storage) {
    let connected = state.session_state == SessionState::Connected;
    let wide = matches!(state.settings_form.bandwidth.trim(), "2300" | "2750");
    let body_limit = if state.settings_form.emcomm_mode || (wide && connected) { 10_000 } else { 500 };
    let editing_outbox = state.compose_editing.as_ref().is_some_and(|(_, outbox)| *outbox);
    let mut action = None;
    Frame::default()
        .fill(theme::SURFACE_2).rounding(theme::RADIUS)
        .inner_margin(Margin::same(8.0))
        .stroke(Stroke::new(2.0_f32, YELLOW))
        .show(ui, |ui| {
            let title = if state.compose_editing.is_some() { "Edit VMail" } else { "Compose VMail" };
            ui.colored_label(YELLOW, RichText::new(title).strong());
            ui.separator();
            ui.horizontal(|ui| {
                ui.label("To:");
                ui.add_enabled(!editing_outbox, TextEdit::singleline(&mut state.compose_to).desired_width(220.0)
                    .hint_text("CALLSIGN (comma for several) or email"));
            });
            ui.horizontal(|ui| {
                ui.label("Subject:");
                ui.add(TextEdit::singleline(&mut state.compose_subject).desired_width(300.0).hint_text("Subject"));
                truncate_chars(&mut state.compose_subject, 50);
                ui.label(RichText::new(format!("{}/50", state.compose_subject.chars().count())).small().color(GRAY));
            });
            ui.horizontal(|ui| {
                ui.label("Body:");
                ui.label(RichText::new(format!("{}/{body_limit}", state.compose_body.chars().count())).small().color(GRAY));
            });
            ui.add(TextEdit::multiline(&mut state.compose_body).desired_width(ui.available_width()).desired_rows(3)
                .hint_text("Message body (tags like <NAME> <RIG> are filled in)"));
            truncate_chars(&mut state.compose_body, body_limit);
            ui.horizontal(|ui| {
                ui.checkbox(&mut state.compose_urgent, RichText::new("Urgent").color(RED));
                ui.separator();
                ui.menu_button("Templates", |ui| {
                    for (name, subject, body) in VMAIL_TEMPLATES {
                        if ui.button(*name).clicked() {
                            state.compose_subject = subject.to_string();
                            state.compose_body = body.replace("[NL]", "\n");
                            ui.close_menu();
                        }
                    }
                });
            });
            let is_email = state.compose_to.contains('@') && state.compose_to.contains('.');
            let recipients = compose_recipients(&state.compose_to);
            if !state.compose_to.trim().is_empty() && recipients.is_none() {
                ui.colored_label(RED, RichText::new("Not a valid callsign (use commas between several) or email").small());
            }
            if is_email {
                ui.horizontal(|ui| {
                    ui.label("Reply to:");
                    ui.add(TextEdit::singleline(&mut state.compose_reply_to).desired_width(240.0)
                        .hint_text("empty = back through the email gateway"));
                });
                ui.checkbox(&mut state.compose_raw, "Raw email (no gateway footer, e.g. for Saildocs)");
            }
            let reply_to_ok = !is_email || state.compose_reply_to.trim().is_empty()
                || state.compose_reply_to.trim().parse::<lettre::Address>().is_ok();
            if !reply_to_ok {
                ui.colored_label(RED, RichText::new("Not a valid reply-to email address").small());
            }
            // Attachments: VarAC zips them and sends them during a QSO with the addressee
            ui.horizontal_wrapped(|ui| {
                if ui.button("Attach file…").clicked() {
                    if let Some(paths) = rfd::FileDialog::new().pick_files() {
                        state.compose_attachments.extend(paths);
                    }
                }
                let mut remove = None;
                for (i, p) in state.compose_attachments.iter().enumerate() {
                    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    if ui.small_button(format!("{name}  ×")).on_hover_text("Remove").clicked() {
                        remove = Some(i);
                    }
                }
                if let Some(i) = remove {
                    state.compose_attachments.remove(i);
                }
            });
            ui.horizontal(|ui| {
                let can_send = recipients.is_some() && reply_to_ok;
                let send_label = if editing_outbox { "UPDATE" } else { "SEND" };
                if ui.add_enabled(can_send, egui::Button::new(RichText::new(send_label).color(GREEN).strong())).clicked() {
                    action = Some(ComposeAction::Send);
                }
                if connected && !editing_outbox
                    && ui.add_enabled(can_send, egui::Button::new(RichText::new("SEND AND DISCONNECT").color(GREEN)))
                        .on_hover_text("Once a receipt confirmation is received, the link will be closed automatically.")
                        .clicked()
                {
                    action = Some(ComposeAction::SendAndDisconnect);
                }
                if !editing_outbox && ui.button("SAVE TO DRAFTS").clicked() {
                    action = Some(ComposeAction::SaveDraft);
                }
                if ui.add(theme::accent_button("CANCEL", RED)).clicked() {
                    if state.compose_subject.is_empty() && state.compose_body.is_empty() {
                        clear_compose(state);
                    } else {
                        state.compose_cancel_confirm = true;
                    }
                }
            });
            if state.compose_cancel_confirm {
                ui.separator();
                ui.label(RichText::new("You have started composing a VMail. Canceling now will discard everything you've written so far.").color(RED));
                ui.horizontal(|ui| {
                    ui.label("Are you sure you want to cancel this VMail?");
                    if ui.button("YES").clicked() {
                        clear_compose(state);
                    }
                    if ui.button("NO").clicked() {
                        state.compose_cancel_confirm = false;
                    }
                });
            }
        });
    if let Some(action) = action {
        submit_compose(state, db, action);
    }
}

fn truncate_chars(s: &mut String, max: usize) {
    if s.chars().count() > max {
        *s = s.chars().take(max).collect();
    }
}

/// The addressees in the To field: one email address, or callsigns separated by commas
/// (VarAC; spaces ignored). None when one of them isn't valid.
fn compose_recipients(to: &str) -> Option<Vec<String>> {
    let to = to.trim();
    if to.contains('@') {
        return (to.contains('.') && !to.contains(',')).then(|| vec![to.to_string()]);
    }
    let calls: Vec<String> = to.split(',').map(|c| c.replace(' ', "").to_uppercase()).collect();
    (!to.is_empty() && calls.iter().all(|c| messages::varac_callsign_ok(c))).then_some(calls)
}

fn clear_compose(state: &mut GuiState) {
    state.compose_to.clear();
    state.compose_subject.clear();
    state.compose_body.clear();
    state.compose_urgent = false;
    state.compose_park = false;
    state.compose_reply_to.clear();
    state.compose_raw = false;
    state.compose_attachments.clear();
    state.compose_editing = None;
    state.compose_cancel_confirm = false;
    state.compose_open = false;
}

/// SEND / SEND AND DISCONNECT / SAVE TO DRAFTS, the VarAC way: one Outbox VMail per
/// addressee (an edited one replaces the old row). Connected to the addressee: it goes now
/// (off the calling frequency); connected to someone else: VarAC asks whether to relay
/// it through them; not connected: it waits in the Outbox ("VMail will be queued").
fn submit_compose(state: &mut GuiState, db: &Storage, action: ComposeAction) {
    let Some(recipients) = compose_recipients(&state.compose_to) else { return };
    let is_email = recipients.first().is_some_and(|r| r.contains('@'));
    if let Some((old, _)) = state.compose_editing.take() {
        let _ = db.delete_vmail_row(&old);
    }
    let subject = state.expand_tags(&state.compose_subject);
    let subject = if subject.trim().is_empty() { "(no subject)".to_string() } else { subject };
    let body = state.expand_tags(&state.compose_body);
    let folder = if action == ComposeAction::SaveDraft { crate::storage::VMailFolder::Drafts } else { crate::storage::VMailFolder::Outbox };
    let rows: Vec<String> = if action == ComposeAction::SaveDraft { vec![state.compose_to.trim().to_uppercase()] } else { recipients.clone() };
    let mut guids = Vec::new();
    for to in &rows {
        let guid = uuid::Uuid::new_v4().to_string();
        let _ = db.insert_vmail(&crate::storage::VMailRecord {
            guid: guid.clone(),
            creation_time: Utc::now(),
            sent_time: None,
            received_time: None,
            folder_id: folder as i32,
            vmail_to: to.clone(),
            vmail_from: state.my_callsign.clone(),
            vmail_via: None,
            subject: subject.clone(),
            msg: body.clone(),
            read_status: true,
            is_deleted: false,
            urgent: state.compose_urgent,
            delivery_band: None,
            delivery_snr: None,
            frequency: None,
        });
        for p in &state.compose_attachments {
            let _ = db.add_vmail_attachment(&guid, p);
        }
        if is_email {
            let reply_to = Some(state.compose_reply_to.trim().to_string()).filter(|r| !r.is_empty());
            let _ = db.set_vmail_email(&guid, reply_to.as_deref(), state.compose_raw);
        }
        guids.push(guid);
    }
    let to_text = recipients.join(", ");
    clear_compose(state);
    refresh_all_vmail_lists(state, db);
    if action == ComposeAction::SaveDraft {
        state.status_msg = "VMail saved to Drafts".into();
        return;
    }
    let disconnect_after = action == ComposeAction::SendAndDisconnect;
    let partner = state.connected_to.clone().filter(|_| state.session_state == SessionState::Connected);
    let Some(partner) = partner else {
        state.status_msg = format!("VMail to {to_text} queued in the Outbox");
        state.vmail_queued_popup = !crate::config::vmail_popups_disabled().0;
        return;
    };
    let partner_base = base_callsign(&partner);
    if is_email {
        // A gateway takes it now; through any other station, VarAC asks first
        if state.partner_services.is_some_and(|s| s.has_email()) {
            state.send_cmd(GuiCommand::FlushEmailOutbox);
        } else {
            state.relay_prompt = Some(state::RelayPrompt {
                guids, to: to_text, via: partner, email: true,
                until: Utc::now() + chrono::Duration::seconds(30), disconnect_after,
            });
        }
        return;
    }
    let (direct, other): (Vec<_>, Vec<_>) = guids.into_iter().zip(&recipients)
        .partition(|(_, to)| base_callsign(to) == partner_base);
    if !direct.is_empty() {
        // For the partner itself: now, or after the QSY (VarAC's outbox loop)
        state.send_cmd(GuiCommand::RelayNow {
            guids: direct.into_iter().map(|(g, _)| g).collect(), check_cf: true, quiet: true, disconnect_after,
        });
        state.status_msg = format!("VMail to {partner_base} on its way");
    }
    if !other.is_empty() {
        let to = other.iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>().join(", ");
        state.relay_prompt = Some(state::RelayPrompt {
            guids: other.into_iter().map(|(g, _)| g).collect(), to, via: partner, email: false,
            until: Utc::now() + chrono::Duration::seconds(30), disconnect_after,
        });
    }
}

/// VarAC's "Sending Vmail to a different callsign" question (30 s, then the VMail stays
/// queued), its "VMail will be queued" note and "Leave a message" offer.
fn draw_vmail_popups(ctx: &egui::Context, state: &mut GuiState) {
    if let Some(p) = &state.relay_prompt {
        let left = (p.until - Utc::now()).num_seconds();
        let (title, text) = if p.email {
            ("Sending Vmail to an Email through VarAC Email Gateway", format!(
                "You are sending a Vmail to: {}\nBut you are currently connected to: {}\n\nDo you want to relay this VMail through {}? Make sure they support an Email gateway.\n\nClick YES to relay now.\n\nClick NO to queue this VMail in the outbox to be relayed through other stations who operates a VarAC Email gateway.",
                p.to, p.via, p.via))
        } else {
            ("Sending Vmail to a different callsign", format!(
                "You are sending a Vmail to: {}\nBut you are currently connected to: {}\n\nDo you want to relay this VMail through {}?\n\nClick YES to relay now.\n\nClick NO to queue this VMail in the outbox until connected with {}",
                p.to, p.via, p.via, p.to))
        };
        let mut answer = (left < 0 || state.session_state != SessionState::Connected).then_some(false);
        egui::Window::new(title).collapsible(false).resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(text);
                ui.horizontal(|ui| {
                    if ui.add(theme::accent_button("YES", GREEN)).clicked() {
                        answer = Some(true);
                    }
                    if ui.button(format!("NO ({})", left.max(0))).clicked() {
                        answer = Some(false);
                    }
                });
            });
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
        if let Some(yes) = answer {
            let p = state.relay_prompt.take().expect("prompt");
            if yes {
                state.send_cmd(GuiCommand::RelayNow { guids: p.guids, check_cf: true, quiet: false, disconnect_after: p.disconnect_after });
            } else {
                state.status_msg = format!("VMail to {} stays in the Outbox", p.to);
            }
        }
    }
    if state.vmail_queued_popup {
        let mut close = false;
        let mut dont_show = false;
        egui::Window::new("VMail will be queued").collapsible(false).resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label("You are not currently connected to anyone.\n\nYour VMail is waiting to be sent in your OUTBOX folder. Once you connect to the destination station, it will be automatically forwarded.");
                ui.horizontal(|ui| {
                    dont_show = ui.button("Do not show this again").clicked();
                    close = ui.button("CLOSE").clicked();
                });
            });
        if dont_show {
            crate::config::update_config_file(|c| c.disable_vmail_queued_popup = Some(true));
        }
        if close || dont_show {
            state.vmail_queued_popup = false;
        }
    }
    if let Some(partner) = state.leave_message_popup.clone() {
        let mut answer = None;
        egui::Window::new("Leave a message").collapsible(false).resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label("Your partner seems to be away...\n\nWould you like to leave a message in the mailbox?");
                ui.horizontal(|ui| {
                    if ui.add(theme::accent_button("LEAVE A MESSAGE", YELLOW)).clicked() {
                        answer = Some(1);
                    }
                    if ui.button("CLOSE").clicked() {
                        answer = Some(0);
                    }
                    if ui.button("Do not show this again").clicked() {
                        answer = Some(2);
                    }
                });
            });
        match answer {
            Some(1) => {
                state.leave_message_popup = None;
                state.compose_to = partner;
                state.compose_open = true;
            }
            Some(2) => {
                state.leave_message_popup = None;
                crate::config::update_config_file(|c| c.disable_leave_vmail_popup = Some(true));
            }
            Some(_) => state.leave_message_popup = None,
            None => {}
        }
    }
}

/// VarAC's reply: "RE: subject", To = the sender, urgency kept, and
/// the body starts with "SENDER> " + the first 50 characters of the original (after
/// its own quote, if it has one) and a "---" line.
fn open_reply(state: &mut GuiState, db: &Storage, guid: &str) {
    let Some(vm) = db.vmail_by_guid(guid).ok().flatten() else { return };
    clear_compose(state);
    state.compose_to = vm.vmail_from.clone();
    state.compose_subject = if vm.subject.starts_with("RE: ") { vm.subject.clone() } else { format!("RE: {}", vm.subject) };
    truncate_chars(&mut state.compose_subject, 50);
    state.compose_body = reply_quote(&vm.vmail_from, &vm.msg);
    state.compose_urgent = vm.urgent;
    state.compose_open = true;
}

/// VarAC's reply quote: "SENDER> " + the original (after its own "---", if any) on one
/// line, cut to 50 characters + "...", then a "---" line.
fn reply_quote(from: &str, original: &str) -> String {
    let original = original.split_once("---").map_or(original, |(_, rest)| rest);
    let flat = original.replace("\r\n", " ").replace('\n', " ").trim().to_string();
    let preview = if flat.chars().count() > 50 { format!("{}...", flat.chars().take(50).collect::<String>()) } else { flat };
    format!("{from}> {preview}\n---\n")
}

/// Edit an Outbox VMail or a draft (VarAC: the old one is replaced by a new one).
fn open_edit(state: &mut GuiState, db: &Storage, guid: &str) {
    let Some(vm) = db.vmail_by_guid(guid).ok().flatten() else { return };
    if vm.folder_id == crate::storage::VMailFolder::Sent as i32 {
        state.tx_alert = Some(("Can't edit VMails".into(), "This VMail cannot be edited as it has already been sent.".into()));
        return;
    }
    clear_compose(state);
    state.compose_to = vm.vmail_to.clone();
    state.compose_subject = vm.subject.clone();
    state.compose_body = vm.msg.clone();
    state.compose_urgent = vm.urgent;
    state.compose_attachments = db.vmail_attachments(guid).unwrap_or_default();
    if let Ok((reply_to, raw)) = db.vmail_email(guid) {
        state.compose_reply_to = reply_to.unwrap_or_default();
        state.compose_raw = raw;
    }
    let outbox = vm.folder_id == crate::storage::VMailFolder::Outbox as i32;
    state.compose_editing = Some((guid.to_string(), outbox));
    state.compose_open = true;
}

/// VarAC's "VMail Relay notifications": stations that told us they hold VMail for us.
/// CONNECT (VarAC: double-click) retunes to where the notice was heard and connects.
/// VarAC's "Bandwidth not allowed!" box, with a one-click switch to 500 Hz.
fn draw_bw_refused(ctx: &egui::Context, state: &mut GuiState) {
    if !state.bw_refused {
        return;
    }
    egui::Window::new("Bandwidth not allowed!")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.set_max_width(420.0);
            ui.label("You can't use a 2300/2750Hz Bandwidth on a calling QRG as it may cause QRM to nearby VarAC stations.\n\
                Please note that 500Hz provides great & fast chat experience.\n\n\
                PLEASE SWITCH YOUR BANDWIDTH TO 500HZ OR CHANGE FREQUENCY.");
            ui.horizontal(|ui| {
                if ui.button(RichText::new("Switch to 500 Hz").strong()).clicked() {
                    state.set_bandwidth("500");
                    state.bw_refused = false;
                }
                if ui.button("OK").clicked() {
                    state.bw_refused = false;
                }
            });
        });
}

/// VarAC's question before EmComm mode on a calling frequency.
fn draw_emcomm_confirm(ctx: &egui::Context, state: &mut GuiState) {
    if !state.emcomm_confirm {
        return;
    }
    egui::Window::new("PAY ATTENTION! EmComm on a calling frequency")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.set_max_width(460.0);
            ui.label("You're transitioning into Emergency Communications (EmComm) mode while on a designated calling frequency.\n\n\
                EmComm mode on the calling frequency is strictly reserved for genuine emergency situations or emergency drills.\n\n\
                Utilizing EmComm mode on the calling frequency, such as beaconing as an EmComm station, when not facing a real emergency or as part of a drill may lead to a temporary or permanent ban from the VarAC network.\n\n\
                Exercise discretion when employing EmComm mode.\n\n\
                For testing or practice, kindly move away from the calling frequency.\n\n\
                ARE YOU SURE YOU WANT TO TRANSITION TO EMCOMM MODE ON THE CALLING FREQUENCY?");
            ui.horizontal(|ui| {
                if ui.button("Yes").clicked() {
                    state.emcomm_confirm = false;
                    state.set_emcomm(true);
                }
                if ui.button("No").clicked() {
                    state.emcomm_confirm = false;
                }
            });
        });
}

fn draw_relay_notifications(ctx: &egui::Context, state: &mut GuiState, db: &Storage) {
    if !state.relay_notifications_open {
        return;
    }
    let mut open = true;
    let rows = db.relay_notifications().unwrap_or_default();
    let mut connect: Option<(String, i64)> = None;
    let mut delete: Option<String> = None;
    egui::Window::new("VMail Relay notifications").open(&mut open).default_width(560.0).show(ctx, |ui| {
        ui.label(RichText::new("The following stations notified that they have VMails waiting for you to collect. \
            CONNECT tunes to where the notification was heard and connects to collect them.").small().color(GRAY));
        ui.add_space(4.0);
        egui::Grid::new("relay_notifications").num_columns(6).striped(true).spacing([10.0, 4.0]).show(ui, |ui| {
            for h in ["Last notification", "Frequency", "Callsign", "Last heard", "Urgent", ""] {
                ui.label(RichText::new(h).small().strong());
            }
            ui.end_row();
            for (at, freq, call, urgent) in &rows {
                ui.label(at.format("%Y-%m-%d %H:%M").to_string());
                ui.label(if *freq > 0 { format!("{:.3}", *freq as f64 / 1e6) } else { "—".into() });
                ui.label(RichText::new(call).strong().color(GREEN));
                let heard = db.last_heard_callsign(call).ok().flatten()
                    .filter(|e| e.source != "QSO")
                    .map(|e| format!("{} {} hours ago on {}", if e.source == "BEACON" { "Beacon" } else { "CQ" },
                        (Utc::now() - e.time).num_hours(), e.band))
                    .unwrap_or_default();
                ui.label(RichText::new(heard).small());
                ui.label(if *urgent { RichText::new("⚑").color(RED) } else { RichText::new("") });
                ui.horizontal(|ui| {
                    if ui.small_button("CONNECT").clicked() {
                        connect = Some((call.clone(), *freq));
                    }
                    if ui.small_button("DELETE").clicked() {
                        delete = Some(call.clone());
                    }
                });
                ui.end_row();
            }
        });
        if rows.is_empty() {
            ui.label(RichText::new("No relay notifications").small().italics().color(GRAY));
        }
    });
    if let Some(call) = delete {
        let _ = db.clear_relay_notifications(&call);
        state.relay_notification_count = db.relay_notifications().map(|r| r.len()).unwrap_or(0);
    }
    if let Some((call, freq)) = connect {
        if state.session_state != SessionState::Idle {
            state.tx_alert = Some(("Operation can not be performed now".into(),
                "You are currently in connection or trying to connect station".into()));
        } else {
            if freq > 0 && freq as u64 != state.current_frequency {
                state.current_frequency = freq as u64;
                state.tune_to = Some(freq as u64);
            }
            state.add_log(format!("Collecting VMail: connecting to {call}"));
            state.connect_started = Some(Utc::now());
            state.last_connect_callsign = Some(call.clone());
            state.send_cmd(GuiCommand::Connect(call));
            open = false;
        }
    }
    state.relay_notifications_open = open && state.relay_notifications_open;
}

/// VarAC's "VMail relay alert" (closes itself after 30 s).
fn draw_vmail_relay_alert(ctx: &egui::Context, state: &mut GuiState) {
    let Some((partner, until)) = state.vmail_relay_alert.clone() else { return };
    let left = (until - Utc::now()).num_seconds();
    if left < 0 || state.connected_to.is_none() {
        state.vmail_relay_alert = None;
        return;
    }
    let mut close = false;
    egui::Window::new("VMail relay alert")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label(format!("You have VMails waiting to be relayed to {partner}."));
            ui.label(RichText::new("PLEASE INVITE YOUR QSO PARTNER TO QSY TO ALLOW THE RELAY.").strong());
            ui.label("After the QSY, the waiting VMail is relayed automatically.");
            ui.label(RichText::new("If the other station is AWAY, it is relayed on the calling frequency.").small().color(GRAY));
            if ui.button(format!("CLOSE ({left})")).clicked() {
                close = true;
            }
        });
    ctx.request_repaint_after(std::time::Duration::from_millis(500));
    if close {
        state.vmail_relay_alert = None;
    }
}

/// VarAC's "QSO log: Set as Name / QTH / Locator" on a chat line: trim the text to the
/// part you want, then set it in the QSO log row.
fn draw_qso_log_pick(ctx: &egui::Context, state: &mut GuiState) {
    let Some(mut text) = state.qso_log_pick.take() else { return };
    let mut open = true;
    let mut done = false;
    egui::Window::new("QSO log: set Name/QTH/LOC").open(&mut open).collapsible(false).resizable(true)
        .default_width(380.0)
        .show(ctx, |ui| {
            ui.label(RichText::new("Keep just the part you want, then:").small().color(GRAY));
            ui.add(TextEdit::multiline(&mut text).desired_width(360.0).desired_rows(2));
            ui.horizontal(|ui| {
                let v = text.trim().to_string();
                if ui.button("Set as Name").clicked() {
                    state.qso_log.name = v.clone();
                    done = true;
                }
                if ui.button("Set as QTH").clicked() {
                    state.qso_log.qth = v.clone();
                    done = true;
                }
                if ui.button("Set as Locator").clicked() {
                    state.qso_log.locator = v.replace('Ø', "0").to_uppercase();
                    done = true;
                }
            });
        });
    if open && !done {
        state.qso_log_pick = Some(text);
    }
}

/// VarAC's "QSY NOW" on both sides of an invitation, and its "Still waiting for
/// your partner to QSY" form. The default button counts down and decides by itself.
fn draw_qsy_popup(ctx: &egui::Context, state: &mut GuiState) {
    let Some(p) = state.qsy_popup.as_ref() else { return };
    let left = (p.until - Utc::now()).num_seconds();
    let (target, offset, default_accept, oor, auto, recovery) =
        (p.target_hz, p.offset_hz, p.default_accept, p.out_of_range, p.auto, p.recovery);
    let freq = crate::varac::messages::freq_dotted(target as i64);
    let partner = state.connected_to.clone().unwrap_or_else(|| "your partner".into());
    let mut answer = (left < 0).then_some(default_accept);
    let title = if recovery { "Still waiting for your partner to QSY" } else { "QSY NOW" };
    egui::Window::new(title).collapsible(false).resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            if recovery {
                ui.label(format!(
                    "Seems like {partner} has not accepted your QSY invitation yet.\nYou can continue to wait but it is most likely that the QSY invitation was rejected.\nThere are various reasons in which this can happen:\n\n1. {partner} has explicitly rejected your invitation.\n2. {partner} is away and has not enabled auto-qsy.\n3. The QSY frequency is not in the allowed auto-qsy range of {partner}\n\nQSY back to {freq} ?"
                ));
            } else {
                ui.label(RichText::new(format!("A QSY request to {freq} has been initiated.")).strong());
                if auto {
                    ui.label(format!("Openslot will automatically {} this invitation in {} seconds.",
                        if default_accept { "ACCEPT" } else { "REJECT" }, left.max(0)));
                    if oor {
                        ui.label(RichText::new("(It is outside your allowed auto-QSY frequencies.)").small().color(GRAY));
                    }
                } else {
                    ui.label("Click 'QSY' to QSY now. If you fail to QSY quickly, the link might be closed. HURRY UP!");
                }
                ui.label(RichText::new("Always be respectful to others. Make sure you QSY to a free frequency.").small().color(GRAY));
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let qsy = if default_accept { format!("QSY ({})", left.max(0)) } else { "QSY".into() };
                let reject = if default_accept { "REJECT".to_string() } else { format!("REJECT ({})", left.max(0)) };
                if ui.add(theme::accent_button(&qsy, GREEN)).clicked() {
                    answer = Some(true);
                }
                if ui.add(theme::accent_button(&reject, RED)).clicked() {
                    answer = Some(false);
                }
            });
        });
    ctx.request_repaint_after(std::time::Duration::from_millis(500));
    let _ = (target, offset, oor);
    if let Some(accept) = answer {
        resolve_qsy_popup(state, accept);
    }
}

/// The "QSY NOW" (or "QSY back") answer, by the operator or by the countdown.
fn resolve_qsy_popup(state: &mut GuiState, accept: bool) {
    let Some(p) = state.qsy_popup.take() else { return };
    let freq = crate::varac::messages::freq_dotted(p.target_hz as i64);
    if p.recovery {
        if accept {
            state.current_frequency = p.target_hz;
            state.tune_to = Some(p.target_hz);
            state.add_log(format!("QSY back to {freq}"));
        }
    } else {
        state.send_cmd(GuiCommand::QsyAnswer { accept, oor: !accept && p.out_of_range, offset_hz: p.offset_hz });
        if !accept {
            state.add_system_chat(format!("QSY to {freq} rejected"));
        }
    }
}

/// VarAC's busy-channel question ("Do you still wish to …?", Yes by default) and its
/// OK boxes for refused actions ("PTT is disabled!", "Frequency is busy", "Wrong band").
fn draw_tx_held(ctx: &egui::Context, state: &mut GuiState) {
    if let Some((title, text)) = state.tx_alert.clone() {
        let mut ok = false;
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(&text);
                ui.add_space(6.0);
                ok = ui.button("   OK   ").clicked();
            });
        if ok {
            state.tx_alert = None;
        }
        return;
    }
    let Some((_, title, text)) = state.tx_held.as_ref() else { return };
    let (title, text) = (title.clone(), text.clone());
    let mut answer = None;
    egui::Window::new(title)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label(&text);
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.add(theme::accent_button("YES", RED)).clicked() {
                    answer = Some(true);
                }
                if ui.button("NO").clicked() {
                    answer = Some(false);
                }
            });
        });
    if let Some(go) = answer {
        if let Some((cmd, _, _)) = state.tx_held.take() {
            if go {
                state.send_cmd(GuiCommand::Confirmed(Box::new(cmd)));
            }
        }
    }
}

/// VarAC's "QSY back" popup after a QSO that left the calling frequency.
fn draw_return_to_cf(ctx: &egui::Context, state: &mut GuiState) {
    let Some((cf, until, auto)) = state.return_cf_prompt else { return };
    if state.connected_to.is_some() {
        state.return_cf_prompt = None; // a new QSO started meanwhile
        return;
    }
    let left = (until - Utc::now()).num_seconds();
    let mut answer = (left < 0).then_some(auto);
    egui::Window::new("QSY back")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label("Your QSO is over.");
            ui.label(format!("Do you want to switch back to the calling frequency on {:.3} MHz?", cf as f64 / 1e6));
            ui.horizontal(|ui| {
                let (yes, no) = if auto { (format!("YES ({left})"), "NO".to_string()) } else { ("YES".to_string(), format!("NO ({left})")) };
                if ui.button(yes).clicked() {
                    answer = Some(true);
                }
                if ui.button(no).clicked() {
                    answer = Some(false);
                }
            });
        });
    ctx.request_repaint_after(std::time::Duration::from_millis(500));
    if let Some(go) = answer {
        state.return_cf_prompt = None;
        if go {
            state.go_to_frequency(cf);
        }
    }
}

/// VarAC's "Verbose SNR invitation": the partner asks to exchange SNR every minute.
fn draw_verbose_snr_invite(ctx: &egui::Context, state: &mut GuiState) {
    if !state.verbose_snr_invite {
        return;
    }
    let partner = state.connected_to.clone().unwrap_or_default();
    let mut answer = None;
    egui::Window::new("Verbose SNR invitation")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label(format!("{partner} invites you to exchange SNR reports every minute."));
            ui.horizontal(|ui| {
                if ui.add(theme::accent_button("ACCEPT", GREEN)).clicked() {
                    answer = Some(true);
                }
                if ui.add(theme::accent_button("REJECT", RED)).clicked() {
                    answer = Some(false);
                }
            });
        });
    if let Some(accept) = answer {
        state.verbose_snr_invite = false;
        state.verbose_snr = accept;
        state.send_cmd(GuiCommand::VerboseSnrAnswer(accept));
    }
}

/// VarAC's email gateway relay popup: RELAY (n) / CLOSE (n). The countdown itself runs
/// in the background task, so it still happens while this window is hidden.
fn draw_relay_offer(ctx: &egui::Context, state: &mut GuiState) {
    let Some((gateway, count, auto, deadline)) = state.relay_offer.clone() else { return };
    let left = deadline.saturating_duration_since(std::time::Instant::now()).as_secs();
    let mut answer = None;
    egui::Window::new("Email gateway")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label(format!(
                "{gateway} is an email gateway. Relay your {count} queued email{} through it now?",
                if count == 1 { "" } else { "s" }
            ));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let relay = if auto { format!("RELAY ({left})") } else { "RELAY".into() };
                let close = if auto { "CLOSE".to_string() } else { format!("CLOSE ({left})") };
                if ui.add(theme::accent_button(&relay, GREEN)).clicked() {
                    answer = Some(true);
                }
                if ui.add(theme::accent_button(&close, RED)).clicked() {
                    answer = Some(false);
                }
            });
        });
    ctx.request_repaint_after(std::time::Duration::from_millis(500));
    if let Some(relay) = answer {
        state.relay_offer = None;
        state.send_cmd(if relay { GuiCommand::FlushEmailOutbox } else { GuiCommand::DeclineEmailRelay });
    }
}

fn draw_broadcast_window(ctx: &egui::Context, state: &mut GuiState) {
    if !state.broadcast_compose_open {
        return;
    }

    let mut open = state.broadcast_compose_open;
    egui::Window::new("Send Broadcast")
        .open(&mut open)
        .resizable(false)
        .default_width(420.0)
        .show(ctx, |ui| {
            egui::Grid::new("bc_grid").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                ui.label("To:");
                ui.add(TextEdit::singleline(&mut state.broadcast_to).desired_width(110.0).hint_text("ALL"))
                    .on_hover_text("ALL, or one station's callsign");
                ui.end_row();
                ui.label("Via:");
                ui.add(TextEdit::singleline(&mut state.broadcast_via).desired_width(160.0).hint_text("up to 2 digipeaters"))
                    .on_hover_text("Digipeaters, space separated (VARA FM)");
                ui.end_row();
                ui.label("QSY to (kHz):");
                ui.add(TextEdit::singleline(&mut state.broadcast_qsy_khz).desired_width(110.0).hint_text("optional"))
                    .on_hover_text("Announce a frequency: receivers can click it to QSY there");
                ui.end_row();
            });
            let to = state.broadcast_to.trim().to_uppercase();
            let to_all = to.is_empty() || to == "ALL";
            if to_all {
                state.broadcast_snr_request = false;
            } else {
                ui.checkbox(&mut state.broadcast_snr_request, "Ask for an SNR report")
                    .on_hover_text("The station answers with how it heard this broadcast");
            }
            ui.label("Message (max 81 chars):");
            ui.add(
                TextEdit::singleline(&mut state.broadcast_text)
                    .desired_width(380.0)
                    .hint_text("Message to broadcast"),
            );
            let via: Vec<String> = state.broadcast_via.split_whitespace().map(|v| v.to_uppercase()).collect();
            let qsy_hz = state.broadcast_qsy_khz.trim().replace(',', ".").parse::<f64>().ok()
                .filter(|k| *k > 0.0).map(|k| (k * 1000.0).round() as u64);
            // VarAC: 81 bytes on a calling frequency with VARA HF, 150 elsewhere,
            // 91 / 84 / 76 with 0 / 1 / 2 digipeaters; UTF-8 bytes, the QSY tag included
            let hf_cf = state.modem_type() == crate::vara::commands::ModemType::Hf && messages::is_calling_frequency(state.dial());
            let mut limit = if hf_cf { 81 } else { 150 };
            if !via.is_empty() {
                limit = limit.min([91, 84, 76][via.len().min(2)]);
            }
            let qsy_tag = qsy_hz.map(|hz| format!("<QF:{hz}>")).unwrap_or_default();
            let char_count = qsy_tag.len() + state.broadcast_text.len();
            let count_color = if char_count > limit { RED } else { GRAY };
            ui.colored_label(count_color, format!("{char_count}/{limit} Bytes"));
            if via.len() > 2 {
                ui.colored_label(RED, "Up to 2 digipeaters are allowed");
            }

            ui.horizontal(|ui| {
                let can_send = (!state.broadcast_text.trim().is_empty() || qsy_hz.is_some())
                    && char_count <= limit && via.len() <= 2 && state.modem_connected;
                if ui.add_enabled(can_send, egui::Button::new(RichText::new("SEND").color(GREEN).strong())).clicked() {
                    // (VarAC: the QSY tag first, <SR> added when sending)
                    let mut text = format!("{qsy_tag}{}", std::mem::take(&mut state.broadcast_text));
                    if state.broadcast_snr_request {
                        text.push_str("<SR>");
                    }
                    let to = if to_all { "ALL".to_string() } else { to.clone() };
                    state.send_cmd(GuiCommand::SendBroadcast { to, via, text });
                    state.broadcast_qsy_khz.clear();
                    state.broadcast_compose_open = false;
                    state.status_msg = "Broadcast queued (goes out when the channel is free)".into();
                }
                if ui.button("Cancel").clicked() {
                    state.broadcast_compose_open = false;
                }
            });
        });
    state.broadcast_compose_open = open && state.broadcast_compose_open; // (a button inside may have closed it)
}

fn draw_path_finder_window(ctx: &egui::Context, state: &mut GuiState) {
    if !state.path_finder_open {
        return;
    }

    let mut open = state.path_finder_open;
    egui::Window::new("Path Finder")
        .open(&mut open)
        .resizable(false)
        .default_width(450.0)
        .show(ctx, |ui| {
            ui.label(RichText::new(
                "Find a relay path to a station you cannot reach directly.\n\
                 A path finder request is broadcast; stations that heard your target\n\
                 in the last 24 hours answer with the band, SNR and how long ago."
            ).small().color(GRAY));
            ui.add_space(8.0);

            ui.horizontal(|ui| {
                ui.label("Find path to:");
                ui.add(
                    TextEdit::singleline(&mut state.path_finder_callsign)
                        .desired_width(120.0)
                        .hint_text("CALLSIGN"),
                );

                let call = state.path_finder_callsign.trim().to_uppercase();
                let valid = !call.is_empty() && !call.contains(' ') && call.len() <= 7
                    && call.split('-').next() != state.my_callsign.split('-').next();
                let can_find = valid && state.modem_connected;
                if ui.add_enabled(can_find, egui::Button::new(RichText::new("FIND PATH").color(GREEN).strong())).clicked() {
                    // VarAC: one request a minute while answers come in
                    let wait = state.path_finder_sent.map(|t| 60 - (Utc::now() - t).num_seconds()).unwrap_or(0);
                    if wait > 0 {
                        state.status_msg = format!(
                            "You can only send one path finder request every 60 seconds ({wait} s left)"
                        );
                    } else {
                        state.add_log(format!("Path finder request for {call}"));
                        state.status_msg = format!("Path finder: asking who heard {call}");
                        state.path_finder_target = Some(call.clone());
                        state.path_finder_sent = Some(Utc::now());
                        state.send_cmd(GuiCommand::PathFinderRequest(call));
                        state.path_finder_open = false;
                    }
                }
                if ui.button("CANCEL").clicked() {
                    state.path_finder_open = false;
                }
            });
        });
    state.path_finder_open = open && state.path_finder_open; // (a button inside may have closed it)
}

fn draw_file_send_window(ctx: &egui::Context, state: &mut GuiState) {
    if !state.file_send_open {
        return;
    }

    let mut open = state.file_send_open;
    egui::Window::new("Send File")
        .open(&mut open)
        .resizable(false)
        .default_width(500.0)
        .show(ctx, |ui| {
            ui.label(RichText::new(
                "Send a file to the connected station via BBS transfer.\n\
                 Files are gzip-compressed before sending. Max 32 KiB compressed."
            ).small().color(GRAY));
            ui.add_space(8.0);

            ui.horizontal(|ui| {
                ui.label("File path:");
                ui.add(
                    TextEdit::singleline(&mut state.file_path_input)
                        .desired_width(300.0)
                        .hint_text("/path/to/file"),
                );
                if ui.button("Browse...").clicked() {
                    if let Some(path) = rfd::FileDialog::new().pick_file() {
                        state.file_path_input = path.display().to_string();
                    }
                }
            });

            let path = std::path::Path::new(&state.file_path_input);
            let file_exists = !state.file_path_input.is_empty() && path.is_file();
            if !state.file_path_input.is_empty() && !file_exists {
                ui.colored_label(RED, "File not found");
            }

            // VarAC refuses file transfers on a calling frequency (it would block the channel)
            let on_calling_freq = messages::is_calling_frequency(state.dial());
            if on_calling_freq {
                ui.colored_label(YELLOW, "You can't send files on the calling frequency. QSY first (QSY button), then send.");
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let can_send = file_exists && state.session_state == SessionState::Connected && !on_calling_freq;
                if ui.add_enabled(can_send, egui::Button::new(RichText::new("SEND").color(GREEN).strong())).clicked() {
                    let file_path = std::path::PathBuf::from(&state.file_path_input);
                    // VarAC opens its image shrinker for pictures
                    if crate::varac::images::is_image(&file_path) {
                        if let Ok(data) = std::fs::read(&file_path) {
                            if let Ok(img) = image::load_from_memory(&data) {
                                state.shrinker = Some(state::Shrinker {
                                    path: file_path,
                                    size: (img.width(), img.height()),
                                    original: data,
                                    pct: 100,
                                    quality: 50,
                                    overlay: false,
                                    result: None,
                                    dirty: true,
                                });
                                state.file_send_open = false;
                                state.file_path_input.clear();
                                return;
                            }
                        }
                    }
                    state.sent_files.push(file_path.clone());
                    state.send_cmd(GuiCommand::SendFile(file_path));
                    state.file_send_open = false;
                    state.status_msg = format!("Sending file: {}", state.file_path_input);
                    state.file_path_input.clear();
                }
                if ui.button("Cancel").clicked() {
                    state.file_send_open = false;
                }
            });
        });
    state.file_send_open = open && state.file_send_open; // (a button inside may have closed it)
}

fn draw_canned_editor_window(ctx: &egui::Context, state: &mut GuiState) {
    if !state.canned_editor_open {
        return;
    }

    let mut open = state.canned_editor_open;
    egui::Window::new("Canned Messages (F1–F12)")
        .open(&mut open)
        .resizable(true)
        .default_width(550.0)
        .show(ctx, |ui| {
            ui.colored_label(GRAY, RichText::new(
                "Tags (as in VarAC): <CALL> <NAME> <QTH> <LOC> <EMAIL> <RIG> <PWR> <ANT> <ICE> <UTCDT> <UTCD> <UTCT> <MYDT> \
                 <HCALL> <HNAME> <HQTH> <HLOC> · <SND> sends at once · <DISC> disconnects after sending"
            ).small().italics());
            ui.add_space(4.0);

            egui::Grid::new("canned_grid")
                .num_columns(4)
                .spacing([8.0, 4.0])
                .show(ui, |ui| {
                    ui.colored_label(BLUE_HEADER, RichText::new("Key").strong().small());
                    ui.colored_label(BLUE_HEADER, RichText::new("Label").strong().small());
                    ui.colored_label(BLUE_HEADER, RichText::new("Message text").strong().small());
                    ui.colored_label(BLUE_HEADER, RichText::new("Disc").strong().small());
                    ui.end_row();

                    for i in 0..12 {
                        ui.label(RichText::new(format!("F{}", i + 1)).strong());
                        ui.add(TextEdit::singleline(&mut state.canned_messages[i].label).desired_width(60.0));
                        ui.add(TextEdit::singleline(&mut state.canned_messages[i].text).desired_width(350.0));
                        ui.checkbox(&mut state.canned_messages[i].disconnect_after, "");
                        ui.end_row();
                    }
                });

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.add(theme::accent_button("SAVE & CLOSE", GREEN)).clicked() {
                    save_canned_messages(&state.canned_messages);
                    state.canned_editor_open = false;
                    state.status_msg = "Canned messages saved".into();
                }
                if ui.button("Close").clicked() {
                    state.canned_editor_open = false;
                }
            });
        });
    state.canned_editor_open = open && state.canned_editor_open; // (a button inside may have closed it)
}

fn save_canned_messages(messages: &[CannedMessage; 12]) {
    if let Some(dir) = crate::config::project_dirs() {
        let path = dir.config_dir().join("canned_messages.toml");
        let _ = std::fs::create_dir_all(dir.config_dir());

        let mut entries = Vec::new();
        for (i, cm) in messages.iter().enumerate() {
            entries.push(format!(
                "[[messages]]\nkey = \"F{}\"\nlabel = {:?}\ntext = {:?}\ndisconnect_after = {}",
                i + 1, cm.label, cm.text, cm.disconnect_after
            ));
        }
        let toml_str = entries.join("\n\n");
        let _ = std::fs::write(&path, toml_str);
    }
}

fn load_canned_messages() -> Option<[CannedMessage; 12]> {
    let dir = crate::config::project_dirs()?;
    let path = dir.config_dir().join("canned_messages.toml");
    let content = std::fs::read_to_string(path).ok()?;

    #[derive(serde::Deserialize)]
    struct CannedEntry {
        #[allow(dead_code)]
        key: String,
        label: String,
        text: String,
        disconnect_after: bool,
    }
    #[derive(serde::Deserialize)]
    struct CannedFile {
        messages: Vec<CannedEntry>,
    }

    let parsed: CannedFile = toml::from_str(&content).ok()?;
    let mut result: [CannedMessage; 12] = std::array::from_fn(|_| CannedMessage::default());
    for (i, entry) in parsed.messages.into_iter().take(12).enumerate() {
        result[i] = CannedMessage {
            label: entry.label,
            text: entry.text,
            disconnect_after: entry.disconnect_after,
        };
    }
    Some(result)
}

fn draw_vmail_folders(ui: &mut egui::Ui, state: &mut GuiState, db: &Storage) {
    // VMAIL section with folder tabs
    let vmail_label = if state.unread_vmails > 0 {
        format!("VMAIL ({})", state.unread_vmails)
    } else {
        "VMAIL".into()
    };
    ui.colored_label(YELLOW, RichText::new(vmail_label).strong());

    ui.horizontal(|ui| {
        let inbox_label = if state.unread_vmails > 0 {
            format!("Inbox ({})", state.unread_vmails)
        } else {
            "Inbox".into()
        };
        if ui.selectable_label(state.vmail_folder_tab == 0, RichText::new(inbox_label).small()).clicked() {
            state.vmail_folder_tab = 0;
        }
        if ui.selectable_label(state.vmail_folder_tab == 1, RichText::new("Sent").small()).clicked() {
            state.vmail_folder_tab = 1;
        }
        let outbox_label = if !state.vmail_outbox_list.is_empty() {
            format!("Outbox ({})", state.vmail_outbox_list.len())
        } else {
            "Outbox".into()
        };
        if ui.selectable_label(state.vmail_folder_tab == 2, RichText::new(outbox_label).small()).clicked() {
            state.vmail_folder_tab = 2;
        }
        let parking_label = if !state.vmail_parking_list.is_empty() {
            format!("Parking ({})", state.vmail_parking_list.len())
        } else {
            "Parking".into()
        };
        if ui.selectable_label(state.vmail_folder_tab == 3, RichText::new(parking_label).small()).clicked() {
            state.vmail_folder_tab = 3;
        }
        let drafts_label = if state.vmail_drafts_list.is_empty() { "Drafts".to_string() } else { format!("Drafts ({})", state.vmail_drafts_list.len()) };
        if ui.selectable_label(state.vmail_folder_tab == 4, RichText::new(drafts_label).small()).clicked() {
            state.vmail_folder_tab = 4;
        }
        if state.relay_notification_count > 0
            && ui.small_button(format!("Relay notifications ({})", state.relay_notification_count))
                .on_hover_text("Stations holding VMail for you").clicked()
        {
            state.relay_notifications_open = true;
        }
    });

    Frame::default()
        .fill(theme::SURFACE).rounding(theme::RADIUS)
        .inner_margin(Margin::same(2.0))
        .show(ui, |ui| {
            ScrollArea::vertical()
                .id_salt("vmail_scroll")
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    let list = match state.vmail_folder_tab {
                        1 => &state.vmail_sent_list,
                        2 => &state.vmail_outbox_list,
                        3 => &state.vmail_parking_list,
                        4 => &state.vmail_drafts_list,
                        _ => &state.vmail_list,
                    };
                    let vm_snapshot: Vec<_> = list.iter().enumerate()
                        .map(|(i, vm)| (i, vm.guid.clone(), vm.from.clone(), vm.to.clone(), vm.subject.clone(), vm.time.clone(), vm.read, vm.urgent))
                        .collect();
                    for (i, guid, from, to, subject, time, read, urgent) in &vm_snapshot {
                        let marker = if *urgent { "!" } else if !*read { "*" } else { " " };
                        let style = if *urgent { RED } else if !*read { WHITE } else { GRAY };
                        let label_call = if state.vmail_folder_tab == 0 { from } else { to };
                        let resp = ui.horizontal(|ui| {
                            ui.colored_label(style, RichText::new(format!("{marker}{time}")).small());
                            ui.colored_label(GREEN, RichText::new(label_call).small().strong());
                            ui.colored_label(style, RichText::new(subject).small());
                        }).response;

                        if resp.clicked() {
                            state.selected_vmail = Some(*i);
                            state.viewing_vmail = true;
                        }

                        let ctx_from = from.clone();
                        let ctx_subject = subject.clone();
                        resp.context_menu(|ui| {
                            if ui.button("Open").clicked() {
                                state.selected_vmail = Some(*i);
                                state.viewing_vmail = true;
                                ui.close_menu();
                            }
                            let tab = state.vmail_folder_tab;
                            // VarAC: Reply on Inbox / Sent / Outbox, Edit on Outbox / Drafts
                            if matches!(tab, 0..=2) && ui.button("Reply").clicked() {
                                open_reply(state, db, guid);
                                ui.close_menu();
                            }
                            if matches!(tab, 2 | 4) && ui.button("Edit").clicked() {
                                open_edit(state, db, guid);
                                ui.close_menu();
                            }
                            if matches!(tab, 2 | 3) {
                                let connected = state.session_state == SessionState::Connected;
                                if connected && ui.button("Relay now through connected station").clicked() {
                                    state.send_cmd(GuiCommand::RelayNow { guids: vec![guid.clone()], check_cf: false, quiet: false, disconnect_after: false });
                                    ui.close_menu();
                                }
                                let ctx_to = to.clone();
                                if !connected && ui.button("Connect and relay now").clicked() {
                                    if messages::varac_callsign_ok(&ctx_to) {
                                        state.connect_started = Some(Utc::now());
                                        state.last_connect_callsign = Some(ctx_to.clone());
                                        state.send_cmd(GuiCommand::Connect(ctx_to.clone()));
                                    } else {
                                        state.tx_alert = Some(("Can not perform this".into(), "You can only perform this operation with valid callsigns.".into()));
                                    }
                                    ui.close_menu();
                                }
                            }
                            let _ = &ctx_subject;
                            if ui.button("Copy callsign").clicked() {
                                ui.output_mut(|o| o.copied_text = ctx_from.clone());
                                ui.close_menu();
                            }
                            if ui.button(format!("Lookup {ctx_from} on QRZ")).clicked() {
                                open_qrz_lookup(&ctx_from);
                                ui.close_menu();
                            }
                            if matches!(state.vmail_folder_tab, 2..=4) {
                                ui.separator();
                                if ui.add(theme::accent_button("Cancel (delete)", RED)).clicked() {
                                    let _ = db.mark_vmail_deleted(guid);
                                    ui.close_menu();
                                }
                                if state.vmail_folder_tab == 3 {
                                    if ui.button("Deliver to outbox").clicked() {
                                        let _ = db.move_vmail_to_folder(guid, crate::storage::VMailFolder::Outbox);
                                        ui.close_menu();
                                    }
                                }
                            }
                        });
                    }
                    if vm_snapshot.is_empty() {
                        let empty_text = match state.vmail_folder_tab {
                            1 => "No sent VMails",
                            2 => "Outbox empty",
                            3 => "No parked VMails",
                            4 => "No drafts",
                            _ => "No VMails",
                        };
                        ui.colored_label(GRAY, RichText::new(empty_text).small().italics());
                    }
                });
        });
}

enum StationAction {
    Connect(String),
    /// Answer a CQ that named a slot: (call, slot, age of the CQ in seconds)
    ConnectOnSlot(String, u8, i64),
    SendVMail(String),
    AddToWatchlist(String),
    /// VarAC's "Ping (get report)"
    Ping(String),
    /// "Block callsign" (asks first)
    Block(String),
    /// "Callsign history"
    History(String),
    /// "DX Cluster spot"
    DxSpot(String),
}

/// VarAC's "PSK Reporter lookup" (with its default PSKReporterCustomLink).
pub fn open_psk_lookup(call: &str) {
    let _ = open::that(format!(
        "https://pskreporter.info/pskmap.html?preset&callsign={}&mode=VARAC&timerange=21600&showsnr=1&showlines=1",
        call.trim().to_uppercase()
    ));
}

/// The right-click items VarAC adds to every heard list (after Connect and VMail).
fn station_menu_extras(ui: &mut egui::Ui, call: &str, action: &mut Option<StationAction>) {
    // (the DX cluster setting, read once a few seconds rather than each frame)
    let dx_on = {
        let id = egui::Id::new("dxcluster_enabled_cache");
        let now = ui.ctx().input(|i| i.time);
        match ui.ctx().data(|d| d.get_temp::<(f64, bool)>(id)) {
            Some((t, v)) if now - t < 5.0 => v,
            _ => {
                let v = crate::config::read_dxcluster_enabled();
                ui.ctx().data_mut(|d| d.insert_temp(id, (now, v)));
                v
            }
        }
    };
    if ui.button("Ping (get report)").clicked() {
        *action = Some(StationAction::Ping(call.to_string()));
        ui.close_menu();
    }
    if ui.button("PSK Reporter lookup").clicked() {
        open_psk_lookup(call);
        ui.close_menu();
    }
    if dx_on && ui.button("DX Cluster spot").clicked() {
        *action = Some(StationAction::DxSpot(call.to_string()));
        ui.close_menu();
    }
    if ui.button("Block callsign").clicked() {
        *action = Some(StationAction::Block(call.to_string()));
        ui.close_menu();
    }
    ui.separator();
    if ui.button("Callsign history").clicked() {
        *action = Some(StationAction::History(call.to_string()));
        ui.close_menu();
    }
}

fn handle_station_action(action: StationAction, state: &mut GuiState) {
    if let StationAction::Connect(call) | StationAction::ConnectOnSlot(call, _, _) = &action {
        if state.refuse_wrong_band(call) {
            return;
        }
    }
    match action {
        StationAction::ConnectOnSlot(call, slot, age) if !state.settings_form.skip_cq_slot_selector && slot != 0 => {
            if state.modem_connected {
                if age > crate::varac::slots::CQ_SLOT_STALE_SECS {
                    state.stale_cq_confirm = Some((call, slot, age / 60));
                } else {
                    state.connect_on_slot(&call, slot);
                }
            }
        }
        StationAction::ConnectOnSlot(call, _, _) => handle_station_action(StationAction::Connect(call), state),
        StationAction::Connect(call) => {
            if state.modem_connected && !state.refuse_wide_on_calling_freq() {
                state.add_log(format!("Connecting to {call}..."));
                state.status_msg = format!("Connecting to {call}...");
                state.connect_started = Some(Utc::now());
                state.last_connect_callsign = Some(call.clone());
                state.send_cmd(GuiCommand::Connect(call));
            }
        }
        StationAction::SendVMail(call) => {
            state.compose_open = true;
            state.compose_to = call;
        }
        StationAction::Ping(call) => {
            if state.modem_connected && !state.refuse_wide_on_calling_freq() {
                let dest = call.to_uppercase();
                state.add_log(format!("Pinging {dest}..."));
                state.status_msg = format!("Pinging {dest}...");
                state.send_cmd(GuiCommand::Ping(dest));
            }
        }
        StationAction::Block(call) => state.block_confirm = Some(call.to_uppercase()),
        StationAction::History(call) => state.history_call = Some(call.to_uppercase()),
        StationAction::DxSpot(call) => {
            state.dx_spot = Some((call.to_uppercase(), crate::varac::dxcluster::khz(state.dial()), String::new()));
        }
        StationAction::AddToWatchlist(call) => {
            let upper = call.to_uppercase();
            if !state.watchlist.contains(&upper) {
                state.watchlist.push(upper.clone());
                state.status_msg = format!("{upper} added to watchlist");
            } else {
                state.status_msg = format!("{upper} already in watchlist");
            }
        }
    }
}

fn draw_station_table(ui: &mut egui::Ui, entries: &VecDeque<CqEntry>, callsign_tags: &std::collections::HashMap<String, String>, my_locator: &str, filter: &str, db: &Storage) -> Option<StationAction> {
    let mut action = None;

    ui.horizontal(|ui| {
        ui.colored_label(GRAY, RichText::new("Bnd").small().strong());
        ui.add_space(4.0);
        ui.colored_label(GRAY, RichText::new("Time").small().strong());
        ui.add_space(4.0);
        ui.colored_label(GRAY, RichText::new("TΔ").small().strong());
        ui.add_space(8.0);
        ui.colored_label(GRAY, RichText::new("Callsign").small().strong());
        ui.add_space(8.0);
        ui.colored_label(GRAY, RichText::new("Type").small().strong());
        ui.add_space(8.0);
        ui.colored_label(GRAY, RichText::new("BW").small().strong());
        ui.add_space(8.0);
        ui.colored_label(GRAY, RichText::new("SNR").small().strong());
    });
    ui.separator();

    let filter_lower = filter.to_lowercase();
    for entry in entries.iter().rev().take(30) {
        if !filter_lower.is_empty() && !entry.callsign.to_lowercase().contains(&filter_lower) {
            continue;
        }
        let base_call = entry.callsign.split('-').next().unwrap_or(&entry.callsign).to_string();

        let resp = ui.horizontal(|ui| {
            let age_secs = (Utc::now() - entry.timestamp).num_seconds().max(0);
            let time_str = entry.time.clone();
            let time_color = if age_secs > 300 {
                theme::SURFACE_3
            } else {
                GRAY
            };
            ui.colored_label(GRAY, RichText::new(&entry.band).small());
            ui.colored_label(time_color, RichText::new(time_str).small());
            // VarAC's TΔ: hours:minutes since heard
            ui.colored_label(time_color, RichText::new(format!("{:02}:{:02}", age_secs / 3600, age_secs / 60 % 60)).small().monospace());
            let call_color = if entry.is_beacon { BEACON_PURPLE } else { GREEN };
            match entry.alert {
                // an alert tag matched: VarAC paints the row in the tag's colours
                Some(((fr, fg, fb), (br, bg, bb))) => {
                    ui.label(RichText::new(&entry.callsign).small().strong()
                        .color(Color32::from_rgb(fr, fg, fb)).background_color(Color32::from_rgb(br, bg, bb)));
                }
                None => {
                    ui.colored_label(call_color, RichText::new(&entry.callsign).small().strong());
                }
            }
            if let Some(tag) = callsign_tags.get(&base_call.to_uppercase()) {
                ui.colored_label(CYAN, RichText::new(format!("({tag})")).small());
            }
            if let Some(ref cq_t) = entry.cq_type {
                ui.colored_label(YELLOW, RichText::new(cq_t).small());
            }
            if let Some(ref loc) = entry.locator {
                if my_locator.len() >= 4 {
                    if let Some(dist) = grid_distance_str(my_locator, loc) {
                        ui.colored_label(GRAY, RichText::new(dist).small());
                    }
                } else {
                    ui.colored_label(GRAY, RichText::new(loc).small());
                }
            }
            ui.colored_label(WHITE, RichText::new(format!("{}", entry.bandwidth)).small());
            let snr = entry.snr.as_deref().unwrap_or("--");
            ui.colored_label(WHITE, RichText::new(snr).small());
        }).response;

        let resp = resp.on_hover_ui(|ui| {
            if let Ok(Some(contact)) = db.get_contact(&base_call) {
                let dxcc = dxcc_entity(&base_call).unwrap_or("Unknown");
                ui.colored_label(GREEN, RichText::new(format!("{base_call}  ({dxcc})")).strong());
                if let Some(ref name) = contact.name {
                    if !name.is_empty() { ui.label(format!("Name: {name}")); }
                }
                if let Some(ref qth) = contact.qth {
                    if !qth.is_empty() { ui.label(format!("QTH: {qth}")); }
                }
                if let Some(ref loc) = contact.locator {
                    if !loc.is_empty() { ui.label(format!("Grid: {loc}")); }
                }
                if contact.qso_count > 0 {
                    ui.label(format!("QSOs: {}", contact.qso_count));
                }
                if let Some(ref last) = contact.last_qso {
                    ui.colored_label(GRAY, RichText::new(format!("Last: {}", last.format("%Y-%m-%d"))).small());
                }
            } else {
                let dxcc = dxcc_entity(&base_call).unwrap_or("Unknown");
                ui.label(format!("{base_call} ({dxcc}) — no contact info"));
            }
            ui.colored_label(GRAY, RichText::new("Double-click to connect").small());
        });
        let age = (Utc::now() - entry.timestamp).num_seconds();
        if resp.double_clicked() {
            action = Some(StationAction::ConnectOnSlot(base_call.clone(), entry.slot, age));
        }

        resp.context_menu(|ui| {
            if entry.slot != 0 && ui.button(format!("QSY to slot {} and connect to {}", entry.slot, base_call)).clicked() {
                action = Some(StationAction::ConnectOnSlot(base_call.clone(), entry.slot, age));
                ui.close_menu();
            }
            if ui.button(format!("Connect to {} (here)", base_call)).clicked() {
                action = Some(StationAction::Connect(base_call.clone()));
                ui.close_menu();
            }
            if ui.button(format!("Send VMail to {}", base_call)).clicked() {
                action = Some(StationAction::SendVMail(base_call.clone()));
                ui.close_menu();
            }
            if ui.button("Copy callsign").clicked() {
                ui.output_mut(|o| o.copied_text = base_call.clone());
                ui.close_menu();
            }
            if ui.button(format!("Lookup {} on QRZ", base_call)).clicked() {
                open_qrz_lookup(&base_call);
                ui.close_menu();
            }
            station_menu_extras(ui, &base_call, &mut action);
            ui.separator();
            if ui.button(format!("Add {} to watchlist", base_call)).clicked() {
                action = Some(StationAction::AddToWatchlist(base_call.clone()));
                ui.close_menu();
            }
        });
    }

    if entries.is_empty() {
        ui.colored_label(GRAY, RichText::new("Listening...").small().italics());
    }

    action
}

fn draw_settings_window(
    ctx: &egui::Context,
    state: &mut GuiState,
    station_config: &StationConfig,
    modem_config: &crate::tui::ModemConfig,
) {
    if !state.settings_open {
        return;
    }

    // Populate form on first open
    if state.settings_form.callsign.is_empty() && !station_config.callsign.is_empty() {
        state.settings_form.callsign = station_config.callsign.clone();
        state.settings_form.name = station_config.name.clone().unwrap_or_default();
        state.settings_form.qth = station_config.qth.clone().unwrap_or_default();
        state.settings_form.locator = station_config.locator.clone().unwrap_or_default();
        state.settings_form.modem_host = modem_config.host.clone();
        state.settings_form.cmd_port = modem_config.cmd_port.to_string();
        state.settings_form.data_port = modem_config.data_port.to_string();
        state.settings_form.kiss_port = modem_config.kiss_port.to_string();
        // (the bandwidth is kept live: modem start and the Bandwidth menu set it)
        let flags = station_config.services.0;
        state.settings_form.vmail_enabled = (flags & crate::varac::messages::ServiceFlags::VMAIL) != 0;
        state.settings_form.bbs_enabled = (flags & crate::varac::messages::ServiceFlags::BBS) != 0;
    }

    let mut open = state.settings_open;
    egui::Window::new("Settings")
        .open(&mut open)
        .resizable(true)
        .default_width(760.0)
        .show(ctx, |ui| {
            // Tab bar
            ui.horizontal(|ui| {
                // Same order as VarAC's Settings window (indices are stable ids)
                let tabs = [(5, "RIG"), (1, "Vara"), (2, "QSO"), (3, "CQ / Beacon"), (0, "My Information"), (6, "Email"), (7, "AI / HamPlay"), (4, "Appearance")];
                for (i, label) in tabs {
                    if ui.selectable_label(state.settings_form.settings_tab == i, label).clicked() {
                        state.settings_form.settings_tab = i;
                    }
                }
            });
            ui.separator();

            match state.settings_form.settings_tab {
                0 => {
                    // Station tab
                    egui::Grid::new("station_grid")
                        .num_columns(2)
                        .spacing([10.0, 6.0])
                        .show(ui, |ui| {
                            ui.label("Callsign:");
                            ui.add(TextEdit::singleline(&mut state.settings_form.callsign).desired_width(150.0));
                            ui.end_row();

                            ui.label("GPS position (manual):");
                            ui.add(TextEdit::singleline(&mut state.settings_form.gps_manual).desired_width(200.0).hint_text("e.g. 30.2672,-97.7431"))
                                .on_hover_text("Answered to a GPS request as <GPS:…> (empty: NO GPS DATA)");
                            ui.end_row();

                            ui.label("Complex callsign:");
                            ui.add(TextEdit::singleline(&mut state.settings_form.full_callsign).desired_width(150.0).hint_text("e.g. W/4Z1AC/QRP"))
                                .on_hover_text("Prefix/suffix form sent in your opening as <FC:..> (VARA links with the plain callsign)");
                            ui.end_row();

                            ui.label("Name:");
                            ui.add(TextEdit::singleline(&mut state.settings_form.name).desired_width(200.0));
                            ui.end_row();

                            ui.label("QTH:");
                            ui.add(TextEdit::singleline(&mut state.settings_form.qth).desired_width(200.0));
                            ui.end_row();

                            ui.label("Locator:");
                            ui.add(TextEdit::singleline(&mut state.settings_form.locator).desired_width(100.0).hint_text("e.g. FN42"));
                            ui.end_row();

                            ui.label("Email:");
                            ui.add(TextEdit::singleline(&mut state.settings_form.email).desired_width(200.0))
                                .on_hover_text("Shared when a station asks for your INFO (leave empty to keep it private)");
                            ui.end_row();

                            // VarAC [MY_INFO]: sent in INFO replies, usable as <RIG> <PWR> <ANT> <ICE> tags
                            for (label, value, hint) in [
                                ("Rig:", &mut state.settings_form.my_rig, "e.g. FTDX3000"),
                                ("Power (W):", &mut state.settings_form.my_power, "e.g. 100"),
                                ("Antenna:", &mut state.settings_form.my_antenna, "e.g. EFHW 40-10m"),
                                ("Age:", &mut state.settings_form.my_age, ""),
                                ("Profession:", &mut state.settings_form.my_profession, ""),
                                ("Hobbies:", &mut state.settings_form.my_hobbies, ""),
                            ] {
                                ui.label(label);
                                ui.add(TextEdit::singleline(value).desired_width(200.0).hint_text(hint));
                                ui.end_row();
                            }
                        });

                    ui.add_space(8.0);
                    ui.heading("Services");
                    ui.checkbox(&mut state.settings_form.vmail_enabled, "VMail");
                    ui.checkbox(&mut state.settings_form.bbs_enabled, "BBS File Transfer");
                    if state.settings_form.bbs_enabled {
                        ui.horizontal(|ui| {
                            ui.label("BBS Directory:");
                            ui.add(TextEdit::singleline(&mut state.settings_form.bbs_directory)
                                .desired_width(200.0)
                                .hint_text("/path/to/bbs/files"));
                            if ui.button("Browse...").clicked() {
                                if let Some(path) = rfd::FileDialog::new().pick_folder() {
                                    state.settings_form.bbs_directory = path.display().to_string();
                                }
                            }
                        });
                        let f = &mut state.settings_form;
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut f.bbs_limit, "Limit BBS to these callsigns:");
                            ui.add_enabled(f.bbs_limit, TextEdit::singleline(&mut f.bbs_allow).desired_width(220.0).hint_text("W5TST,K5XYZ"));
                        });
                        ui.add_enabled_ui(!f.bbs_limit, |ui| {
                            ui.checkbox(&mut f.bbs_announce, "Announce the BBS in my beacons (ADVANCED beacons, open BBS only)");
                        });
                    }
                }
                1 => {
                    // VARA Modem tab
                    egui::Grid::new("modem_grid")
                        .num_columns(2)
                        .spacing([10.0, 6.0])
                        .show(ui, |ui| {
                            ui.label("Host:");
                            ui.add(TextEdit::singleline(&mut state.settings_form.modem_host).desired_width(150.0));
                            ui.end_row();

                            ui.label("Command Port:");
                            ui.add(TextEdit::singleline(&mut state.settings_form.cmd_port).desired_width(80.0));
                            ui.end_row();

                            ui.label("Data Port:");
                            ui.add(TextEdit::singleline(&mut state.settings_form.data_port).desired_width(80.0));
                            ui.end_row();

                            ui.label("VARA modem:");
                            ui.horizontal(|ui| {
                                for k in ["VaraHF", "VaraFM", "VaraSAT"] {
                                    ui.radio_value(&mut state.settings_form.vara_modem_type, k.to_string(), k);
                                }
                            });
                            ui.end_row();
                            ui.label("Digipeat via:");
                            ui.add_enabled(state.settings_form.vara_modem_type == "VaraFM",
                                TextEdit::singleline(&mut state.settings_form.digipeat_via).desired_width(200.0).hint_text("DIGI1 DIGI2"))
                                .on_hover_text("VARA FM only: CQs, beacons and broadcasts go through these digipeaters (space between them). To connect through one, type CALL VIA DIGI in the callsign box.");
                            ui.end_row();

                            ui.label("VARA program:");
                            ui.horizontal(|ui| {
                                ui.add(TextEdit::singleline(&mut state.settings_form.vara_path).desired_width(260.0).hint_text("C:\\VARA\\VARA.exe"));
                                if ui.small_button("Browse…").clicked() {
                                    if let Some(p) = rfd::FileDialog::new().pick_file() {
                                        state.settings_form.vara_path = p.display().to_string();
                                    }
                                }
                            });
                            ui.end_row();
                            ui.label("");
                            ui.horizontal(|ui| {
                                ui.checkbox(&mut state.settings_form.vara_launch, "Start VARA when connecting the modem")
                                    .on_hover_text("Only when VARA isn't running yet. Sets its command and KISS ports. A .exe runs through Wine on Linux/macOS.");
                                ui.label("then wait");
                                ui.add(TextEdit::singleline(&mut state.settings_form.vara_launch_delay_secs).desired_width(30.0));
                                ui.label("s");
                            });
                            ui.end_row();

                            ui.label("KISS Port:");
                            ui.add(TextEdit::singleline(&mut state.settings_form.kiss_port).desired_width(80.0))
                                .on_hover_text("VARA's KISS interface (CQs, beacons, broadcasts). VARA's default is 8100.");
                            ui.end_row();

                            ui.label("Bandwidth:");
                            egui::ComboBox::from_id_salt("bw_combo")
                                .selected_text(&state.settings_form.bandwidth)
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(&mut state.settings_form.bandwidth, "500".into(), "500 Hz");
                                    ui.selectable_value(&mut state.settings_form.bandwidth, "2300".into(), "2300 Hz");
                                    ui.selectable_value(&mut state.settings_form.bandwidth, "2750".into(), "2750 Hz");
                                });
                            ui.end_row();
                        });
                }
                2 => {
                    // QSO tab
                    ui.checkbox(&mut state.settings_form.auto_log_qso, "Auto log QSO");
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label("CQ slot wait time (seconds):");
                        ui.add(TextEdit::singleline(&mut state.settings_form.cq_slot_wait_secs).desired_width(50.0));
                        ui.add_space(12.0);
                        ui.label("Slot size (Hz):");
                        ui.add(TextEdit::singleline(&mut state.settings_form.slot_step_hz).desired_width(50.0));
                    });
                    ui.checkbox(&mut state.settings_form.skip_cq_slot_selector, "Skip slot selector")
                        .on_hover_text("Call CQ without a slot (VARA FM, or your own net off the VarAC calling frequencies). \
                            On the official VarAC HF calling frequencies, use slots.");
                    ui.colored_label(GRAY, RichText::new(
                        "VarAC slots: your CQ names a slot (750 Hz steps around the calling frequency); you then wait \
                         there for a reply and return to the calling frequency after the wait time. Stations answering \
                         your CQ use the same slot size.",
                    ).small().italics());
                    ui.add_space(4.0);
                    ui.checkbox(&mut state.settings_form.load_history, "Load history upon connection");
                    ui.checkbox(&mut state.settings_form.allow_pings, "Allow incoming pings");
                    ui.checkbox(&mut state.settings_form.allow_info_request, "Allow info/inquire requests")
                        .on_hover_text("Off: info and gateway-address requests are refused with <INFOJ>, like VarAC");
                    ui.horizontal(|ui| {
                        ui.label("Call ID every (min):");
                        ui.add(TextEdit::singleline(&mut state.settings_form.call_id_minutes).desired_width(30.0))
                            .on_hover_text("Sends \"de CALL <R+NN>\" during the QSO (5–30, like VarAC)");
                        ui.add_space(10.0);
                        ui.label("Disconnect when idle (min, 0 = never):");
                        ui.add(TextEdit::singleline(&mut state.settings_form.idle_disconnect_minutes).desired_width(30.0))
                            .on_hover_text("Tells the partner <IE> and disconnects");
                    });
                    ui.horizontal(|ui| {
                        ui.label("Welcome message:");
                        ui.add(TextEdit::singleline(&mut state.settings_form.welcome_message).desired_width(260.0)
                            .hint_text("sent to stations that connect; \"de CALL\" is added"));
                        ui.checkbox(&mut state.settings_form.welcome_always, "Send always (even when away)");
                    });
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut state.settings_form.allow_last_heard, "Allow last heard peeking");
                        ui.checkbox(&mut state.settings_form.multi_band_last_heard, "Multi-band");
                        ui.checkbox(&mut state.settings_form.allow_path_finder, "Answer path finder requests")
                            .on_hover_text("Tell stations looking for a path when you heard their target in the last 24 hours");
                    });
                    ui.checkbox(&mut state.settings_form.verbose_snr_auto_accept, "Auto accept verbose SNR")
                        .on_hover_text("Accept a partner's invitation to exchange SNR every minute without asking");
                    ui.add_space(6.0);
                    ui.label(RichText::new("Logging").strong());
                    ui.horizontal(|ui| {
                        ui.label("ADIF log file:");
                        ui.add(TextEdit::singleline(&mut state.settings_form.adif_log_path).desired_width(300.0)
                            .hint_text(crate::config::default_adif_path().display().to_string()));
                    });
                    ui.horizontal(|ui| {
                        ui.label("SUBMODE:");
                        ui.add(TextEdit::singleline(&mut state.settings_form.adif_submode).desired_width(80.0).hint_text("as the modem"))
                            .on_hover_text("VarAC writes MODE DYNAMIC, SUBMODE VARA HF (NONE leaves it out)");
                        ui.label("Comment:");
                        ui.add(TextEdit::singleline(&mut state.settings_form.default_log_comment).desired_width(180.0));
                    });
                    for (label, app, ip, port) in [
                        ("Logger:", &mut state.settings_form.logger_app, &mut state.settings_form.logger_ip, &mut state.settings_form.logger_port),
                        ("Second logger:", &mut state.settings_form.logger_app2, &mut state.settings_form.logger_ip2, &mut state.settings_form.logger_port2),
                    ] {
                        ui.horizontal(|ui| {
                            ui.label(label);
                            egui::ComboBox::from_id_salt(label).selected_text(app.clone()).show_ui(ui, |ui| {
                                for (name, default_port) in crate::varac::logbook::LOGGERS {
                                    if ui.selectable_label(app == name, *name).clicked() {
                                        *app = name.to_string();
                                        *port = if *default_port > 0 { default_port.to_string() } else { String::new() };
                                    }
                                }
                            });
                            if app != "NONE" {
                                ui.add(TextEdit::singleline(ip).desired_width(100.0).hint_text("127.0.0.1"));
                                ui.add(TextEdit::singleline(port).desired_width(50.0).hint_text("port"));
                            }
                        });
                    }
                    ui.checkbox(&mut state.settings_form.logger_callsign_lookup, "Log4OM callsign lookup")
                        .on_hover_text("Show the station you connect to in Log4OM (UDP to its API port 2241)");
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut state.settings_form.pskreporter_upload, "Report stations heard to PSKReporter")
                            .on_hover_text("CQs, beacons, broadcasts and QSO partners, as VarAC does (mode VARAC)");
                        ui.add_enabled(state.settings_form.pskreporter_upload,
                            egui::Checkbox::new(&mut state.settings_form.pskreporter_self_report, "Self report"));
                    });
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut state.settings_form.dxcluster_enabled, "Spot QSOs to a DX cluster")
                            .on_hover_text("Right-click a station: \"DX Cluster spot\" (telnet, as VarAC)");
                        if state.settings_form.dxcluster_enabled {
                            ui.add(TextEdit::singleline(&mut state.settings_form.dxcluster_host).desired_width(110.0).hint_text("k1ttt.net"));
                            ui.add(TextEdit::singleline(&mut state.settings_form.dxcluster_port).desired_width(45.0).hint_text("7373"));
                            ui.add(TextEdit::singleline(&mut state.settings_form.dxcluster_username).desired_width(70.0).hint_text("login"));
                            ui.add(TextEdit::singleline(&mut state.settings_form.dxcluster_password).desired_width(70.0).password(true).hint_text("password"));
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut state.settings_form.allow_parking, "Allow parking")
                            .on_hover_text("Hold VMail for other stations until they connect (off: refused with <SMFP>)");
                        ui.checkbox(&mut state.settings_form.send_relay_notifications, "Send relay notifications")
                            .on_hover_text("Tell a station whose beacon you hear that VMail waits here for it (again after 20 minutes)");
                    });
                    ui.horizontal(|ui| {
                        ui.label("Incoming files folder:");
                        ui.add(TextEdit::singleline(&mut state.settings_form.incoming_files_dir).desired_width(260.0).hint_text("Downloads"));
                        if ui.small_button("Browse…").clicked() {
                            if let Some(d) = rfd::FileDialog::new().pick_folder() {
                                state.settings_form.incoming_files_dir = d.display().to_string();
                            }
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("Incoming files size limit (bytes):");
                        ui.add(TextEdit::singleline(&mut state.settings_form.incoming_files_size_limit).desired_width(70.0))
                            .on_hover_text("Smaller files and VMail attachments are accepted at once; bigger ones are refused while away, else you're asked (VarAC: 5000)");
                    });
                    ui.add_space(6.0);
                    ui.label(RichText::new("Callsigns block list").strong());
                    ui.add(TextEdit::singleline(&mut state.settings_form.callsigns_block_list).desired_width(420.0).hint_text("CALL1,CALL2"))
                        .on_hover_text("Comma separated. Their connects are aborted and their beacons, CQs and broadcasts are ignored.");
                    ui.add_space(6.0);
                    let mut em = state.settings_form.emcomm_mode;
                    if ui.checkbox(&mut em, RichText::new("EmComm mode").strong().color(RED))
                        .on_hover_text("VarAC's emergency communications mode: beacons as CALL-8, check-in requests, \
                                        and the options below. Avoid it on a calling frequency.").changed()
                    {
                        // (VarAC asks first on a calling frequency)
                        if em && crate::varac::messages::is_calling_frequency(state.dial()) {
                            state.emcomm_confirm = true;
                        } else {
                            state.set_emcomm(em);
                        }
                    }
                    ui.add_enabled_ui(state.settings_form.emcomm_mode, |ui| {
                        ui.indent("emcomm_opts", |ui| {
                            ui.checkbox(&mut state.settings_form.emcomm_auto_reply_checkin, "Answer check-in requests with a beacon");
                            ui.checkbox(&mut state.settings_form.emcomm_allow_last_connections, "Allow last-connections peeking")
                                .on_hover_text("Stations may ask for your last 5 QSOs (<LCR>)");
                            ui.checkbox(&mut state.settings_form.emcomm_auto_file_approval, "Accept incoming files automatically");
                            ui.checkbox(&mut state.settings_form.emcomm_qsy_notify, "Notify a scheduled QSY by broadcast")
                                .on_hover_text("Before the frequency scheduler retunes, a broadcast to ALL tells where we go (<QF:…>)");
                            ui.checkbox(&mut state.settings_form.emcomm_disable_snr, "Don't send SNR reports");
                            ui.checkbox(&mut state.settings_form.emcomm_disable_call_id, "Don't send Call ID");
                        });
                    });
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut state.settings_form.auto_away_enabled, "Auto away in");
                        ui.add(TextEdit::singleline(&mut state.settings_form.auto_away_minutes).desired_width(40.0));
                        ui.label("minutes");
                    });
                    ui.checkbox(&mut state.auto_qsy_enabled, "Allow auto-QSY (within allowed frequency ranges)");
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label("Connection timeout:");
                        let mut timeout = state.connect_timeout_secs.to_string();
                        if ui.add(TextEdit::singleline(&mut timeout).desired_width(40.0)).changed() {
                            if let Ok(v) = timeout.parse::<u32>() {
                                state.connect_timeout_secs = v.clamp(10, 120);
                            }
                        }
                        ui.label("seconds (10-120)");
                    });
                }
                3 => {
                    // CQ / Beacon tab
                    ui.horizontal(|ui| {
                        ui.label("Beacon interval (minutes):");
                        // VarAC's choices (at least 15 minutes on VARA HF)
                        egui::ComboBox::from_id_salt("beacon_interval").selected_text(state.settings_form.beacon_interval.clone()).show_ui(ui, |ui| {
                            // (VARA FM, and EmComm, may beacon every 5–10 minutes)
                            let short = state.settings_form.vara_modem_type == "VaraFM" || state.settings_form.emcomm_mode;
                            let choices: &[&str] = if short { &["5", "6", "7", "8", "9", "10", "15", "20", "30", "45", "60", "90", "120", "180"] } else { &["15", "20", "30", "45", "60", "90", "120", "180"] };
                            for m in choices.iter().copied() {
                                ui.selectable_value(&mut state.settings_form.beacon_interval, m.to_string(), m);
                            }
                        });
                    });
                    ui.checkbox(&mut state.settings_form.start_beacon_on_startup, "Start beacon on startup");
                    ui.checkbox(&mut state.settings_form.unlimited_beacon_time, "Unlimited beacon time")
                        .on_hover_text("Off (VarAC's default): the beacon shuts down within 72 hours and must be re-enabled.");
                    ui.horizontal(|ui| {
                        ui.label("Beacon type:");
                        let f = &mut state.settings_form;
                        if ui.radio(f.beacon_type != "advanced", "STANDARD").clicked() {
                            f.beacon_type = "standard".into();
                        }
                        if ui.radio(f.beacon_type == "advanced", "ADVANCED").clicked() {
                            f.beacon_type = "advanced".into();
                        }
                    });
                    ui.colored_label(GRAY, RichText::new(
                        "Standard: a VARA CQ frame (CALL-9). Advanced: VarAC's KISS beacon with your locator and \
                         announced services (e.g. your email gateway); needs the modem's KISS port.",
                    ).small().italics());
                    ui.horizontal(|ui| {
                        ui.label("Auto-CQ repeat interval (seconds):");
                        let mut interval = state.auto_cq_interval_secs.to_string();
                        if ui.add(TextEdit::singleline(&mut interval).desired_width(40.0)).changed() {
                            if let Ok(v) = interval.parse::<u32>() {
                                state.auto_cq_interval_secs = v.clamp(10, 300);
                            }
                        }
                    });
                }
                4 => {
                    // Display tab
                    ui.horizontal(|ui| {
                        ui.label("Font size:");
                        ui.add(TextEdit::singleline(&mut state.settings_form.font_size).desired_width(40.0));
                        ui.label("px (10-24, default 14)");
                    });
                    ui.checkbox(&mut state.settings_form.notifications_enabled, "Desktop notifications");
                    ui.colored_label(GRAY, RichText::new("Notifications for connections, VMail, file offers, and messages while away.").small().italics());
                    ui.checkbox(&mut state.settings_form.sound_alerts, "Sound alerts");
                    ui.add_space(6.0);
                    features::draw_sound_settings(ui);
                    ui.colored_label(GRAY, RichText::new("Play audio alert on connection, incoming VMail, and file offers.").small().italics());
                    ui.add_space(4.0);
                    ui.checkbox(&mut state.settings_form.use_local_time, "Use local time (instead of UTC)");
                    ui.checkbox(&mut state.settings_form.modem_auto_reconnect, "Auto-reconnect modem on disconnect");
                    ui.colored_label(GRAY, RichText::new("Automatically retry modem connection every 10 seconds when disconnected.").small().italics());
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label("Theme:");
                        if ui.selectable_label(state.dark_mode, "Dark").clicked() {
                            state.dark_mode = true;
                        }
                        if ui.selectable_label(!state.dark_mode && !state.scifi_theme, "Light").clicked() {
                            state.dark_mode = false;
                            state.scifi_theme = false;
                        }
                        if ui.selectable_label(state.scifi_theme, "Sci-Fi").on_hover_text("VarAC's SCIFI theme (dark)").clicked() {
                            state.scifi_theme = !state.scifi_theme;
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("Distances in:");
                        ui.radio_value(&mut state.settings_form.distance_unit, "MI".to_string(), "miles");
                        ui.radio_value(&mut state.settings_form.distance_unit, "KM".to_string(), "km");
                    });
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label("QSO duration alert:");
                        ui.add(TextEdit::singleline(&mut state.settings_form.qso_alert_minutes).desired_width(40.0));
                        ui.label("min (0 = off)");
                    });
                    ui.colored_label(GRAY, RichText::new("Notification after N minutes in a QSO.").small().italics());
                    ui.add_space(4.0);
                    ui.checkbox(&mut state.dx_alert_enabled, "DX alert (notify on foreign CQ stations)");
                    ui.colored_label(GRAY, RichText::new("Desktop notification when a CQ from another country is heard.").small().italics());
                }
                6 => draw_email_settings(ui, &mut state.settings_form, station_config.email_gate.as_ref()),
                5 => {
                    // RIG tab — grouped like VarAC's: PTT + CAT on the left, frequency control + Hamlib on the right
                    let group = |ui: &mut egui::Ui, title: &str, body: &mut dyn FnMut(&mut egui::Ui)| {
                        theme::card().show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            theme::caption(ui, title);
                            ui.add_space(2.0);
                            body(ui);
                        });
                        ui.add_space(6.0);
                    };
                    ui.columns(2, |cols| {
                        // ---- PTT Configuration
                        group(&mut cols[0], "PTT Configuration", &mut |ui| {
                            let rig_on = state.rig_settings_backend != "none";
                            ui.add_enabled_ui(rig_on, |ui| {
                                ui.radio_value(&mut state.rig_settings_ptt, true, "CAT — key the radio through rig control");
                            });
                            ui.radio_value(&mut state.rig_settings_ptt, false, "None — VARA keys the radio (VOX / VARA PTT)");
                            ui.horizontal(|ui| {
                                ui.label("TEST");
                                let can = state.rig_connected;
                                if ui.add_enabled(can, theme::accent_button("PTT ON", theme::RED)).on_hover_text("Key the transmitter (use a dummy load or a clear frequency)").clicked() {
                                    state.rig_test_ptt = Some(true);
                                }
                                if ui.add_enabled(can, egui::Button::new("PTT OFF")).clicked() {
                                    state.rig_test_ptt = Some(false);
                                }
                                if state.rig_ptt {
                                    theme::pill(ui, "TX", theme::TX_RED);
                                }
                            });
                        });

                        // ---- CAT Configuration
                        group(&mut cols[0], "CAT Configuration", &mut |ui| {
                            ui.horizontal(|ui| {
                                ui.radio_value(&mut state.rig_settings_backend, "cat".into(), "COM Port");
                                ui.radio_value(&mut state.rig_settings_backend, "rigctld".into(), "TCP (rigctld)");
                                ui.radio_value(&mut state.rig_settings_backend, "flrig".into(), "FLRig");
                                ui.radio_value(&mut state.rig_settings_backend, "none".into(), "None");
                            });
                            if state.rig_settings_backend != "none" {
                                ui.horizontal(|ui| {
                                    ui.label("Radio mode");
                                    egui::ComboBox::from_id_salt("usb_mode")
                                        .selected_text(state.settings_form.usb_mode.clone())
                                        .show_ui(ui, |ui| {
                                            for m in ["USB", "USB-D", "DATA-U", "FM", "FM-D"] {
                                                ui.selectable_value(&mut state.settings_form.usb_mode, m.to_string(), m);
                                            }
                                        });
                                    ui.colored_label(GRAY, RichText::new("set before every frequency change, as in VarAC").small().italics());
                                });
                            }
                            if state.rig_settings_backend == "cat" {
                                egui::Grid::new("cat_grid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                                    ui.label("Radio");
                                    egui::ComboBox::from_id_salt("rig_model")
                                        .selected_text(match state.rig_settings_model.as_str() {
                                            "yaesu" => "Yaesu FT-991/891/710, FTDX10/101",
                                            "kenwood" | "qmx" => "Kenwood / QRP Labs QMX",
                                            _ => "Yaesu FTDX3000 / FT-950 / FTDX1200",
                                        })
                                        .width(230.0)
                                        .show_ui(ui, |ui| {
                                            ui.selectable_value(&mut state.rig_settings_model, "ftdx3000".into(), "Yaesu FTDX3000 / FT-950 / FTDX1200");
                                            ui.selectable_value(&mut state.rig_settings_model, "yaesu".into(), "Yaesu FT-991/891/710, FTDX10/101");
                                            ui.selectable_value(&mut state.rig_settings_model, "kenwood".into(), "Kenwood / QRP Labs QMX");
                                        });
                                    ui.end_row();
                                    ui.label("Port");
                                    ui.horizontal(|ui| {
                                        let shown = state
                                            .rig_ports
                                            .iter()
                                            .find(|p| p.name == state.rig_settings_serial)
                                            .map_or_else(|| state.rig_settings_serial.clone(), |p| p.label());
                                        egui::ComboBox::from_id_salt("rig_serial_pick")
                                            .selected_text(shown)
                                            .width(320.0)
                                            .show_ui(ui, |ui| {
                                                if state.rig_ports.is_empty() {
                                                    ui.label("No serial ports found");
                                                }
                                                for p in state.rig_ports.clone() {
                                                    ui.selectable_value(&mut state.rig_settings_serial, p.name.clone(), p.label());
                                                }
                                            });
                                        if ui.small_button("Rescan").clicked() {
                                            state.rig_ports = crate::rig::list_serial_ports();
                                        }
                                    });
                                    ui.end_row();
                                    ui.label("");
                                    ui.add(TextEdit::singleline(&mut state.rig_settings_serial).desired_width(230.0).hint_text("/dev/ttyUSB0 or COM5"));
                                    ui.end_row();
                                    ui.label("Baud");
                                    egui::ComboBox::from_id_salt("rig_baud")
                                        .selected_text(&state.rig_settings_baud)
                                        .width(100.0)
                                        .show_ui(ui, |ui| {
                                            for b in ["4800", "9600", "19200", "38400", "57600", "115200"] {
                                                ui.selectable_value(&mut state.rig_settings_baud, b.to_string(), b);
                                            }
                                        });
                                    ui.end_row();
                                });
                                ui.horizontal(|ui| {
                                    let detecting = state.rig_detect_rx.is_some();
                                    if ui
                                        .add_enabled(!detecting, egui::Button::new("Detect radio"))
                                        .on_hover_text("Asks each serial port for the radio's ID (read-only, never keys the radio) and fills in radio, port and baud")
                                        .clicked()
                                    {
                                        state.rig_detect_requested = true;
                                    }
                                    if detecting {
                                        ui.spinner();
                                        ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
                                    }
                                    if let Some(s) = &state.rig_detect_status {
                                        let color = if s.starts_with("Found") { theme::GREEN } else if detecting { GRAY } else { RED };
                                        ui.colored_label(color, RichText::new(s).small());
                                    }
                                });
                                ui.label(RichText::new("8 data bits, no parity; 2 stop bits for Yaesu, 1 for Kenwood. DTR held low, RTS high for Yaesu (CAT RTS).").small().color(GRAY));
                                ui.label(RichText::new("Radios with two serial ports (Yaesu FTDX3000, FT-991, FT-891, FT-710, FTDX10/101) take CAT on the one marked (CAT), the Enhanced port; the other is for PTT/keying. The baud rate is the radio's CAT RATE menu setting.").small().color(GRAY));
                            } else if state.rig_settings_backend == "rigctld" {
                                ui.label(RichText::new("Host and port are set under Hamlib (Rigctld) on the right").small().color(GRAY));
                            } else if state.rig_settings_backend == "flrig" {
                                ui.label(RichText::new("Host and port are set under FLRig on the right").small().color(GRAY));
                            }
                        });

                        // ---- Frequency Control (status)
                        group(&mut cols[1], "Frequency Control", &mut |ui| {
                            ui.horizontal(|ui| {
                                if state.rig_connected {
                                    theme::status_pill(ui, "Connected", theme::GREEN);
                                } else if state.rig_backend != crate::rig::RigBackend::None {
                                    theme::status_pill(ui, "Not connected", theme::YELLOW);
                                } else {
                                    theme::status_pill(ui, "Off", theme::MUTED);
                                }
                                ui.label(format!("{}", state.rig_backend));
                            });
                            if state.rig_connected {
                                if let Some(f) = state.rig_freq {
                                    ui.label(RichText::new(format!("{:.3} kHz", f as f64 / 1000.0)).monospace().size(18.0));
                                }
                                if let Some(m) = state.rig_mode {
                                    ui.label(format!("Mode: {m}"));
                                }
                                ui.label(RichText::new("Frequency is read every 1.5 s; band changes and accepted QSYs tune the radio.").small().color(GRAY));
                            } else if let Some(ref e) = state.rig_error {
                                ui.colored_label(RED, RichText::new(e).small());
                            }
                            ui.add_space(4.0);
                            if ui.add(theme::accent_button("TEST / Apply & reconnect", CYAN)).clicked() {
                                state.rig_reconnect_requested = true;
                            }
                        });

                        // ---- Hamlib (Rigctld)
                        group(&mut cols[1], "Hamlib (Rigctld) / RigHelm", &mut |ui| {
                            ui.add_enabled_ui(state.rig_settings_backend == "rigctld", |ui| {
                                egui::Grid::new("rigctld_grid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                                    ui.label("Host");
                                    ui.add(TextEdit::singleline(&mut state.rig_settings_host).desired_width(160.0));
                                    ui.end_row();
                                    ui.label("Port");
                                    ui.horizontal(|ui| {
                                        ui.add(TextEdit::singleline(&mut state.rig_settings_port).desired_width(60.0));
                                        if ui.small_button("Default 4532").clicked() {
                                            state.rig_settings_host = "127.0.0.1".into();
                                            state.rig_settings_port = "4532".into();
                                        }
                                    });
                                    ui.end_row();
                                });
                            });
                            ui.label(RichText::new("RigHelm: enable its rigctld server (Settings > Ports & sharing). It drives ~30 Yaesu, Icom, Kenwood and Elecraft radios and shares the CAT port with WSJT-X and others.").small().color(GRAY));
                        });

                        // ---- FLRig (XML-RPC)
                        group(&mut cols[1], "FLRig", &mut |ui| {
                            ui.add_enabled_ui(state.rig_settings_backend == "flrig", |ui| {
                                egui::Grid::new("flrig_grid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                                    ui.label("Host");
                                    ui.add(TextEdit::singleline(&mut state.rig_settings_host).desired_width(160.0));
                                    ui.end_row();
                                    ui.label("Port");
                                    ui.horizontal(|ui| {
                                        ui.add(TextEdit::singleline(&mut state.rig_settings_port).desired_width(60.0));
                                        if ui.small_button("Default 12345").clicked() {
                                            state.rig_settings_host = "127.0.0.1".into();
                                            state.rig_settings_port = "12345".into();
                                        }
                                    });
                                    ui.end_row();
                                });
                            });
                            ui.label(RichText::new("FLRig's XML-RPC server: frequency, mode, PTT and the antenna tuner, as VarAC uses it.").small().color(GRAY));
                        });
                    });
                }
                7 => features::draw_ai_settings(ui, &mut state.settings_form),
                _ => {}
            }

            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.add(theme::accent_button("SAVE AND EXIT", GREEN)).clicked() {
                    save_config_file(state);
                    // Who may use the gateway applies right away
                    if let Some(gate) = &station_config.email_gate {
                        let (enabled, allow, _) = crate::config::gateway_policy();
                        let address = state.settings_form.gateway_email.trim().to_string();
                        gate.update(|g| {
                            g.enabled = enabled;
                            g.allow = allow;
                            g.address = address;
                        });
                    }
                    if let Some(gate) = &station_config.bbs_gate {
                        let (allow, _) = crate::config::bbs_policy();
                        gate.update(|g| g.allow = allow);
                    }
                    let f = &state.settings_form;
                    station_config.policy.set(f.allow_info_request, f.allow_pings, f.verbose_snr_auto_accept);
                    station_config.policy.set_welcome(&f.welcome_message, f.welcome_always);
                    station_config.policy.set_email(&f.email);
                    station_config.policy.set_full_callsign(&f.full_callsign);
                    station_config.policy.set_profile(crate::config::profile_extra());
                    state.psk_reconfigure = true;
                    state.block_list = crate::config::parse_block_list(&state.settings_form.callsigns_block_list);
                    station_config.policy.set_blocked(state.block_list.clone());
                    station_config.policy.file_size_limit.store(f.incoming_files_size_limit.trim().parse().unwrap_or(5000), std::sync::atomic::Ordering::Relaxed);
                    station_config.policy.auto_file.store(f.emcomm_mode && f.emcomm_auto_file_approval, std::sync::atomic::Ordering::Relaxed);
                    station_config.policy.allow_parking.store(f.allow_parking, std::sync::atomic::Ordering::Relaxed);
                    station_config.policy.ai_gateway.store(f.ai_enabled && f.ai_announce, std::sync::atomic::Ordering::Relaxed);
                    station_config.policy.set_emcomm(
                        f.emcomm_mode && f.emcomm_disable_snr,
                        f.emcomm_mode && f.emcomm_allow_last_connections,
                    );
                    state.send_cmd(GuiCommand::ApplyPingPolicy);
                    state.auto_log_qso = state.settings_form.auto_log_qso;
                    let new_size: f32 = state.settings_form.font_size.parse().unwrap_or(14.0);
                    state.font_size = new_size.clamp(10.0, 24.0);
                    state.use_local_time = state.settings_form.use_local_time;
                    state.modem_auto_reconnect = state.settings_form.modem_auto_reconnect;
                    state.qso_alert_minutes = state.settings_form.qso_alert_minutes.parse().unwrap_or(0);
                    // Apply bandwidth change live
                    match state.settings_form.bandwidth.as_str() {
                        "500" => state.send_cmd(GuiCommand::SetBandwidth(Bandwidth::Bw500)),
                        "2300" => state.send_cmd(GuiCommand::SetBandwidth(Bandwidth::Bw2300)),
                        "2750" => state.send_cmd(GuiCommand::SetBandwidth(Bandwidth::Bw2750)),
                        _ => {}
                    }
                    state.rig_reconnect_requested = true;
                    state.status_msg = "Settings saved".into();
                    state.settings_open = false;
                }
                if ui.button("Close").clicked() {
                    state.settings_open = false;
                }
            });

            ui.add_space(4.0);
            ui.colored_label(GRAY, RichText::new("Note: Callsign and modem host/port changes take effect on next restart. Bandwidth applies immediately.").small().italics());
        });
    state.settings_open = open && state.settings_open; // (a button inside may have closed it)
}

/// Settings → Email: host an email gateway (VarAC stations send email through us).
fn draw_email_settings(ui: &mut egui::Ui, f: &mut state::SettingsForm, gate: Option<&crate::varac::email_gateway::EmailGate>) {
    if let Some(rx) = &f.email_test_rx {
        if let Ok(r) = rx.try_recv() {
            f.email_test_status = Some(match r {
                Ok(()) => format!("Test email sent to {}", f.gateway_email.trim()),
                Err(e) => format!("Failed: {e}"),
            });
            f.email_test_rx = None;
        }
    }
    ui.checkbox(&mut f.email_gateway, "Act as an email gateway");
    ui.colored_label(GRAY, RichText::new(
        "Announces E to connecting stations; VMails they address to an email address are mailed out \
         through the SMTP server below as \"CALL VIA YOURCALL\", with VarAC's reply-routing footer.",
    ).small().italics());
    ui.add_space(6.0);
    egui::Grid::new("email_grid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
        ui.label("SMTP server:");
        ui.add(TextEdit::singleline(&mut f.smtp_server).desired_width(220.0).hint_text("smtp.gmail.com"));
        ui.end_row();
        ui.label("Port:");
        ui.add(TextEdit::singleline(&mut f.smtp_port).desired_width(60.0));
        ui.end_row();
        ui.label("Security:");
        ui.horizontal(|ui| {
            for (key, label, port) in [("starttls", "STARTTLS", "587"), ("tls", "SSL/TLS", "465"), ("none", "None", "25")] {
                if ui.radio(f.smtp_security == key, label).clicked() {
                    f.smtp_security = key.into();
                    f.smtp_port = port.into();
                }
            }
        });
        ui.end_row();
        ui.label("Username:");
        ui.add(TextEdit::singleline(&mut f.smtp_username).desired_width(220.0).hint_text("usually the email address"));
        ui.end_row();
        ui.label("Password:");
        ui.add(TextEdit::singleline(&mut f.smtp_password).password(true).desired_width(220.0).hint_text("app password"));
        ui.end_row();
        ui.label("Gateway email:");
        ui.add(TextEdit::singleline(&mut f.gateway_email).desired_width(220.0).hint_text("you@example.com"));
        ui.end_row();
    });
    ui.colored_label(GRAY, RichText::new(
        "The password is stored in plain text in the config file. With Gmail, use an app password.",
    ).small().italics());
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let sending = f.email_test_rx.is_some();
        if ui.add_enabled(!sending, egui::Button::new("Send test email")).clicked() {
            let cfg = crate::varac::email_gateway::GatewayConfig {
                server: f.smtp_server.trim().into(),
                port: f.smtp_port.trim().parse().unwrap_or(587),
                security: crate::varac::email_gateway::SmtpSecurity::from_key(&f.smtp_security),
                username: f.smtp_username.trim().into(),
                password: f.smtp_password.clone(),
                from_address: f.gateway_email.trim().into(),
            };
            let call = if f.callsign.trim().is_empty() { "N0CALL".to_string() } else { f.callsign.trim().to_uppercase() };
            let vm = crate::varac::messages::VMail {
                guid: String::new(),
                from: call.clone(),
                to: cfg.from_address.clone(),
                subject: "Openslot email gateway test".into(),
                body: "This is a test email from your Openslot email gateway.".into(),
                compose_time: Utc::now().format("%Y-%m-%d %H:%M:%S").to_string(),
                timestamp: Utc::now(),
                urgent: false,
                is_parked: false,
                has_attachment: false,
                reply_to: None,
                raw: false,
            };
            let d = crate::varac::email_gateway::Delivery { sender_call: call.clone(), gateway_call: call, ..Default::default() };
            let (tx, rx) = mpsc::channel();
            f.email_test_rx = Some(rx);
            f.email_test_status = Some("Sending…".into());
            std::thread::spawn(move || {
                let _ = tx.send(crate::varac::email_gateway::send(&cfg, &vm, &d));
            });
        }
        if let Some(s) = &f.email_test_status {
            let color = if s.starts_with("Failed") { egui::Color32::from_rgb(230, 90, 90) } else { GRAY };
            ui.colored_label(color, s);
        }
    });
    ui.colored_label(GRAY, RichText::new(
        "Turning the gateway on or off changes what we announce after a restart; server details apply to the next email.",
    ).small().italics());

    ui.add_space(8.0);
    if let Some(gate) = gate.filter(|g| g.state().suspended) {
        ui.horizontal(|ui| {
            ui.colored_label(egui::Color32::from_rgb(230, 90, 90), "Gateway shut down after repeated send failures.");
            if ui.button("Resume gateway").clicked() {
                gate.update(|g| g.suspended = false);
            }
        });
    }
    ui.checkbox(&mut f.email_gateway_auto_relay, "Auto relay: send my queued emails through a gateway without asking");
    ui.horizontal(|ui| {
        ui.label("Retries before shutting down:");
        ui.add(TextEdit::singleline(&mut f.smtp_retries).desired_width(40.0));
    });
    ui.checkbox(&mut f.email_gateway_limit, "Limit access to these callsigns:");
    ui.add_enabled(
        f.email_gateway_limit,
        TextEdit::singleline(&mut f.email_gateway_allow).desired_width(320.0).hint_text("W5TST,K5XYZ-1 (full callsigns, comma separated)"),
    );
    ui.colored_label(GRAY, RichText::new(
        "Others are refused with <EJ> (\"does not allow you to send emails\"). Applies when you save.",
    ).small().italics());
    ui.add_enabled_ui(!f.email_gateway_limit, |ui| {
        ui.checkbox(&mut f.email_gateway_announce, "Announce the gateway in my beacons");
    });
    ui.colored_label(GRAY, RichText::new(
        "Like VarAC: only an open gateway (no callsign limit) is announced, and only in ADVANCED beacons \
         (Settings → CQ / Beacon).",
    ).small().italics());

    ui.add_space(10.0);
    ui.separator();
    ui.heading("Incoming emails (IMAP)");
    if let Some(rx) = &f.imap_test_rx {
        if let Ok(r) = rx.try_recv() {
            f.imap_test_status = Some(match r {
                Ok(s) => format!("Connected. {s}"),
                Err(e) => format!("Failed: {e}"),
            });
            f.imap_test_rx = None;
        }
    }
    ui.checkbox(&mut f.imap_enabled, "Enable incoming emails");
    ui.colored_label(GRAY, RichText::new(
        "Checks the gateway mailbox and parks each email for its station: replies to forwarded \
         emails, or mail addressed to \"CALL VIA YOURCALL\". The station collects it on its next \
         connect (relay notification on its beacon), and the sender is emailed when it's delivered.",
    ).small().italics());
    egui::Grid::new("imap_grid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
        ui.label("IMAP server:");
        ui.add(TextEdit::singleline(&mut f.imap_server).desired_width(220.0).hint_text("imap.gmail.com"));
        ui.end_row();
        ui.label("Port:");
        ui.add(TextEdit::singleline(&mut f.imap_port).desired_width(60.0));
        ui.end_row();
        ui.label("Security:");
        ui.horizontal(|ui| {
            for (key, label, port) in [("tls", "SSL/TLS", "993"), ("starttls", "STARTTLS", "143"), ("none", "None", "143")] {
                if ui.radio(f.imap_security == key, label).clicked() {
                    f.imap_security = key.into();
                    f.imap_port = port.into();
                }
            }
        });
        ui.end_row();
        ui.label("Username:");
        ui.add(TextEdit::singleline(&mut f.imap_username).desired_width(220.0).hint_text("empty = SMTP username"));
        ui.end_row();
        ui.label("Password:");
        ui.add(TextEdit::singleline(&mut f.imap_password).password(true).desired_width(220.0).hint_text("empty = SMTP password"));
        ui.end_row();
        ui.label("Check every (sec):");
        ui.add(TextEdit::singleline(&mut f.imap_poll_seconds).desired_width(60.0));
        ui.end_row();
    });
    ui.horizontal(|ui| {
        let testing = f.imap_test_rx.is_some();
        if ui.add_enabled(!testing, egui::Button::new("IMAP test")).clicked() {
            let pick = |v: &str, smtp: &str| if v.trim().is_empty() { smtp.trim().to_string() } else { v.trim().to_string() };
            let cfg = crate::varac::email_inbound::ImapConfig {
                server: f.imap_server.trim().into(),
                port: f.imap_port.trim().parse().unwrap_or(993),
                security: crate::varac::email_gateway::SmtpSecurity::from_key(&f.imap_security),
                username: pick(&f.imap_username, &f.smtp_username),
                password: if f.imap_password.is_empty() { f.smtp_password.clone() } else { f.imap_password.clone() },
            };
            let (tx, rx) = mpsc::channel();
            f.imap_test_rx = Some(rx);
            f.imap_test_status = Some("Connecting…".into());
            std::thread::spawn(move || {
                let _ = tx.send(crate::varac::email_inbound::test_connection(&cfg));
            });
        }
        if let Some(s) = &f.imap_test_status {
            let color = if s.starts_with("Failed") { egui::Color32::from_rgb(230, 90, 90) } else { GRAY };
            ui.colored_label(color, s);
        }
    });
}

fn save_config_file(state: &GuiState) {
    let form = &state.settings_form;
    // Keys only the config file sets (no Settings field) are kept, not wiped
    let existing = load_config_file().unwrap_or_default();
    let cfg = crate::config::ConfigFile {
        callsign: if form.callsign.is_empty() { None } else { Some(form.callsign.clone()) },
        name: if form.name.is_empty() { None } else { Some(form.name.clone()) },
        qth: if form.qth.is_empty() { None } else { Some(form.qth.clone()) },
        locator: if form.locator.is_empty() { None } else { Some(form.locator.clone()) },
        host: if form.modem_host.is_empty() { None } else { Some(form.modem_host.clone()) },
        cmd_port: form.cmd_port.parse().ok(),
        data_port: form.data_port.parse().ok(),
        kiss_port: form.kiss_port.trim().parse().ok(),
        bandwidth: if form.bandwidth.is_empty() { None } else { Some(form.bandwidth.clone()) },
        vmail: Some(form.vmail_enabled),
        bbs: Some(form.bbs_enabled),
        db_path: existing.db_path.clone(),
        beacon_interval: form.beacon_interval.parse().ok(),
        auto_log_qso: Some(form.auto_log_qso),
        auto_away_enabled: Some(form.auto_away_enabled),
        auto_away_minutes: form.auto_away_minutes.parse().ok(),
        allow_pings: Some(form.allow_pings),
        allow_info_request: Some(form.allow_info_request),
        load_history: Some(form.load_history),
        font_size: form.font_size.parse().ok(),
        notifications_enabled: Some(form.notifications_enabled),
        sound_alerts: Some(form.sound_alerts),
        use_local_time: Some(form.use_local_time),
        modem_auto_reconnect: Some(form.modem_auto_reconnect),
        frequency: Some(state.current_frequency),
        band: if state.current_band.is_empty() { None } else { Some(state.current_band.clone()) },
        bbs_directory: if form.bbs_directory.is_empty() { None } else { Some(form.bbs_directory.clone()) },
        watchlist: if state.watchlist.is_empty() { None } else { Some(state.watchlist.clone()) },
        callsign_tags: if state.callsign_tags.is_empty() { None } else { Some(state.callsign_tags.clone()) },
        auto_qsy_ranges: existing.auto_qsy_ranges.clone(),
        my_frequencies: existing.my_frequencies.clone(),
        dark_mode: Some(state.dark_mode),
        ui_theme: Some(if state.scifi_theme { "SCIFI" } else { "DEFAULT" }.into()),
        qso_alert_minutes: Some(state.qso_alert_minutes),
        dx_alert_enabled: Some(state.dx_alert_enabled),
        connect_timeout_secs: Some(state.connect_timeout_secs),
        scanner_dwell_secs: None,
        auto_cq_interval_secs: Some(state.auto_cq_interval_secs),
        auto_qsy_enabled: Some(state.auto_qsy_enabled),
        beacon_auto_enabled: Some(state.beacon_auto_enabled),
        start_beacon_on_startup: Some(state.settings_form.start_beacon_on_startup),
        vara_keepalive_secs: existing.vara_keepalive_secs,
        send_is_typing: Some(state.is_typing_enabled),
        suppress_typing_low_snr: Some(state.settings_form.suppress_typing_low_snr),
        ptt_disabled: Some(state.ptt_disabled),
        disable_vmail_queued_popup: Some(crate::config::vmail_popups_disabled().0),
        disable_leave_vmail_popup: Some(crate::config::vmail_popups_disabled().1),
        ignore_dcd: Some(state.ignore_dcd),
        unattended_links: Some(state.unattended_links),
        cq_slot: Some(state.cq_slot),
        rig_backend: if state.rig_settings_backend == "none" { None } else { Some(state.rig_settings_backend.clone()) },
        rig_host: if state.rig_settings_host.is_empty() { None } else { Some(state.rig_settings_host.clone()) },
        rig_port: state.rig_settings_port.parse().ok(),
        rig_serial_port: if state.rig_settings_serial.is_empty() { None } else { Some(state.rig_settings_serial.clone()) },
        rig_baud: state.rig_settings_baud.parse().ok(),
        rig_model: if state.rig_settings_model.is_empty() { None } else { Some(state.rig_settings_model.clone()) },
        rig_ptt: Some(state.rig_settings_ptt),
        email_gateway: Some(form.email_gateway),
        smtp_server: Some(form.smtp_server.trim().to_string()).filter(|s| !s.is_empty()),
        smtp_port: form.smtp_port.trim().parse().ok(),
        smtp_security: Some(form.smtp_security.clone()),
        smtp_username: Some(form.smtp_username.trim().to_string()).filter(|s| !s.is_empty()),
        smtp_password: Some(form.smtp_password.clone()).filter(|s| !s.is_empty()),
        gateway_email: Some(form.gateway_email.trim().to_string()).filter(|s| !s.is_empty()),
        email_gateway_limit: Some(form.email_gateway_limit),
        email_gateway_allow: Some(form.email_gateway_allow.trim().to_uppercase()).filter(|s| !s.is_empty()),
        smtp_retries: form.smtp_retries.trim().parse().ok(),
        email_gateway_announce: Some(form.email_gateway_announce),
        email_gateway_auto_relay: Some(form.email_gateway_auto_relay),
        verbose_snr_auto_accept: Some(form.verbose_snr_auto_accept),
        allow_last_heard: Some(form.allow_last_heard),
        allow_path_finder: Some(form.allow_path_finder),
        adif_log_path: Some(form.adif_log_path.trim().to_string()).filter(|v| !v.is_empty()),
        adif_submode: Some(form.adif_submode.trim().to_string()).filter(|s| !s.is_empty()),
        default_log_comment: Some(form.default_log_comment.clone()),
        logger_app: Some(form.logger_app.clone()),
        logger_ip: Some(form.logger_ip.trim().to_string()).filter(|v| !v.is_empty()),
        logger_port: form.logger_port.trim().parse().ok(),
        logger_app2: Some(form.logger_app2.clone()),
        logger_ip2: Some(form.logger_ip2.trim().to_string()).filter(|v| !v.is_empty()),
        logger_port2: form.logger_port2.trim().parse().ok(),
        logger_callsign_lookup: Some(form.logger_callsign_lookup),
        logger_api_port: None,
        pskreporter_upload: Some(form.pskreporter_upload),
        pskreporter_self_report: Some(form.pskreporter_self_report),
        callsigns_block_list: Some(form.callsigns_block_list.trim().to_uppercase()),
        unlimited_beacon_time: Some(form.unlimited_beacon_time),
        hamplay_enabled: Some(form.hamplay_enabled),
        ai_enabled: Some(form.ai_enabled),
        ai_api_key: Some(form.ai_api_key.trim().to_string()).filter(|v| !v.is_empty()),
        ai_model: Some(form.ai_model.trim().to_string()).filter(|v| !v.is_empty()),
        ai_timeout_secs: form.ai_timeout_secs.trim().parse().ok(),
        ai_limit_chars: form.ai_limit_chars.trim().parse().ok(),
        ai_chat_context: Some(form.ai_chat_context),
        ai_limit_per_qso: form.ai_limit_per_qso.trim().parse().ok(),
        ai_announce: Some(form.ai_announce),
        ai_customization: Some(form.ai_customization.clone()).filter(|v| !v.trim().is_empty()),
        dxcluster_enabled: Some(form.dxcluster_enabled),
        dxcluster_host: Some(form.dxcluster_host.trim().to_string()).filter(|v| !v.is_empty()),
        dxcluster_port: form.dxcluster_port.trim().parse().ok(),
        dxcluster_username: Some(form.dxcluster_username.trim().to_string()).filter(|v| !v.is_empty()),
        dxcluster_password: Some(form.dxcluster_password.clone()).filter(|v| !v.is_empty()),
        schedule_start_mode: Some(form.schedule_start_mode.clone()),
        scanner_busy_hold_secs: form.scanner_busy_hold_secs.trim().parse().ok(),
        scanner_announce: Some(form.scanner_announce),
        vara_path: Some(form.vara_path.trim().to_string()).filter(|v| !v.is_empty()),
        vara_launch: Some(form.vara_launch),
        vara_launch_delay_secs: form.vara_launch_delay_secs.trim().parse().ok(),
        schedule_last_state: existing.schedule_last_state.clone(),
        qsl_image_file: existing.qsl_image_file.clone(),
        qsl_message: existing.qsl_message.clone(),
        qsl_fade: existing.qsl_fade,
        gps_manual: Some(form.gps_manual.trim().to_string()).filter(|g| !g.is_empty()),
        distance_unit: Some(form.distance_unit.clone()),
        incoming_files_dir: Some(form.incoming_files_dir.trim().to_string()).filter(|v| !v.is_empty()),
        digipeat_via: Some(form.digipeat_via.trim().to_uppercase()).filter(|v| !v.is_empty()),
        vara_modem_type: Some(form.vara_modem_type.clone()),
        usb_mode: Some(form.usb_mode.clone()),
        ale_snr_threshold: form.ale_snr_threshold.trim().parse().ok(),
        ale_history_days: form.ale_history_days.trim().parse().ok(),
        ale_recent_minutes: form.ale_recent_minutes.trim().parse().ok(),
        ale_skip_approval: Some(form.ale_skip_approval),
        ale_enabled: Some(form.ale_enabled),
        send_relay_notifications: Some(form.send_relay_notifications),
        allow_parking: Some(form.allow_parking),
        incoming_files_size_limit: form.incoming_files_size_limit.trim().parse().ok(),
        mute_all_sounds: existing.mute_all_sounds,
        sounds_off: existing.sounds_off.clone(),
        narrator: existing.narrator,
        narrator_female: existing.narrator_female,
        narrate_incoming: existing.narrate_incoming,
        narrate_alerts: existing.narrate_alerts,
        narrate_beacons: existing.narrate_beacons,
        narrate_my_beacons: existing.narrate_my_beacons,
        narrate_cqs: existing.narrate_cqs,
        narrate_broadcasts: existing.narrate_broadcasts,
        narrate_my_broadcasts: existing.narrate_my_broadcasts,
        emcomm_mode: Some(form.emcomm_mode),
        emcomm_disable_snr: Some(form.emcomm_disable_snr),
        emcomm_disable_call_id: Some(form.emcomm_disable_call_id),
        emcomm_allow_last_connections: Some(form.emcomm_allow_last_connections),
        emcomm_auto_file_approval: Some(form.emcomm_auto_file_approval),
        emcomm_qsy_notify: Some(form.emcomm_qsy_notify),
        emcomm_auto_reply_checkin: Some(form.emcomm_auto_reply_checkin),
        call_id_minutes: form.call_id_minutes.trim().parse().ok(),
        idle_disconnect_minutes: form.idle_disconnect_minutes.trim().parse().ok(),
        welcome_message: Some(form.welcome_message.trim().to_string()),
        welcome_always: Some(form.welcome_always),
        email: Some(form.email.trim().to_string()).filter(|e| !e.is_empty()),
        full_callsign: Some(form.full_callsign.trim().to_uppercase()).filter(|c| !c.is_empty()),
        my_rig: Some(form.my_rig.trim().to_string()).filter(|v| !v.is_empty()),
        my_power: Some(form.my_power.trim().trim_end_matches(['W', 'w']).trim().to_string()).filter(|v| !v.is_empty()),
        my_antenna: Some(form.my_antenna.trim().to_string()).filter(|v| !v.is_empty()),
        my_age: Some(form.my_age.trim().to_string()).filter(|v| !v.is_empty()),
        my_profession: Some(form.my_profession.trim().to_string()).filter(|v| !v.is_empty()),
        my_hobbies: Some(form.my_hobbies.trim().to_string()).filter(|v| !v.is_empty()),
        multi_band_last_heard: Some(form.multi_band_last_heard),
        bbs_limit: Some(form.bbs_limit),
        bbs_allow: Some(form.bbs_allow.trim().to_uppercase()).filter(|s| !s.is_empty()),
        bbs_announce: Some(form.bbs_announce),
        beacon_type: Some(form.beacon_type.clone()),
        cq_slot_wait_secs: form.cq_slot_wait_secs.trim().parse().ok(),
        skip_cq_slot_selector: Some(form.skip_cq_slot_selector),
        slot_step_hz: form.slot_step_hz.trim().parse().ok(),
        imap_enabled: Some(form.imap_enabled),
        imap_server: Some(form.imap_server.trim().to_string()).filter(|s| !s.is_empty()),
        imap_port: form.imap_port.trim().parse().ok(),
        imap_security: Some(form.imap_security.clone()),
        imap_username: Some(form.imap_username.trim().to_string()).filter(|s| !s.is_empty()),
        imap_password: Some(form.imap_password.clone()).filter(|s| !s.is_empty()),
        imap_poll_seconds: form.imap_poll_seconds.trim().parse().ok(),
    };

    if let Some(path) = crate::config::active_config_path() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(toml_str) = toml::to_string_pretty(&cfg) {
            let _ = std::fs::write(&path, toml_str);
        }
    }
}

fn export_chat_log(state: &mut GuiState) {
    if state.chat_lines.is_empty() {
        state.status_msg = "No chat messages to export".into();
        return;
    }

    let mut content = String::new();
    content.push_str(&format!("Chat log — {} — {}\n",
        state.my_callsign,
        Utc::now().format("%Y-%m-%d %H:%M:%S UTC"),
    ));
    content.push_str(&format!("==========================================\n\n"));

    for line in state.chat_lines.iter() {
        if line.is_system {
            content.push_str(&format!("[{}] --- {} ---\n", line.time, line.text));
        } else {
            content.push_str(&format!("[{}] {}: {}\n", line.time, line.callsign, line.text));
        }
    }

    let filename = format!("varac-chat-{}.txt", Utc::now().format("%Y%m%d-%H%M%S"));
    if let Some(path) = rfd::FileDialog::new()
        .set_file_name(&filename)
        .add_filter("Text files", &["txt"])
        .save_file()
    {
        match std::fs::write(&path, &content) {
            Ok(()) => {
                state.status_msg = format!("Chat exported to {}", path.display());
            }
            Err(e) => {
                state.status_msg = format!("Chat export error: {e}");
            }
        }
    }
}

fn export_contacts_csv(state: &mut GuiState, db: &Storage) {
    match db.all_contacts() {
        Ok(contacts) => {
            if contacts.is_empty() {
                state.status_msg = "No contacts to export".into();
                return;
            }
            let mut csv = String::from("Callsign,Name,QTH,Locator,Notes,QSO Count\n");
            for c in &contacts {
                let name = c.name.as_deref().unwrap_or("");
                let qth = c.qth.as_deref().unwrap_or("");
                let loc = c.locator.as_deref().unwrap_or("");
                let notes = c.notes.as_deref().unwrap_or("").replace('"', "\"\"");
                csv.push_str(&format!(
                    "{},\"{}\",\"{}\",{},\"{}\",{}\n",
                    c.callsign, name, qth, loc, notes, c.qso_count,
                ));
            }
            let filename = format!("varac-contacts-{}.csv", Utc::now().format("%Y%m%d-%H%M%S"));
            if let Some(path) = rfd::FileDialog::new()
                .set_file_name(&filename)
                .add_filter("CSV files", &["csv"])
                .save_file()
            {
                match std::fs::write(&path, &csv) {
                    Ok(()) => {
                        state.status_msg = format!("Contacts exported to {}", path.display());
                    }
                    Err(e) => {
                        state.status_msg = format!("Contact export error: {e}");
                    }
                }
            }
        }
        Err(e) => {
            state.status_msg = format!("Contact export error: {e}");
        }
    }
}

fn import_contacts_csv(state: &mut GuiState, db: &Storage) {
    let path = match rfd::FileDialog::new()
        .add_filter("CSV files", &["csv"])
        .add_filter("All files", &["*"])
        .pick_file()
    {
        Some(p) => p,
        None => return,
    };

    let content = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            state.status_msg = format!("CSV read error: {e}");
            return;
        }
    };

    let mut count = 0;
    for line in content.lines().skip(1) {
        let fields: Vec<&str> = parse_csv_line(line);
        if fields.is_empty() {
            continue;
        }
        let callsign = fields[0].trim().to_uppercase();
        if callsign.is_empty() {
            continue;
        }
        let name = fields.get(1).map(|s| s.trim()).filter(|s| !s.is_empty());
        let qth = fields.get(2).map(|s| s.trim()).filter(|s| !s.is_empty());
        let locator = fields.get(3).map(|s| s.trim()).filter(|s| !s.is_empty());
        let _ = db.auto_save_contact_from_qso(&callsign, name, qth, locator);
        count += 1;
    }
    state.status_msg = format!("Imported {count} contacts from CSV");
}

fn parse_csv_line(line: &str) -> Vec<&str> {
    let mut fields = Vec::new();
    let mut start = 0;
    let mut in_quotes = false;
    let bytes = line.as_bytes();

    for i in 0..bytes.len() {
        if bytes[i] == b'"' {
            in_quotes = !in_quotes;
        } else if bytes[i] == b',' && !in_quotes {
            let field = &line[start..i];
            fields.push(field.trim_matches('"'));
            start = i + 1;
        }
    }
    let field = &line[start..];
    fields.push(field.trim_matches('"'));
    fields
}

fn import_contacts_from_adif(state: &mut GuiState, db: &Storage) {
    let path = match rfd::FileDialog::new()
        .add_filter("ADIF files", &["adi", "adif"])
        .add_filter("All files", &["*"])
        .pick_file()
    {
        Some(p) => p,
        None => return,
    };

    match std::fs::read_to_string(&path) {
        Ok(content) => {
            let contacts = crate::storage::parse_adif_contacts(&content);
            let mut imported = 0;
            for (call, name, qth, grid) in &contacts {
                let _ = db.auto_save_contact_from_qso(call, name.as_deref(), qth.as_deref(), grid.as_deref());
                imported += 1;
            }
            state.status_msg = format!("Imported {} contacts from {}", imported, path.display());
        }
        Err(e) => {
            state.status_msg = format!("ADIF import error: {e}");
        }
    }
}

fn snr_sparkline(samples: &[f32]) -> String {
    const BARS: &[char] = &['\u{2581}', '\u{2582}', '\u{2583}', '\u{2584}', '\u{2585}', '\u{2586}', '\u{2587}', '\u{2588}'];
    let recent: Vec<f32> = samples.iter().rev().take(16).copied().collect::<Vec<_>>().into_iter().rev().collect();
    if recent.is_empty() {
        return String::new();
    }
    let min = recent.iter().copied().fold(f32::INFINITY, f32::min);
    let max = recent.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let range = (max - min).max(1.0_f32);
    recent
        .iter()
        .map(|&v| {
            let idx = (((v - min) / range) * 7.0_f32) as usize;
            BARS[idx.min(7)]
        })
        .collect()
}


fn load_config_file() -> Option<crate::config::ConfigFile> {
    let path = crate::config::active_config_path()?;
    let content = std::fs::read_to_string(path).ok()?;
    toml::from_str(&content).ok()
}

fn draw_log_window(ctx: &egui::Context, state: &mut GuiState) {
    if !state.log_open {
        return;
    }

    let mut open = state.log_open;
    egui::Window::new("System Log")
        .open(&mut open)
        .default_size([500.0, 400.0])
        .show(ctx, |ui| {
            ScrollArea::vertical()
                .auto_shrink([false; 2])
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    for line in state.log_lines.iter() {
                        ui.colored_label(WHITE, RichText::new(line).small().monospace());
                    }
                    if state.log_lines.is_empty() {
                        ui.colored_label(GRAY, "No log entries yet.");
                    }
                });
        });
    state.log_open = open && state.log_open; // (a button inside may have closed it)
}

fn draw_qso_log_window(ctx: &egui::Context, state: &mut GuiState, db: &Storage) {
    if !state.qso_log_open {
        return;
    }

    let mut open = state.qso_log_open;
    egui::Window::new("QSO Log")
        .open(&mut open)
        .default_size([600.0, 400.0])
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.add(theme::accent_button("Export ADIF", GREEN)).clicked() {
                    if let Ok(adif) = db.export_adif() {
                        if let Some(path) = rfd::FileDialog::new()
                            .set_file_name("openslot-qso.adi")
                            .add_filter("ADIF files", &["adi", "adif"])
                            .save_file()
                        {
                            match std::fs::write(&path, &adif) {
                                Ok(()) => {
                                    state.status_msg = format!("ADIF exported to {}", path.display());
                                }
                                Err(e) => {
                                    state.status_msg = format!("ADIF export error: {e}");
                                }
                            }
                        }
                    }
                }
                if ui.button("Import ADIF").on_hover_text("Import QSOs from an ADIF file").clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("ADIF files", &["adi", "adif"])
                        .add_filter("All files", &["*"])
                        .pick_file()
                    {
                        match std::fs::read_to_string(&path) {
                            Ok(content) => {
                                let qsos = crate::storage::parse_adif_qsos(&content, &state.my_callsign);
                                let mut imported = 0;
                                for qso in &qsos {
                                    if db.insert_qso(qso).is_ok() {
                                        imported += 1;
                                    }
                                }
                                state.status_msg = format!("Imported {imported} QSOs from {}", path.display());
                            }
                            Err(e) => {
                                state.status_msg = format!("ADIF import error: {e}");
                            }
                        }
                    }
                }
                ui.separator();
                ui.label("Search:");
                ui.add(TextEdit::singleline(&mut state.qso_search).desired_width(150.0).hint_text("callsign, name, QTH..."));
            });
            ui.separator();
            ui.horizontal(|ui| {
                ui.colored_label(BLUE_HEADER, RichText::new("Date/Time").strong());
                ui.add_space(20.0);
                ui.colored_label(BLUE_HEADER, RichText::new("Callsign").strong());
                ui.add_space(20.0);
                ui.colored_label(BLUE_HEADER, RichText::new("Band").strong());
                ui.add_space(20.0);
                ui.colored_label(BLUE_HEADER, RichText::new("BW").strong());
                ui.add_space(20.0);
                ui.colored_label(BLUE_HEADER, RichText::new("Duration").strong());
                ui.add_space(20.0);
                ui.colored_label(BLUE_HEADER, RichText::new("Name").strong());
                ui.add_space(20.0);
                ui.colored_label(BLUE_HEADER, RichText::new("QTH").strong());
            });
            ui.separator();

            let qso_filter = state.qso_search.to_lowercase();

            ScrollArea::vertical()
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    if let Ok(qsos) = db.recent_qsos(200) {
                        for qso in &qsos {
                            if !qso_filter.is_empty() {
                                let matches = qso.callsign.to_lowercase().contains(&qso_filter)
                                    || qso.name.as_deref().unwrap_or("").to_lowercase().contains(&qso_filter)
                                    || qso.qth.as_deref().unwrap_or("").to_lowercase().contains(&qso_filter)
                                    || qso.comments.as_deref().unwrap_or("").to_lowercase().contains(&qso_filter);
                                if !matches {
                                    continue;
                                }
                            }
                            let start = if state.use_local_time {
                                qso.starttime.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string()
                            } else {
                                qso.starttime.format("%Y-%m-%d %H:%M").to_string()
                            };
                            let duration = qso.endtime
                                .map(|end| {
                                    let secs = (end - qso.starttime).num_seconds();
                                    format!("{}:{:02}", secs / 60, secs % 60)
                                })
                                .unwrap_or_else(|| "ongoing".into());
                            let name = qso.name.as_deref().unwrap_or("");
                            let qth = qso.qth.as_deref().unwrap_or("");
                            let is_selected = state.qso_notes_guid.as_deref() == Some(&qso.guid);

                            let resp = ui.horizontal(|ui| {
                                ui.colored_label(GRAY, RichText::new(&start).small());
                                ui.colored_label(GREEN, RichText::new(&qso.callsign).small().strong());
                                if let Some(entity) = dxcc_entity(&qso.callsign) {
                                    ui.colored_label(theme::MUTED, RichText::new(entity).small());
                                }
                                if !qso.band.is_empty() {
                                    ui.colored_label(CYAN, RichText::new(&qso.band).small());
                                }
                                ui.colored_label(WHITE, RichText::new(&qso.bandwidth).small());
                                ui.colored_label(WHITE, RichText::new(&duration).small());
                                ui.colored_label(WHITE, RichText::new(name).small());
                                ui.colored_label(WHITE, RichText::new(qth).small());
                                if let Some(snr) = qso.snr_received {
                                    ui.colored_label(CYAN, RichText::new(format!("R:{snr:+}dB")).small());
                                }
                                if let Some(snr) = qso.snr_sent {
                                    ui.colored_label(YELLOW, RichText::new(format!("S:{snr:+}dB")).small());
                                }
                            }).response;

                            if resp.clicked() {
                                if is_selected {
                                    state.qso_notes_guid = None;
                                } else {
                                    state.qso_notes_guid = Some(qso.guid.clone());
                                    state.qso_notes_text = qso.comments.clone().unwrap_or_default();
                                }
                            }

                            let qso_guid_copy = qso.guid.clone();
                            let qso_call_copy = qso.callsign.clone();
                            resp.context_menu(|ui| {
                                if ui.button(format!("Connect to {qso_call_copy}")).clicked() {
                                    if state.modem_connected {
                                        state.send_cmd(GuiCommand::Connect(qso_call_copy.clone()));
                                        state.connect_started = Some(Utc::now());
                                    }
                                    ui.close_menu();
                                }
                                if ui.button(format!("Send VMail to {qso_call_copy}")).clicked() {
                                    state.compose_open = true;
                                    state.compose_to = qso_call_copy.clone();
                                    ui.close_menu();
                                }
                                if ui.button("Copy callsign").clicked() {
                                    ui.output_mut(|o| o.copied_text = qso_call_copy.clone());
                                    ui.close_menu();
                                }
                                if ui.button(format!("Lookup {qso_call_copy} on QRZ")).clicked() {
                                    open_qrz_lookup(&qso_call_copy);
                                    ui.close_menu();
                                }
                                ui.separator();
                                if ui.add(theme::accent_button("Delete QSO", RED)).clicked() {
                                    let _ = db.delete_qso(&qso_guid_copy);
                                    state.status_msg = format!("QSO with {qso_call_copy} deleted");
                                    ui.close_menu();
                                }
                            });

                            if is_selected {
                                ui.horizontal(|ui| {
                                    ui.add_space(16.0);
                                    ui.label(RichText::new("Notes:").small().color(GRAY));
                                    let changed = ui.add(
                                        TextEdit::singleline(&mut state.qso_notes_text)
                                            .desired_width(300.0)
                                            .hint_text("Add notes...")
                                    ).changed();
                                    if ui.small_button(RichText::new("Save").color(GREEN)).clicked() || changed {
                                        let _ = db.update_qso_comments(&qso.guid, &state.qso_notes_text);
                                    }
                                });
                            }
                        }
                        if qsos.is_empty() {
                            ui.colored_label(GRAY, "No QSOs logged yet.");
                        }
                    }
                });
        });
    state.qso_log_open = open && state.qso_log_open; // (a button inside may have closed it)
}

fn draw_qso_stats_window(ctx: &egui::Context, state: &mut GuiState, db: &Storage) {
    if !state.qso_stats_open {
        return;
    }

    let mut open = state.qso_stats_open;
    egui::Window::new("QSO Statistics")
        .open(&mut open)
        .resizable(true)
        .default_width(350.0)
        .show(ctx, |ui| {
            match db.qso_statistics() {
                Ok(stats) => {
                    ui.horizontal(|ui| {
                        ui.colored_label(WHITE, RichText::new("Total QSOs:").strong());
                        ui.colored_label(CYAN, RichText::new(format!("{}", stats.total)).strong());
                    });
                    ui.horizontal(|ui| {
                        ui.colored_label(WHITE, RichText::new("Unique callsigns:").strong());
                        ui.colored_label(CYAN, RichText::new(format!("{}", stats.unique_calls)).strong());
                    });
                    ui.horizontal(|ui| {
                        ui.colored_label(WHITE, RichText::new("Unique grids:").strong());
                        ui.colored_label(CYAN, RichText::new(format!("{}", stats.unique_grids)).strong());
                    });

                    if !stats.band_counts.is_empty() {
                        ui.add_space(8.0);
                        ui.colored_label(YELLOW, RichText::new("QSOs by Band").strong());
                        ui.separator();

                        let max_count = stats.band_counts.iter().map(|(_, c)| *c).max().unwrap_or(1);
                        for (band, count) in &stats.band_counts {
                            ui.horizontal(|ui| {
                                ui.colored_label(CYAN, RichText::new(format!("{band:>6}")).strong().monospace());
                                let bar_width = (*count as f32 / max_count as f32) * 200.0;
                                let (rect, _) = ui.allocate_exact_size(
                                    egui::vec2(bar_width.max(2.0), 14.0),
                                    egui::Sense::hover(),
                                );
                                ui.painter().rect_filled(rect, 2.0, GREEN);
                                ui.colored_label(WHITE, format!("{count}"));
                            });
                        }
                    }

                    if stats.total == 0 {
                        ui.add_space(8.0);
                        ui.colored_label(GRAY, RichText::new("No QSOs logged yet").italics());
                    }
                }
                Err(e) => {
                    ui.colored_label(Color32::RED, format!("Error loading stats: {e}"));
                }
            }
        });
    state.qso_stats_open = open && state.qso_stats_open; // (a button inside may have closed it)
}

fn draw_heard_stations_window(ctx: &egui::Context, state: &mut GuiState, db: &Storage) {
    if !state.heard_stations_open {
        return;
    }

    let mut open = state.heard_stations_open;
    egui::Window::new("Heard Stations")
        .open(&mut open)
        .default_size([400.0, 350.0])
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Export CSV").clicked() {
                    if let Ok(stations) = db.heard_stations(10000) {
                        if let Some(path) = rfd::FileDialog::new()
                            .set_file_name("heard-stations.csv")
                            .add_filter("CSV files", &["csv"])
                            .save_file()
                        {
                            let mut csv = String::from("Callsign,Last QSO,QSO Count\n");
                            for (call, last, count) in &stations {
                                csv.push_str(&format!("{call},{last},{count}\n"));
                            }
                            match std::fs::write(&path, &csv) {
                                Ok(()) => state.status_msg = format!("Exported {} heard stations", stations.len()),
                                Err(e) => state.status_msg = format!("Export error: {e}"),
                            }
                        }
                    }
                }
            });
            ui.separator();
            ui.horizontal(|ui| {
                ui.colored_label(BLUE_HEADER, RichText::new("Callsign").strong());
                ui.add_space(30.0);
                ui.colored_label(BLUE_HEADER, RichText::new("Last QSO").strong());
                ui.add_space(30.0);
                ui.colored_label(BLUE_HEADER, RichText::new("QSOs").strong());
            });
            ui.separator();

            ScrollArea::vertical()
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    if let Ok(stations) = db.heard_stations(200) {
                        let hs_snapshot: Vec<_> = stations.iter()
                            .map(|(call, last, count)| (call.clone(), last.clone(), *count))
                            .collect();
                        for (callsign, last_heard, count) in &hs_snapshot {
                            let resp = ui.horizontal(|ui| {
                                ui.colored_label(GREEN, RichText::new(callsign.as_str()).small().strong());
                                if let Some(entity) = dxcc_entity(callsign) {
                                    ui.colored_label(theme::MUTED, RichText::new(entity).small());
                                }
                                ui.colored_label(GRAY, RichText::new(last_heard.as_str()).small());
                                ui.colored_label(WHITE, RichText::new(format!("{count}")).small());
                            }).response;

                            let ctx_call = callsign.clone();
                            resp.context_menu(|ui| {
                                if ui.button("Connect").clicked() {
                                    if state.modem_connected {
                                        state.add_log(format!("Connecting to {ctx_call}..."));
                                        state.connect_started = Some(Utc::now());
                                        state.last_connect_callsign = Some(ctx_call.clone());
                                        state.send_cmd(GuiCommand::Connect(ctx_call.clone()));
                                        state.heard_stations_open = false;
                                    }
                                    ui.close_menu();
                                }
                                if ui.button("VMail").clicked() {
                                    state.compose_open = true;
                                    state.compose_to = ctx_call.clone();
                                    ui.close_menu();
                                }
                                if ui.button("Copy Callsign").clicked() {
                                    ui.output_mut(|o| o.copied_text = ctx_call.clone());
                                    ui.close_menu();
                                }
                                if ui.button("QRZ Lookup").clicked() {
                                    open_qrz_lookup(&ctx_call);
                                    ui.close_menu();
                                }
                            });

                            if resp.double_clicked() && state.modem_connected {
                                let dest = callsign.clone();
                                state.add_log(format!("Connecting to {dest}..."));
                                state.connect_started = Some(Utc::now());
                                state.last_connect_callsign = Some(dest.clone());
                                state.send_cmd(GuiCommand::Connect(dest));
                                state.heard_stations_open = false;
                            }
                            let dxcc = dxcc_entity(callsign).unwrap_or("Unknown");
                            resp.on_hover_text(format!("{callsign} ({dxcc}) — Double-click to connect"));
                        }
                        if hs_snapshot.is_empty() {
                            ui.colored_label(GRAY, "No stations heard yet.");
                        }
                    }
                });
        });
    state.heard_stations_open = open && state.heard_stations_open; // (a button inside may have closed it)
}

fn draw_freq_ref_window(ctx: &egui::Context, state: &mut GuiState) {
    if !state.freq_ref_open {
        return;
    }

    let mut open = state.freq_ref_open;
    egui::Window::new("VarAC Calling Frequencies")
        .open(&mut open)
        .resizable(false)
        .default_width(350.0)
        .show(ctx, |ui| {
            ui.colored_label(GRAY, RichText::new(
                "Standard VarAC calling frequencies for VARA HF.\n\
                 Set your radio to USB on these frequencies."
            ).small().italics());
            ui.add_space(8.0);

            ui.horizontal(|ui| {
                ui.colored_label(BLUE_HEADER, RichText::new("Band").strong());
                ui.add_space(40.0);
                ui.colored_label(BLUE_HEADER, RichText::new("Frequency").strong());
                ui.add_space(40.0);
                ui.colored_label(BLUE_HEADER, RichText::new("Dial USB").strong());
            });
            ui.separator();

            for &(freq_hz, band) in messages::CALLING_FREQUENCIES_HZ {
                let freq_khz = freq_hz as f64 / 1000.0;
                ui.horizontal(|ui| {
                    ui.colored_label(CYAN, RichText::new(format!("{band:>4}")).strong());
                    ui.add_space(30.0);
                    ui.colored_label(WHITE, RichText::new(format!("{freq_khz:.1} kHz")));
                    ui.add_space(30.0);
                    let dial_khz = (freq_hz as f64 - 1500.0) / 1000.0;
                    ui.colored_label(GRAY, RichText::new(format!("{dial_khz:.1} kHz")).small());
                });
            }

            ui.add_space(8.0);
            ui.colored_label(GRAY, RichText::new("Dial USB = Calling freq - 1500 Hz (VARA center offset)").small().italics());
        });
    state.freq_ref_open = open && state.freq_ref_open; // (a button inside may have closed it)
}

fn draw_bbs_listing_window(ctx: &egui::Context, state: &mut GuiState) {
    if !state.bbs_listing_open {
        return;
    }

    let mut open = state.bbs_listing_open;
    egui::Window::new("BBS File Listing")
        .open(&mut open)
        .default_size([400.0, 300.0])
        .show(ctx, |ui| {
            ui.label(format!("{} files available", state.bbs_listing_entries.len()));
            ui.separator();

            ScrollArea::vertical()
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    let snapshot: Vec<_> = state.bbs_listing_entries.iter()
                        .map(|e| (e.name.clone(), e.date.clone(), e.size))
                        .collect();
                    // (VarAC: a size over our incoming-file limit is shown red)
                    let limit: usize = state.settings_form.incoming_files_size_limit.trim().parse().unwrap_or(5000);
                    let mut download = None;
                    for (name, date, size) in &snapshot {
                        let resp = ui.horizontal(|ui| {
                            ui.colored_label(GREEN, RichText::new(name).strong());
                            ui.colored_label(GRAY, RichText::new(date).small());
                            let text = RichText::new(format!("{size} bytes")).small();
                            if *size > limit {
                                ui.label(text.color(egui::Color32::WHITE).background_color(egui::Color32::RED));
                            } else {
                                ui.colored_label(GRAY, text);
                            }
                        }).response;

                        resp.context_menu(|ui| {
                            if ui.button(format!("Download {name}")).clicked() {
                                download = Some(name.clone());
                                ui.close_menu();
                            }
                            if ui.button("Copy filename").clicked() {
                                ui.output_mut(|o| o.copied_text = name.clone());
                                ui.close_menu();
                            }
                        });

                        if resp.double_clicked() {
                            download = Some(name.clone());
                        }
                    }
                    if let Some(name) = download {
                        match state.bbs_refusal(true) {
                            Some(why) => state.tx_alert = Some(why),
                            None => {
                                state.send_cmd(GuiCommand::RequestBbsFile(name.clone()));
                                state.status_msg = format!("Requesting {name}...");
                            }
                        }
                    }
                    if snapshot.is_empty() {
                        ui.colored_label(GRAY, "No files available");
                    }
                });

            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Refresh").clicked() {
                    match state.bbs_refusal(false) {
                        Some(why) => state.tx_alert = Some(why),
                        None => state.send_cmd(GuiCommand::RequestBbsList),
                    }
                }
                if ui.button("Close").clicked() {
                    state.bbs_listing_open = false;
                }
            });
        });
    state.bbs_listing_open = open && state.bbs_listing_open; // (a button inside may have closed it)
}

fn draw_qsy_window(ctx: &egui::Context, state: &mut GuiState) {
    if !state.qsy_dialog_open {
        return;
    }
    let mut open = state.qsy_dialog_open;
    egui::Window::new("QSY Request")
        .open(&mut open)
        .resizable(false)
        .collapsible(false)
        .default_width(280.0)
        .show(ctx, |ui| {
            ui.label("Request partner to change frequency.");
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label("Offset (Hz):");
                ui.add(TextEdit::singleline(&mut state.qsy_offset_input)
                    .desired_width(100.0)
                    .hint_text("e.g. 750 or -1500"));
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let can_send = state.session_state == SessionState::Connected
                    && !state.qsy_offset_input.is_empty();
                if ui.add_enabled(can_send, egui::Button::new("Send QSY")).clicked() {
                    if let Ok(hz) = state.qsy_offset_input.trim().parse::<i32>() {
                        state.send_cmd(GuiCommand::QsyRequest { offset_hz: hz });
                        state.add_log(format!("Sent QSY request: {hz} Hz"));
                        state.status_msg = format!("QSY request sent: {hz} Hz");
                        state.qsy_dialog_open = false;
                        state.qsy_offset_input.clear();
                    } else {
                        state.status_msg = "Invalid offset — enter a number in Hz".into();
                    }
                }
                if ui.button("Cancel").clicked() {
                    state.qsy_dialog_open = false;
                }
            });
            ui.add_space(4.0);
            ui.colored_label(GRAY, RichText::new("Positive = up, negative = down. Common: 750, 1500").small().italics());
        });
    state.qsy_dialog_open = open && state.qsy_dialog_open; // (a button inside may have closed it)
}

fn draw_watchlist_window(ctx: &egui::Context, state: &mut GuiState) {
    if !state.watchlist_open {
        return;
    }
    let mut open = state.watchlist_open;
    egui::Window::new("Callsign Watchlist")
        .open(&mut open)
        .resizable(true)
        .default_width(300.0)
        .show(ctx, |ui| {
            ui.label("Get alerted when these callsigns are heard on CQ:");
            ui.separator();

            ui.horizontal(|ui| {
                ui.add(TextEdit::singleline(&mut state.watchlist_input)
                    .desired_width(120.0)
                    .hint_text("CALLSIGN"));
                if ui.button("Add").clicked() && !state.watchlist_input.is_empty() {
                    let call = state.watchlist_input.to_uppercase();
                    if !state.watchlist.contains(&call) {
                        state.watchlist.push(call);
                    }
                    state.watchlist_input.clear();
                }
            });
            ui.separator();

            let mut remove_idx = None;
            for (i, call) in state.watchlist.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(call).strong().color(YELLOW));
                    if ui.small_button("X").clicked() {
                        remove_idx = Some(i);
                    }
                });
            }
            if let Some(idx) = remove_idx {
                state.watchlist.remove(idx);
            }

            if state.watchlist.is_empty() {
                ui.colored_label(GRAY, RichText::new("No callsigns in watchlist").italics());
            }
        });
    state.watchlist_open = open && state.watchlist_open; // (a button inside may have closed it)
}

fn draw_callsign_tags_window(ctx: &egui::Context, state: &mut GuiState) {
    if !state.callsign_tags_open {
        return;
    }
    let mut open = state.callsign_tags_open;
    egui::Window::new("Callsign Tags")
        .open(&mut open)
        .resizable(true)
        .default_width(350.0)
        .show(ctx, |ui| {
            ui.label("Assign friendly names to callsigns (shown in CQ/beacon lists):");
            ui.separator();

            ui.horizontal(|ui| {
                ui.add(TextEdit::singleline(&mut state.callsign_tag_call_input)
                    .desired_width(90.0)
                    .hint_text("CALLSIGN"));
                ui.add(TextEdit::singleline(&mut state.callsign_tag_name_input)
                    .desired_width(120.0)
                    .hint_text("Friendly name"));
                if ui.button("Add").clicked()
                    && !state.callsign_tag_call_input.is_empty()
                    && !state.callsign_tag_name_input.is_empty()
                {
                    let call = state.callsign_tag_call_input.to_uppercase();
                    state.callsign_tags.insert(call, state.callsign_tag_name_input.clone());
                    state.callsign_tag_call_input.clear();
                    state.callsign_tag_name_input.clear();
                }
            });
            ui.separator();

            let mut remove_key = None;
            let mut sorted: Vec<_> = state.callsign_tags.iter().collect();
            sorted.sort_by_key(|(k, _)| (*k).clone());
            for (call, name) in &sorted {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(*call).strong().color(GREEN));
                    ui.colored_label(CYAN, format!("= {name}"));
                    if ui.small_button("X").clicked() {
                        remove_key = Some(call.to_string());
                    }
                });
            }
            if let Some(key) = remove_key {
                state.callsign_tags.remove(&key);
            }

            if state.callsign_tags.is_empty() {
                ui.colored_label(GRAY, RichText::new("No callsign tags defined").italics());
            }
        });
    state.callsign_tags_open = open && state.callsign_tags_open; // (a button inside may have closed it)
}

fn draw_bookmarks_window(ctx: &egui::Context, state: &mut GuiState) {
    if !state.bookmarks_open {
        return;
    }

    let mut open = state.bookmarks_open;
    egui::Window::new("Bookmarked Messages")
        .open(&mut open)
        .resizable(true)
        .default_width(450.0)
        .default_height(300.0)
        .show(ctx, |ui| {
            if state.bookmarked_messages.is_empty() {
                ui.colored_label(GRAY, RichText::new("No bookmarks yet. Right-click a chat message to bookmark it.").italics());
            } else {
                ui.horizontal(|ui| {
                    ui.colored_label(CYAN, RichText::new(format!("{} bookmarked messages", state.bookmarked_messages.len())).strong());
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.small_button(RichText::new("Clear All").color(RED)).clicked() {
                            state.bookmarked_messages.clear();
                        }
                        if ui.small_button("Export").clicked() {
                            if let Some(path) = rfd::FileDialog::new()
                                .set_file_name("bookmarks.txt")
                                .add_filter("Text", &["txt"])
                                .save_file()
                            {
                                let content: String = state.bookmarked_messages.iter()
                                    .map(|bm| format!("[{}] {}: {}\n", bm.time, bm.callsign, bm.text))
                                    .collect();
                                let _ = std::fs::write(&path, content);
                                state.add_log(format!("Bookmarks exported to {}", path.display()));
                            }
                        }
                    });
                });
                ui.separator();

                let mut remove_idx = None;
                ScrollArea::vertical().show(ui, |ui| {
                    for (i, bm) in state.bookmarked_messages.iter().enumerate() {
                        let resp = ui.horizontal(|ui| {
                            ui.colored_label(GRAY, RichText::new(&bm.time).small());
                            ui.colored_label(callsign_color(&bm.callsign), RichText::new(format!("{}:", bm.callsign)).strong());
                            ui.label(&bm.text);
                        }).response;
                        resp.context_menu(|ui| {
                            if ui.button("Copy").clicked() {
                                ui.output_mut(|o| o.copied_text = format!("[{}] {}: {}", bm.time, bm.callsign, bm.text));
                                ui.close_menu();
                            }
                            if ui.button("Remove bookmark").clicked() {
                                remove_idx = Some(i);
                                ui.close_menu();
                            }
                        });
                    }
                });
                if let Some(idx) = remove_idx {
                    state.bookmarked_messages.remove(idx);
                }
            }
        });
    state.bookmarks_open = open && state.bookmarks_open; // (a button inside may have closed it)
}

fn draw_about_window(ctx: &egui::Context, state: &mut GuiState) {
    if !state.about_open {
        return;
    }

    let mut open = state.about_open;
    egui::Window::new("About Openslot")
        .open(&mut open)
        .resizable(false)
        .collapsible(false)
        .default_width(380.0)
        .show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(8.0);
                ui.colored_label(CYAN, RichText::new("Openslot").heading().strong());
                ui.colored_label(WHITE, RichText::new(format!("v{}", env!("CARGO_PKG_VERSION"))).strong());
                ui.add_space(8.0);
                ui.colored_label(WHITE, "Independent cross-platform VarAC-compatible client");
                ui.colored_label(GRAY, "Built on the VARA modem TCP interface");
                ui.add_space(12.0);
                ui.colored_label(GRAY, "https://github.com/NR5P/OpenSlot");
                ui.add_space(12.0);
            });

            ui.separator();
            ui.add_space(4.0);
            ui.colored_label(YELLOW, RichText::new("Features").strong());
            let features = [
                "Chat, VMail (urgent + parking), BBS file transfer",
                "BBS server mode (serve files from directory)",
                "CQ/beacon monitoring and sending",
                "Broadcast messaging, QSY negotiation",
                "Contacts / address book (auto-populated)",
                "QSO logging with ADIF import/export",
                "Canned messages (F1–F12) with VarAC's tags",
                "Path finder, heard stations, cluster",
                "Desktop notifications, sound alerts",
                "Auto-away, auto-beacon, auto-reconnect",
                "Chat search, QSO log search, chat export",
                "Callsign watchlist, callsign tags",
                "Reply-to threading, extended SNR",
                "Grid distance calculation (Maidenhead)",
                "Calling frequencies reference",
                "UTC / local time display",
                "Band scanner, auto-CQ repeat, SNR sparkline",
                "DXCC entity lookup, contact quicklook",
                "QSO duration alert, chat bookmarks",
                "Dark / light theme toggle",
            ];
            for f in features {
                ui.colored_label(WHITE, RichText::new(format!("  - {f}")).small());
            }
            ui.add_space(8.0);
            ui.colored_label(GRAY, RichText::new(
                "This is an independent project. VarAC is a product of EA5HJX.\n\
                 VARA modem is a product of EA5HVK."
            ).small().italics());
        });
    state.about_open = open && state.about_open; // (a button inside may have closed it)
}

fn draw_contacts_window(ctx: &egui::Context, state: &mut GuiState, db: &Storage) {
    if !state.contacts_open {
        return;
    }

    let mut open = state.contacts_open;
    egui::Window::new("Contacts / Address Book")
        .open(&mut open)
        .default_size([600.0, 450.0])
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Search:");
                ui.add(TextEdit::singleline(&mut state.contact_search)
                    .desired_width(150.0)
                    .hint_text("Filter contacts..."));

                ui.separator();

                if ui.add(theme::accent_button("+ Add Contact", GREEN)).clicked() {
                    state.contact_adding = true;
                    state.contact_edit_callsign.clear();
                    state.contact_edit_name.clear();
                    state.contact_edit_qth.clear();
                    state.contact_edit_locator.clear();
                    state.contact_edit_notes.clear();
                }

                if ui.button("Export CSV").clicked() {
                    export_contacts_csv(state, db);
                }
                if ui.button("Import CSV").on_hover_text("Import contacts from a CSV file").clicked() {
                    import_contacts_csv(state, db);
                }
                if ui.button("Import ADIF").on_hover_text("Import contacts from an ADIF log file").clicked() {
                    import_contacts_from_adif(state, db);
                }
            });

            if state.contact_adding {
                ui.separator();
                Frame::default()
                    .fill(theme::SURFACE_2).rounding(theme::RADIUS)
                    .inner_margin(Margin::same(6.0))
                    .stroke(Stroke::new(1.0_f32, GREEN))
                    .show(ui, |ui| {
                        ui.colored_label(GREEN, RichText::new("New Contact").strong());
                        egui::Grid::new("contact_add_grid")
                            .num_columns(2)
                            .spacing([8.0, 4.0])
                            .show(ui, |ui| {
                                ui.label("Callsign:");
                                ui.add(TextEdit::singleline(&mut state.contact_edit_callsign).desired_width(120.0));
                                ui.end_row();
                                ui.label("Name:");
                                ui.add(TextEdit::singleline(&mut state.contact_edit_name).desired_width(200.0));
                                ui.end_row();
                                ui.label("QTH:");
                                ui.add(TextEdit::singleline(&mut state.contact_edit_qth).desired_width(200.0));
                                ui.end_row();
                                ui.label("Locator:");
                                ui.add(TextEdit::singleline(&mut state.contact_edit_locator).desired_width(100.0));
                                ui.end_row();
                                ui.label("Notes:");
                                ui.add(TextEdit::singleline(&mut state.contact_edit_notes).desired_width(300.0));
                                ui.end_row();
                            });
                        ui.horizontal(|ui| {
                            let valid = !state.contact_edit_callsign.is_empty()
                                && messages::varac_callsign_ok(&state.contact_edit_callsign);
                            if ui.add_enabled(valid, egui::Button::new(RichText::new("SAVE").color(GREEN).strong())).clicked() {
                                let _ = db.upsert_contact(&crate::storage::ContactRecord {
                                    callsign: state.contact_edit_callsign.to_uppercase(),
                                    name: if state.contact_edit_name.is_empty() { None } else { Some(state.contact_edit_name.clone()) },
                                    qth: if state.contact_edit_qth.is_empty() { None } else { Some(state.contact_edit_qth.clone()) },
                                    locator: if state.contact_edit_locator.is_empty() { None } else { Some(state.contact_edit_locator.clone()) },
                                    notes: if state.contact_edit_notes.is_empty() { None } else { Some(state.contact_edit_notes.clone()) },
                                    last_qso: None,
                                    qso_count: 0,
                                });
                                state.contact_adding = false;
                                state.status_msg = format!("Contact {} saved", state.contact_edit_callsign.to_uppercase());
                            }
                            if !state.contact_edit_callsign.is_empty() && !messages::varac_callsign_ok(&state.contact_edit_callsign) {
                                ui.colored_label(RED, RichText::new("Invalid callsign format").small());
                            }
                            if ui.button("Cancel").clicked() {
                                state.contact_adding = false;
                            }
                        });
                    });
            }

            ui.separator();

            // Table header
            ui.horizontal(|ui| {
                ui.colored_label(BLUE_HEADER, RichText::new("Callsign").strong());
                ui.add_space(20.0);
                ui.colored_label(BLUE_HEADER, RichText::new("Name").strong());
                ui.add_space(30.0);
                ui.colored_label(BLUE_HEADER, RichText::new("QTH").strong());
                ui.add_space(30.0);
                ui.colored_label(BLUE_HEADER, RichText::new("Grid").strong());
                ui.add_space(20.0);
                ui.colored_label(BLUE_HEADER, RichText::new("QSOs").strong());
                ui.add_space(20.0);
                ui.colored_label(BLUE_HEADER, RichText::new("Notes").strong());
            });
            ui.separator();

            ScrollArea::vertical()
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    let contacts = if state.contact_search.is_empty() {
                        db.all_contacts().unwrap_or_default()
                    } else {
                        db.search_contacts(&state.contact_search).unwrap_or_default()
                    };

                    for contact in &contacts {
                        let name = contact.name.as_deref().unwrap_or("");
                        let qth = contact.qth.as_deref().unwrap_or("");
                        let locator = contact.locator.as_deref().unwrap_or("");
                        let notes = contact.notes.as_deref().unwrap_or("");

                        let resp = ui.horizontal(|ui| {
                            ui.colored_label(GREEN, RichText::new(&contact.callsign).small().strong());
                            ui.colored_label(WHITE, RichText::new(name).small());
                            ui.colored_label(GRAY, RichText::new(qth).small());
                            ui.colored_label(CYAN, RichText::new(locator).small());
                            ui.colored_label(WHITE, RichText::new(format!("{}", contact.qso_count)).small());
                            if !notes.is_empty() {
                                ui.colored_label(GRAY, RichText::new(notes).small().italics());
                            }
                        }).response;

                        if resp.double_clicked() && state.modem_connected {
                            let dest = contact.callsign.clone();
                            state.add_log(format!("Connecting to {dest}..."));
                            state.connect_started = Some(Utc::now());
                            state.last_connect_callsign = Some(dest.clone());
                            state.send_cmd(GuiCommand::Connect(dest));
                            state.contacts_open = false;
                        }
                        let resp = resp.on_hover_text("Double-click to connect · Right-click for options");

                        resp.context_menu(|ui| {
                            if ui.button("Connect").clicked() && state.modem_connected {
                                let dest = contact.callsign.clone();
                                state.add_log(format!("Connecting to {dest}..."));
                                state.status_msg = format!("Connecting to {dest}...");
                                state.connect_started = Some(Utc::now());
                                state.send_cmd(GuiCommand::Connect(dest));
                                state.contacts_open = false;
                                ui.close_menu();
                            }
                            if ui.button("Send VMail").clicked() {
                                state.compose_open = true;
                                state.compose_to = contact.callsign.clone();
                                state.contacts_open = false;
                                ui.close_menu();
                            }
                            if ui.button("Copy Callsign").clicked() {
                                ui.output_mut(|o| o.copied_text = contact.callsign.clone());
                                ui.close_menu();
                            }
                            if ui.button("QRZ Lookup").clicked() {
                                open_qrz_lookup(&contact.callsign);
                                ui.close_menu();
                            }
                            ui.separator();
                            if ui.button("Edit").clicked() {
                                state.contact_adding = true;
                                state.contact_edit_callsign = contact.callsign.clone();
                                state.contact_edit_name = contact.name.clone().unwrap_or_default();
                                state.contact_edit_qth = contact.qth.clone().unwrap_or_default();
                                state.contact_edit_locator = contact.locator.clone().unwrap_or_default();
                                state.contact_edit_notes = contact.notes.clone().unwrap_or_default();
                                ui.close_menu();
                            }
                            if ui.add(theme::accent_button("Delete contact", RED)).clicked() {
                                let _ = db.delete_contact(&contact.callsign);
                                ui.close_menu();
                            }
                        });
                    }

                    if contacts.is_empty() {
                        ui.colored_label(GRAY, RichText::new("No contacts yet. Contacts are auto-saved from QSOs.").small().italics());
                    }
                });
        });
    state.contacts_open = open && state.contacts_open; // (a button inside may have closed it)
}

fn draw_vmail_viewer_window(ctx: &egui::Context, state: &mut GuiState, db: &Storage) {
    if !state.viewing_vmail {
        return;
    }

    let active_list = match state.vmail_folder_tab {
        1 => &state.vmail_sent_list,
        2 => &state.vmail_outbox_list,
        3 => &state.vmail_parking_list,
        _ => &state.vmail_list,
    };
    let vmail = state.selected_vmail
        .and_then(|i| active_list.get(i))
        .map(|v| (v.guid.clone(), v.from.clone(), v.to.clone(), v.subject.clone(), v.body.clone(), v.time.clone(), v.urgent));

    let Some((guid, from, to, subject, body, time, urgent)) = vmail else {
        state.viewing_vmail = false;
        return;
    };

    let mut open = state.viewing_vmail;
    egui::Window::new("VMail")
        .open(&mut open)
        .default_size([500.0, 350.0])
        .show(ctx, |ui| {
            egui::Grid::new("vmail_hdr")
                .num_columns(2)
                .spacing([10.0, 4.0])
                .show(ui, |ui| {
                    ui.colored_label(GRAY, "From:");
                    ui.colored_label(GREEN, RichText::new(&from).strong());
                    ui.end_row();
                    ui.colored_label(GRAY, "To:");
                    ui.colored_label(CYAN, &to);
                    ui.end_row();
                    ui.colored_label(GRAY, "Date:");
                    ui.label(&time);
                    ui.end_row();
                    ui.colored_label(GRAY, "Subject:");
                    ui.colored_label(WHITE, RichText::new(&subject).strong());
                    ui.end_row();
                    if urgent {
                        ui.label("");
                        ui.colored_label(RED, RichText::new("URGENT").strong());
                        ui.end_row();
                    }
                });

            ui.separator();

            // the files that came with it
            let files = db.vmail_attachments(&guid).unwrap_or_default();
            if !files.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(GRAY, "Attachments:");
                    for f in &files {
                        let name = f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                        if ui.small_button(format!("📎 {name}")).on_hover_text(f.display().to_string()).clicked() {
                            let _ = open::that(f);
                        }
                    }
                });
                ui.separator();
            }
            ScrollArea::vertical()
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    let display = body.replace("//n", "\n");
                    ui.label(&display);
                });

            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Reply").clicked() {
                    state.compose_open = true;
                    state.compose_to = from.clone();
                    state.compose_subject = format!("Re: {subject}");
                    state.viewing_vmail = false;
                }
                if ui.button("Forward").clicked() {
                    state.compose_open = true;
                    state.compose_to.clear();
                    state.compose_subject = format!("Fwd: {subject}");
                    state.compose_body = format!("--- Forwarded from {from} ---//n{body}");
                    state.viewing_vmail = false;
                }
                if state.vmail_folder_tab == 3 {
                    if ui.add(theme::accent_button("Deliver", CYAN)).on_hover_text("Move to outbox for delivery to recipient").clicked() {
                        let _ = db.move_vmail_to_folder(&guid, crate::storage::VMailFolder::Outbox);
                        state.viewing_vmail = false;
                        refresh_all_vmail_lists(state, db);
                        state.status_msg = format!("Parked VMail moved to outbox for {to}");
                    }
                }
                if ui.add(theme::accent_button("Delete", RED)).clicked() {
                    let _ = db.mark_vmail_deleted(&guid);
                    state.viewing_vmail = false;
                    refresh_all_vmail_lists(state, db);
                    state.unread_vmails = db.unread_vmail_count().unwrap_or(0);
                    state.status_msg = "VMail deleted".into();
                }
                if ui.button("Close").clicked() {
                    state.viewing_vmail = false;
                }
            });
        });
    state.viewing_vmail = open && state.viewing_vmail; // (a button inside may have closed it)

    // Mark as read (only if currently unread, inbox only)
    if state.vmail_folder_tab == 0 {
        let needs_mark = state.selected_vmail
            .and_then(|i| state.vmail_list.get(i))
            .is_some_and(|vm| !vm.read);
        if needs_mark {
            let _ = db.mark_vmail_read(&guid);
            if let Some(i) = state.selected_vmail {
                if let Some(vm) = state.vmail_list.get_mut(i) {
                    vm.read = true;
                }
            }
            state.unread_vmails = db.unread_vmail_count().unwrap_or(0);
        }
    }
}

fn refresh_vmail_list(state: &mut GuiState, db: &Storage) {
    state.vmail_list.clear();
    if let Ok(inbox) = db.vmails_in_folder(crate::storage::VMailFolder::Inbox, 50) {
        for vm in inbox {
            state.vmail_list.push(VMailEntry {
                guid: vm.guid,
                time: vm.creation_time.format("%m-%d %H:%M").to_string(),
                from: vm.vmail_from,
                to: vm.vmail_to,
                subject: vm.subject,
                body: vm.msg,
                read: vm.read_status,
                urgent: vm.urgent,
            });
        }
    }
}

fn refresh_all_vmail_lists(state: &mut GuiState, db: &Storage) {
    refresh_vmail_list(state, db);

    state.vmail_sent_list.clear();
    if let Ok(sent) = db.vmails_in_folder(crate::storage::VMailFolder::Sent, 50) {
        for vm in sent {
            state.vmail_sent_list.push(VMailEntry {
                guid: vm.guid,
                time: vm.creation_time.format("%m-%d %H:%M").to_string(),
                from: vm.vmail_from,
                to: vm.vmail_to,
                subject: vm.subject,
                body: vm.msg,
                read: true,
                urgent: vm.urgent,
            });
        }
    }

    state.vmail_outbox_list.clear();
    if let Ok(outbox) = db.vmails_in_folder(crate::storage::VMailFolder::Outbox, 50) {
        for vm in outbox {
            state.vmail_outbox_list.push(VMailEntry {
                guid: vm.guid,
                time: vm.creation_time.format("%m-%d %H:%M").to_string(),
                from: vm.vmail_from,
                to: vm.vmail_to,
                subject: vm.subject,
                body: vm.msg,
                read: true,
                urgent: vm.urgent,
            });
        }
    }

    state.vmail_drafts_list.clear();
    if let Ok(drafts) = db.vmails_in_folder(crate::storage::VMailFolder::Drafts, 50) {
        for vm in drafts {
            state.vmail_drafts_list.push(VMailEntry {
                guid: vm.guid,
                time: vm.creation_time.format("%m-%d %H:%M").to_string(),
                from: vm.vmail_from,
                to: vm.vmail_to,
                subject: vm.subject,
                body: vm.msg,
                read: true,
                urgent: vm.urgent,
            });
        }
    }
    state.relay_notification_count = db.relay_notifications().map(|r| r.len()).unwrap_or(0);
    state.unread_alerts = db.unread_alert_count().unwrap_or(0);
    state.outbox_count = db.vmails_in_folder(crate::storage::VMailFolder::Outbox, 100_000).map(|v| v.len()).unwrap_or(0);
    state.parking_count = db.vmails_in_folder(crate::storage::VMailFolder::Parking, 100_000).map(|v| v.len()).unwrap_or(0);

    state.vmail_parking_list.clear();
    if let Ok(parking) = db.vmails_in_folder(crate::storage::VMailFolder::Parking, 50) {
        for vm in parking {
            state.vmail_parking_list.push(VMailEntry {
                guid: vm.guid,
                time: vm.creation_time.format("%m-%d %H:%M").to_string(),
                from: vm.vmail_from,
                to: vm.vmail_to,
                subject: vm.subject,
                body: vm.msg,
                read: vm.read_status,
                urgent: vm.urgent,
            });
        }
    }
}

// === Async session bridge ===

pub fn spawn_session_bridge(
    rt: &tokio::runtime::Handle,
    mut session: Option<VaracSession>,
    station_config: StationConfig,
    modem_config: crate::tui::ModemConfig,
    callsign: String,
    session_tx: mpsc::Sender<SessionMsg>,
    mut cmd_rx: tokio::sync::mpsc::UnboundedReceiver<GuiCommand>,
    ptt_router: crate::rig::PttRouter,
    bg_db: Option<Storage>,
) {
    spawn_imap_poller(rt, callsign.clone(), bg_db.as_ref().and_then(|d| d.reopen()), session_tx.clone(), station_config.email_gate.clone());
    // Email gateway sender: mails out what stations hand us, and tells a still-connected
    // sender <ER>/<EF> through `gw_tag_rx`
    let gw_notify = std::sync::Arc::new(tokio::sync::Notify::new());
    let (gw_tag_tx, mut gw_tag_rx) = tokio::sync::mpsc::unbounded_channel::<(String, &'static str)>();
    spawn_gateway_sender(
        rt, callsign.clone(), bg_db.as_ref().and_then(|d| d.reopen()), session_tx.clone(),
        gw_notify.clone(), station_config.email_gate.clone(), gw_tag_tx,
    );
    // The session answers pings with <R+NN> <VW> when we hold mail for the pinger (VarAC)
    if let Some(db) = bg_db.as_ref().and_then(|d| d.reopen()) {
        let db = std::sync::Mutex::new(db);
        station_config.policy.mail_waiting.set(move |call| has_mail_for(&db, call));
    }
    // rusqlite connections aren't Sync; lock only around the (sync) queries
    let bg_db = bg_db.map(std::sync::Mutex::new);
    let session_tx2 = session_tx.clone();
    let mut last_snr: Option<f32> = None;
    // VMails we handed over and await <SMR> for, in order. Like VarAC they move to Sent
    // only when acknowledged (a broken link leaves them queued for next time); Some(vm) =
    // it came in by email, so its sender gets VarAC's "has been delivered" notice.
    let mut awaiting_ack: std::collections::VecDeque<(String, Option<VMail>)> = std::collections::VecDeque::new();
    // "Relay now" requests from the GUI, and VMails whose <SMR> ends the link (SEND AND DISCONNECT)
    let mut relay_requests: Vec<(Vec<String>, bool, bool, bool)> = Vec::new();
    // Our own QSY invitation: after we've moved, VarAC waits 35 s for any sign of the
    // partner before offering to go back ("Still waiting for your partner to QSY")
    let mut qsy_inviting = false;
    let mut qsy_wait: Option<(std::time::Instant, u64)> = None;
    let mut disconnect_on_ack: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut relay_notified = RelayNotified::default();
    // Email VMail with attachment: the offer and data arrive before the VMail header
    let mut att_offer: Option<crate::varac::messages::BbsFileOffer> = None;
    let mut att_payload: Option<Vec<u8>> = None;
    // Our queued emails go to a gateway once the link is fully open (after the QSY) and
    // its opening has announced E — whichever of the two comes last
    let (mut link_open, mut partner_gateway) = (false, false);
    // <ER>/<EF> for the connected sender wait until the link is open: original VarAC
    // ignores them while it's still in the QSY handshake (captured)
    let mut held_tags: Vec<(String, &'static str)> = Vec::new();
    // VarAC's "relay through this gateway?" popup: (deadline, relay when it runs out).
    // Decided once per link; the GUI answers with FlushEmailOutbox / DeclineEmailRelay.
    let mut relay_offer: Option<(tokio::time::Instant, bool)> = None;
    let mut relay_decided = false;
    // VarAC relays VMail only off the calling frequency: a link that started on a CF waits
    // for the QSY (or an AWAY partner) and the partner just hears <VW> meanwhile
    let (mut started_on_cf, mut qsy_done, mut partner_away, mut vw_sent) =
        (false, false, false, false);
    // VMail not to send again on this link (refused / unsendable), and forwarding stopped
    // after the partner's <SMF> (VarAC)
    let mut vmail_skip: std::collections::HashSet<String> = Default::default();
    let mut vmail_stopped = false;
    // QSO timers (VarAC): Call ID, idle disconnect (<IE>), calling-frequency limits (<AE>)
    let mut link_since: Option<std::time::Instant> = None;
    let mut last_id = std::time::Instant::now();
    // VarAC's modem watchdog: the last IAMALIVE (armed by the first one)
    let mut last_alive: Option<std::time::Instant> = None;
    let mut last_activity = std::time::Instant::now();
    // Our automatic Call ID going out isn't QSO activity (it mustn't hold off the idle timer)
    let mut id_on_air = false;
    let (mut bye_tag_pending, mut bye_sent, mut cf_abort_done, mut cf_warned) = (false, false, false, false);
    let mut idle_bye_at: Option<std::time::Instant> = None;
    // Broadcasts we send (ours, Path Finder, automatic answers) and the channel state
    let mut ui_out: std::collections::VecDeque<UiOut> = std::collections::VecDeque::new();
    let mut next_ui_at = tokio::time::Instant::now();
    let mut chan_busy = false;
    // EmComm: the one-time beacon answering a check-in request
    let mut checkin_beacon_at: Option<tokio::time::Instant> = None;
    // VarAC's transmit switches (PTT disabled, Ignore DCD, unattended links), a connect
    // or CQ waiting for the channel, and the 2–6 s VarAC waits after BUSY OFF
    let mut guard = TxGuard { unattended: true, ..Default::default() };
    let mut busy_cleared = tokio::time::Instant::now();
    // (VarAC draws its 2–6 s quiet hold-off again after every beacon and broadcast)
    let mut holdoff = std::time::Duration::from_secs(rand_between(2, 7));
    if let Some(s) = session.as_mut() {
        s.ask_qsy_invite = true; // the GUI shows VarAC's slot picker
    }
    rt.spawn(async move {
        // Verbose SNR: one <R+NN> a minute while it runs (VarAC)
        let mut vsnr_tick = tokio::time::interval(std::time::Duration::from_secs(2));
        vsnr_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut last_queue: Vec<String> = Vec::new();
        let mut conduct_tick = tokio::time::interval(std::time::Duration::from_secs(2));
        conduct_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            // Drain pending commands (non-blocking)
            // The session offers a QSY (<Q>) only when a link starts on a calling frequency
            station_config.policy.on_calling_frequency.store(
                ptt_router.dial().is_none_or(|d| crate::varac::messages::is_calling_frequency(d)),
                std::sync::atomic::Ordering::Relaxed,
            );
            // VarAC's "In queue" view
            {
                let snap = queue_view(session.as_ref(), &ui_out);
                if snap != last_queue {
                    last_queue = snap.clone();
                    let _ = session_tx2.send(SessionMsg::Queue(snap));
                }
            }
            // VMail to relay now through the connected station (VarAC "Relay now")
            for (guids, check_cf, quiet, disconnect_after) in std::mem::take(&mut relay_requests) {
                let (Some(s), Some(db)) = (session.as_mut(), bg_db.as_ref()) else { continue };
                if !s.is_connected() {
                    if !quiet {
                        let _ = session_tx2.send(SessionMsg::TxRefused {
                            title: "You are not connected to a station now".into(),
                            text: "You can't Relay this message now.\nYou need to be connected first.".into(),
                        });
                    }
                } else if check_cf && started_on_cf && !qsy_done && !partner_away {
                    // VarAC relays only off the calling frequency (or to an AWAY partner)
                    if !quiet {
                        let _ = session_tx2.send(SessionMsg::TxRefused {
                            title: "Please QSY".into(),
                            text: "You must first QSY away from the calling frequency.\n\nOnce you've moved, go to your outbox, right-click the VMail, and select 'Relay now through connected station'".into(),
                        });
                    }
                } else {
                    let sent = relay_vmails_now(s, db, &guids, &session_tx2).await;
                    if disconnect_after {
                        disconnect_on_ack.extend(sent.iter().map(|(g, _)| g.clone()));
                    }
                    awaiting_ack.extend(sent);
                }
            }
            // A connect / CQ that waited for the channel goes now
            if !chan_busy || guard.ignore_dcd {
                if let Some(cmd) = guard.waiting.take() {
                    if matches!(cmd, GuiCommand::ConnectModem | GuiCommand::DisconnectModem) {
                    last_alive = None;
                }
                process_command(cmd, &mut session, &callsign, &modem_config, &station_config, &session_tx2).await;
                }
            }
            while let Ok(cmd) = cmd_rx.try_recv() {
                let Some(cmd) = guard_tx(cmd, &mut guard, chan_busy, &ptt_router, &session_tx2) else { continue };
                let Some(cmd) = queue_ui_command(cmd, &callsign, &mut ui_out) else { continue };
                if let GuiCommand::RelayNow { guids, check_cf, quiet, disconnect_after } = cmd {
                    relay_requests.push((guids, check_cf, quiet, disconnect_after));
                    continue;
                }
                if let GuiCommand::ClearQueue { edit } = cmd {
                    clear_queues(edit, &mut session, &mut ui_out, &session_tx2).await;
                    continue;
                }
                if matches!(cmd, GuiCommand::FlushEmailOutbox | GuiCommand::DeclineEmailRelay) {
                    let relay = matches!(cmd, GuiCommand::FlushEmailOutbox);
                    (relay_offer, relay_decided) = (None, true);
                    if let (Some(s), Some(db), true) = (session.as_mut(), bg_db.as_ref(), relay && link_open && partner_gateway) {
                        awaiting_ack.extend(deliver_queued_email(s, db, &session_tx2).await);
                    }
                    continue;
                }
                process_command(cmd, &mut session, &callsign, &modem_config, &station_config, &session_tx2).await;
            }

            // Poll session for events or next command
            if let Some(sess) = session.as_mut() {
                if sess.modem_closed() {
                    // The modem quit or the network dropped: say so (auto-reconnect can retry)
                    session = None;
                    ptt_router.key(false);
                    let _ = session_tx2.send(SessionMsg::Log("VARA modem closed the connection".into()));
                    let _ = session_tx2.send(SessionMsg::ModemDisconnected);
                    continue;
                }
                // Only *waiting* races the command channel; processing an input always runs
                // to completion (an interrupted poll() lost decoded frames, e.g. a VMail
                // attachment offer that then never got its <SFRD>).
                let input = tokio::select! {
                    Some(cmd) = cmd_rx.recv() => {
                        let Some(cmd) = guard_tx(cmd, &mut guard, chan_busy, &ptt_router, &session_tx2) else { continue };
                        let Some(cmd) = queue_ui_command(cmd, &callsign, &mut ui_out) else { continue };
                        if let GuiCommand::RelayNow { guids, check_cf, quiet, disconnect_after } = cmd {
                            relay_requests.push((guids, check_cf, quiet, disconnect_after));
                            continue;
                        }
                        if let GuiCommand::ClearQueue { edit } = cmd {
                            clear_queues(edit, &mut session, &mut ui_out, &session_tx2).await;
                            continue;
                        }
                        if matches!(cmd, GuiCommand::FlushEmailOutbox | GuiCommand::DeclineEmailRelay) {
                            let relay = matches!(cmd, GuiCommand::FlushEmailOutbox);
                            (relay_offer, relay_decided) = (None, true);
                            if let (Some(s), Some(db), true) = (session.as_mut(), bg_db.as_ref(), relay && link_open && partner_gateway) {
                                awaiting_ack.extend(deliver_queued_email(s, db, &session_tx2).await);
                            }
                            continue;
                        }
                        process_command(cmd, &mut session, &callsign, &modem_config, &station_config, &session_tx2).await;
                        continue;
                    }
                    _ = tokio::time::sleep_until(relay_offer.map(|o| o.0).unwrap_or_else(tokio::time::Instant::now)), if relay_offer.is_some() => {
                        // The popup ran out: VarAC's default (relay with Auto relay on)
                        let relay = relay_offer.take().is_some_and(|o| o.1);
                        relay_decided = true;
                        let _ = session_tx2.send(SessionMsg::EmailRelayOfferClosed);
                        if let (Some(s), Some(db), true) = (session.as_mut(), bg_db.as_ref(), relay && link_open && partner_gateway) {
                            awaiting_ack.extend(deliver_queued_email(s, db, &session_tx2).await);
                        }
                        continue;
                    }
                    // VarAC's outgoing broadcast queue: only while not linked and the channel
                    // is quiet, a random 0.5–2 s before keying, 10 s between frames
                    _ = tokio::time::sleep_until(ui_out.front().map(|o| o.at.max(next_ui_at)).unwrap_or_else(tokio::time::Instant::now)), if !ui_out.is_empty() => {
                        let Some(s) = session.as_mut() else { continue };
                        // (not on the session's Connecting: a modem PENDING around our own
                        // broadcast isn't a link, and XARA sends no CANCELPENDING for it)
                        // (VarAC: and only after the channel has been quiet a few seconds, and not
                        // while away with unattended links off)
                        let quiet = guard.ignore_dcd || (!chan_busy && busy_cleared.elapsed() >= holdoff);
                        // (VarAC: not while our own beacon is going out, nor 10 s after it)
                        let own_beacon = guard.beacon_until.is_some_and(|t| tokio::time::Instant::now() < t + std::time::Duration::from_secs(10));
                        if matches!(s.state, SessionState::Connected | SessionState::AwaitingDisconnect) || s.is_dialing() || !quiet || own_beacon
                            || (s.is_away() && !guard.unattended)
                        {
                            if let Some(o) = ui_out.front_mut() {
                                o.at = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
                            }
                            continue;
                        }
                        let Some(o) = ui_out.pop_front() else { continue };
                        // Listen only: automatic answers and queued broadcasts are dropped
                        if guard.ptt_disabled {
                            let _ = session_tx2.send(SessionMsg::Log(format!("PTT disabled: broadcast to {} not sent", o.frame.dest)));
                            continue;
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(rand_between(500, 2000))).await;
                        match s.send_ui(&o.frame).await {
                            Ok(()) => {
                                let relay = matches!(&o.frame.info[..], b"<SMP>" | b"<SMU>");
                                if relay {
                                    relay_notified.sync(ptt_router.dial());
                                    relay_notified.sent.insert(o.frame.dest.to_uppercase(), std::time::Instant::now());
                                }
                                // Our own broadcasts are listed too (VarAC; not relay notifications)
                                if !relay { let _ = session_tx2.send(SessionMsg::Message(IncomingMessage::Broadcast(
                                    crate::varac::messages::Broadcast {
                                        from: callsign.clone(),
                                        to: o.frame.dest.clone(),
                                        via: (!o.frame.digis.is_empty()).then(|| o.frame.digis.join(" ")),
                                        message: crate::varac::messages::desanitize(&String::from_utf8_lossy(&o.frame.info)),
                                        band: None,
                                        timestamp: Utc::now(),
                                        snr: None,
                                    },
                                ))); }
                            }
                            Err(e) => {
                                let _ = session_tx2.send(SessionMsg::Log(format!("Broadcast not sent: {e}")));
                            }
                        }
                        next_ui_at = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
                        holdoff = std::time::Duration::from_secs(rand_between(2, 7));
                        continue;
                    }
                    _ = tokio::time::sleep_until(checkin_beacon_at.unwrap_or_else(tokio::time::Instant::now)), if checkin_beacon_at.is_some() => {
                        checkin_beacon_at = None;
                        let linked = session.as_ref().is_some_and(|s| matches!(s.state, SessionState::Connected | SessionState::AwaitingDisconnect));
                        if !linked && !chan_busy && !guard.ptt_disabled {
                            let bandwidth = match modem_config.bandwidth {
                                Bandwidth::Bw500 => 500,
                                Bandwidth::Bw2300 => 2300,
                                Bandwidth::Bw2750 => 2750,
                            };
                            let base = callsign.split('-').next().unwrap_or(&callsign).to_string();
                            process_command(GuiCommand::SendCq { source: format!("{base}-8"), bandwidth },
                                &mut session, &callsign, &modem_config, &station_config, &session_tx2).await;
                        }
                        continue;
                    }
                    _ = conduct_tick.tick() => {
                        // VarAC's safety net: PTT on for 25 s is switched off (and a tune stopped)
                        if guard.ptt_on_since.is_some_and(|t| t.elapsed() >= std::time::Duration::from_secs(25)) {
                            guard.ptt_on_since = None;
                            ptt_router.key(false);
                            let _ = session_tx2.send(SessionMsg::Log("PTT is ON for too long (25 seconds). Turning PTT OFF. This is a safety mechanism".into()));
                            if std::mem::take(&mut guard.tuning) {
                                let _ = session_tx2.send(SessionMsg::Log("Stopping TUNE operation. Tune time limit exceeded.".into()));
                                process_command(GuiCommand::TuneStop, &mut session, &callsign, &modem_config, &station_config, &session_tx2).await;
                            }
                        }
                        if qsy_wait.is_some_and(|(t, _)| t.elapsed() >= std::time::Duration::from_secs(35)) {
                            if let Some((_, back)) = qsy_wait.take() {
                                let _ = session_tx2.send(SessionMsg::QsyRecovery(back));
                            }
                        }
                        let limit = crate::config::vara_keepalive_secs();
                        if session.is_some() && last_alive.is_some_and(|t| t.elapsed() >= std::time::Duration::from_secs(limit)) {
                            last_alive = None;
                            let _ = session_tx2.send(SessionMsg::Log(format!(
                                "ERROR: VARA Modem did not report its alive for over {limit} Seconds. Restarting VARA modem connection!"
                            )));
                            session = None;
                            ptt_router.key(false);
                            let _ = session_tx2.send(SessionMsg::ModemDisconnected);
                            process_command(GuiCommand::ConnectModem, &mut session, &callsign, &modem_config, &station_config, &session_tx2).await;
                        }
                        let Some(s) = session.as_mut() else { continue };
                        // VarAC's outbox (every second while linked): the next VMail for
                        // the partner once the last one was answered — also mail queued or
                        // parked for it during the QSO
                        if link_open && awaiting_ack.is_empty() && !vmail_stopped && !s.is_transferring() && !s.is_ping()
                            && (!started_on_cf || qsy_done || partner_away || s.is_away())
                        {
                            if let (Some(db), Some(partner)) = (bg_db.as_ref(), s.connected_to.clone()) {
                                awaiting_ack.extend(deliver_queued_vmail(s, db, &partner, &session_tx2, &mut vmail_skip).await);
                            }
                        }
                        let Some(since) = link_since.filter(|_| s.is_connected()) else { continue };
                        let now = std::time::Instant::now();
                        let age = now - since;
                        let (id_min, idle_min) = crate::config::qso_timers();
                        let log = |m: String| { let _ = session_tx2.send(SessionMsg::Log(m)); };
                        let sys = |m: String| { let _ = session_tx2.send(SessionMsg::SystemChat(m)); };
                        // Station ID every N minutes: "de CALL <R+NN>" (VarAC)
                        let em = crate::config::emcomm();
                        // (VarAC sends it during pings too; without Call ID (EmComm) just " <R+NN>")
                        if now - last_id >= std::time::Duration::from_secs(id_min * 60) {
                            let snr = last_snr.filter(|_| !em.no_snr)
                                .map(|v| format!(" <R{}>", crate::varac::messages::snr_text(v.round() as i32))).unwrap_or_default();
                            // (VarAC identifies with the full callsign, e.g. 4X/W1ABC/P)
                            let full = crate::config::full_callsign();
                            let id = if full.trim().is_empty() { callsign.clone() } else { full.trim().to_uppercase() };
                            let frame = if em.no_call_id { snr.clone() } else { format!("de {id}{snr}") };
                            if !frame.is_empty() {
                                let _ = s.send_chat(&frame).await;
                            }
                            (last_id, id_on_air) = (now, true);
                            log("Call ID sent".into());
                        }
                        // Idle: tell the partner <IE> and disconnect (the called side first)
                        let idle_limit = std::time::Duration::from_secs(idle_min * 60 + if s.is_incoming() { 0 } else { 10 });
                        if idle_min > 0 && !bye_sent && !s.is_transferring() && now - last_activity >= idle_limit {
                            sys("IDLE TIME LIMIT EXCEEDED. DISCONNECTING...".into());
                            let _ = s.send_chat("<IE>").await;
                            (bye_sent, bye_tag_pending) = (true, true);
                            idle_bye_at = Some(now);
                        }
                        // still linked 30 s after <IE>: VarAC aborts the link
                        if idle_bye_at.is_some_and(|t| now - t >= std::time::Duration::from_secs(30)) {
                            idle_bye_at = None;
                            log("Idle disconnect: still linked 30 s after <IE>, aborting".into());
                            let _ = s.vara.send_command(&crate::vara::commands::VaraCommand::Abort).await;
                        }
                        let on_cf = ptt_router.dial()
                            .is_some_and(|d| crate::varac::messages::is_calling_frequency(d));
                        if on_cf && !cf_abort_done {
                            // Away on a calling frequency: 5 minutes, then <AE> and disconnect
                            if s.is_away() && s.is_incoming() && !s.is_ping() && !bye_sent && age >= std::time::Duration::from_secs(300) {
                                sys("CONNECTION ON CALLING FREQUENCY IN AWAY STATUS TIME LIMIT EXCEEDED. DISCONNECTING...".into());
                                let _ = s.send_chat("<AE>").await;
                                (bye_sent, bye_tag_pending) = (true, true);
                            }
                            // VarAC aborts links left on a calling frequency: 240 s, pings 90 s
                            let limit: u64 = if s.is_ping() { 90 } else { 240 };
                            // ...after a reminder (caller at 150 s, called station at 210 s)
                            let warn_at = if s.is_incoming() { 210 } else { 150 };
                            if !s.is_ping() && !cf_warned && age >= std::time::Duration::from_secs(warn_at) {
                                cf_warned = true;
                                sys(format!("You're on a calling Freq. PSE QSY OR LINK WILL BE CLOSED IN {} SEC.",
                                    limit.saturating_sub(age.as_secs())));
                            }
                            if age >= std::time::Duration::from_secs(limit) {
                                cf_abort_done = true;
                                sys(if s.is_ping() {
                                    "PINGING ON CALLING FREQUENCY TIME LIMIT EXCEEDED (90 SECONDS) DISCONNECTING...".into()
                                } else {
                                    "CONNECTION ON CALLING FREQUENCY TIME LIMIT EXCEEDED (240 SECONDS) DISCONNECTING...".into()
                                });
                                let _ = s.vara.send_command(&crate::vara::commands::VaraCommand::Abort).await;
                            }
                        }
                        continue;
                    }
                    _ = vsnr_tick.tick() => {
                        if let Some(s) = session.as_mut() {
                            let _ = s.verbose_snr_tick().await;
                        }
                        continue;
                    }
                    Some((call, tag)) = gw_tag_rx.recv() => {
                        // VarAC tells the sender only while it's still connected
                        if let Some(s) = session.as_mut() {
                            let partner = s.connected_to.clone().unwrap_or_default();
                            if !partner.is_empty() && base_callsign(&partner) == base_callsign(&call) {
                                if link_open {
                                    let _ = s.send_chat(tag).await;
                                } else {
                                    held_tags.push((call, tag));
                                }
                            }
                        }
                        continue;
                    }
                    input = sess.next_input() => input,
                };
                let Some(sess) = session.as_mut() else { continue };
                let Some(input) = input else { continue };
                if let Some(evt) = sess.handle_input(input).await {
                        let msg = match evt {
                            crate::varac::session::SessionEvent::Connected { source, destination, bandwidth } => {
                                (link_open, partner_gateway) = (false, false);
                                (relay_offer, relay_decided) = (None, false);
                                started_on_cf = ptt_router.dial()
                                    .is_some_and(|d| crate::varac::messages::is_calling_frequency(d));
                                (qsy_done, partner_away, vw_sent) = (false, false, false);
                                vmail_skip.clear();
                                vmail_stopped = false;
                                link_since = Some(std::time::Instant::now());
                                (last_id, last_activity) = (std::time::Instant::now(), std::time::Instant::now());
                                (bye_tag_pending, bye_sent, cf_abort_done, cf_warned) = (false, false, false, false);
                                idle_bye_at = None;
                                // VarAC's outbox doesn't wait for the handshake: on a calling
                                // frequency it says <VW> within a second of the link coming up
                                if started_on_cf && !sess.is_ping() {
                                    if let Some(db) = bg_db.as_ref() {
                                        if has_mail_for(db, &destination) {
                                            vw_sent = true;
                                            let _ = sess.send_chat("<VW>").await;
                                            let _ = session_tx2.send(SessionMsg::VMailRelayAlert(destination.clone()));
                                        }
                                    }
                                }
                                SessionMsg::Connected { source, destination, bandwidth }
                            }
                            crate::varac::session::SessionEvent::Disconnected => {
                                awaiting_ack.clear();
                                held_tags.clear();
                                (link_open, partner_gateway) = (false, false);
                                (relay_offer, relay_decided) = (None, false);
                                (att_offer, att_payload) = (None, None);
                                SessionMsg::Disconnected
                            }
                            crate::varac::session::SessionEvent::Ptt(on) => {
                                // Key the radio right here — waiting for the next GUI frame adds latency.
                                // (VarAC ignores the modem's PTT ON with PTT disabled, or while
                                // away with unattended links off — except for our own beacon)
                                let may = guard.may_key(sess.is_away());
                                if on && !may {
                                    let _ = session_tx2.send(SessionMsg::Log(
                                        "Ignoring PTT request by the modem: PTT is disabled, or you are away with unattended links off".into(),
                                    ));
                                }
                                ptt_router.key(on && may);
                                guard.ptt_on_since = (on && may).then(tokio::time::Instant::now);
                                SessionMsg::Ptt(on)
                            }
                            crate::varac::session::SessionEvent::BufferUpdate(n) => {
                                if n == 0 {
                                    qsy_wait = None; // (sent and acknowledged on the new frequency)
                                    if !std::mem::take(&mut id_on_air) {
                                        last_activity = std::time::Instant::now();
                                    }
                                    // <IE>/<AE> is on the air: now drop the link (VarAC sends
                                    // VARA DISCONNECT, no <DISC>)
                                    if std::mem::take(&mut bye_tag_pending) {
                                        let _ = sess.vara.send_command(&crate::vara::commands::VaraCommand::Disconnect).await;
                                    }
                                }
                                SessionMsg::Buffer(n)
                            }
                            crate::varac::session::SessionEvent::Busy(on) => {
                                if chan_busy && !on {
                                    busy_cleared = tokio::time::Instant::now();
                                }
                                chan_busy = on;
                                SessionMsg::Busy(on)
                            }
                            crate::varac::session::SessionEvent::AdvancedBeacon(b) => {
                                if let Some(db) = bg_db.as_ref() {
                                    if guard.may_key(sess.is_away()) {
                                        relay_notified.sync(ptt_router.dial());
                                        if let Some(frame) = relay_notification_frame(db, &callsign, &b.callsign, &relay_notified, &ui_out, &session_tx2) {
                                            ui_out.push_back(UiOut { at: tokio::time::Instant::now(), frame });
                                        }
                                    }
                                }
                                SessionMsg::AdvancedBeacon(b, last_snr)
                            }
                            crate::varac::session::SessionEvent::CqFrame { source, bandwidth } => {
                                // Beacon (or EmComm beacon) from a station we hold parked VMail for:
                                // notify it (VarAC: again after 20 minutes)
                                if source.ends_with("-9") || source.ends_with("-8") {
                                    if let Some(db) = bg_db.as_ref() {
                                        if guard.may_key(sess.is_away()) {
                                            relay_notified.sync(ptt_router.dial());
                                            let heard = source.split('-').next().unwrap_or(&source);
                                            if let Some(frame) = relay_notification_frame(db, &callsign, heard, &relay_notified, &ui_out, &session_tx2) {
                                                ui_out.push_back(UiOut { at: tokio::time::Instant::now(), frame });
                                            }
                                        }
                                    }
                                }
                                SessionMsg::CqFrame { source, bandwidth, snr: last_snr }
                            }
                            crate::varac::session::SessionEvent::Message(crate::varac::messages::IncomingMessage::VMail(vm)) if vm.is_email() => {
                                // We took it as gateway (<SMR> sent): queue it like VarAC (Parking,
                                // To = the address) for the gateway sender, which retries on failure
                                let files = if vm.has_attachment {
                                    match (att_offer.take(), att_payload.take()) {
                                        (Some(offer), Some(data)) => crate::varac::bbs::receive_file(String::from_utf8_lossy(&data).trim(), &offer.md5_hex)
                                            .map_err(|e| e.to_string())
                                            .and_then(|zip| extract_vmail_attachments(&zip, crate::varac::bbs::strip_gzip_suffix(&offer.name)))
                                            .map(|(dir, names)| names.into_iter().map(|n| dir.join(n)).collect::<Vec<_>>())
                                            .unwrap_or_default(),
                                        _ => Vec::new(),
                                    }
                                } else {
                                    Vec::new()
                                };
                                let guid = uuid::Uuid::new_v4().to_string();
                                let stored = bg_db.as_ref().and_then(|db| db.lock().ok()).map(|d| {
                                    let r = d.insert_vmail(&crate::storage::VMailRecord {
                                        guid: guid.clone(),
                                        creation_time: vm.timestamp,
                                        sent_time: None,
                                        received_time: Some(Utc::now()),
                                        folder_id: crate::storage::VMailFolder::Parking as i32,
                                        vmail_to: vm.to.clone(),
                                        vmail_from: vm.from.clone(),
                                        vmail_via: None,
                                        subject: vm.subject.clone(),
                                        msg: vm.body.clone(),
                                        read_status: true,
                                        is_deleted: false,
                                        urgent: vm.urgent,
                                        delivery_band: ptt_router.dial().and_then(freq_to_band).map(str::to_string),
                                        delivery_snr: last_snr.map(|s| s.round() as i32),
                                        frequency: ptt_router.dial().map(|f| f as i64),
                                    });
                                    let _ = d.set_vmail_email(&guid, vm.reply_to.as_deref(), vm.raw);
                                    for f in &files {
                                        let _ = d.add_vmail_attachment(&guid, f);
                                    }
                                    r
                                });
                                gw_notify.notify_one();
                                SessionMsg::EmailQueued {
                                    from: vm.from.clone(),
                                    to: vm.to.clone(),
                                    subject: vm.subject.clone(),
                                    error: match stored {
                                        Some(Ok(())) => None,
                                        Some(Err(e)) => Some(e.to_string()),
                                        None => Some("no database".into()),
                                    },
                                }
                            }
                            crate::varac::session::SessionEvent::Message(crate::varac::messages::IncomingMessage::EmailGatewayRejected) => {
                                // The gateway refused our email: it stays in the Outbox
                                awaiting_ack.pop_front();
                                SessionMsg::Message(crate::varac::messages::IncomingMessage::EmailGatewayRejected)
                            }
                            crate::varac::session::SessionEvent::Message(crate::varac::messages::IncomingMessage::VMailAck) => {
                                let mut acked = awaiting_ack.pop_front();
                                let full = sess.connected_to.clone().unwrap_or_default().to_uppercase();
                                let base = full.split('-').next().unwrap_or_default().to_string();
                                if let (Some((guid, notice)), Some(db)) = (acked.as_mut(), bg_db.as_ref()) {
                                    if let Ok(d) = db.lock() {
                                        let _ = d.move_vmail_to_sent(guid);
                                        // VarAC: mail for someone else went via the partner;
                                        // the email sender hears of it only when the partner
                                        // was the addressee
                                        if let Ok(Some(vm)) = d.vmail_by_guid(guid) {
                                            let to = vm.vmail_to.to_uppercase();
                                            let direct = to == base || to == full;
                                            if !direct && !to.contains(&format!("{base}/")) {
                                                let _ = d.set_vmail_via(guid, &full);
                                            }
                                            if !direct {
                                                *notice = None;
                                            }
                                        }
                                    }
                                }
                                // SEND AND DISCONNECT: the receipt came, close the link (VarAC)
                                if acked.as_ref().is_some_and(|(g, _)| disconnect_on_ack.remove(g)) {
                                    let _ = session_tx2.send(SessionMsg::SystemChat("VMAIL RECEIPT CONFIRMED. DISCONNECTING...".into()));
                                    let _ = sess.send_disconnect().await;
                                }
                                if let Some((_, Some(vm))) = acked {
                                    let d = crate::varac::email_gateway::Delivery {
                                        sender_call: sess.connected_to.clone().unwrap_or_default(),
                                        gateway_call: callsign.clone(),
                                        band: ptt_router.dial().and_then(freq_to_band).map(str::to_string),
                                        frequency_hz: ptt_router.dial(),
                                        snr: last_snr,
                                    };
                                    let tx = session_tx2.clone();
                                    tokio::task::spawn_blocking(move || {
                                        let msg = match crate::config::gateway_config() {
                                            Some(cfg) => match crate::varac::email_gateway::send_delivery_notice(&cfg, &vm, &d) {
                                                Ok(()) => format!("Email gateway: delivery notice sent to {}", vm.from),
                                                Err(e) => format!("Email gateway: delivery notice to {} failed: {e}", vm.from),
                                            },
                                            None => return,
                                        };
                                        let _ = tx.send(SessionMsg::Log(msg));
                                    });
                                }
                                SessionMsg::Message(crate::varac::messages::IncomingMessage::VMailAck)
                            }
                            crate::varac::session::SessionEvent::Message(m) => {
                                last_activity = std::time::Instant::now();
                                qsy_wait = None; // (any sign of the partner)
                                match &m {
                                    // VarAC's automatic answers to broadcasts (Path Finder, <SR>,
                                    // <LOCR>, <GPSR>, <VER>, <EGA>, <LHC:>), through the queue
                                    crate::varac::messages::IncomingMessage::Broadcast(bc) => {
                                        let version = sess.version_text();
                                        let now = tokio::time::Instant::now();
                                        for (delay, frame) in broadcast_answers(bc, &callsign, &station_config, bg_db.as_ref(), &version) {
                                            ui_out.push_back(UiOut { at: now + std::time::Duration::from_secs(delay), frame });
                                        }
                                        // EmComm check-in request: answer with a beacon in 1–20 s, unless
                                        // we're on a calling frequency (as in VarAC)
                                        let on_cf = ptt_router.dial()
                                            .is_some_and(|d| crate::varac::messages::is_calling_frequency(d));
                                        if bc.to.eq_ignore_ascii_case("EMCOMM") && bc.message.contains("<EC>")
                                            && crate::config::emcomm().auto_checkin && !on_cf && checkin_beacon_at.is_none()
                                        {
                                            checkin_beacon_at = Some(now + std::time::Duration::from_secs(rand_between(1, 21)));
                                            let _ = session_tx2.send(SessionMsg::Log(format!("EmComm check-in request from {}: beacon follows", bc.from)));
                                        }
                                    }
                                    // EmComm last-connections peek, allowed: our last 5 QSOs (VarAC)
                                    crate::varac::messages::IncomingMessage::LastConnectionsRequest
                                        if station_config.policy.emcomm_last_connections.load(std::sync::atomic::Ordering::Relaxed) =>
                                    {
                                        let rows = bg_db.as_ref().and_then(|d| d.lock().ok())
                                            .and_then(|d| d.last_connections(5).ok()).unwrap_or_default();
                                        if !rows.is_empty() {
                                            let _ = sess.send_chat(&crate::varac::messages::last_connections_reply(&rows)).await;
                                        }
                                    }
                                    // An AWAY partner gets its VMail on the calling frequency (VarAC)
                                    crate::varac::messages::IncomingMessage::Away(_) => {
                                        partner_away = true;
                                        if link_open && awaiting_ack.is_empty() && !vmail_stopped {
                                            if let (Some(db), Some(partner)) = (bg_db.as_ref(), sess.connected_to.clone()) {
                                                awaiting_ack.extend(deliver_queued_vmail(sess, db, &partner, &session_tx2, &mut vmail_skip).await);
                                            }
                                        }
                                    }
                                    // VarAC last-heard peeks, answered from our heard list
                                    crate::varac::messages::IncomingMessage::LastHeardRequest => {
                                        let (allow, multi) = crate::config::last_heard_policy();
                                        let reply = if !allow {
                                            "<LHJ>".to_string()
                                        } else {
                                            let band = if multi { None } else { ptt_router.dial().and_then(freq_to_band) };
                                            bg_db.as_ref().and_then(|db| db.lock().ok())
                                                .and_then(|d| d.last_heard(Utc::now() - chrono::Duration::hours(24), band, if crate::config::modem_type() == crate::vara::commands::ModemType::Fm { 20 } else { 10 }).ok())
                                                .and_then(|e| crate::varac::messages::last_heard_reply(&e))
                                                .unwrap_or_else(|| "<LHE>".into())
                                        };
                                        let _ = sess.send_chat(&reply).await;
                                    }
                                    crate::varac::messages::IncomingMessage::LastHeardCallsignRequest(call) => {
                                        let reply = if !crate::config::last_heard_policy().0 {
                                            "<LHJ>".to_string()
                                        } else {
                                            bg_db.as_ref().and_then(|db| db.lock().ok())
                                                .and_then(|d| d.last_heard_callsign(call).ok().flatten())
                                                .map(|e| crate::varac::messages::last_heard_callsign_reply(&e))
                                                .unwrap_or_else(|| "<LHCE>".into())
                                        };
                                        let _ = sess.send_chat(&reply).await;
                                    }
                                    // VarAC <VRP>: report the partner's VMail parked here
                                    // (parking off: VarAC refuses the peek)
                                    crate::varac::messages::IncomingMessage::RelayPeek
                                        if !station_config.policy.allow_parking.load(std::sync::atomic::Ordering::Relaxed) =>
                                    {
                                        let _ = sess.send_chat("<VRPJ>").await;
                                    }
                                    crate::varac::messages::IncomingMessage::RelayPeek => {
                                        let partner = sess.connected_to.clone().unwrap_or_default();
                                        let entries: Vec<_> = bg_db.as_ref().and_then(|db| db.lock().ok())
                                            .and_then(|d| d.parked_vmails_from(&partner).ok())
                                            .unwrap_or_default()
                                            .into_iter()
                                            .map(|v| (v.vmail_to, v.creation_time, v.sent_time))
                                            .collect();
                                        let _ = sess.send_chat(&crate::varac::messages::relay_peek_reply(&entries, Utc::now())).await;
                                    }
                                    // Refused / failed on the partner's side: it stays queued
                                    crate::varac::messages::IncomingMessage::VMailRejected
                                    | crate::varac::messages::IncomingMessage::VMailFailed
                                    | crate::varac::messages::IncomingMessage::ParkingNotAllowed
                                    | crate::varac::messages::IncomingMessage::AttachmentRefused => {
                                        if let Some((g, _)) = awaiting_ack.pop_front() {
                                            vmail_skip.insert(g);
                                        }
                                        // <SMF>: VarAC stops forwarding VMail for the rest of the link
                                        if matches!(m, crate::varac::messages::IncomingMessage::VMailFailed) {
                                            vmail_stopped = true;
                                        }
                                    }
                                    crate::varac::messages::IncomingMessage::VMailAttachmentOffer(o) => {
                                        (att_offer, att_payload) = (Some(o.clone()), None);
                                    }
                                    crate::varac::messages::IncomingMessage::BbsFilePayload(data) if att_offer.is_some() => {
                                        att_payload = Some(data.clone());
                                    }
                                    // Partner runs an email gateway: hand it our queued emails
                                    // (once the link is open; VarAC won't take them on the calling frequency)
                                    crate::varac::messages::IncomingMessage::Opening(r) if r.services.has_email() && !partner_gateway => {
                                        partner_gateway = true;
                                        if link_open && !relay_decided && relay_offer.is_none() {
                                            relay_offer = offer_email_relay(bg_db.as_ref(), sess.connected_to.as_deref(), &session_tx2);
                                        }
                                    }
                                    _ => {}
                                }
                                SessionMsg::Message(m)
                            }
                            crate::varac::session::SessionEvent::Handshake(step) => SessionMsg::Log(format!("Handshake: {step}")),
                            crate::varac::session::SessionEvent::Notice(m) => SessionMsg::SystemChat(m),
                            crate::varac::session::SessionEvent::QsyInvitation { target, inviter } => {
                                qsy_inviting = inviter;
                                SessionMsg::QsyInvitation { target, inviter }
                            }
                            crate::varac::session::SessionEvent::QsyResolved => SessionMsg::QsyResolved,
                            crate::varac::session::SessionEvent::TypedCq(cq) => SessionMsg::TypedCq(cq, last_snr),
                            crate::varac::session::SessionEvent::FrameId { outgoing, id, text } => SessionMsg::FrameId { outgoing, id, text },
                            crate::varac::session::SessionEvent::TxStarted { text, bytes } => SessionMsg::TxStarted { text, bytes },
                            crate::varac::session::SessionEvent::HandshakeComplete => {
                                link_open = true;
                                for (_, tag) in held_tags.drain(..) {
                                    let _ = sess.send_chat(tag).await;
                                }
                                // Mail queued while the gateway was down may go out now, while
                                // its sender can still hear <ER>
                                gw_notify.notify_one();
                                if let (Some(db), Some(partner)) = (bg_db.as_ref(), sess.connected_to.clone()) {
                                    if (!started_on_cf || qsy_done || partner_away || sess.is_away()) && !vmail_stopped {
                                        awaiting_ack = deliver_queued_vmail(sess, db, &partner, &session_tx2, &mut vmail_skip).await.into();
                                    } else if !vw_sent && has_mail_for(db, &partner) {
                                        // Still on the calling frequency: tell the partner (once)
                                        // and ask our operator to invite a QSY
                                        vw_sent = true;
                                        let _ = sess.send_chat("<VW>").await;
                                        let _ = session_tx2.send(SessionMsg::VMailRelayAlert(partner.clone()));
                                    }
                                    if partner_gateway && !relay_decided && relay_offer.is_none() {
                                        relay_offer = offer_email_relay(Some(db), Some(&partner), &session_tx2);
                                    }
                                }
                                SessionMsg::HandshakeComplete
                            }
                            crate::varac::session::SessionEvent::QsyBy(hz) => {
                                if std::mem::take(&mut qsy_inviting) {
                                    qsy_wait = ptt_router.dial().map(|d| (std::time::Instant::now(), d));
                                }
                                // Retune now, not on the next GUI frame (the window may be hidden)
                                ptt_router.offset_freq(hz as i64);
                                qsy_done = true;
                                // Off the calling frequency now: VMail waiting for the QSY goes out
                                if link_open && awaiting_ack.is_empty() && !vmail_stopped {
                                    if let (Some(db), Some(partner)) = (bg_db.as_ref(), sess.connected_to.clone()) {
                                        awaiting_ack.extend(deliver_queued_vmail(sess, db, &partner, &session_tx2, &mut vmail_skip).await);
                                    }
                                }
                                SessionMsg::QsyBy(hz)
                            }
                            crate::varac::session::SessionEvent::QsyInvitationRequested => SessionMsg::QsyInvitationRequested,
                            crate::varac::session::SessionEvent::VaraEvent(crate::vara::events::VaraEvent::SignalReport(db)) => {
                                last_snr = Some(db);
                                SessionMsg::Snr(db)
                            }
                            // VarAC checks the modem is the VARA it's set up for
                            crate::varac::session::SessionEvent::VaraEvent(crate::vara::events::VaraEvent::Version(v))
                                if !v.to_uppercase().contains(crate::config::modem_type().version_word()) =>
                            {
                                SessionMsg::SystemChat(format!(
                                    "The modem says \"{v}\" but Settings → Vara has {}: check the VARA modem type.",
                                    crate::config::modem_type().key()
                                ))
                            }
                            crate::varac::session::SessionEvent::VaraEvent(crate::vara::events::VaraEvent::TuneLevel(db)) => {
                                DRIVE_LEVEL.store(db, std::sync::atomic::Ordering::Relaxed);
                                SessionMsg::DriveLevel(db)
                            }
                            crate::varac::session::SessionEvent::VaraEvent(crate::vara::events::VaraEvent::IAmAlive) => {
                                last_alive = Some(std::time::Instant::now());
                                continue;
                            }
                            // "BITRATE (4)  88 bps TX": the speed level and rate, for the send progress
                            crate::varac::session::SessionEvent::VaraEvent(crate::vara::events::VaraEvent::Bitrate(b))
                                if parse_bitrate(&b).is_some() =>
                            {
                                let (level, bps, tx) = parse_bitrate(&b).unwrap_or_default();
                                let _ = session_tx2.send(SessionMsg::Log(format!("Bitrate(\"{b}\")")));
                                SessionMsg::Bitrate { level, bps, tx }
                            }
                            crate::varac::session::SessionEvent::VaraEvent(e) => SessionMsg::Log(format!("{e:?}")),
                        };
                        let _ = session_tx2.send(msg);
                }
            } else {
                // No session — wait for a command (e.g. ConnectModem)
                if let Some(cmd) = cmd_rx.recv().await {
                    process_command(cmd, &mut session, &callsign, &modem_config, &station_config, &session_tx2).await;
                }
            }
        }
    });
}

/// Email gateway sender (as in VarAC): mails out the
/// emails stations handed us, oldest first. On failure it retries every 10 s up to the
/// retry limit, then shuts the gateway down until the operator resumes it; the mail
/// stays queued. A sender still connected hears <ER> (sent) or <EF> (failed).
fn spawn_gateway_sender(
    rt: &tokio::runtime::Handle,
    my_call: String,
    db: Option<Storage>,
    tx: mpsc::Sender<SessionMsg>,
    notify: std::sync::Arc<tokio::sync::Notify>,
    gate: Option<crate::varac::email_gateway::EmailGate>,
    tags: tokio::sync::mpsc::UnboundedSender<(String, &'static str)>,
) {
    let (Some(db), Some(gate)) = (db, gate) else { return };
    let db = std::sync::Arc::new(std::sync::Mutex::new(db));
    rt.spawn(async move {
        let mut failures: u32 = 0;
        // (as in VarAC: not tried again this run)
        let mut attachment_failed = std::collections::HashSet::new();
        loop {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(60), notify.notified()).await;
            loop {
                let st = gate.state();
                if !st.enabled || st.suspended {
                    break;
                }
                let Some(cfg) = crate::config::gateway_config() else { break };
                let job = db.lock().ok().and_then(|d| {
                    let rec = d.gateway_emails_pending().ok()?.into_iter().find(|r| !attachment_failed.contains(&r.guid))?;
                    let (reply_to, raw) = d.vmail_email(&rec.guid).unwrap_or((None, false));
                    let paths = d.vmail_attachments(&rec.guid).unwrap_or_default();
                    Some((rec, reply_to, raw, paths))
                });
                let Some((rec, reply_to, raw, paths)) = job else { break };
                // VarAC: the attachments go as ONE zip; when one can't
                // be read the email isn't sent and the VMail stays where it is
                let package = (!paths.is_empty()).then(|| -> std::io::Result<(String, Vec<u8>)> {
                    let mut files = Vec::new();
                    for p in &paths {
                        files.push((p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), std::fs::read(p)?));
                    }
                    Ok(match &files[..] {
                        [(name, bytes)] if name.to_uppercase().ends_with(".ZIP") => (name.clone(), bytes.clone()),
                        _ => (crate::varac::session::vmail_package_name(&rec.subject, Utc::now()), crate::varac::session::vmail_package(&files)?),
                    })
                });
                let files = match package.transpose() {
                    Ok(p) => p.into_iter().collect::<Vec<_>>(),
                    Err(e) => {
                        let list = paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("|");
                        let _ = tx.send(SessionMsg::Log(format!(
                            "Email gateway: attachment of VMail {} ({list}) is missing or unreadable. Email not sent, VMail left in place: {e}", rec.guid
                        )));
                        attachment_failed.insert(rec.guid.clone());
                        continue;
                    }
                };
                let vm = VMail {
                    guid: rec.guid.clone(),
                    from: rec.vmail_from.clone(),
                    to: rec.vmail_to.clone(),
                    subject: rec.subject.clone(),
                    body: rec.msg.clone(),
                    compose_time: rec.creation_time.format("%Y-%m-%d %H:%M:%S").to_string(),
                    timestamp: rec.creation_time,
                    urgent: rec.urgent,
                    is_parked: false,
                    has_attachment: !files.is_empty(),
                    reply_to,
                    raw,
                };
                let d = crate::varac::email_gateway::Delivery {
                    sender_call: rec.vmail_from.clone(),
                    gateway_call: my_call.clone(),
                    band: rec.delivery_band.clone(),
                    frequency_hz: rec.frequency.map(|f| f as u64),
                    snr: rec.delivery_snr.map(|s| s as f32),
                };
                let gateway_address = cfg.from_address.trim().to_string();
                let r = tokio::task::spawn_blocking(move || crate::varac::email_gateway::send_with(&cfg, &vm, &d, &files))
                    .await
                    .unwrap_or_else(|e| Err(e.to_string()));
                let log = |s: String| { let _ = tx.send(SessionMsg::Log(s)); };
                match r {
                    Ok(()) => {
                        failures = 0;
                        if let Ok(d) = db.lock() {
                            let _ = d.move_vmail_to_sent(&rec.guid);
                            // (VarAC: via = the gateway's own address)
                            let _ = d.set_vmail_via(&rec.guid, &gateway_address);
                        }
                        log(format!("Email sent to {} through Email gateway successfully.", rec.vmail_to));
                        let _ = tags.send((rec.vmail_from.clone(), "<ER>"));
                    }
                    Err(e) => {
                        failures += 1;
                        let limit = crate::config::gateway_policy().2;
                        if failures <= limit {
                            log(format!("Failed to send Email. Retry {failures}/{limit}. Next retry in 10 seconds: {e}"));
                            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                        } else {
                            failures = 0;
                            gate.update(|g| g.suspended = true);
                            log(format!("SHUTTING DOWN EMAIL GATEWAY. Too many retries (exceeded limit of {limit} retries)"));
                            let _ = tags.send((rec.vmail_from.clone(), "<EF>"));
                            let _ = tx.send(SessionMsg::EmailGatewaySuspended(e));
                            break;
                        }
                    }
                }
            }
        }
    });
}

/// Queued emails and a gateway on the link: ask, like VarAC's relay popup. With Auto
/// relay on it relays after 10 s unless declined; off, it doesn't after 30 s unless told to.
fn offer_email_relay(
    db: Option<&std::sync::Mutex<Storage>>,
    partner: Option<&str>,
    tx: &mpsc::Sender<SessionMsg>,
) -> Option<(tokio::time::Instant, bool)> {
    let count = db?.lock().ok()?
        .vmails_in_folder(crate::storage::VMailFolder::Outbox, 200).ok()?
        .iter().filter(|v| v.vmail_to.contains('@')).count();
    if count == 0 {
        return None;
    }
    let auto = crate::config::email_auto_relay();
    let secs = if auto { 10 } else { 30 };
    let _ = tx.send(SessionMsg::EmailRelayOffer {
        gateway: partner.unwrap_or("").to_string(),
        count,
        auto,
        deadline: std::time::Instant::now() + std::time::Duration::from_secs(secs),
    });
    Some((tokio::time::Instant::now() + std::time::Duration::from_secs(secs), auto))
}

/// Outbox or parked VMail waiting for `partner`?
/// VarAC's "In queue" lines: relay notifications and broadcasts waiting, then the QSO's
/// messages (the one in the modem first), control data worded as VarAC shows it.
fn queue_view(session: Option<&VaracSession>, ui_out: &std::collections::VecDeque<UiOut>) -> Vec<String> {
    let mut out: Vec<String> = ui_out.iter().map(|o| {
        let info = String::from_utf8_lossy(&o.frame.info).to_string();
        if info.contains("<SMP>") || info.contains("<SMU>") {
            format!("RELAY NOTIFICATION to {}", o.frame.dest)
        } else {
            format!("BROADCAST to {}: {}", o.frame.dest, crate::varac::messages::desanitize(&info))
        }
    }).collect();
    for t in session.map(|s| s.queued_texts()).unwrap_or_default() {
        out.push(if t.starts_with("&IT&") {
            "<< CHAT CONTROL DATA >>".into()
        } else if t.starts_with("&L") {
            "<< SENDING REACTION >>".into()
        } else if t.starts_with("<SFB") {
            "<< SEND FILE >>".into()
        } else {
            t.replace("//n", " ")
        });
    }
    out
}

/// CLR (VarAC "Clearing outgoing queues."): the QSO's waiting messages and the broadcast
/// queue are dropped; EDIT gives the QSO's texts back to the message box.
async fn clear_queues(edit: bool, session: &mut Option<VaracSession>, ui_out: &mut std::collections::VecDeque<UiOut>, tx: &std::sync::mpsc::Sender<SessionMsg>) {
    let taken = session.as_mut().map(|s| s.take_queue()).unwrap_or_default();
    if !edit {
        ui_out.clear();
    }
    let _ = tx.send(SessionMsg::Log("Clearing outgoing queues.".into()));
    if edit {
        let texts: Vec<String> = taken.into_iter().filter(|t| !t.starts_with("&IT&") && !t.starts_with("&L") && !t.starts_with("<SFB")).collect();
        let _ = tx.send(SessionMsg::QueueReturned(texts));
    }
}

/// One frame in VarAC's outgoing broadcast queue, to go out at (or after) `at`.
struct UiOut {
    at: tokio::time::Instant,
    frame: crate::vara::kiss::UiFrame,
}

/// A number in lo..hi, random enough for VarAC's transmit jitter.
fn rand_between(lo: u64, hi: u64) -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
    lo + h.finish() % hi.saturating_sub(lo).max(1)
}

/// VarAC's transmit switches, as the bridge sees them.
#[derive(Default)]
struct TxGuard {
    ptt_disabled: bool,
    ignore_dcd: bool,
    unattended: bool,
    /// Connect / CQ waiting for the channel to clear ("WAITING FOR FREQUENCY TO CLEAR")
    waiting: Option<GuiCommand>,
    /// Our own beacon is on the air until then (VarAC keys for it even when away
    /// with unattended links off)
    beacon_until: Option<tokio::time::Instant>,
    tuning: bool,
    ptt_on_since: Option<tokio::time::Instant>,
}

impl TxGuard {
    /// May the radio be keyed now, for the modem's PTT ON? (as in VarAC)
    fn may_key(&self, away: bool) -> bool {
        let own_beacon = self.beacon_until.is_some_and(|t| tokio::time::Instant::now() < t);
        !self.ptt_disabled && (self.unattended || !away || own_beacon)
    }
}

fn ptt_disabled_alert(tx: &mpsc::Sender<SessionMsg>) {
    let _ = tx.send(SessionMsg::TxRefused {
        title: "PTT is disabled!".into(),
        text: "You can't perform this action while your PTT is disabled.\nPlease untick the 'PTT Disabled' checkbox and try again.".into(),
    });
}

/// Transmit safety in front of every GUI command, as VarAC does it:
/// - PTT disabled: connect, CQ, tune, broadcasts and Path Finder are refused ("PTT is
///   disabled!"); a ping or one-time beacon goes ahead, but the radio isn't keyed.
/// - Busy channel (unless Ignore DCD): connect and CQ wait for it to clear; a ping is
///   refused; tune and a one-time beacon ask "Do you still wish to …?" first.
/// Returns the command to run now, if any.
fn guard_tx(
    cmd: GuiCommand,
    g: &mut TxGuard,
    busy: bool,
    ptt_router: &crate::rig::PttRouter,
    tx: &mpsc::Sender<SessionMsg>,
) -> Option<GuiCommand> {
    let (cmd, confirmed) = match cmd {
        GuiCommand::Confirmed(inner) => (*inner, true),
        GuiCommand::SetPttDisabled(on) => {
            g.ptt_disabled = on;
            if on {
                ptt_router.key(false);
                g.waiting = None;
            }
            return None;
        }
        GuiCommand::SetIgnoreDcd(on) => {
            g.ignore_dcd = on;
            return None;
        }
        GuiCommand::SetUnattended(on) => {
            g.unattended = on;
            return None;
        }
        // ABORT also gives up waiting for the channel
        GuiCommand::Abort => {
            if g.waiting.take().is_some() {
                let _ = tx.send(SessionMsg::SystemChat("STOPPED WAITING FOR THE FREQUENCY TO CLEAR".into()));
            }
            return Some(GuiCommand::Abort);
        }
        other => (other, false),
    };
    let busy = busy && !g.ignore_dcd && !confirmed;
    let beacon = matches!(&cmd, GuiCommand::SendCq { source, .. } if source.ends_with("-9") || source.ends_with("-8"));
    let held = |what: &str| SessionMsg::TxHeld {
        cmd: GuiCommand::Abort, // replaced below
        title: "Frequency is busy!".into(),
        text: format!("The frequency appears to be busy.\nPlease wait until the frequency is cleared.\n\nDo you still wish to {what}?"),
    };
    match &cmd {
        GuiCommand::Connect(_) | GuiCommand::SendCq { .. } | GuiCommand::SendTypedCq { .. } if !beacon => {
            if g.ptt_disabled {
                ptt_disabled_alert(tx);
                return None;
            }
            if busy {
                let _ = tx.send(SessionMsg::SystemChat("WAITING FOR FREQUENCY TO CLEAR".into()));
                g.waiting = Some(cmd);
                return None;
            }
        }
        GuiCommand::SendCq { .. } => {
            if busy {
                if let SessionMsg::TxHeld { title, text, .. } = held("send a one-time beacon") {
                    let _ = tx.send(SessionMsg::TxHeld { cmd, title, text });
                }
                return None;
            }
            g.beacon_until = Some(tokio::time::Instant::now() + std::time::Duration::from_secs(20));
        }
        GuiCommand::Tune => {
            if g.ptt_disabled {
                ptt_disabled_alert(tx);
                return None;
            }
            if busy {
                if let SessionMsg::TxHeld { title, text, .. } = held("TUNE") {
                    let _ = tx.send(SessionMsg::TxHeld { cmd, title, text });
                }
                return None;
            }
            g.tuning = true;
        }
        GuiCommand::TuneStop => g.tuning = false,
        GuiCommand::Ping(_) if busy => {
            let _ = tx.send(SessionMsg::TxRefused {
                title: "Frequency is busy".into(),
                text: "You can not connect while the frequency appears to be busy.\n\nIf it is free, then wait a few seconds until the busy\nindicator is cleared and try again.".into(),
            });
            return None;
        }
        GuiCommand::SendBroadcast { .. } | GuiCommand::PathFinderRequest(_) | GuiCommand::EmCommCheckIn if g.ptt_disabled => {
            ptt_disabled_alert(tx);
            return None;
        }
        _ => {}
    }
    Some(cmd)
}

/// Broadcasts and Path Finder requests join the outgoing broadcast queue; other
/// commands go on to `process_command`.
fn queue_ui_command(cmd: GuiCommand, callsign: &str, out: &mut std::collections::VecDeque<UiOut>) -> Option<GuiCommand> {
    use crate::varac::messages as m;
    let frame = match cmd {
        GuiCommand::SendBroadcast { to, via, text } => m::ui_frame_via(callsign, &to, &via, &m::sanitize(&text)),
        GuiCommand::PathFinderRequest(target) => m::ui_frame(callsign, "ALL", &m::path_finder_request(&target)),
        // (raw, not sanitized: VarAC's check-in is a literal <EC>)
        GuiCommand::EmCommCheckIn => m::ui_frame(callsign, "EMCOMM", "<EC>"),
        other => return Some(other),
    };
    out.push_back(UiOut { at: tokio::time::Instant::now(), frame });
    None
}

/// VarAC's answers to a broadcast, as (seconds to wait, frame): a Path Finder request
/// to ALL (after 5–44 s; the target itself answers `«R+NN»`, others that heard it in the
/// last 24 h `~45F3`), and requests addressed to us: `<SR>` → `«R+NN»`, `<LOCR>`,
/// `<GPSR>`, `<VER>`, `<EGA>`, `<LHC:CALL>`. Replies go back through the same digipeaters.
fn broadcast_answers(
    bc: &crate::varac::messages::Broadcast,
    callsign: &str,
    station: &crate::varac::session::StationConfig,
    db: Option<&std::sync::Mutex<Storage>>,
    version: &str,
) -> Vec<(u64, crate::vara::kiss::UiFrame)> {
    use crate::varac::messages as m;
    let me = base_callsign(callsign);
    if base_callsign(&bc.from) == me {
        return Vec::new();
    }
    let via: Vec<String> = bc.via.as_deref().unwrap_or("").split_whitespace().map(str::to_string).collect();
    // (as in VarAC: destination and via upper-cased; the answer goes as it is,
    // only the SNR report sanitized: «R+NN»)
    let via: Vec<String> = via.iter().map(|v| m::sanitize(v).to_uppercase()).collect();
    let to = m::sanitize(&bc.from).to_uppercase();
    let reply = |text: &str| m::ui_frame_via(callsign, &to, &via, text);
    let snr = m::sanitize(&format!("<R{}>", m::snr_text(bc.snr.unwrap_or(0.0).round() as i32)));
    let msg = bc.message.trim();
    let mut out = Vec::new();
    if bc.to.eq_ignore_ascii_case("ALL") {
        if let Some(target) = msg.strip_prefix('^').map(|t| t.trim().to_uppercase()).filter(|t| !t.is_empty()) {
            if crate::config::path_finder_enabled() {
                let delay = rand_between(5, 45);
                if base_callsign(&target) == me {
                    out.push((delay, reply(&snr)));
                } else if let Some((t, s, f)) = db
                    .and_then(|d| d.lock().ok())
                    .and_then(|d| d.path_finder_heard(&target, &(Utc::now() - chrono::Duration::hours(24))).ok().flatten())
                {
                    let hours = (Utc::now() - t).num_hours();
                    out.push((delay, reply(&m::path_finder_answer(s.unwrap_or(0), f.max(0) as u64, hours))));
                }
            }
        }
        return out;
    }
    if base_callsign(&bc.to) != me {
        return out;
    }
    let info_ok = station.policy.allow_info.load(std::sync::atomic::Ordering::Relaxed);
    if msg.contains("<SR>") {
        out.push((0, reply(&snr)));
    }
    if msg.contains("<LOCR>") {
        let loc = station.locator.clone().filter(|l| !l.trim().is_empty()).unwrap_or_else(|| "NO LOCATOR".into());
        out.push((0, reply(if info_ok { &loc } else { "<INFOJ>" })));
    }
    if msg.contains("<GPSR>") {
        let gps = crate::config::gps_answer();
        out.push((0, reply(if info_ok { &gps } else { "<INFOJ>" })));
    }
    if msg.contains("<VER>") {
        out.push((0, reply(version)));
    }
    if msg.contains("<EGA>") {
        let addr = station.email_gate.as_ref().map(|g| g.address_reply()).unwrap_or_else(|| "N/A".into());
        out.push((0, reply(&addr)));
    }
    if let Some(i) = msg.find("<LHC:") {
        let call = msg[i + 5..].split('>').next().unwrap_or("").trim().to_string();
        let text = db
            .and_then(|d| d.lock().ok())
            .and_then(|d| d.last_heard_callsign(&call).ok().flatten())
            .map(|e| m::last_heard_callsign_reply(&e))
            .unwrap_or_else(|| "<LHCE>".into());
        out.push((0, reply(&text)));
    }
    out
}

fn has_mail_for(db: &std::sync::Mutex<Storage>, partner: &str) -> bool {
    let base = base_callsign(partner);
    db.lock().ok().is_some_and(|d| {
        !d.outbox_vmails_for(partner).unwrap_or_default().is_empty()
            || d.vmails_in_folder(crate::storage::VMailFolder::Parking, 200).unwrap_or_default()
                .iter().any(|m| base_callsign(&m.vmail_to) == base)
    })
}

fn base_callsign(call: &str) -> String {
    call.split(['-', '/']).next().unwrap_or(call).trim().to_uppercase()
}

/// When we last transmitted a relay notification to each station (as in
/// VarAC, forgotten on any frequency change).
#[derive(Default)]
struct RelayNotified {
    sent: std::collections::HashMap<String, std::time::Instant>,
    dial: Option<u64>,
}

impl RelayNotified {
    fn sync(&mut self, dial: Option<u64>) {
        if dial != self.dial {
            self.sent.clear();
            self.dial = dial;
        }
    }
}

/// A station we hold VMail for was heard beaconing: queue VarAC's relay notification.
/// `heard` is the callsign as the beacon gave it.
fn relay_notification_frame(
    db: &std::sync::Mutex<Storage>,
    my_call: &str,
    heard: &str,
    notified: &RelayNotified,
    queued: &std::collections::VecDeque<UiOut>,
    tx: &mpsc::Sender<SessionMsg>,
) -> Option<crate::vara::kiss::UiFrame> {
    let target = heard.trim().to_uppercase();
    let (my_full, my_base) = (my_call.to_uppercase(), base_callsign(my_call));
    if target.is_empty() || target == my_full || target == my_base || !crate::config::send_relay_notifications() {
        return None;
    }
    // VMail in Outbox or Parking (folders 3, 4) whose To is the station, or holds
    // "CALL/" or "/CALL" (SQL LIKE '%CALL/%', '%/CALL%'); <SMU> when one is urgent
    let addressed = |to: &str| {
        let to = to.to_uppercase();
        to == target || to.contains(&format!("{target}/")) || to.contains(&format!("/{target}"))
    };
    let mail: Vec<bool> = db.lock().ok()
        .map(|d| {
            let mut v = d.vmails_in_folder(crate::storage::VMailFolder::Parking, 1000).unwrap_or_default();
            v.extend(d.vmails_in_folder(crate::storage::VMailFolder::Outbox, 1000).unwrap_or_default());
            v.iter().filter(|m| addressed(&m.vmail_to)).map(|m| m.urgent).collect()
        })
        .unwrap_or_default();
    if mail.is_empty() {
        return None;
    }
    let log = |m: String| { let _ = tx.send(SessionMsg::Log(m)); };
    // VarAC waits 20 minutes between relay notifications, counted from when it went out
    if notified.sent.get(&target).is_some_and(|t| t.elapsed().as_secs() <= 20 * 60) {
        log(format!("Relay notification was NOT sent to {target} as it was already sent in the past 20 minutes"));
        return None;
    }
    if queued.iter().any(|o| o.frame.dest.eq_ignore_ascii_case(&target)) {
        log(format!("Skipping queueing relay notification for {target}. Already in relay notification queue"));
        return None;
    }
    if target.len() > 7 || my_base.len() > 7 {
        log("WARNING: Can not send relay notification as source & target callsigns must be no longer than 7 characers".into());
        return None;
    }
    log(format!("Preparing to send a VMail relay notifiaction to {target}"));
    // (it goes through VarAC's broadcast queue: quiet channel, not while linked)
    Some(crate::varac::messages::ui_frame(&my_base, &target, if mail.iter().any(|u| *u) { "<SMU>" } else { "<SMP>" }))
}

/// Send one VMail (with its attachments); an attachment VarAC wouldn't send on this
/// link is reported and the VMail stays where it is.
enum VmailSend { Sent, NotNow, Skipped }

async fn send_one_vmail(sess: &mut VaracSession, vmail: VMail, paths: &[std::path::PathBuf], tx: &mpsc::Sender<SessionMsg>) -> VmailSend {
    if paths.is_empty() {
        return if sess.send_vmail(&vmail).await.is_ok() { VmailSend::Sent } else { VmailSend::Skipped };
    }
    let skip = |why: String| {
        let _ = tx.send(SessionMsg::SystemChat(format!("VMAIL WITH ATTACHMENT NOT SENT: {why}. IT STAYS IN THE OUTBOX.")));
        VmailSend::Skipped
    };
    // (VarAC: only to a station that announced VMail attachments, asked with <SQ>)
    match sess.attachment_gate().await {
        crate::varac::session::AttachmentGate::Wait => return VmailSend::NotNow,
        crate::varac::session::AttachmentGate::Skip(why) => return skip(why),
        crate::varac::session::AttachmentGate::Send => {}
    }
    let mut files = Vec::new();
    for p in paths {
        match std::fs::read(p) {
            Ok(b) => files.push((p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), b)),
            Err(e) => return skip(format!("ATTACHMENT FILE(S) MISSING OR UNREADABLE: {e}")),
        }
    }
    match sess.send_vmail_with_attachments(&VMail { has_attachment: true, ..vmail }, &files).await {
        Ok(()) => VmailSend::Sent,
        Err(e) => skip(e.to_string()),
    }
}

/// VarAC "Relay now through connected station": these VMails go to the station we're
/// connected to whoever they're for (it parks what isn't for it).
async fn relay_vmails_now(
    sess: &mut VaracSession,
    db: &std::sync::Mutex<Storage>,
    guids: &[String],
    tx: &mpsc::Sender<SessionMsg>,
) -> Vec<(String, Option<VMail>)> {
    let jobs: Vec<(String, VMail, Vec<std::path::PathBuf>)> = {
        let Ok(d) = db.lock() else { return Vec::new() };
        guids.iter().filter_map(|g| {
            let vm = d.vmail_by_guid(g).ok().flatten()?;
            let (reply_to, raw) = d.vmail_email(g).unwrap_or((None, false));
            let vmail = VMail {
                guid: vm.guid.clone(),
                from: vm.vmail_from.clone(),
                to: vm.vmail_to.clone(),
                subject: vm.subject.clone(),
                body: vm.msg.clone(),
                compose_time: vm.creation_time.format("%Y-%m-%d %H:%M:%S").to_string(),
                timestamp: vm.creation_time,
                urgent: vm.urgent,
                is_parked: false,
                has_attachment: false,
                reply_to,
                raw,
            };
            Some((vm.guid.clone(), vmail, d.vmail_attachments(g).unwrap_or_default()))
        }).collect()
    };
    let mut sent = Vec::new();
    for (guid, vmail, paths) in jobs {
        let to = vmail.to.clone();
        if matches!(send_one_vmail(sess, vmail, &paths, tx).await, VmailSend::Sent) {
            let _ = tx.send(SessionMsg::Log(format!("VMail to {to} relayed through {}", sess.connected_to.clone().unwrap_or_default())));
            sent.push((guid, None));
        }
    }
    sent
}

/// After the opening handshake, send the partner its Outbox VMail (with any
/// attachments) and VMail parked here for it, as VarAC does.
async fn deliver_queued_vmail(
    sess: &mut VaracSession,
    db: &std::sync::Mutex<Storage>,
    partner: &str,
    tx: &mpsc::Sender<SessionMsg>,
    skip: &mut std::collections::HashSet<String>,
) -> Vec<(String, Option<VMail>)> {
    // VarAC's outbox: ONE VMail at a time (Outbox and Parking alike, in the order
    // stored), the next only after the partner answered this one
    let mut delivered = Vec::new();
    let to_vmail = |vm: &crate::storage::VMailRecord| VMail {
        guid: vm.guid.clone(),
        from: vm.vmail_from.clone(),
        to: vm.vmail_to.clone(),
        subject: vm.subject.clone(),
        body: vm.msg.clone(),
        // (the time it was written / first sent, kept when we relay it)
        compose_time: vm.sent_time.unwrap_or(vm.creation_time).format("%Y-%m-%d %H:%M:%S").to_string(),
        timestamp: vm.sent_time.unwrap_or(vm.creation_time),
        urgent: vm.urgent,
        is_parked: false,
        has_attachment: false,
        reply_to: None,
        raw: false,
    };
    let full = partner.to_uppercase();
    let base = partner.split('-').next().unwrap_or(partner).to_string();
    let job = {
        let Ok(d) = db.lock() else { return delivered };
        d.vmails_waiting_for(&base, &full).unwrap_or_default().into_iter()
            .filter(|vm| !skip.contains(&vm.guid) && !vm.vmail_to.contains('@'))
            .map(|vm| (vm.guid.clone(), to_vmail(&vm), d.vmail_attachments(&vm.guid).unwrap_or_default(), vm.folder_id == crate::storage::VMailFolder::Parking as i32))
            .next()
    };
    let Some((guid, vmail, paths, relayed)) = job else { return delivered };
    let to = vmail.to.clone();
    // Pulled in by the email gateway (From is an address): notify the sender on <SMR>
    let from_email = vmail.from.contains('@').then(|| vmail.clone());
    let sent = send_one_vmail(sess, vmail, &paths, tx).await;
    if matches!(sent, VmailSend::NotNow) {
        // (asked the partner's services, or a file is on the way: next round)
        return delivered;
    }
    if matches!(sent, VmailSend::Sent) {
        // Filed as Sent when the partner acknowledges it (<SMR>), see the bridge
        delivered.push((guid, from_email));
        let what = if relayed { "Relayed parked VMail to" } else { "Outbox VMail sent to" };
        let _ = tx.send(SessionMsg::Log(format!("{what} {to}")));
    } else {
        // (not sendable on this link, as in VarAC)
        skip.insert(guid);
    }
    delivered
}

/// Email gateway, incoming: check the gateway mailbox every poll interval (settings are
/// re-read each time) and park what arrives for its station, like original VarAC.
fn spawn_imap_poller(rt: &tokio::runtime::Handle, my_call: String, db: Option<Storage>, tx: mpsc::Sender<SessionMsg>, gate: Option<crate::varac::email_gateway::EmailGate>) {
    let Some(db) = db else { return };
    let db = std::sync::Arc::new(std::sync::Mutex::new(db));
    rt.spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(15)).await;
        loop {
            // (VarAC polls only while its gateway is on: not when switched off or shut down)
            let running = gate.as_ref().is_none_or(|g| { let s = g.state(); s.enabled && !s.suspended });
            let wait = match crate::config::imap_config().filter(|_| running) {
                Some((cfg, every)) => {
                    let (db, tx, call) = (db.clone(), tx.clone(), my_call.clone());
                    let _ = tokio::task::spawn_blocking(move || pull_emails(&cfg, &call, &db, &tx)).await;
                    every
                }
                None => 30,
            };
            tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
        }
    });
}

fn pull_emails(
    cfg: &crate::varac::email_inbound::ImapConfig,
    my_call: &str,
    db: &std::sync::Mutex<Storage>,
    tx: &mpsc::Sender<SessionMsg>,
) {
    use crate::varac::email_inbound::{Handled, ATTACHMENT_LIMIT_BYTES};
    let log = |s: String| { let _ = tx.send(SessionMsg::Log(s)); };
    let my_base = my_call.split(['-', '/']).next().unwrap_or(my_call).to_uppercase();
    let result = crate::varac::email_inbound::poll(cfg, my_call, |email| {
        let Some(e) = email else {
            log("Email gateway: incoming email has no <RELAY_TO_CALLSIGN:…> tag or \"CALL VIA MYCALL\" recipient. Skipping".into());
            return Handled::Done;
        };
        let guid = uuid::Uuid::new_v4().to_string();
        let mut paths = Vec::new();
        if !e.attachments.is_empty() {
            let size = crate::varac::session::vmail_package(&e.attachments).map(|z| z.len()).unwrap_or(usize::MAX);
            if size > ATTACHMENT_LIMIT_BYTES {
                log(format!("Email gateway: attachments of email from {} to {} exceed the radio limit. Bouncing.", e.from, e.to_call));
                if let Some(gw) = crate::config::gateway_config() {
                    if let Err(err) = crate::varac::email_gateway::send_attachment_bounce(
                        &gw, my_call, &e.from, &e.to_call, &e.subject, ATTACHMENT_LIMIT_BYTES / 1000,
                    ) {
                        log(format!("Email gateway: bounce to {} failed: {err}", e.from));
                    }
                }
                return Handled::Done;
            }
            let Some(dir) = crate::config::project_dirs()
                .map(|d| d.data_dir().join("attachments").join(format!("email_{guid}")))
            else {
                return Handled::Retry;
            };
            let saved = std::fs::create_dir_all(&dir).and_then(|_| {
                for (name, data) in &e.attachments {
                    let p = dir.join(safe_file_name(name));
                    std::fs::write(&p, data)?;
                    paths.push(p);
                }
                Ok(())
            });
            if let Err(err) = saved {
                log(format!("Email gateway: failed to save attachments of email from {}. Will retry on next poll: {err}", e.from));
                return Handled::Retry;
            }
        }
        // (as in VarAC: ours when TO is our call, our full call, or contains "MYCALL/")
        let to = e.to_call.trim().to_uppercase();
        let full = crate::config::full_callsign().trim().to_uppercase();
        let mine = to == my_call.to_uppercase() || (!full.is_empty() && to == full) || to.contains(&format!("{my_base}/"));
        let parked = !mine;
        let folder = if parked { crate::storage::VMailFolder::Parking } else { crate::storage::VMailFolder::Inbox };
        let Ok(d) = db.lock() else { return Handled::Retry };
        let stored = d.insert_vmail(&crate::storage::VMailRecord {
            guid: guid.clone(),
            creation_time: e.date,
            sent_time: None,
            received_time: Some(Utc::now()),
            folder_id: folder as i32,
            vmail_to: e.to_call.clone(),
            vmail_from: e.from.clone(),
            vmail_via: Some("EMAIL GATEWAY".into()),
            subject: e.subject.clone(),
            msg: e.body.clone(),
            read_status: false,
            is_deleted: false,
            urgent: e.urgent,
            delivery_band: None,
            delivery_snr: None,
            frequency: None,
        });
        if let Err(err) = stored {
            log(format!("Email gateway: could not store email from {}: {err}", e.from));
            return Handled::Retry;
        }
        for p in &paths {
            let _ = d.add_vmail_attachment(&guid, p);
        }
        log(format!(
            "Email gateway: incoming email through IMAP server detected: Destination callsign: {}. Pulling to the VMailbox.",
            e.to_call
        ));
        let _ = tx.send(SessionMsg::EmailIncoming { to_call: e.to_call, from: e.from, subject: e.subject, parked });
        Handled::Done
    });
    if let Err(e) = result {
        log(format!("Email gateway error: Unable to receive email from IMAP server. {e}"));
    }
}

/// Send Outbox VMail addressed to email addresses through the connected gateway.
/// Hand a gateway partner our Outbox emails (VarAC's "auto relay"). Like other VMail
/// they move to Sent on <SMR> and stay queued on <EJ>; returns them in send order.
async fn deliver_queued_email(
    sess: &mut VaracSession,
    db: &std::sync::Mutex<Storage>,
    tx: &mpsc::Sender<SessionMsg>,
) -> Vec<(String, Option<VMail>)> {
    let jobs: Vec<(String, VMail)> = {
        let Ok(d) = db.lock() else { return Vec::new() };
        d.vmails_in_folder(crate::storage::VMailFolder::Outbox, 200).unwrap_or_default().iter()
            .filter(|vm| vm.vmail_to.contains('@'))
            .map(|vm| {
                let (reply_to, raw) = d.vmail_email(&vm.guid).unwrap_or((None, false));
                (vm.guid.clone(), VMail {
                    guid: vm.guid.clone(),
                    from: vm.vmail_from.clone(),
                    to: vm.vmail_to.clone(),
                    subject: vm.subject.clone(),
                    body: vm.msg.clone(),
                    compose_time: vm.creation_time.format("%Y-%m-%d %H:%M:%S").to_string(),
                    timestamp: vm.creation_time,
                    urgent: vm.urgent,
                    is_parked: false,
                    has_attachment: false,
                    reply_to,
                    raw,
                })
            })
            .filter(|(_, v)| v.is_email())
            .collect()
    };
    let mut sent = Vec::new();
    for (guid, vm) in jobs {
        if sess.send_vmail(&vm).await.is_ok() {
            let _ = tx.send(SessionMsg::Log(format!("Email to {} handed to the gateway", vm.to)));
            sent.push((guid, None));
        }
    }
    sent
}

/// The modem's TX drive level as last read (TUNE ?) or set (DRIVELEVEL); MIN = not known.
static DRIVE_LEVEL: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(i32::MIN);

async fn process_command(
    cmd: GuiCommand,
    session: &mut Option<VaracSession>,
    callsign: &str,
    modem_config: &crate::tui::ModemConfig,
    station_config: &StationConfig,
    session_tx: &mpsc::Sender<SessionMsg>,
) {
    match cmd {
        GuiCommand::Connect(dest) => {
            if let Some(sess) = session.as_mut() {
                sess.mark_dialing();
                let _ = sess.vara.send_command(
                    &crate::vara::commands::VaraCommand::Connect {
                        source: callsign.to_string(),
                        destination: dest,
                    },
                ).await;
            }
        }
        GuiCommand::Disconnect => {
            if let Some(sess) = session.as_mut() {
                let _ = sess.send_disconnect().await;
            }
        }
        GuiCommand::DisconnectAfterSent => {
            if let Some(sess) = session.as_mut() {
                let _ = sess.disconnect_after_sent().await;
            }
        }
        GuiCommand::Abort => {
            if let Some(sess) = session.as_mut() {
                let _ = sess.vara.send_command(
                    &crate::vara::commands::VaraCommand::Abort,
                ).await;
            }
        }
        GuiCommand::SendChat(text) => {
            if let Some(sess) = session.as_mut() {
                let _ = sess.send_chat(&text).await;
            }
        }
        GuiCommand::SendVMail(vmail) => {
            if let Some(sess) = session.as_mut() {
                let _ = sess.send_vmail(&vmail).await;
            }
        }
        GuiCommand::FileReady => {
            if let Some(sess) = session.as_mut() {
                let _ = sess.send_file_ready().await;
            }
        }
        GuiCommand::FileOk => {
            if let Some(sess) = session.as_mut() {
                let _ = sess.send_file_ok().await;
            }
        }
        GuiCommand::FileAbort => {
            if let Some(sess) = session.as_mut() {
                let _ = sess.send_file_abort().await;
            }
        }
        GuiCommand::FileFailed => {
            if let Some(sess) = session.as_mut() {
                let _ = sess.send_file_failed().await;
            }
        }
        GuiCommand::AnswerAttachment(accept) => {
            if let Some(sess) = session.as_mut() {
                let _ = sess.answer_attachment(accept).await;
            }
        }
        // Handled in the bridge loop, which holds the delivery bookkeeping
        GuiCommand::FlushEmailOutbox | GuiCommand::DeclineEmailRelay | GuiCommand::ClearQueue { .. } => {}
        GuiCommand::SendCq { source, bandwidth } => {
            if let Some(sess) = session.as_mut() {
                // A beacon (CALL-9) in ADVANCED mode is VarAC's KISS beacon instead, carrying
                // our services; the email gateway is announced only when it's open to all
                // (VarAC: Announce on, no LimitAccessToCallsigns) and running
                if source.ends_with("-9") || source.ends_with("-8") {
                    // (VarAC sends the advanced beacon over KISS even with digipeaters set)
                    let (advanced, announce) = crate::config::beacon_settings();
                    if advanced {
                        let open_gateway = station_config.email_gate.as_ref()
                            .map(|g| g.state())
                            .is_some_and(|g| g.enabled && !g.suspended && g.allow.is_none());
                        let mut services = if announce && open_gateway { crate::vara::kiss::AdvancedBeacon::EMAIL_GATEWAY } else { 0 };
                        let (bbs_allow, bbs_announce) = crate::config::bbs_policy();
                        if bbs_announce && bbs_allow.is_none() && station_config.services.has_bbs() {
                            services |= crate::vara::kiss::AdvancedBeacon::BBS;
                        }
                        let ai = crate::config::ai_settings();
                        if ai.enabled && ai.announce {
                            services |= crate::vara::kiss::AdvancedBeacon::AI_GATEWAY;
                        }
                        if station_config.policy.scanning.load(std::sync::atomic::Ordering::Relaxed) {
                            services |= crate::vara::kiss::AdvancedBeacon::SCANNING;
                        }
                        // EmComm rides in the tag, not as -8 (as in VarAC)
                        if source.ends_with("-8") {
                            services |= crate::vara::kiss::AdvancedBeacon::EMCOMM;
                        }
                        let restart = match sess.send_advanced_beacon(services).await {
                            Ok(()) => {
                                let _ = session_tx.send(SessionMsg::Log(format!(
                                    "Advanced beacon sent{}",
                                    match services { 0 => "", s if s & crate::vara::kiss::AdvancedBeacon::EMAIL_GATEWAY != 0 => " (announcing email gateway)", _ => " (announcing BBS)" }
                                )));
                                let _ = session_tx.send(SessionMsg::OwnCq { source: source.clone(), bandwidth: Some(bandwidth) });
                                false
                            }
                            // (VarAC: an over-long field skips this beacon; a failed KISS send
                            // restarts the modem connection, no standard beacon instead)
                            Err(e) if e.to_string().contains("too long") => {
                                let _ = session_tx.send(SessionMsg::Log(e.to_string()));
                                false
                            }
                            Err(e) => {
                                let _ = session_tx.send(SessionMsg::Log(format!("Advanced beacon failed: {e}")));
                                let _ = session_tx.send(SessionMsg::Log("Initiating VARA restart due to KISS unformatted BEACON command failure.".into()));
                                true
                            }
                        };
                        if restart {
                            *session = None;
                            let _ = session_tx.send(SessionMsg::ModemDisconnected);
                            match connect_modem(modem_config, callsign, station_config).await {
                                Ok(sess) => {
                                    *session = Some(sess);
                                    let _ = session_tx.send(SessionMsg::ModemConnected);
                                }
                                Err(e) => {
                                    let _ = session_tx.send(SessionMsg::ModemFailed(e.to_string()));
                                }
                            }
                        }
                        return;
                    }
                }
                // VARA FM / SAT: no bandwidth; FM through the digipeaters (VarAC DigipeatVia)
                let cmd = match crate::config::modem_type() {
                    crate::vara::commands::ModemType::Hf => crate::vara::commands::VaraCommand::CqFrame { source: source.clone(), bandwidth },
                    _ => crate::vara::commands::VaraCommand::CqFrameNoBw { source: source.clone(), via: crate::config::digipeat_via() },
                };
                let hf = crate::config::modem_type() == crate::vara::commands::ModemType::Hf;
                if sess.vara.send_command(&cmd).await.is_ok() {
                    let _ = session_tx.send(SessionMsg::OwnCq { source, bandwidth: hf.then_some(bandwidth) });
                }
            }
        }
        GuiCommand::SetBandwidth(bw) => {
            if let Some(sess) = session.as_mut() {
                let _ = sess.vara.send_command(
                    &crate::vara::commands::VaraCommand::SetBandwidth(bw),
                ).await;
            }
        }
        GuiCommand::SetListen(mode) => {
            if let Some(sess) = session.as_mut() {
                let _ = sess.vara.send_command(
                    &crate::vara::commands::VaraCommand::Listen(mode),
                ).await;
            }
        }
        GuiCommand::SetAway(msg) => {
            if let Some(sess) = session.as_mut() {
                // (VarAC announces away only when a link starts, never during a QSO)
                sess.set_away(msg.clone());
            }
        }
        GuiCommand::ConnectModem => {
            if session.is_none() {
                match connect_modem(modem_config, callsign, station_config).await {
                    Ok(sess) => {
                        *session = Some(sess);
                        let _ = session_tx.send(SessionMsg::ModemConnected);
                    }
                    Err(e) => {
                        let _ = session_tx.send(SessionMsg::ModemFailed(e.to_string()));
                    }
                }
            }
        }
        GuiCommand::DisconnectModem => {
            if let Some(sess) = session.take() {
                drop(sess);
                let _ = session_tx.send(SessionMsg::ModemDisconnected);
            }
            // VarAC closes the VARA it started with the modem connection
            crate::vara::launch::close();
        }
        GuiCommand::Ping(dest) => {
            if let Some(sess) = session.as_mut() {
                // VarAC pings by linking to the station's "-T" alias; it answers with <R+NN>
                // (VARA FM through digipeaters: "CALL VIA DIGIS" → "CALL-T VIA DIGIS")
                let (call_part, via) = match dest.split_once(" VIA ") {
                    Some((c, v)) => (c.trim().to_string(), format!(" VIA {}", v.trim())),
                    None => (dest.clone(), String::new()),
                };
                let base = call_part.split('-').next().unwrap_or(&call_part).to_string();
                sess.mark_dialing();
                let _ = sess.vara.send_command(
                    &crate::vara::commands::VaraCommand::Connect {
                        source: callsign.to_string(),
                        destination: format!("{base}-T{via}"),
                    },
                ).await;
                let _ = session_tx.send(SessionMsg::Log(format!("Pinging {base} ({base}-T)")));
            }
        }
        GuiCommand::SendVMailWithAttachments(vmail, paths) => {
            if let Some(sess) = session.as_mut() {
                let mut files = Vec::new();
                for p in &paths {
                    match std::fs::read(p) {
                        Ok(data) => files.push((p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into()), data)),
                        Err(e) => {
                            let _ = session_tx.send(SessionMsg::Log(format!("Attachment {} not readable: {e}", p.display())));
                        }
                    }
                }
                if let Err(e) = sess.send_vmail_with_attachments(&vmail, &files).await {
                    let _ = session_tx.send(SessionMsg::Log(format!("VMail with attachment not sent: {e}")));
                }
            }
        }
        GuiCommand::SendQsyInvitation(tens) => {
            if let Some(sess) = session.as_mut() {
                let _ = sess.send_qsy_invitation(tens).await;
            }
        }
        // Queued by the session bridge (VarAC's outgoing broadcast queue)
        GuiCommand::SendBroadcast { .. } | GuiCommand::PathFinderRequest(_) | GuiCommand::EmCommCheckIn => {}
        // Unwrapped / handled by guard_tx before commands get here
        GuiCommand::Confirmed(_) | GuiCommand::SetPttDisabled(_) | GuiCommand::SetIgnoreDcd(_) | GuiCommand::SetUnattended(_)
        | GuiCommand::RelayNow { .. } => {}
        GuiCommand::SendFile(path) => {
            if let Some(sess) = session.as_mut() {
                match std::fs::read(&path) {
                    Ok(data) => {
                        let filename = path.file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("file");
                        match crate::varac::bbs::prepare_file(filename, &data) {
                            Ok(prepared) => {
                                let _ = session_tx.send(SessionMsg::Log(format!(
                                    "File prepared: {} ({} bytes compressed)",
                                    filename, prepared.compressed_size
                                )));
                                // Offer now; the data goes out when the partner answers <SFRD>
                                match sess.send_file(prepared).await {
                                    Ok(()) => {
                                        let _ = session_tx.send(SessionMsg::Log(format!("File offered: {filename}")));
                                    }
                                    Err(e) => {
                                        let _ = session_tx.send(SessionMsg::Log(format!("Send file offer error: {e}")));
                                    }
                                }
                            }
                            Err(e) => {
                                let _ = session_tx.send(SessionMsg::Log(format!("File prepare error: {e}")));
                            }
                        }
                    }
                    Err(e) => {
                        let _ = session_tx.send(SessionMsg::Log(format!("File read error: {e}")));
                    }
                }
            }
        }
        GuiCommand::InfoRequest => {
            if let Some(sess) = session.as_mut() {
                let _ = sess.send_info_request().await;
            }
        }
        GuiCommand::VerboseSnr(on) => {
            if let Some(sess) = session.as_mut() {
                let _ = sess.set_verbose_snr(on).await;
            }
        }
        GuiCommand::VerboseSnrAnswer(accept) => {
            if let Some(sess) = session.as_mut() {
                let _ = sess.answer_verbose_snr(accept).await;
            }
        }
        GuiCommand::ApplyPingPolicy => {
            if let Some(sess) = session.as_mut() {
                let _ = sess.apply_ping_policy().await;
            }
        }
        GuiCommand::SendInquiry(tag) => {
            if let Some(sess) = session.as_mut() {
                let _ = sess.send_chat(&tag).await;
            }
        }
        GuiCommand::GatewayAddressRequest => {
            if let Some(sess) = session.as_mut() {
                let _ = sess.send_chat("<EGA>").await;
            }
        }
        GuiCommand::SnrRequest => {
            if let Some(sess) = session.as_mut() {
                let _ = sess.send_snr_request().await;
            }
        }
        GuiCommand::Tune => {
            if let Some(sess) = session.as_mut() {
                // VarAC starts it as `TUNE <drive level>` (VARA's form); `TUNE ON` only
                // until the modem has said its level
                let level = DRIVE_LEVEL.load(std::sync::atomic::Ordering::Relaxed);
                let cmd = if level == i32::MIN {
                    crate::vara::commands::VaraCommand::Tune(true)
                } else {
                    crate::vara::commands::VaraCommand::TuneAt(level)
                };
                let _ = sess.vara.send_command(&cmd).await;
            }
        }
        GuiCommand::TuneStop => {
            if let Some(sess) = session.as_mut() {
                let _ = sess.vara.send_command(
                    &crate::vara::commands::VaraCommand::Tune(false),
                ).await;
            }
        }
        GuiCommand::DriveLevel(db) => {
            DRIVE_LEVEL.store(db, std::sync::atomic::Ordering::Relaxed);
            if let Some(sess) = session.as_mut() {
                let _ = sess.vara.send_command(&crate::vara::commands::VaraCommand::DriveLevel(db)).await;
            }
        }
        GuiCommand::ClearTxBuffer => {
            if let Some(sess) = session.as_mut() {
                let _ = sess.vara.send_command(
                    &crate::vara::commands::VaraCommand::CleanTxBuffer,
                ).await;
                let _ = session_tx.send(SessionMsg::Log("TX buffer cleared".into()));
            }
        }
        GuiCommand::QsyAnswer { accept, oor, offset_hz } => {
            if let Some(sess) = session.as_mut() {
                let _ = sess.answer_qsy(accept, oor, offset_hz).await;
            }
        }
        GuiCommand::SendTypedCq { slot, cq_type } => {
            if let Some(sess) = session.as_mut() {
                let r = sess.send_typed_cq(slot, &cq_type).await;
                let _ = session_tx.send(SessionMsg::Log(match r {
                    Ok(()) => format!("CQ {cq_type} sent (slot {slot}, with locator)"),
                    Err(e) => format!("CQ {cq_type} not sent: {e}"),
                }));
            }
        }
        GuiCommand::QsyStep { up, step_hz } => {
            if let Some(sess) = session.as_mut() {
                let _ = sess.send_qsy_step(up, step_hz).await;
            }
        }
        GuiCommand::SendReaction { id, like } => {
            if let Some(sess) = session.as_mut() {
                let _ = sess.send_reaction(id, like).await;
            }
        }
        GuiCommand::SendIsTyping => {
            if let Some(sess) = session.as_mut() {
                let _ = sess.send_is_typing().await;
            }
        }
        GuiCommand::RequestBbsList => {
            if let Some(sess) = session.as_mut() {
                let _ = sess.send_bbs_list_request().await;
                let _ = session_tx.send(SessionMsg::Log("BBS listing requested".into()));
            }
        }
        GuiCommand::RequestBbsFile(name) => {
            if let Some(sess) = session.as_mut() {
                let _ = sess.send_bbs_file_request(&name).await;
                let _ = session_tx.send(SessionMsg::Log(format!("BBS file requested: {name}")));
            }
        }
        GuiCommand::QsyRequest { offset_hz } => {
            if let Some(sess) = session.as_mut() {
                let tens = offset_hz / 10;
                let _ = sess.send_qsy_request(tens).await;
            }
        }
        GuiCommand::QsyAbsoluteRequest { freq_hz } => {
            if let Some(sess) = session.as_mut() {
                let _ = sess.send_qsy_absolute(freq_hz).await;
            }
        }
        GuiCommand::QsyAccept => {
            if let Some(sess) = session.as_mut() {
                let _ = sess.send_qsy_accept().await;
            }
        }
        GuiCommand::QsyReject => {
            if let Some(sess) = session.as_mut() {
                let _ = sess.send_qsy_reject().await;
            }
        }
    }
}

async fn connect_modem(
    cfg: &crate::tui::ModemConfig,
    callsign: &str,
    station_config: &StationConfig,
) -> Result<VaracSession, Box<dyn std::error::Error + Send + Sync>> {
    if let Some(line) = crate::config::launch_vara_if_set(&cfg.host, cfg.cmd_port, cfg.kiss_port).await {
        tracing::info!(target: "gui", "{line}");
    }
    let mut vara = VaraConnection::connect(&cfg.host, cfg.cmd_port, cfg.data_port).await?;
    // VarAC: VARA HF always starts at 500 Hz
    let pings = station_config.policy.allow_pings.load(std::sync::atomic::Ordering::Relaxed);
    vara.initialize_for(callsign, crate::vara::commands::Bandwidth::Bw500, crate::config::modem_type(), pings).await?;
    // (VarAC reads the modem's drive level for its tune slider)
    let _ = vara.send_command(&crate::vara::commands::VaraCommand::TuneQuery).await;
    let mut sess = VaracSession::new(vara, station_config.clone());
    sess.ask_qsy_invite = true;
    // Broadcasts go over the modem's KISS port; carry on without it if it's disabled
    if let Err(e) = sess.attach_kiss(&cfg.host, cfg.kiss_port).await {
        tracing::warn!("VARA KISS port {} not available: {e}", cfg.kiss_port);
    }
    Ok(sess)
}

/// The Openslot icon for the window and taskbar (rendered by tools/make_icon.py).
pub fn app_icon() -> Option<std::sync::Arc<egui::IconData>> {
    eframe::icon_data::from_png_bytes(include_bytes!("../../assets/icon/openslot-256.png"))
        .ok()
        .map(std::sync::Arc::new)
}

/// Linux desktops (Wayland above all) take the window icon from a .desktop entry whose
/// name matches the app id, not from the window: put ours and the icons in the user's
/// ~/.local/share once, and again when the program has moved.
#[cfg(target_os = "linux")]
fn install_desktop_entry() {
    let Some(data) = std::env::var_os("XDG_DATA_HOME").map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share")))
    else {
        return;
    };
    let Ok(exe) = std::env::current_exe() else { return };
    let entry = include_str!("../../assets/linux/openslot.desktop")
        .replace("Exec=openslot", &format!("Exec=\"{}\"", exe.display()));
    let apps = data.join("applications");
    let desktop = apps.join("openslot.desktop");
    if std::fs::read_to_string(&desktop).is_ok_and(|old| old == entry) {
        return;
    }
    let icons: [(&str, &[u8]); 8] = [
        ("16x16", include_bytes!("../../assets/icon/openslot-16.png")),
        ("32x32", include_bytes!("../../assets/icon/openslot-32.png")),
        ("48x48", include_bytes!("../../assets/icon/openslot-48.png")),
        ("64x64", include_bytes!("../../assets/icon/openslot-64.png")),
        ("128x128", include_bytes!("../../assets/icon/openslot-128.png")),
        ("256x256", include_bytes!("../../assets/icon/openslot-256.png")),
        ("512x512", include_bytes!("../../assets/icon/openslot-512.png")),
        ("scalable", include_bytes!("../../assets/icon/openslot.svg")),
    ];
    for (size, bytes) in icons {
        let dir = data.join("icons/hicolor").join(size).join("apps");
        let name = if size == "scalable" { "openslot.svg" } else { "openslot.png" };
        if std::fs::create_dir_all(&dir).is_ok() {
            let _ = std::fs::write(dir.join(name), bytes);
        }
    }
    if std::fs::create_dir_all(&apps).is_ok() && std::fs::write(&desktop, entry).is_ok() {
        tracing::info!(target: "gui", "Desktop entry installed: {}", desktop.display());
        // (refresh the caches when the tools are there; desktops also notice on their own)
        let _ = std::process::Command::new("update-desktop-database").arg(&apps)
            .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
        let _ = std::process::Command::new("gtk-update-icon-cache").args(["-q", "-t", "-f"]).arg(data.join("icons/hicolor"))
            .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
    }
}

/// Launch the GUI application.
pub fn run_gui(
    session: Option<VaracSession>,
    station_config: StationConfig,
    db: Storage,
    callsign: String,
    modem_config: crate::tui::ModemConfig,
    rt: tokio::runtime::Handle,
) -> Result<(), Box<dyn std::error::Error>> {
    let (session_tx, session_rx) = mpsc::channel::<SessionMsg>();
    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel::<GuiCommand>();

    let modem_connected = session.is_some();
    let ptt_router = crate::rig::PttRouter::default();

    spawn_session_bridge(
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
        // (the side column pairs its buttons up when short; a 1366×768 laptop leaves ~690 px)
        .with_min_inner_size([1000.0, 600.0])
        .with_title(format!("Openslot — {callsign}"))
        // (Linux: matches openslot.desktop, where the dock and Alt-Tab find the icon)
        .with_app_id("openslot");
    #[cfg(target_os = "linux")]
    install_desktop_entry();
    if let Some(icon) = app_icon() {
        viewport = viewport.with_icon(icon);
    }
    let native_options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "openslot",
        native_options,
        Box::new(move |cc| {
            let app = VaracApp::new(
                cc,
                callsign,
                modem_connected,
                session_rx,
                cmd_tx,
                db,
                station_config,
                modem_config,
                rt,
                ptt_router,
            );
            Ok(Box::new(GuiShell::start(app, &cc.egui_ctx)))
        }),
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// VarAC's compose rules: several callsigns with commas, one email, reply quoting.
    #[test]
    fn vmail_compose_like_varac() {
        assert_eq!(compose_recipients("nr5p, k1abc"), Some(vec!["NR5P".to_string(), "K1ABC".to_string()]));
        assert_eq!(compose_recipients("NR5P,,K1ABC"), None, "empty part");
        assert_eq!(compose_recipients("NR5P,"), None);
        assert_eq!(compose_recipients("bob@example.com"), Some(vec!["bob@example.com".to_string()]));
        assert_eq!(compose_recipients("a@b.com, NR5P"), None);
        assert_eq!(reply_quote("NR5P", "Short note"), "NR5P> Short note\n---\n");
        assert_eq!(
            reply_quote("NR5P", "W5TST> old stuff\n---\nThe new text that is quite a bit longer than fifty characters"),
            "NR5P> The new text that is quite a bit longer than fifty...\n---\n"
        );
    }

    /// VarAC's transmit safety: what each action does on a busy channel, with PTT
    /// disabled, with Ignore DCD, and when the modem may key the radio.
    #[test]
    fn transmit_guard_follows_varac() {
        let (tx, rx) = mpsc::channel();
        let ptt = crate::rig::PttRouter::default();
        let mut g = TxGuard { unattended: true, ..Default::default() };
        let mut run = |cmd, g: &mut TxGuard, busy| guard_tx(cmd, g, busy, &ptt, &tx);
        let last = || rx.try_iter().last();

        // Busy: connect waits (no question), ping is refused, tune asks, "yes" sends it
        assert!(run(GuiCommand::Connect("NR5P".into()), &mut g, true).is_none());
        assert!(matches!(g.waiting, Some(GuiCommand::Connect(_))));
        assert!(matches!(last(), Some(SessionMsg::SystemChat(m)) if m == "WAITING FOR FREQUENCY TO CLEAR"));
        assert!(matches!(run(GuiCommand::Abort, &mut g, true), Some(GuiCommand::Abort)));
        assert!(g.waiting.is_none(), "ABORT stops waiting");
        assert!(run(GuiCommand::Ping("NR5P".into()), &mut g, true).is_none());
        assert!(matches!(last(), Some(SessionMsg::TxRefused { title, .. }) if title == "Frequency is busy"));
        assert!(run(GuiCommand::Tune, &mut g, true).is_none());
        assert!(matches!(last(), Some(SessionMsg::TxHeld { text, .. }) if text.ends_with("Do you still wish to TUNE?")));
        assert!(matches!(run(GuiCommand::Confirmed(Box::new(GuiCommand::Tune)), &mut g, true), Some(GuiCommand::Tune)));
        let beacon = || GuiCommand::SendCq { source: "W5TST-9".into(), bandwidth: 500 };
        assert!(run(beacon(), &mut g, true).is_none());
        assert!(matches!(last(), Some(SessionMsg::TxHeld { text, .. }) if text.contains("one-time beacon")));

        // Ignore DCD: busy doesn't matter
        run(GuiCommand::SetIgnoreDcd(true), &mut g, false);
        assert!(run(GuiCommand::Connect("NR5P".into()), &mut g, true).is_some());
        run(GuiCommand::SetIgnoreDcd(false), &mut g, false);

        // PTT disabled: connect / CQ / tune / broadcasts refused; ping and beacon go on
        // (the radio just isn't keyed)
        run(GuiCommand::SetPttDisabled(true), &mut g, false);
        for cmd in [
            GuiCommand::Connect("NR5P".into()),
            GuiCommand::SendCq { source: "W5TST".into(), bandwidth: 500 },
            GuiCommand::Tune,
            GuiCommand::PathFinderRequest("K1ABC".into()),
        ] {
            assert!(run(cmd, &mut g, false).is_none());
            assert!(matches!(last(), Some(SessionMsg::TxRefused { title, .. }) if title == "PTT is disabled!"));
        }
        assert!(run(GuiCommand::Ping("NR5P".into()), &mut g, false).is_some());
        assert!(run(beacon(), &mut g, false).is_some());
        assert!(!g.may_key(false), "never key with PTT disabled");
        run(GuiCommand::SetPttDisabled(false), &mut g, false);

        // Away with unattended links off: keyed only for our own beacon
        g.beacon_until = None;
        assert!(g.may_key(false));
        run(GuiCommand::SetUnattended(false), &mut g, false);
        assert!(g.may_key(false), "unattended only counts while away");
        assert!(!g.may_key(true));
        run(beacon(), &mut g, false);
        assert!(g.may_key(true), "our beacon may key");
    }

    #[test]
    fn app_icon_decodes() {
        let icon = app_icon().expect("bundled icon is a valid PNG");
        assert_eq!((icon.width, icon.height), (256, 256));
    }

    /// Background delivery after the handshake: an Outbox VMail for the partner and a
    /// VMail parked here for it go out in VarAC's format and move to Sent.
    #[test]
    fn queued_vmail_is_delivered_by_the_background_task() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let cmd = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let data = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let (cp, dp) = (cmd.local_addr().unwrap().port(), data.local_addr().unwrap().port());
            let modem = tokio::spawn(async move {
                let (mut c, _) = cmd.accept().await.unwrap();
                let (mut d, _) = data.accept().await.unwrap();
                c.write_all(b"CONNECTED W5TST NR5P 500\r").await.unwrap();
                let mut frames = Vec::new();
                for _ in 0..2 {
                    let mut buf = vec![0u8; 1024];
                    let n = tokio::time::timeout(std::time::Duration::from_secs(3), d.read(&mut buf)).await.unwrap().unwrap();
                    frames.push(String::from_utf8_lossy(&buf[..n]).into_owned());
                    c.write_all(b"BUFFER 0\r").await.unwrap();
                }
                frames
            });
            let vara = VaraConnection::connect("127.0.0.1", cp, dp).await.unwrap();
            let cfg = StationConfig {
                callsign: "W5TST".into(), name: None, qth: None, locator: None,
                services: crate::varac::messages::ServiceFlags(1), bbs_dir: None, email_gate: None, bbs_gate: None, policy: Default::default(),
            };
            let mut sess = VaracSession::new(vara, cfg);
            let _ = tokio::time::timeout(std::time::Duration::from_secs(2), sess.poll()).await; // CONNECTED
            let db = Storage::open_in_memory().unwrap();
            let rec = |guid: &str, folder: crate::storage::VMailFolder, to: &str, from: &str, subj: &str| crate::storage::VMailRecord {
                guid: guid.into(), creation_time: Utc::now(), sent_time: None, received_time: None,
                folder_id: folder as i32, vmail_to: to.into(), vmail_from: from.into(), vmail_via: None,
                subject: subj.into(), msg: "body".into(), read_status: true, is_deleted: false, urgent: false,
                delivery_band: None, delivery_snr: None, frequency: None,
            };
            db.insert_vmail(&rec("out-1", crate::storage::VMailFolder::Outbox, "NR5P", "W5TST", "queued")).unwrap();
            db.insert_vmail(&rec("park-1", crate::storage::VMailFolder::Parking, "NR5P", "K5XYZ", "relayed")).unwrap();
            let db = std::sync::Mutex::new(db);
            let (tx, _rx) = mpsc::channel();
            // VarAC's outbox: one VMail, then (after the partner's answer) the next
            let mut skip = std::collections::HashSet::new();
            let mut awaiting = deliver_queued_vmail(&mut sess, &db, "NR5P", &tx, &mut skip).await;
            assert_eq!(awaiting.len(), 1, "one at a time");
            // (the bridge skips one already sent and waiting for its answer)
            skip.insert(awaiting[0].0.clone());
            let _ = tokio::time::timeout(std::time::Duration::from_millis(1500), async { loop { sess.poll().await; } }).await;
            awaiting.extend(deliver_queued_vmail(&mut sess, &db, "NR5P", &tx, &mut skip).await);
            let _ = tokio::time::timeout(std::time::Duration::from_secs(3), async { loop { sess.poll().await; } }).await;
            let frames = modem.await.unwrap();
            // (VarAC: no <TME> for fresh mail, no <TO> for the partner, no <FRM> for us)
            assert!(frames[0].ends_with("<SM><SBJ:queued><MSG:body>"), "{frames:?}");
            assert!(frames[1].contains("<FRM:K5XYZ><SBJ:relayed>"), "relayed mail keeps its sender: {frames:?}");
            // Like VarAC, nothing is filed as sent until the partner's <SMR>: both stay
            // queued, handed back in send order for the bridge to settle on each ack
            let d = db.lock().unwrap();
            assert_eq!(d.outbox_vmails_for("NR5P").unwrap().len(), 1);
            assert_eq!(d.vmails_in_folder(crate::storage::VMailFolder::Parking, 10).unwrap().len(), 1);
            let guids: Vec<&str> = awaiting.iter().map(|(g, _)| g.as_str()).collect();
            assert_eq!(guids, ["out-1", "park-1"]);
            assert!(awaiting.iter().all(|(_, notice)| notice.is_none()), "neither came in by email");
        });
    }

    #[test]
    fn received_file_names_are_flattened() {
        assert_eq!(safe_file_name("notes.txt"), "notes.txt");
        assert_eq!(safe_file_name("../../.bashrc"), "bashrc");
        assert_eq!(safe_file_name("C:\\Windows\\evil.dll"), "evil.dll");
        assert_eq!(safe_file_name("/etc/passwd"), "passwd");
        assert_eq!(safe_file_name(".."), "received_file");
        assert_eq!(safe_file_name(""), "received_file");
    }

    #[test]
    fn bbs_requests_cannot_escape_the_bbs_folder() {
        let base = std::env::temp_dir().join(format!("bbs_test_{}", std::process::id()));
        let dir = base.join("bbs");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("ok.txt"), b"hi").unwrap();
        std::fs::write(base.join("secret.txt"), b"no").unwrap();
        let d = dir.to_str().unwrap();
        assert!(crate::varac::bbs::resolve_request(d, "ok.txt").is_some());
        for bad in ["../secret.txt", "..", ".", "", "/etc/passwd", "sub/ok.txt", "..\\secret.txt", "C:secret.txt", "missing.txt"] {
            assert!(crate::varac::bbs::resolve_request(d, bad).is_none(), "{bad:?} must be rejected");
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(base.join("secret.txt"), dir.join("link.txt")).unwrap();
            assert!(crate::varac::bbs::resolve_request(d, "link.txt").is_none(), "symlink out of the folder must be rejected");
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn maidenhead_to_latlon_basic() {
        let (lat, lon) = maidenhead_to_latlon("FN42").unwrap();
        assert!((lat - 42.5).abs() < 1.0);
        assert!((lon - (-71.0)).abs() < 1.0);
    }

    #[test]
    fn grid_distance_same_square() {
        // (VarAC: 6-character locators only)
        assert!(grid_distance_str("FN42", "FN42").is_none());
        assert!(grid_distance_str("FN42AA", "FN42AA").unwrap().starts_with("0 "));
    }

    #[test]
    fn grid_distance_across_atlantic() {
        let (mi, km, brg) = messages::loc_distance("FN42MM", "JO01MM").unwrap();
        assert!(km > 4000 && km < 7000, "{km}");
        assert!(mi > 2500 && mi < 4400, "{mi}");
        assert!((40..=70).contains(&brg), "{brg}");
    }

    #[test]
    fn csv_line_parser_basic() {
        let fields = parse_csv_line("W1ABC,\"John\",\"Boston, MA\",FN42,\"\",5");
        assert_eq!(fields[0], "W1ABC");
        assert_eq!(fields[1], "John");
        assert_eq!(fields[2], "Boston, MA");
        assert_eq!(fields[3], "FN42");
        assert_eq!(fields[4], "");
        assert_eq!(fields[5], "5");
    }

    #[test]
    fn csv_line_parser_quoted_comma() {
        let fields = parse_csv_line("NR5P,\"Jane\",\"City, State\",EM12,\"notes here\",10");
        assert_eq!(fields.len(), 6);
        assert_eq!(fields[2], "City, State");
        assert_eq!(fields[4], "notes here");
    }

    #[test]
    fn parse_qsy_ranges_from_config() {
        let ranges = vec![
            "14.101.250-14.108.750".to_string(),
            "7.101.250-7.108.750".to_string(),
        ];
        let parsed = state::parse_qsy_ranges(&ranges);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0], (14_101_250, 14_108_750));
        assert_eq!(parsed[1], (7_101_250, 7_108_750));
    }

    #[test]
    fn snr_sparkline_renders() {
        let samples = vec![-10.0, -5.0, 0.0, 5.0, 10.0];
        let spark = snr_sparkline(&samples);
        assert_eq!(spark.chars().count(), 5);
        assert!(spark.starts_with('\u{2581}'));
        assert!(spark.ends_with('\u{2588}'));
    }

    #[test]
    fn snr_sparkline_flat() {
        let samples = vec![3.0, 3.0, 3.0];
        let spark = snr_sparkline(&samples);
        assert_eq!(spark.chars().count(), 3);
    }

    #[test]
    fn send_progress_inputs() {
        assert_eq!(parse_bitrate("BITRATE (4)  88 bps TX"), Some((4, 88, true)));
        assert_eq!(parse_bitrate("BITRATE (6)  270 bps RX"), Some((6, 270, false)));
        assert_eq!(parse_bitrate("BITRATE"), None);
        let mk = |t: &str, tx| ChatLine { text: t.into(), is_outgoing: true, tx, ..Default::default() };
        let q = Some(state::TxStage::Queued);
        let lines: std::collections::VecDeque<ChatLine> = [mk("hi", q), mk("old", None), mk("how are you", q)].into_iter().collect();
        let queued = |l: &ChatLine| l.tx == Some(state::TxStage::Queued);
        // three queued lines joined into one frame, and a reply with its <C:n> prefix
        assert_eq!(our_lines_in_frame(&lines, "hi\r\nhow are you", queued), vec![0, 2]);
        assert_eq!(our_lines_in_frame(&lines, "<C:7>how are you", queued), vec![2]);
        assert!(our_lines_in_frame(&lines, "old", queued).is_empty());
    }

    #[test]
    fn dxcc_us_callsigns() {
        assert_eq!(dxcc_entity("W1ABC"), Some("United States of America"));
        assert_eq!(dxcc_entity("K5XYZ"), Some("United States of America"));
        assert_eq!(dxcc_entity("N0CALL"), Some("United States of America"));
        assert_eq!(dxcc_entity("AA1BB"), Some("United States of America"));
    }

    #[test]
    fn dxcc_international() {
        assert_eq!(dxcc_entity("DL1ABC"), Some("Federal Republic of Germany"));
        assert_eq!(dxcc_entity("VK2XYZ"), Some("Australia"));
        assert_eq!(dxcc_entity("JA1ABC"), Some("Japan"));
        assert_eq!(dxcc_entity("G3XYZ"), Some("England"));
        assert_eq!(dxcc_entity("VE3ABC"), Some("Canada"));
        assert_eq!(dxcc_entity("PY2ABC"), Some("Brazil"));
        assert_eq!(dxcc_entity("LU1ABC"), Some("Argentina"));
    }

    #[test]
    fn dxcc_with_ssid() {
        assert_eq!(dxcc_entity("W1ABC-1"), Some("United States of America"));
        assert_eq!(dxcc_entity("DL5XYZ-3"), Some("Federal Republic of Germany"));
    }

    #[test]
    fn dxcc_unknown() {
        assert_eq!(dxcc_entity(""), None);
    }
}
