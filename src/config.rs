use clap::Parser;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "openslot", about = "Cross-platform VarAC-compatible client")]
pub struct Cli {
    /// Station callsign (required)
    #[arg(short, long, env = "OPENSLOT_CALLSIGN")]
    pub callsign: Option<String>,

    /// VARA modem host
    #[arg(long, default_value = "127.0.0.1", env = "OPENSLOT_HOST")]
    pub host: String,

    /// VARA modem command port
    #[arg(long, default_value_t = 8300, env = "OPENSLOT_CMD_PORT")]
    pub cmd_port: u16,

    /// VARA modem data port (default: cmd_port + 1)
    #[arg(long, env = "OPENSLOT_DATA_PORT")]
    pub data_port: Option<u16>,

    /// Bandwidth: 500, 2300, or 2750
    #[arg(short, long, default_value = "2300")]
    pub bandwidth: String,

    /// Config file path
    #[arg(long, env = "OPENSLOT_CONFIG")]
    pub config: Option<PathBuf>,

    /// Database file path
    #[arg(long, env = "OPENSLOT_DB")]
    pub db: Option<PathBuf>,

    /// Operator name
    #[arg(long)]
    pub name: Option<String>,

    /// Station QTH (location)
    #[arg(long)]
    pub qth: Option<String>,

    /// Maidenhead grid locator
    #[arg(long)]
    pub locator: Option<String>,

    /// Enable VMail service
    #[arg(long)]
    pub vmail: bool,

    /// Enable BBS service
    #[arg(long)]
    pub bbs: bool,

    /// Log level (trace, debug, info, warn, error)
    #[arg(long, default_value = "info")]
    pub log_level: String,

    /// Use terminal UI instead of desktop GUI
    #[arg(long)]
    pub tui: bool,
}

impl Cli {
    /// Parse the command line. The settings variables were called `VARAC_*` before; each old
    /// name still works when neither the option nor its `OPENSLOT_*` variable is given.
    pub fn parse_with_old_env() -> Self {
        use clap::CommandFactory;
        Self::with_old_env(&Self::command().get_matches(), |var| std::env::var(var).ok())
    }

    fn with_old_env(m: &clap::ArgMatches, env: impl Fn(&str) -> Option<String>) -> Self {
        use clap::FromArgMatches;
        use clap::parser::ValueSource;
        let mut cli = Self::from_arg_matches(m).unwrap_or_else(|e| e.exit());
        let old = |id: &str, var: &str| {
            let given = matches!(m.value_source(id), Some(ValueSource::CommandLine | ValueSource::EnvVariable));
            if given { None } else { env(var).filter(|v| !v.is_empty()) }
        };
        if let Some(v) = old("callsign", "VARAC_CALLSIGN") {
            cli.callsign = Some(v);
        }
        if let Some(v) = old("host", "VARAC_HOST") {
            cli.host = v;
        }
        if let Some(v) = old("cmd_port", "VARAC_CMD_PORT").and_then(|v| v.parse().ok()) {
            cli.cmd_port = v;
        }
        if let Some(v) = old("data_port", "VARAC_DATA_PORT").and_then(|v| v.parse().ok()) {
            cli.data_port = Some(v);
        }
        if let Some(v) = old("config", "VARAC_CONFIG") {
            cli.config = Some(PathBuf::from(v));
        }
        if let Some(v) = old("db", "VARAC_DB") {
            cli.db = Some(PathBuf::from(v));
        }
        cli
    }
}

/// Persistent config file (TOML).
/// A frequency the operator keeps in the frequency dropdown.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedFrequency {
    pub hz: u64,
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ConfigFile {
    pub callsign: Option<String>,
    pub name: Option<String>,
    pub qth: Option<String>,
    pub locator: Option<String>,
    /// Own email address, shared in INFO replies as `<EM:..>` (VarAC profile "Email").
    pub email: Option<String>,
    /// VarAC [MY_INFO]: MyRIG, MyPower (W), MyAntenna, MyAge, MyProfession, MyHobbies.
    pub my_rig: Option<String>,
    pub my_power: Option<String>,
    pub my_antenna: Option<String>,
    pub my_age: Option<String>,
    pub my_profession: Option<String>,
    pub my_hobbies: Option<String>,
    /// Complex callsign (W/4Z1AC/QRP) sent in the opening as `<FC:..>`.
    pub full_callsign: Option<String>,
    pub host: Option<String>,
    pub cmd_port: Option<u16>,
    pub data_port: Option<u16>,
    /// VARA KISS port (VarAC's VarahfMainKissPort; default command port − 200 = 8100)
    pub kiss_port: Option<u16>,
    pub bandwidth: Option<String>,
    pub vmail: Option<bool>,
    pub bbs: Option<bool>,
    pub db_path: Option<String>,
    pub beacon_interval: Option<u32>,
    pub auto_log_qso: Option<bool>,
    pub auto_away_enabled: Option<bool>,
    pub auto_away_minutes: Option<u32>,
    pub allow_pings: Option<bool>,
    pub allow_info_request: Option<bool>,
    pub load_history: Option<bool>,
    pub font_size: Option<f32>,
    /// Main window: the broadcasts' share of the space they split with the chat (drag bar)
    pub broadcasts_share: Option<f32>,
    /// Main window: the chat grows over the broadcasts and top cards while hovered (the
    /// chat's 📌 button); None = on
    pub chat_grow_on_hover: Option<bool>,
    pub notifications_enabled: Option<bool>,
    pub sound_alerts: Option<bool>,
    pub use_local_time: Option<bool>,
    pub modem_auto_reconnect: Option<bool>,
    pub frequency: Option<u64>,
    pub band: Option<String>,
    pub bbs_directory: Option<String>,
    pub watchlist: Option<Vec<String>>,
    pub callsign_tags: Option<std::collections::HashMap<String, String>>,
    pub auto_qsy_ranges: Option<Vec<String>>,
    /// The operator's own frequencies in the frequency dropdown
    pub my_frequencies: Option<Vec<SavedFrequency>>,
    pub rig_backend: Option<String>,
    pub rig_host: Option<String>,
    pub rig_port: Option<u16>,
    pub rig_serial_port: Option<String>,
    pub rig_baud: Option<u32>,
    pub rig_model: Option<String>,
    /// Key the radio via rig control when VARA asks for PTT.
    pub rig_ptt: Option<bool>,
    /// Act as a VarAC email gateway (forward email VMails over SMTP).
    pub email_gateway: Option<bool>,
    pub smtp_server: Option<String>,
    pub smtp_port: Option<u16>,
    /// "starttls" (default), "tls" or "none"
    pub smtp_security: Option<String>,
    pub smtp_username: Option<String>,
    /// Stored in plain text in this file (keep it readable only by you).
    pub smtp_password: Option<String>,
    /// The gateway's own mailbox (From address).
    pub gateway_email: Option<String>,
    /// Relay queued emails through a gateway we're connected to without asking
    /// (VarAC "Auto VMail to Email gateway relay"; off = ask, default no after 30 s).
    pub email_gateway_auto_relay: Option<bool>,
    /// Accept a partner's verbose-SNR invitation without asking.
    pub verbose_snr_auto_accept: Option<bool>,
    /// Answer the partner's last-heard peek (VarAC AllowLastHeardPeeking, ON)
    pub allow_last_heard: Option<bool>,
    /// VarAC's "Do not show this again" for the "VMail will be queued" and
    /// "Leave a message" popups ([VMAIL] DisableVmailWillBeQueuedPopup, [QSO] DisableLeaveVmailPopupAlert).
    pub disable_vmail_queued_popup: Option<bool>,
    pub disable_leave_vmail_popup: Option<bool>,
    /// VarAC [QSO_LOGGING]: the live ADIF file, its SUBMODE and COMMENT, logging programs
    /// (primary / secondary, Log4OM callsign lookup) and PSKReporter.
    pub adif_log_path: Option<String>,
    pub adif_submode: Option<String>,
    pub default_log_comment: Option<String>,
    pub logger_app: Option<String>,
    pub logger_ip: Option<String>,
    pub logger_port: Option<u16>,
    pub logger_app2: Option<String>,
    pub logger_ip2: Option<String>,
    pub logger_port2: Option<u16>,
    pub logger_callsign_lookup: Option<bool>,
    pub logger_api_port: Option<u16>,
    pub pskreporter_upload: Option<bool>,
    pub pskreporter_self_report: Option<bool>,
    /// VarAC [QSO] CallsignsBlockList (comma separated)
    pub callsigns_block_list: Option<String>,
    /// VarAC [BEACON] UnlimitedBeaconTime (else beacons stop after 72 hours)
    pub unlimited_beacon_time: Option<bool>,
    /// VarAC [HAMPLAY] HamPlayEnabled
    pub hamplay_enabled: Option<bool>,
    /// VarAC [AI]: our AI gateway (Google Gemini)
    pub ai_enabled: Option<bool>,
    pub ai_api_key: Option<String>,
    pub ai_model: Option<String>,
    pub ai_timeout_secs: Option<u64>,
    pub ai_limit_chars: Option<u32>,
    pub ai_chat_context: Option<bool>,
    pub ai_limit_per_qso: Option<u32>,
    pub ai_announce: Option<bool>,
    pub ai_customization: Option<String>,
    /// VarAC [QSO_LOGGING] DxCluster*: spot QSOs to a telnet DX cluster
    pub dxcluster_enabled: Option<bool>,
    pub dxcluster_host: Option<String>,
    pub dxcluster_port: Option<u16>,
    pub dxcluster_username: Option<String>,
    pub dxcluster_password: Option<String>,
    /// VarAC [RIG_CONTROL] FrequencyScheduleStartMode / LastState, scanner busy hold / announce
    pub schedule_start_mode: Option<String>,
    pub schedule_last_state: Option<String>,
    pub scanner_busy_hold_secs: Option<u32>,
    pub scanner_announce: Option<bool>,
    /// VarAC [VARAHF_CONFIG] VarahfMainPath / VarahfLaunchOnModemConnect / VarahfLaunchDelaySeconds
    pub vara_path: Option<String>,
    pub vara_launch: Option<bool>,
    pub vara_launch_delay_secs: Option<u32>,
    /// VarAC [QSL_CARD] QSLCardImageFile / QSLCardDefaultMessage / QSLCardFadeToBlackAtBottom
    pub qsl_image_file: Option<String>,
    pub qsl_message: Option<String>,
    pub qsl_fade: Option<bool>,
    /// VarAC [FILE_TRANSFER] IncomingFilesSizeLimit (bytes, 5000)
    pub incoming_files_size_limit: Option<u64>,
    /// VarAC [VARAHF_CONFIG] VaraModemType: VaraHF / VaraFM / VaraSAT
    pub vara_modem_type: Option<String>,
    /// VarAC [RIG_CONTROL] UsbMode: the radio mode set before every frequency change
    /// (USB, USB-D, DATA-U, FM, FM-D; default USB-D)
    pub usb_mode: Option<String>,
    /// VarAC [QSO] DigipeatVia (VARA FM): digipeaters for CQs, beacons and broadcasts
    pub digipeat_via: Option<String>,
    /// VarAC [ALE]
    pub ale_enabled: Option<bool>,
    pub ale_skip_approval: Option<bool>,
    pub ale_recent_minutes: Option<u32>,
    pub ale_history_days: Option<u32>,
    pub ale_snr_threshold: Option<i32>,
    /// VarAC's manual GPS data (answered to <GPSR> as <GPS:…>; empty = "NO GPS DATA")
    pub gps_manual: Option<String>,
    /// Distance unit: MI (default) or KM, as in VarAC
    pub distance_unit: Option<String>,
    /// VarAC [FILE_TRANSFER] IncomingFilesDir (empty: Downloads)
    pub incoming_files_dir: Option<String>,
    /// VarAC [VMAIL] SendRelayNotifications / AllowParking (both ON by default)
    pub send_relay_notifications: Option<bool>,
    pub allow_parking: Option<bool>,
    /// VarAC [APPEARANCE] UITheme: DEFAULT or SCIFI
    pub ui_theme: Option<String>,
    /// VarAC MuteAllSounds and the Play…Sound switches (keys of the sounds switched off)
    pub mute_all_sounds: Option<bool>,
    pub sounds_off: Option<Vec<String>>,
    /// VarAC's narrator: on, NarratePersona (female), and its Narrate… switches
    pub narrator: Option<bool>,
    pub narrator_female: Option<bool>,
    pub narrate_incoming: Option<bool>,
    pub narrate_alerts: Option<bool>,
    pub narrate_beacons: Option<bool>,
    pub narrate_my_beacons: Option<bool>,
    pub narrate_cqs: Option<bool>,
    pub narrate_broadcasts: Option<bool>,
    pub narrate_my_broadcasts: Option<bool>,
    /// VarAC [QSO] DisablePTT / IgnoreDCD / UnattendedLinks (the main window's checkboxes).
    pub ptt_disabled: Option<bool>,
    pub ignore_dcd: Option<bool>,
    pub unattended_links: Option<bool>,
    /// VarAC's EmComm operation mode and its [EMCOMM] switches.
    pub emcomm_mode: Option<bool>,
    pub emcomm_disable_snr: Option<bool>,
    pub emcomm_disable_call_id: Option<bool>,
    pub emcomm_allow_last_connections: Option<bool>,
    pub emcomm_auto_file_approval: Option<bool>,
    /// VarAC [EMCOMM] QSYNotifyViaBroadcast: a scheduled QSY is announced first (<QF:hz>)
    pub emcomm_qsy_notify: Option<bool>,
    pub emcomm_auto_reply_checkin: Option<bool>,
    /// Answer Path Finder requests (VarAC [VMAIL] AllowPathFinder, default on).
    pub allow_path_finder: Option<bool>,
    /// Antenna rotator through PstRotator's UDP port (VarAC [ROTATOR] PstRotatorHost/Port,
    /// localhost:12000): "Rotate Antenna" in the beacon and CQ menus
    pub rotator_enabled: Option<bool>,
    pub rotator_host: Option<String>,
    pub rotator_port: Option<u16>,
    /// Station ID during a QSO every N minutes (VarAC MycallBroadcastIntervalMinutes, 10; 5–30)
    pub call_id_minutes: Option<u32>,
    /// Disconnect an idle link after N minutes, telling the partner <IE> (VarAC
    /// IdleTimeDisconnectMinutes, 5; 0 = never)
    pub idle_disconnect_minutes: Option<u32>,
    /// Welcome message for stations that connect (VarAC CannedMsgWelcome; "" = none)
    pub welcome_message: Option<String>,
    /// Send the welcome even while away (VarAC SendAlwaysWelcomeMsg)
    pub welcome_always: Option<bool>,
    /// Report stations heard on every band, not just the current one (VarAC, OFF)
    pub multi_band_last_heard: Option<bool>,
    // Remembered UI state (main window and Appearance)
    pub dark_mode: Option<bool>,
    pub qso_alert_minutes: Option<u32>,
    pub dx_alert_enabled: Option<bool>,
    pub connect_timeout_secs: Option<u32>,
    pub scanner_dwell_secs: Option<u32>,
    pub auto_cq_interval_secs: Option<u32>,
    pub auto_qsy_enabled: Option<bool>,
    pub beacon_auto_enabled: Option<bool>,
    /// VarAC [VARAHF_CONFIG] VaraKeepAliveLimitSeconds (300): no IAMALIVE from VARA for
    /// this long restarts the modem connection
    pub vara_keepalive_secs: Option<u64>,
    /// VarAC [BEACON] StartBeaconUponStartup (OFF): the beacon starts with the program
    pub start_beacon_on_startup: Option<bool>,
    /// VarAC [QSO] SendIsTyping (ON)
    pub send_is_typing: Option<bool>,
    /// VarAC [QSO] SuppressIsTypingInLowSNR (ON): no "is typing" below -14 dB
    pub suppress_typing_low_snr: Option<bool>,
    /// Preferred CQ slot (VarAC remembers it)
    pub cq_slot: Option<u8>,
    /// Only these stations may use our BBS (VarAC [BBS] LimitAccessToCallsigns).
    pub bbs_limit: Option<bool>,
    /// Comma-separated callsigns (base or full).
    pub bbs_allow: Option<String>,
    /// Announce the BBS in advanced beacons (open BBS only, like VarAC).
    pub bbs_announce: Option<bool>,
    /// Only these stations may send email through us (VarAC "Limit access to").
    pub email_gateway_limit: Option<bool>,
    /// Comma-separated full callsigns, e.g. "W5TST,K5XYZ-1".
    pub email_gateway_allow: Option<String>,
    /// Failed sends before the gateway shuts itself down (VarAC SMTPRetriesLimit, 5).
    pub smtp_retries: Option<u32>,
    /// Announce the gateway in advanced beacons (needs no allow list, like VarAC).
    pub email_gateway_announce: Option<bool>,
    /// "standard" (CQFRAME CALL-9) or "advanced" (KISS beacon with services).
    pub beacon_type: Option<String>,
    /// Slot system (VarAC [QSO] CqSlotWaitTimeSeconds / SkipCqSlotSelector / SlotStepHz)
    pub cq_slot_wait_secs: Option<u64>,
    pub skip_cq_slot_selector: Option<bool>,
    pub slot_step_hz: Option<i64>,
    /// Pull emails for stations from the gateway mailbox (VarAC "Enable incoming Emails").
    pub imap_enabled: Option<bool>,
    pub imap_server: Option<String>,
    pub imap_port: Option<u16>,
    /// "tls" (default, port 993), "starttls" or "none"
    pub imap_security: Option<String>,
    /// Empty = same as the SMTP login.
    pub imap_username: Option<String>,
    pub imap_password: Option<String>,
    /// Seconds between mailbox checks (VarAC default 300).
    pub imap_poll_seconds: Option<u64>,
}

/// Resolved configuration with all values determined.
pub struct AppConfig {
    pub callsign: String,
    pub name: Option<String>,
    pub qth: Option<String>,
    pub locator: Option<String>,
    pub host: String,
    pub cmd_port: u16,
    pub data_port: u16,
    pub kiss_port: u16,
    pub bandwidth: crate::vara::commands::Bandwidth,
    pub vmail: bool,
    pub bbs: bool,
    /// Folder served by our BBS (from the config file).
    pub bbs_directory: Option<std::path::PathBuf>,
    /// Email gateway enabled and configured.
    pub email_gateway: bool,
    /// Allow list, when limited.
    pub email_gateway_allow: Option<Vec<String>>,
    /// The gateway's address (answer to <EGA>).
    pub gateway_email: String,
    /// BBS allow list, when limited.
    pub bbs_allow: Option<Vec<String>>,
    pub db_path: PathBuf,
}

impl AppConfig {
    /// Merge CLI args over config file, with CLI taking precedence.
    pub fn resolve(mut cli: Cli) -> Result<Self, String> {
        // VarAC's "Load the last used profile on startup": a pointer next to the default config
        if cli.config.is_none() {
            if let Some(ptr) = default_profile_pointer() {
                match std::fs::read_to_string(&ptr).ok().map(|s| PathBuf::from(s.trim())) {
                    Some(p) if p.is_file() => cli.config = Some(p),
                    Some(p) => {
                        eprintln!("Cant find last used profile: {}. Reverting to the default file", p.display());
                        let _ = std::fs::remove_file(&ptr);
                    }
                    None => {}
                }
            }
        }
        if let Some(p) = cli.config.clone() {
            let _ = CONFIG_PATH.set(p);
        }
        let file_cfg = cli.config.as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| toml::from_str::<ConfigFile>(&s).ok())
            .or_else(|| {
                default_config_path()
                    .and_then(|p| std::fs::read_to_string(p).ok())
                    .and_then(|s| toml::from_str::<ConfigFile>(&s).ok())
            })
            .unwrap_or_default();
        let email_gateway = gateway_from(&file_cfg).is_some();
        let email_gateway_allow = allow_list(&file_cfg);
        let gateway_email = file_cfg.gateway_email.clone().unwrap_or_default();
        let bbs_allow = bbs_allow_list(&file_cfg);

        let callsign = cli.callsign
            .or(file_cfg.callsign)
            .ok_or("Callsign is required: use --callsign or set in config file")?;

        let host = if cli.host != "127.0.0.1" { cli.host } else {
            file_cfg.host.unwrap_or_else(|| "127.0.0.1".into())
        };

        let cmd_port = if cli.cmd_port != 8300 { cli.cmd_port } else {
            file_cfg.cmd_port.unwrap_or(8300)
        };

        let data_port = cli.data_port
            .or(file_cfg.data_port)
            .unwrap_or(cmd_port + 1);
        let kiss_port = file_cfg.kiss_port.unwrap_or(cmd_port.saturating_sub(200));

        let bw_str = if cli.bandwidth != "2300" { &cli.bandwidth } else {
            file_cfg.bandwidth.as_deref().unwrap_or("2300")
        };
        let bandwidth = match bw_str {
            "500" => crate::vara::commands::Bandwidth::Bw500,
            "2300" => crate::vara::commands::Bandwidth::Bw2300,
            "2750" => crate::vara::commands::Bandwidth::Bw2750,
            other => return Err(format!("Invalid bandwidth: {other} (use 500, 2300, or 2750)")),
        };

        let db_path = cli.db
            .or_else(|| file_cfg.db_path.map(PathBuf::from))
            .unwrap_or_else(|| default_db_path().unwrap_or_else(|| PathBuf::from("varac.db")));

        Ok(AppConfig {
            callsign,
            name: cli.name.or(file_cfg.name),
            qth: cli.qth.or(file_cfg.qth),
            locator: cli.locator.or(file_cfg.locator),
            host,
            cmd_port,
            data_port,
            kiss_port,
            bandwidth,
            vmail: cli.vmail || file_cfg.vmail.unwrap_or(false),
            bbs: cli.bbs || file_cfg.bbs.unwrap_or(false),
            bbs_directory: file_cfg.bbs_directory.clone().filter(|d| !d.trim().is_empty()).map(std::path::PathBuf::from),
            email_gateway,
            email_gateway_allow,
            gateway_email,
            bbs_allow,
            db_path,
        })
    }
}

static CONFIG_PATH: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// The config file in use (--config, else the default location).
pub fn active_config_path() -> Option<PathBuf> {
    CONFIG_PATH.get().cloned().or_else(default_config_path)
}

fn bbs_allow_list(f: &ConfigFile) -> Option<Vec<String>> {
    f.bbs_limit
        .unwrap_or(false)
        .then(|| crate::varac::email_gateway::parse_allow_list(f.bbs_allow.as_deref().unwrap_or("")))
}

/// (BBS allow list, announce the BBS) as in the config file now.
pub fn bbs_policy() -> (Option<Vec<String>>, bool) {
    match read_config_file() {
        Some(f) => (bbs_allow_list(&f), f.bbs_announce.unwrap_or(false)),
        None => (None, false),
    }
}

/// (answer info requests, allow pings, auto-accept verbose SNR) as in the config file now.
pub fn station_policy() -> (bool, bool, bool) {
    match read_config_file() {
        Some(f) => (
            f.allow_info_request.unwrap_or(true),
            f.allow_pings.unwrap_or(true),
            f.verbose_snr_auto_accept.unwrap_or(false),
        ),
        None => (true, true, false),
    }
}

/// (Call ID minutes, idle-disconnect minutes) as VarAC clamps them: Call ID 5–30
/// (default 10), idle 0 = off (default 5).
pub fn qso_timers() -> (u64, u64) {
    let f = read_config_file().unwrap_or_default();
    let id = f.call_id_minutes.unwrap_or(10);
    let id = if (5..=30).contains(&id) { id } else { 5 };
    (id as u64, f.idle_disconnect_minutes.unwrap_or(5) as u64)
}

/// Own email for INFO replies (empty = not shared).
pub fn profile_email() -> String {
    read_config_file().and_then(|f| f.email).unwrap_or_default()
}

/// The extra profile fields (rig … hobbies) for INFO replies and message tags.
pub fn profile_extra() -> crate::varac::messages::Profile {
    let f = read_config_file().unwrap_or_default();
    crate::varac::messages::Profile {
        rig: f.my_rig.unwrap_or_default(),
        power: f.my_power.unwrap_or_default(),
        antenna: f.my_antenna.unwrap_or_default(),
        age: f.my_age.unwrap_or_default(),
        profession: f.my_profession.unwrap_or_default(),
        hobbies: f.my_hobbies.unwrap_or_default(),
        ..Default::default()
    }
}

/// Complex callsign for the opening (empty = none).
pub fn full_callsign() -> String {
    read_config_file().and_then(|f| f.full_callsign).unwrap_or_default()
}

/// Welcome message and its "Send always" flag (empty = none).
pub fn welcome() -> (String, bool) {
    let f = read_config_file().unwrap_or_default();
    (f.welcome_message.unwrap_or_default(), f.welcome_always.unwrap_or(false))
}

/// (allow last-heard peeking, report all bands) as in the config file now.
/// VarAC's EmComm mode as set now (the switches count only while it's on).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct EmComm {
    pub on: bool,
    /// No `<R+NN>` in our opening or Call ID (EmComm DisableSNRSending)
    pub no_snr: bool,
    /// No periodic Call ID (DisableCallIDSending)
    pub no_call_id: bool,
    /// Answer `<LCR>` with our last 5 QSOs (AllowLastConnectionsPeeking)
    pub last_connections: bool,
    /// Accept incoming files without asking (AutomaticIncomingFileApproval)
    pub auto_file: bool,
    /// Answer an `<EC>` check-in request with a beacon (AutoReplyCheckinRequest, default on)
    pub auto_checkin: bool,
}

pub fn emcomm() -> EmComm {
    let f = read_config_file().unwrap_or_default();
    let on = f.emcomm_mode.unwrap_or(false);
    EmComm {
        on,
        no_snr: on && f.emcomm_disable_snr.unwrap_or(false),
        no_call_id: on && f.emcomm_disable_call_id.unwrap_or(false),
        last_connections: on && f.emcomm_allow_last_connections.unwrap_or(false),
        auto_file: on && f.emcomm_auto_file_approval.unwrap_or(false),
        auto_checkin: on && f.emcomm_auto_reply_checkin.unwrap_or(true),
    }
}

/// (VMail "will be queued" popup off, "Leave a message" popup off) — VarAC's "Do not show
/// this again".
pub fn vmail_popups_disabled() -> (bool, bool) {
    let f = read_config_file().unwrap_or_default();
    (f.disable_vmail_queued_popup.unwrap_or(false), f.disable_leave_vmail_popup.unwrap_or(false))
}

/// Logging settings as in the config file now.
#[derive(Debug, Clone)]
pub struct Logging {
    pub adif_path: std::path::PathBuf,
    pub submode: String,
    pub comment: String,
    /// (logger, ip, port), primary then secondary; "NONE" = off
    pub loggers: Vec<(String, String, u16)>,
    pub callsign_lookup: bool,
    pub api_port: u16,
    pub pskreporter: bool,
    pub self_report: bool,
}

/// Where the live ADIF log goes by default (VarAC: VarAC_qso_log.adi next to the program).
pub fn default_adif_path() -> std::path::PathBuf {
    project_dirs()
        .map(|d| d.data_dir().join("Openslot_qso_log.adi"))
        .unwrap_or_else(|| std::path::PathBuf::from("Openslot_qso_log.adi"))
}

pub fn logging() -> Logging {
    let f = read_config_file().unwrap_or_default();
    let mut loggers = Vec::new();
    for (app, ip, port) in [
        (f.logger_app.clone(), f.logger_ip.clone(), f.logger_port),
        (f.logger_app2.clone(), f.logger_ip2.clone(), f.logger_port2),
    ] {
        let app = app.unwrap_or_else(|| "NONE".into());
        if app != "NONE" && !app.is_empty() {
            let default_port = crate::varac::logbook::LOGGERS.iter().find(|(n, _)| *n == app).map_or(0, |l| l.1);
            loggers.push((app, ip.unwrap_or_else(|| "127.0.0.1".into()), port.unwrap_or(default_port)));
        }
    }
    Logging {
        adif_path: f.adif_log_path.filter(|p| !p.trim().is_empty()).map(std::path::PathBuf::from).unwrap_or_else(default_adif_path),
        // VarAC's SUBMODE follows the modem: VARA HF / VARA FM 1200 / VARA SATELLITE
        submode: f.adif_submode.filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| crate::vara::commands::ModemType::from_key(f.vara_modem_type.as_deref().unwrap_or("")).submode().into()),
        comment: f.default_log_comment.unwrap_or_else(|| "Made with Openslot".into()),
        loggers,
        callsign_lookup: f.logger_callsign_lookup.unwrap_or(false),
        api_port: f.logger_api_port.unwrap_or(2241),
        // (off until you've seen your spots arrive on pskreporter.info)
        pskreporter: f.pskreporter_upload.unwrap_or(false),
        self_report: f.pskreporter_self_report.unwrap_or(true),
    }
}

/// Answer other stations' Path Finder requests.
/// VARA launched with the modem connection (VarAC VarahfLaunchOnModemConnect), its exe
/// and the delay before connecting.
pub fn vara_launch() -> Option<(String, u64)> {
    let f = read_config_file()?;
    (f.vara_launch.unwrap_or(false) && f.vara_path.as_deref().is_some_and(|p| !p.trim().is_empty()))
        .then(|| (f.vara_path.unwrap_or_default(), f.vara_launch_delay_secs.unwrap_or(5) as u64))
}

/// Start VARA if configured and nothing answers yet, then wait VarAC's launch delay.
pub async fn launch_vara_if_set(host: &str, cmd_port: u16, kiss_port: u16) -> Option<String> {
    let (exe, delay) = vara_launch()?;
    let (h, e) = (host.to_string(), exe.clone());
    let r = tokio::task::spawn_blocking(move || crate::vara::launch::launch(&e, &h, cmd_port, kiss_port)).await.ok().flatten()?;
    if r.is_ok() {
        tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
    }
    Some(r.unwrap_or_else(|e| e))
}

/// The config file as it is now (defaults when missing).
pub fn read_config_file_pub() -> ConfigFile {
    read_config_file().unwrap_or_default()
}

/// (mute all, keys of the sounds switched off)
pub fn sound_settings() -> (bool, Vec<String>) {
    let f = read_config_file().unwrap_or_default();
    // "Sound alerts" off in Settings silences them too
    (f.mute_all_sounds.unwrap_or(false) || f.sound_alerts == Some(false), f.sounds_off.unwrap_or_default())
}

/// The narrator's switches, with VarAC's defaults.
#[derive(Debug, Clone, Copy)]
pub struct Narrator {
    pub on: bool,
    pub female: bool,
    pub incoming: bool,
    pub alerts: bool,
    pub beacons: bool,
    pub my_beacons: bool,
    pub cqs: bool,
    pub broadcasts: bool,
    pub my_broadcasts: bool,
}

pub fn narrator() -> Narrator {
    let f = read_config_file().unwrap_or_default();
    Narrator {
        on: f.narrator.unwrap_or(false),
        female: f.narrator_female.unwrap_or(false),
        incoming: f.narrate_incoming.unwrap_or(false),
        alerts: f.narrate_alerts.unwrap_or(true),
        beacons: f.narrate_beacons.unwrap_or(false),
        my_beacons: f.narrate_my_beacons.unwrap_or(true),
        cqs: f.narrate_cqs.unwrap_or(true),
        broadcasts: f.narrate_broadcasts.unwrap_or(true),
        my_broadcasts: f.narrate_my_broadcasts.unwrap_or(false),
    }
}

/// QSL card: (background image, message template, fade to black).
pub fn read_qsl() -> (String, String, bool) {
    let f = read_config_file().unwrap_or_default();
    (
        f.qsl_image_file.unwrap_or_default(),
        f.qsl_message.unwrap_or_else(|| "TKS for QSO <HNAME>, 73, <NAME>.".into()),
        f.qsl_fade.unwrap_or(false),
    )
}

/// The answer to a GPS request: `<GPS:manual data>`, or "NO GPS DATA".
pub fn gps_answer() -> String {
    match read_config_file().and_then(|f| f.gps_manual).map(|g| g.trim().to_string()).filter(|g| !g.is_empty()) {
        Some(g) => format!("<GPS:{g}>"),
        None => "NO GPS DATA".into(),
    }
}

pub fn modem_type() -> crate::vara::commands::ModemType {
    crate::vara::commands::ModemType::from_key(&read_config_file().and_then(|f| f.vara_modem_type).unwrap_or_default())
}

/// VARA FM digipeaters for CQs and beacons (empty unless VARA FM)
pub fn digipeat_via() -> Option<String> {
    (modem_type() == crate::vara::commands::ModemType::Fm)
        .then(|| read_config_file().and_then(|f| f.digipeat_via).unwrap_or_default().trim().to_uppercase())
        .filter(|v| !v.is_empty())
}

/// Distances in km? (read every few seconds, not on every list row)
pub fn distance_in_km_cached() -> bool {
    static CACHE: std::sync::Mutex<Option<(std::time::Instant, bool)>> = std::sync::Mutex::new(None);
    let mut c = match CACHE.lock() { Ok(c) => c, Err(_) => return false };
    if let Some((t, v)) = *c {
        if t.elapsed().as_secs() < 5 {
            return v;
        }
    }
    let v = read_config_file().and_then(|f| f.distance_unit).is_some_and(|u| u.eq_ignore_ascii_case("KM"));
    *c = Some((std::time::Instant::now(), v));
    v
}

pub fn send_relay_notifications() -> bool {
    read_config_file().and_then(|f| f.send_relay_notifications).unwrap_or(true)
}

/// DX cluster spotting on (VarAC [QSO_LOGGING] DxClusterEnabled).
pub fn read_dxcluster_enabled() -> bool {
    read_config_file().and_then(|f| f.dxcluster_enabled).unwrap_or(false)
}

/// Blocked callsigns (VarAC [QSO] CallsignsBlockList), upper case.
pub fn block_list() -> Vec<String> {
    parse_block_list(&read_config_file().and_then(|f| f.callsigns_block_list).unwrap_or_default())
}

pub fn parse_block_list(s: &str) -> Vec<String> {
    s.split(',').map(|c| c.trim().to_uppercase()).filter(|c| !c.is_empty()).collect()
}

/// AI gateway settings (VarAC [AI]).
pub fn ai_settings() -> crate::varac::ai_gateway::AiSettings {
    let f = read_config_file().unwrap_or_default();
    crate::varac::ai_gateway::AiSettings {
        enabled: f.ai_enabled.unwrap_or(false),
        api_key: f.ai_api_key.unwrap_or_default(),
        model: f.ai_model.unwrap_or_else(|| crate::varac::ai_gateway::DEFAULT_MODEL.into()),
        timeout_secs: f.ai_timeout_secs.unwrap_or(30),
        limit_chars: f.ai_limit_chars.unwrap_or(200),
        chat_context: f.ai_chat_context.unwrap_or(true),
        limit_per_qso: f.ai_limit_per_qso.unwrap_or(0),
        announce: f.ai_announce.unwrap_or(false),
        customization: f.ai_customization.unwrap_or_default(),
    }
}

/// Folder of the config file (alert tags, schedule and scanner lists live next to it).
pub fn config_dir() -> Option<PathBuf> {
    active_config_path().and_then(|p| p.parent().map(|d| d.to_path_buf()))
}

/// The PstRotator to turn (host, UDP port), when an antenna rotator is set up.
pub fn rotator() -> Option<(String, u16)> {
    let f = read_config_file()?;
    f.rotator_enabled.unwrap_or(false).then(|| {
        (
            f.rotator_host.filter(|h| !h.trim().is_empty()).unwrap_or_else(|| "127.0.0.1".into()),
            f.rotator_port.unwrap_or(12000),
        )
    })
}

pub fn path_finder_enabled() -> bool {
    read_config_file().and_then(|f| f.allow_path_finder).unwrap_or(true)
}

pub fn last_heard_policy() -> (bool, bool) {
    match read_config_file() {
        Some(f) => (f.allow_last_heard.unwrap_or(true), f.multi_band_last_heard.unwrap_or(false)),
        None => (true, false),
    }
}

/// Relay queued emails through a connected gateway without asking.
pub fn email_auto_relay() -> bool {
    read_config_file().and_then(|f| f.email_gateway_auto_relay).unwrap_or(false)
}

fn allow_list(f: &ConfigFile) -> Option<Vec<String>> {
    f.email_gateway_limit
        .unwrap_or(false)
        .then(|| crate::varac::email_gateway::parse_allow_list(f.email_gateway_allow.as_deref().unwrap_or("")))
}

/// (advanced beacons, announce the email gateway in them) as in the config file now.
pub fn beacon_settings() -> (bool, bool) {
    match read_config_file() {
        Some(f) => (
            f.beacon_type.as_deref().is_some_and(|t| t.eq_ignore_ascii_case("advanced")),
            f.email_gateway_announce.unwrap_or(false),
        ),
        None => (false, false),
    }
}

/// Gateway policy settings as in the config file now: (enabled, allow list, retry limit).
pub fn gateway_policy() -> (bool, Option<Vec<String>>, u32) {
    match read_config_file() {
        Some(f) => (gateway_from(&f).is_some(), allow_list(&f), f.smtp_retries.unwrap_or(5)),
        None => (false, None, 5),
    }
}

/// SMTP set up well enough to run the email gateway (whether it is on or not).
pub fn gateway_configured() -> bool {
    read_config_file().is_some_and(|mut f| {
        f.email_gateway = Some(true);
        gateway_from(&f).is_some()
    })
}

fn gateway_from(f: &ConfigFile) -> Option<crate::varac::email_gateway::GatewayConfig> {
    if !f.email_gateway.unwrap_or(false) {
        return None;
    }
    let cfg = crate::varac::email_gateway::GatewayConfig {
        server: f.smtp_server.clone().unwrap_or_default(),
        port: f.smtp_port.unwrap_or(587),
        security: crate::varac::email_gateway::SmtpSecurity::from_key(f.smtp_security.as_deref().unwrap_or("starttls")),
        username: f.smtp_username.clone().unwrap_or_default(),
        password: f.smtp_password.clone().unwrap_or_default(),
        from_address: f.gateway_email.clone().unwrap_or_default(),
    };
    cfg.is_usable().then_some(cfg)
}

/// Change settings in the config file right away (VarAC writes its main-window
/// checkboxes to the INI as they're ticked).
pub fn update_config_file(change: impl FnOnce(&mut ConfigFile)) {
    let Some(path) = active_config_path() else { return };
    let mut cfg = read_config_file().unwrap_or_default();
    change(&mut cfg);
    if let Ok(text) = toml::to_string_pretty(&cfg) {
        let _ = std::fs::write(path, text);
    }
}

/// VarAC VaraKeepAliveLimitSeconds (300).
pub fn vara_keepalive_secs() -> u64 {
    read_config_file().and_then(|f| f.vara_keepalive_secs).unwrap_or(300)
}

fn read_config_file() -> Option<ConfigFile> {
    let text = std::fs::read_to_string(active_config_path()?).ok()?;
    toml::from_str::<ConfigFile>(&text).ok()
}

/// Incoming-email settings (and poll interval), re-read from the config file each poll.
/// None = gateway or incoming emails off, or not set up.
pub fn imap_config() -> Option<(crate::varac::email_inbound::ImapConfig, u64)> {
    let f = read_config_file()?;
    if !f.email_gateway.unwrap_or(false) || !f.imap_enabled.unwrap_or(false) {
        return None;
    }
    let or_smtp = |v: &Option<String>, smtp: &Option<String>| {
        v.clone().filter(|s| !s.trim().is_empty()).or_else(|| smtp.clone()).unwrap_or_default()
    };
    let cfg = crate::varac::email_inbound::ImapConfig {
        server: f.imap_server.clone().unwrap_or_default(),
        port: f.imap_port.unwrap_or(993),
        security: crate::varac::email_gateway::SmtpSecurity::from_key(f.imap_security.as_deref().unwrap_or("tls")),
        username: or_smtp(&f.imap_username, &f.smtp_username),
        password: or_smtp(&f.imap_password, &f.smtp_password),
    };
    cfg.is_usable().then_some((cfg, f.imap_poll_seconds.unwrap_or(300).max(30)))
}

/// Current email gateway settings, re-read from the config file so changes made in
/// Settings apply to the next email without a restart. None = gateway off/unconfigured.
pub fn gateway_config() -> Option<crate::varac::email_gateway::GatewayConfig> {
    gateway_from(&read_config_file()?)
}

/// The app's config and data folders. The app was called varac-rust before: the first call
/// moves those folders over, so settings, the QSO database and received files carry across.
pub fn project_dirs() -> Option<directories::ProjectDirs> {
    static MIGRATE: std::sync::Once = std::sync::Once::new();
    let dirs = directories::ProjectDirs::from("", "", "openslot")?;
    MIGRATE.call_once(|| {
        let Some(old) = directories::ProjectDirs::from("", "", "varac-rust") else { return };
        // On macOS config and data are the same folder: the second pair is then a no-op
        for (from, to) in [(old.config_dir(), dirs.config_dir()), (old.data_dir(), dirs.data_dir())] {
            if from.exists() && !to.exists() {
                if let Some(parent) = to.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if let Err(e) = std::fs::rename(from, to) {
                    eprintln!("Could not move {} to {}: {e}", from.display(), to.display());
                }
            }
        }
    });
    Some(dirs)
}

/// "Load the last used profile on startup": the file naming that profile.
pub fn default_profile_pointer() -> Option<PathBuf> {
    default_config_path().map(|p| p.with_extension("toml.default")).filter(|p| p.is_file())
}

/// Profiles: every config file (*.toml) in the config file's folder, as (path, created).
pub fn profiles() -> Vec<(PathBuf, Option<std::time::SystemTime>)> {
    let Some(dir) = config_dir() else { return Vec::new() };
    let mut out: Vec<(PathBuf, Option<std::time::SystemTime>)> = std::fs::read_dir(&dir).into_iter().flatten().flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "toml") && p.is_file())
        .filter(|p| std::fs::read_to_string(p).ok().is_some_and(|t| toml::from_str::<ConfigFile>(&t).is_ok()))
        .map(|p| {
            let t = std::fs::metadata(&p).ok().and_then(|m| m.created().or_else(|_| m.modified()).ok());
            (p, t)
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Remember (or forget) the profile to load at the next start.
pub fn set_default_profile(path: Option<&std::path::Path>) {
    let Some(ptr) = default_config_path().map(|p| p.with_extension("toml.default")) else { return };
    match path {
        Some(p) => {
            let _ = std::fs::create_dir_all(ptr.parent().unwrap_or(std::path::Path::new(".")));
            let _ = std::fs::write(ptr, p.display().to_string());
        }
        None => {
            let _ = std::fs::remove_file(ptr);
        }
    }
}

pub fn default_config_path() -> Option<PathBuf> {
    project_dirs()
        .map(|dirs| dirs.config_dir().join("config.toml"))
}

fn default_db_path() -> Option<PathBuf> {
    project_dirs()
        .map(|dirs| {
            let data = dirs.data_dir();
            std::fs::create_dir_all(data).ok();
            data.join("varac.db")
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn old_env_names_still_work() {
        let old = |var: &str| match var {
            "VARAC_HOST" => Some("10.0.0.5".to_string()),
            "VARAC_CMD_PORT" => Some("8400".to_string()),
            "VARAC_CALLSIGN" => Some("W5TST".to_string()),
            _ => None,
        };
        let m = Cli::command().get_matches_from(["openslot"]);
        let cli = Cli::with_old_env(&m, old);
        assert_eq!((cli.host.as_str(), cli.cmd_port, cli.callsign.as_deref()), ("10.0.0.5", 8400, Some("W5TST")));
        // the command line wins over an old name
        let m = Cli::command().get_matches_from(["openslot", "--host", "127.0.0.2"]);
        assert_eq!(Cli::with_old_env(&m, old).host, "127.0.0.2");
    }
}
