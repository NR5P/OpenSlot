use std::collections::VecDeque;

use chrono::Utc;

use crate::vara::commands::{Bandwidth, ListenMode};
use crate::varac::messages::{BbsFileOffer, IncomingMessage, VMail};
use crate::varac::session::SessionState;

/// Messages from the async session to the GUI.
pub enum SessionMsg {
    /// VarAC opening handshake finished (opening reports exchanged).
    HandshakeComplete,
    /// QSY agreed during the handshake: retune by this many Hz.
    QsyBy(i32),
    /// Partner allows auto-QSY: show the "Send QSY Invitation" picker.
    QsyInvitationRequested,
    Connected {
        source: String,
        destination: String,
        bandwidth: Option<String>,
    },
    Disconnected,
    Ptt(bool),
    Buffer(u32),
    Busy(bool),
    /// The modem's TX drive level (its answer to TUNE ?)
    DriveLevel(i32),
    /// Our own CQ or beacon went out (VarAC stores it in the heard history)
    OwnCq {
        source: String,
        bandwidth: Option<u16>,
    },
    CqFrame {
        source: String,
        bandwidth: u16,
        /// VARA's SN report that came with the frame
        snr: Option<f32>,
    },
    Message(IncomingMessage),
    /// Openslot encryption / signing state or signature check
    Secure(crate::varac::secure::SecureNote),
    /// A station handed us an email as gateway; it's queued for the sender (error = not stored).
    EmailQueued { from: String, to: String, subject: String, error: Option<String> },
    /// Connected gateway and emails queued: ask whether to relay (VarAC's popup).
    EmailRelayOffer { gateway: String, count: usize, auto: bool, deadline: std::time::Instant },
    /// The relay popup timed out (the background task applied the default).
    EmailRelayOfferClosed,
    /// We hold VMail for the partner but the link is still on the calling frequency
    /// (VarAC's "VMail relay alert"; <VW> was sent)
    VMailRelayAlert(String),
    /// The gateway shut itself down after too many failed sends (VarAC's retry limit).
    EmailGatewaySuspended(String),
    /// VarAC ADVANCED beacon heard (callsign, locator, services).
    /// (with VARA's SN for it, the last one measured)
    AdvancedBeacon(crate::vara::kiss::AdvancedBeacon, Option<f32>),
    /// The gateway pulled an email from its mailbox and filed it for a station.
    EmailIncoming { to_call: String, from: String, subject: String, parked: bool },
    Snr(f32),
    Log(String),
    /// What waits to go out (VarAC's "In queue"), already worded for display
    Queue(Vec<String>),
    /// EDIT: these queued texts come back to the message box
    QueueReturned(Vec<String>),
    /// A status line in the chat (VarAC's system lines)
    SystemChat(String),
    /// A transmission was held: the channel is busy (VarAC asks "Do you still wish to …?")
    TxHeld { cmd: GuiCommand, title: String, text: String },
    /// A transmission was refused, with VarAC's message box (PTT disabled, frequency busy …)
    TxRefused { title: String, text: String },
    /// A VarAC typed CQ (CHAT, INFO, SOS …) and the SNR it came in at
    TypedCq(crate::vara::kiss::TypedCq, Option<f32>),
    /// A QSY invitation to count down on (VarAC's "QSY NOW"; inviter = ours)
    QsyInvitation { target: crate::varac::session::QsyTarget, inviter: bool },
    /// The partner settled the pending QSY: close the countdown
    QsyResolved,
    /// We moved for our own invitation and heard nothing for 35 s: offer to go back here
    QsyRecovery(u64),
    /// A QSO frame's VarAC chat number (ours once transmitted, or the partner's)
    FrameId { outgoing: bool, id: u32, text: String },
    /// A frame of ours went to the modem (text, bytes)
    TxStarted { text: String, bytes: usize },
    /// Part of an incoming file frame is here (bytes so far, frame length)
    RxProgress { received: usize, total: usize },
    /// VARA's BITRATE line: speed level, bps, transmitting?
    Bitrate { level: u8, bps: u32, tx: bool },
    /// The modem took our CONNECT: the call is on until CONNECTED or DISCONNECTED
    Dialing,
    ModemConnected,
    ModemDisconnected,
    ModemFailed(String),
}

/// The status line while a broadcast waits in the queue (GuiState::broadcast_went_out).
pub const BROADCAST_QUEUED: &str = "Broadcast queued (goes out when the channel is free)";

/// Commands from the GUI to the async session.
pub enum GuiCommand {
    Connect(String),
    Disconnect,
    Abort,
    SendChat(String),
    /// Hand our Outbox emails to the connected gateway now (composed during the QSO).
    FlushEmailOutbox,
    /// Don't relay our emails through this gateway on this link (relay popup: CLOSE).
    DeclineEmailRelay,
    #[allow(dead_code)] // (VMail now goes through the Outbox and RelayNow)
    SendVMail(VMail),
    FileReady,
    FileOk,
    FileAbort,
    SendCq { source: String, bandwidth: u16 },
    SetBandwidth(Bandwidth),
    SetListen(ListenMode),
    SetAway(Option<String>),
    ConnectModem,
    /// Connect to the built-in modem just started: retried until it takes the connection
    #[allow(dead_code)] // (sent by the apps with a built-in modem)
    ConnectBuiltInModem,
    DisconnectModem,
    Ping(String),
    /// A broadcast to ALL or a callsign, through up to two digipeaters (VarAC)
    SendBroadcast { to: String, via: Vec<String>, text: String },
    /// VMail with attachments, sent in a QSO with the addressee.
    #[allow(dead_code)]
    SendVMailWithAttachments(VMail, Vec<std::path::PathBuf>),
    /// Answer an auto-QSY offer (offset in tens of Hz).
    SendQsyInvitation(i32),
    SendFile(std::path::PathBuf),
    InfoRequest,
    /// Our verbose SNR checkbox: invite (<VSI>) / stop (<VSS>)
    VerboseSnr(bool),
    /// Answer the partner's verbose SNR invitation
    VerboseSnrAnswer(bool),
    /// Settings saved: re-register MYCALL for the ping setting
    ApplyPingPolicy,
    /// Send an inquiry tag as is (<LHR>, <LHC:CALL>, <VRP>, <LOCR>, <VER>, <FSR>)
    SendInquiry(String),
    /// Ask the connected gateway for its email address (<EGA>)
    GatewayAddressRequest,
    SnrRequest,
    Tune,
    TuneStop,
    /// Disconnect once what is queued has gone out (a message with <DISC>)
    DisconnectAfterSent,
    /// The modem's TX drive level, dB -30…0 (VarAC: DRIVELEVEL from its tune slider)
    DriveLevel(i32),
    ClearTxBuffer,
    SendIsTyping,
    /// The "QSY NOW" verdict: accept (retune by `offset_hz`) or reject (`oor`: <QJO>)
    QsyAnswer { accept: bool, oor: bool, offset_hz: i32 },
    /// VarAC's typed CQ (CHAT, INFO, SOS …): KISS frame with our slot, locator and type
    SendTypedCq { slot: u8, cq_type: String },
    /// Mid-QSO one-slot QSY invitation (<QSYU> / <QSYD>)
    QsyStep { up: bool, step_hz: i32 },
    /// EmComm check-in request: a broadcast `<EC>` to EMCOMM (VarAC)
    EmCommCheckIn,
    /// Send these Outbox VMails through the connected station now (VarAC "Relay now";
    /// `check_cf`: refuse on the calling frequency before a QSY, quietly if `quiet`;
    /// `disconnect_after`: SEND AND DISCONNECT, close the link on their <SMR>)
    RelayNow { guids: Vec<String>, check_cf: bool, quiet: bool, disconnect_after: bool },
    /// The operator said "transmit anyway" to a held command (busy channel)
    Confirmed(Box<GuiCommand>),
    /// VarAC's "PTT Disabled": listen only, nothing is transmitted
    SetPttDisabled(bool),
    /// VarAC's "Ignore DCD": transmit even when the channel is busy
    SetIgnoreDcd(bool),
    /// VarAC's "Unattended links" (counts while away): the radio may answer links
    SetUnattended(bool),
    /// Like / unlike the partner's line (VarAC `&L1n` / `&L0n`)
    SendReaction { id: u32, like: bool },
    /// `<SFFA>`: the file we received is broken
    FileFailed,
    /// Take (true) or refuse a big VMail attachment
    AnswerAttachment(bool),
    /// VarAC's queue CLR (`edit` = EDIT: the queued texts come back to the message box)
    ClearQueue { edit: bool },
    PathFinderRequest(String),
    RequestBbsList,
    RequestBbsFile(String),
    QsyRequest { offset_hz: i32 },
    #[allow(dead_code)]
    QsyAbsoluteRequest { freq_hz: u64 },
    QsyAccept,
    QsyReject,
}

/// A file coming in from the partner (the download card above the chat).
#[derive(Debug, Clone)]
pub struct RxFile {
    pub name: String,
    pub from: Option<String>,
    /// The file's gzip size (from the offer) and the frame's length on the air (base64)
    pub file_bytes: usize,
    pub total: usize,
    pub received: usize,
    pub started: std::time::Instant,
    /// (when, bytes so far) at each delivery from the modem, for the speed
    pub samples: Vec<(std::time::Instant, usize)>,
    /// Ended: when, and whether all of it arrived
    pub done: Option<(std::time::Instant, bool)>,
}

impl RxFile {
    /// On-air speed in bits per second over the last minute of deliveries.
    pub fn bps(&self) -> Option<f32> {
        let last = *self.samples.last()?;
        let first = *self.samples.iter().find(|(t, _)| (last.0 - *t).as_secs_f32() <= 60.0)?;
        let secs = (last.0 - first.0).as_secs_f32();
        (secs >= 1.0 && last.1 > first.1).then(|| (last.1 - first.1) as f32 * 8.0 / secs)
    }
}

/// Where one of our lines is on its way out (the send progress under it).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TxStage {
    /// waiting in our queue
    Queued,
    /// in the modem: `total` bytes handed over at `since`
    OnAir { total: u32, since: std::time::Instant },
    /// the modem's buffer drained (VarAC numbers the frame then)
    Sent { at: std::time::Instant },
    /// the link ended before it was out
    Unsent,
}

#[derive(Default)]
pub struct ChatLine {
    pub time: String,
    pub callsign: String,
    pub text: String,
    pub is_outgoing: bool,
    pub is_system: bool,
    pub reply_to: Option<String>,
    /// VarAC's per-QSO frame number (both sides count, it is never sent): what replies
    /// (`<C:n>`) and likes (`&L1n`) refer to
    pub chat_id: Option<u32>,
    /// Liked: by us (their line) or by the partner (our line)
    pub liked: bool,
    /// "REPLIED TO: …" (first 50 characters of the line it answers)
    pub reply_quote: Option<String>,
    pub alert: Option<AlertColors>,
    /// Our line's send progress (lines typed this session)
    pub tx: Option<TxStage>,
    /// Encrypted / signed (Openslot), shown beside the time
    pub secure: Option<LineSecurity>,
}

/// How a chat line went over the air (Openslot encryption / signing).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineSecurity {
    Encrypted,
    /// Ours, signed
    Signed,
    /// Theirs: the signature checks out with their saved key
    Verified,
    /// Theirs: signed with a key that isn't the saved one, or one we don't have
    Unverified,
    /// Theirs: the signature doesn't match
    Forged,
}

impl LineSecurity {
    pub fn of(check: crate::varac::secure::SigCheck) -> Self {
        use crate::varac::secure::{KeyTrust, SigCheck};
        match check {
            SigCheck::Valid(KeyTrust::FirstContact | KeyTrust::Known) => Self::Verified,
            SigCheck::Valid(KeyTrust::Changed) | SigCheck::NoKey => Self::Unverified,
            SigCheck::Bad => Self::Forged,
        }
    }
}

#[allow(dead_code)]
/// A HamPlay game with the QSO partner.
pub struct HamPlaySession {
    pub board: crate::varac::hamplay::Board,
    /// 1 = X / blue (moves first), 2 = O / orange
    pub me: u8,
    /// Both sides are in (after `<PR>`)
    pub active: bool,
    /// We were invited (or challenged to a rematch): ACCEPT is offered
    pub invited: bool,
    pub opponent: String,
    pub status: String,
    pub open: bool,
}

impl HamPlaySession {
    pub fn my_turn(&self) -> bool {
        self.active && self.board.outcome.is_none() && self.board.to_move == self.me
    }
}

pub struct CqEntry {
    pub time: String,
    pub callsign: String,
    pub bandwidth: u16,
    pub snr: Option<String>,
    pub locator: Option<String>,
    pub is_beacon: bool,
    pub cq_type: Option<String>,
    pub timestamp: chrono::DateTime<Utc>,
    /// The slot this CQ's caller waits on (its CQFRAME SSID; 0 = none, VarAC slots)
    pub slot: u8,
    /// Band it was heard on (VarAC refuses to connect a station heard on another band)
    pub band: String,
    /// An alert tag matched: (font, background) colours
    pub alert: Option<AlertColors>,
}

/// An alert tag's (font, background) colours.
pub type AlertColors = ((u8, u8, u8), (u8, u8, u8));

#[allow(dead_code)]
pub struct BroadcastEntry {
    pub time: String,
    pub from: String,
    pub to: String,
    pub via: Option<String>,
    /// As VarAC shows it (Path Finder lines decoded, "(QSY TO: … MHz)" added)
    pub message: String,
    /// A QSY announcement's frequency (`<QF:hz>`): click to go there
    pub qsy_hz: Option<u64>,
    pub alert: Option<AlertColors>,
}

/// VarAC's question when a VMail is for someone other than the connected station:
/// relay it through them now, or keep it in the Outbox (closes itself after 30 s = keep).
/// VarAC's "QSY NOW" countdown, on both sides of an invitation, and its
/// "Still waiting for your partner to QSY" form (`recovery`: go back to `target_hz`).
pub struct QsyPopup {
    pub target_hz: u64,
    pub offset_hz: i32,
    /// What happens when the countdown runs out: accept (10 s) or reject (30 s)
    pub default_accept: bool,
    /// Rejecting because it's outside our allowed auto-QSY ranges (<QJO>)
    pub out_of_range: bool,
    pub auto: bool,
    pub until: chrono::DateTime<Utc>,
    pub recovery: bool,
}

/// VarAC's QSO log row (CALLSIGN, SNR-S, SNR-R, BAND, NAME, LOC, QTH, MyPWR, START, END):
/// filled as the QSO goes, editable, written to the ADIF log and loggers by LOG (or at
/// the end of the QSO with Auto log on).
#[derive(Debug, Clone, Default)]
pub struct QsoLogFields {
    pub call: String,
    /// "+05" (how we hear them)
    pub snr_sent: String,
    /// "+05" (how they hear us)
    pub snr_rcvd: String,
    pub band: String,
    pub name: String,
    pub qth: String,
    pub locator: String,
    pub power: String,
    /// "yyyy-MM-dd HH:mm:ss" UTC
    pub start: String,
    pub end: String,
    pub freq_hz: u64,
    /// Already written (LOG during the QSO): don't log it again at the end
    pub written: bool,
}

pub struct RelayPrompt {
    pub guids: Vec<String>,
    pub to: String,
    pub via: String,
    pub email: bool,
    pub until: chrono::DateTime<Utc>,
    pub disconnect_after: bool,
}

pub struct VMailEntry {
    pub guid: String,
    pub time: String,
    pub from: String,
    pub to: String,
    pub subject: String,
    pub body: String,
    pub read: bool,
    pub urgent: bool,
}

#[derive(Debug, Clone)]
pub struct CannedMessage {
    pub label: String,
    pub text: String,
    pub disconnect_after: bool,
}

impl Default for CannedMessage {
    fn default() -> Self {
        Self { label: String::new(), text: String::new(), disconnect_after: false }
    }
}

pub struct SettingsForm {
    pub callsign: String,
    pub name: String,
    pub qth: String,
    pub locator: String,
    pub modem_host: String,
    pub cmd_port: String,
    pub data_port: String,
    pub kiss_port: String,
    pub bandwidth: String,
    pub vmail_enabled: bool,
    pub bbs_enabled: bool,
    pub settings_tab: usize,
    pub beacon_interval: String,
    /// VarAC StartBeaconUponStartup
    pub start_beacon_on_startup: bool,
    /// VarAC SuppressIsTypingInLowSNR
    pub suppress_typing_low_snr: bool,
    pub auto_log_qso: bool,
    pub auto_away_enabled: bool,
    pub auto_away_minutes: String,
    pub allow_pings: bool,
    pub allow_info_request: bool,
    pub load_history: bool,
    pub font_size: String,
    pub notifications_enabled: bool,
    pub sound_alerts: bool,
    pub use_local_time: bool,
    pub modem_auto_reconnect: bool,
    pub bbs_directory: String,
    pub qso_alert_minutes: String,
    // Email gateway (Settings → Email)
    pub email_gateway: bool,
    pub smtp_server: String,
    pub smtp_port: String,
    /// "starttls" | "tls" | "none"
    pub smtp_security: String,
    pub smtp_username: String,
    pub smtp_password: String,
    pub gateway_email: String,
    /// Result of the last "Send test email" (None while sending / never).
    pub email_test_status: Option<String>,
    pub email_test_rx: Option<std::sync::mpsc::Receiver<Result<(), String>>>,
    /// Only stations on `email_gateway_allow` may send email through us
    pub email_gateway_limit: bool,
    pub email_gateway_allow: String,
    pub smtp_retries: String,
    pub email_gateway_announce: bool,
    pub email_gateway_auto_relay: bool,
    /// Accept a partner's verbose-SNR invitation without asking
    pub verbose_snr_auto_accept: bool,
    pub allow_last_heard: bool,
    pub call_id_minutes: String,
    pub idle_disconnect_minutes: String,
    pub welcome_message: String,
    pub welcome_always: bool,
    pub allow_path_finder: bool,
    /// Antenna rotator (PstRotator over UDP), Settings › RIG
    pub rotator_enabled: bool,
    pub rotator_host: String,
    pub rotator_port: String,
    pub adif_log_path: String,
    pub adif_submode: String,
    pub default_log_comment: String,
    pub logger_app: String,
    pub logger_ip: String,
    pub logger_port: String,
    pub logger_app2: String,
    pub logger_ip2: String,
    pub logger_port2: String,
    pub logger_callsign_lookup: bool,
    pub pskreporter_upload: bool,
    pub pskreporter_self_report: bool,
    pub callsigns_block_list: String,
    pub unlimited_beacon_time: bool,
    pub hamplay_enabled: bool,
    pub ai_enabled: bool,
    pub ai_api_key: String,
    pub ai_model: String,
    pub ai_timeout_secs: String,
    pub ai_limit_chars: String,
    pub ai_chat_context: bool,
    pub ai_limit_per_qso: String,
    pub ai_announce: bool,
    pub ai_customization: String,
    pub dxcluster_enabled: bool,
    pub dxcluster_host: String,
    pub dxcluster_port: String,
    pub dxcluster_username: String,
    pub dxcluster_password: String,
    pub schedule_start_mode: String,
    pub scanner_busy_hold_secs: String,
    pub scanner_announce: bool,
    pub vara_path: String,
    pub vara_launch: bool,
    pub vara_launch_delay_secs: String,
    pub emcomm_mode: bool,
    pub emcomm_disable_snr: bool,
    pub emcomm_disable_call_id: bool,
    pub emcomm_allow_last_connections: bool,
    pub emcomm_auto_file_approval: bool,
    pub emcomm_qsy_notify: bool,
    pub gps_manual: String,
    pub distance_unit: String,
    pub incoming_files_dir: String,
    pub digipeat_via: String,
    pub vara_modem_type: String,
    /// VarAC UsbMode (USB, USB-D, DATA-U, FM, FM-D)
    pub usb_mode: String,
    pub ale_snr_threshold: String,
    pub ale_history_days: String,
    pub ale_recent_minutes: String,
    pub ale_skip_approval: bool,
    pub ale_enabled: bool,
    pub send_relay_notifications: bool,
    pub allow_parking: bool,
    /// Openslot encryption / signing (Security tab)
    pub encrypt_link: bool,
    pub sign_messages: bool,
    pub use_shared_keys: bool,
    /// "Use encryption": the encryption options and the main page's lock are available
    pub use_encryption: bool,
    pub use_aes: bool,
    /// A shared key being added: name, callsigns, key, and why it wasn't taken
    pub shared_new: (String, String, String, Option<String>),
    /// "Make a new key" pressed: asking to be sure before the old key is thrown away
    pub confirm_new_identity: bool,
    pub incoming_files_size_limit: String,
    pub emcomm_auto_reply_checkin: bool,
    pub email: String,
    pub full_callsign: String,
    pub my_rig: String,
    pub my_power: String,
    pub my_antenna: String,
    pub my_age: String,
    pub my_profession: String,
    pub my_hobbies: String,
    pub multi_band_last_heard: bool,
    pub bbs_limit: bool,
    pub bbs_allow: String,
    pub bbs_announce: bool,
    /// "standard" | "advanced"
    pub beacon_type: String,
    // Slots (Settings → QSO)
    pub cq_slot_wait_secs: String,
    pub skip_cq_slot_selector: bool,
    pub slot_step_hz: String,
    // Incoming emails (IMAP)
    pub imap_enabled: bool,
    pub imap_server: String,
    pub imap_port: String,
    /// "tls" | "starttls" | "none"
    pub imap_security: String,
    pub imap_username: String,
    pub imap_password: String,
    pub imap_poll_seconds: String,
    pub imap_test_status: Option<String>,
    pub imap_test_rx: Option<std::sync::mpsc::Receiver<Result<String, String>>>,
}

pub struct GuiState {
    /// The current link is encrypted (Openslot)
    pub link_encrypted: bool,
    /// The lock above SEND: encryption on or off (Openslot; kept as it was left)
    pub encryption_lock: bool,
    /// The lock was clicked: to apply and save
    pub encryption_lock_toggled: bool,
    /// The saved Security choices the lock needs: Use encryption, Encrypt links, Use shared keys
    pub encryption_saved: (bool, bool, bool),
    /// How the partner's next message's signature checked out (Openslot)
    pub pending_signature: Option<crate::varac::secure::SecureNote>,
    pub chat_lines: VecDeque<ChatLine>,
    pub cq_entries: VecDeque<CqEntry>,
    pub beacon_entries: VecDeque<CqEntry>,
    pub broadcasts: VecDeque<BroadcastEntry>,
    pub vmail_list: Vec<VMailEntry>,
    pub log_lines: VecDeque<String>,

    pub connected_to: Option<String>,
    pub session_state: SessionState,
    pub ptt: bool,
    pub busy: bool,
    /// VarAC's "PTT Disabled" checkbox: listen only
    pub ptt_disabled: bool,
    /// A command waiting for "Do you still wish to …?" (busy channel): (command, title, text)
    pub tx_held: Option<(GuiCommand, String, String)>,
    /// VarAC's message box for a refused action: (title, text)
    pub tx_alert: Option<(String, String)>,
    /// VarAC's "Ignore DCD" (and since when: it switches itself off after 2 hours)
    pub ignore_dcd: bool,
    pub ignore_dcd_since: Option<chrono::DateTime<Utc>>,
    /// VarAC's "Unattended links" (on by default; counts while away)
    pub unattended_links: bool,
    /// When the channel last went quiet (automatic beacons wait 2–6 s after that)
    pub busy_cleared_at: Option<chrono::DateTime<Utc>>,
    /// This run's random 2–6 s hold-off after BUSY OFF (as in VarAC)
    pub busy_holdoff_secs: i64,
    pub buffer_bytes: u32,
    /// When BUFFER last came (the send progress runs on between reports)
    pub buffer_at: Option<std::time::Instant>,
    /// The modem's current transmit speed (VARA BITRATE … TX): level, bps
    pub tx_speed: Option<(u8, u32)>,
    pub my_callsign: String,
    pub modem_connected: bool,
    pub snr: Option<f32>,
    pub unread_vmails: i64,

    pub input_text: String,
    /// Replying to this line (chat number, its text): the next message goes "<C:n>text"
    pub reply_to_chat: Option<(u32, String)>,
    /// Chat number of the partner frame being shown right now
    pub rx_chat_id: Option<u32>,
    pub connect_callsign: String,
    pub status_msg: String,

    pub selected_vmail: Option<usize>,
    pub viewing_vmail: bool,

    pub pending_file_offer: Option<BbsFileOffer>,
    /// The pending offer is a VMail attachment zip (`<SFM:>`), not a plain file.
    pub pending_vmail_attachment: bool,
    /// ACCEPT FILE was clicked for the pending offer.
    pub file_accepted: bool,
    /// The file coming in now (or just finished), for the download card.
    pub rx_file: Option<RxFile>,
    /// Files attached in the VMail compose window.
    pub compose_attachments: Vec<std::path::PathBuf>,
    /// Where the last VMail attachment zip was unpacked (noted on the VMail).
    pub last_attachment_dir: Option<std::path::PathBuf>,

    // Compose state
    pub compose_open: bool,
    pub compose_to: String,
    pub compose_subject: String,
    pub compose_body: String,
    pub compose_urgent: bool,
    pub compose_park: bool,
    /// Email: REPLY TO address (empty = replies come back through the gateway)
    pub compose_reply_to: String,
    /// Email: send as typed, without the gateway footer (VarAC "raw email")
    pub compose_raw: bool,
    /// Open "relay through gateway?" popup: (gateway, emails, relays by default, deadline)
    pub relay_offer: Option<(String, usize, bool, std::time::Instant)>,
    // VarAC slot system
    /// CQ slot selector window open
    pub cq_slot_selector_open: bool,
    /// Slot chosen in the selector (VarAC remembers the preferred one)
    pub cq_slot: u8,
    /// CQ sent on a slot: retune there once it's on the air (slot, CF, sent at)
    pub pending_slot_qsy: Option<(u8, u64, chrono::DateTime<Utc>)>,
    /// The CQ under way (VarAC sends it twice)
    pub cq_seq: Option<CqSeq>,
    /// Standing by on a slot after a CQ: (slot, CF, back to the CF at)
    pub slot_wait: Option<(u8, u64, chrono::DateTime<Utc>)>,
    /// Retune the radio to this dial frequency on the next frame
    pub tune_to: Option<u64>,
    /// Connect once the radio has moved to the slot: (callsign, at)
    pub pending_connect: Option<(String, chrono::DateTime<Utc>)>,
    /// "This station called CQ on a slot N minutes ago" confirmation: (call, slot, minutes)
    pub stale_cq_confirm: Option<(String, u8, i64)>,
    /// The calling frequency this QSO moved away from (VarAC asks to go back after it)
    pub qsy_return: Option<u64>,
    /// "Your QSO is over. QSY back to the calling frequency?": (CF, deadline, go back when
    /// it runs out — VarAC accepts after 10 s with auto-QSY allowed, else declines after 30 s)
    pub return_cf_prompt: Option<(u64, chrono::DateTime<Utc>, bool)>,

    // VMail folder tab: 0=Inbox, 1=Sent, 2=Outbox, 3=Parking
    pub vmail_folder_tab: usize,
    pub vmail_sent_list: Vec<VMailEntry>,
    pub vmail_outbox_list: Vec<VMailEntry>,
    pub vmail_parking_list: Vec<VMailEntry>,
    pub vmail_drafts_list: Vec<VMailEntry>,
    /// Editing this VMail (guid; true = an Outbox VMail: To fixed, button "UPDATE")
    pub compose_editing: Option<(String, bool)>,
    /// "Are you sure?" before throwing away a half-written VMail
    pub compose_cancel_confirm: bool,
    pub relay_prompt: Option<RelayPrompt>,
    pub qsy_popup: Option<QsyPopup>,
    pub qso_log: QsoLogFields,
    /// Settings were saved: tell PSKReporter again who we are
    pub psk_reconfigure: bool,
    /// HamPlay game in progress and the game center window
    pub hamplay: Option<HamPlaySession>,
    pub hamplay_last_invite: Option<chrono::DateTime<Utc>>,
    pub hamplay_center_open: bool,
    /// "AI MODE": everything we type goes to the partner's AI gateway (prefixed `AI:`)
    pub ai_mode: bool,
    /// Our AI gateway: questions answered this QSO, and the QSO's questions/answers so far
    pub ai_count: u32,
    pub ai_history: Vec<(bool, String)>,
    pub ai_pending: Option<std::sync::mpsc::Receiver<Result<String, String>>>,
    pub ai_waiting: std::collections::VecDeque<String>,
    /// The first line relayed by a VarAC cluster this QSO got VarAC's explanation
    pub cluster_note_shown: bool,
    /// Blocked callsigns (VarAC CallsignsBlockList)
    pub block_list: Vec<String>,
    pub block_confirm: Option<String>,
    /// VarAC's ALE: the "ALE - Connect" window (its sequence pauses the scanner), the time
    /// of its last tick, the mid-link SNR watch and its QSY suggestion (frequency, until)
    pub ale: Option<crate::varac::ale::Window>,
    pub ale_ticked: Option<chrono::DateTime<Utc>>,
    pub ale_low_since: Option<chrono::DateTime<Utc>>,
    pub ale_declined_at: Option<chrono::DateTime<Utc>>,
    pub ale_suggestion: Option<(u64, chrono::DateTime<Utc>)>,
    /// A connect asked for in ALE mode (the app opens the window), and "this connect is an
    /// ALE attempt"
    pub ale_request: std::cell::RefCell<Option<String>>,
    pub ale_attempt: std::cell::Cell<bool>,
    /// A command that counts as operator activity was sent (as in VarAC:
    /// chat, CQ, beacon, connect, ping)
    pub activity_bump: std::cell::Cell<bool>,
    /// What the ALE window's buttons asked for (carried out by the app)
    pub ale_actions: Vec<crate::varac::ale::Action>,
    /// The QSO partner went to PSKReporter already (once per QSO, as VarAC)
    pub psk_partner_spotted: bool,
    /// A VMail attachment over the size limit: its size, while we ask
    pub attachment_ask: Option<u64>,
    pub profiles_open: bool,
    pub profile_new_name: String,
    /// SNR over the QSO (the chart): the partner as we hear them, us as they hear us
    pub snr_his_series: Vec<(chrono::DateTime<Utc>, f32)>,
    pub snr_my_series: Vec<(chrono::DateTime<Utc>, f32)>,
    /// SNR chart window: None = live, Some(guid) = a past QSO
    pub snr_chart: Option<Option<String>>,
    pub map_open: bool,
    pub map_minutes: i64,
    /// QSL card generator
    pub qsl: Option<QslWindow>,
    /// VarAC's image shrinker for the picture about to be sent
    pub shrinker: Option<Shrinker>,
    /// Files we sent, for the gallery (logged by the app with the partner's call)
    pub sent_files: Vec<std::path::PathBuf>,
    pub gallery_open: bool,
    /// VarAC's frequency scheduler / scanner: mode, lists, progress, editor
    pub freq_mode: crate::varac::scheduler::Mode,
    pub schedule_rules: Vec<crate::varac::scheduler::Rule>,
    pub scan_list: Vec<crate::varac::scheduler::ScanEntry>,
    pub scanner: crate::varac::scheduler::Scanner,
    pub sched_last_check: Option<chrono::DateTime<Utc>>,
    pub sched_queued: Option<u64>,
    /// EmComm: the QSY-notify broadcast went out; QSY at this time
    pub sched_notify_until: Option<chrono::DateTime<Utc>>,
    pub scan_busy_seen: bool,
    pub sched_editor: Option<SchedEditor>,
    pub fsr_dirty: bool,
    /// DX cluster spot being prepared: (callsign, kHz text, comment), and the send result
    pub dx_spot: Option<(String, String, String)>,
    pub dx_spot_rx: Option<std::sync::mpsc::Receiver<Result<String, String>>>,
    /// VarAC's "Confirm QSO" (LOG with Auto log off) and its answer
    pub confirm_qso: Option<String>,
    pub confirm_qso_yes: bool,
    /// "AUTO QSO LOG IS OFF" notice, closed by itself at this time
    pub auto_log_off_notice: Option<chrono::DateTime<Utc>>,
    /// Alert tags (VarAC_alert_tags.conf), unread incoming alerts, Alert center
    pub alert_tags: Vec<crate::varac::alerts::AlertTag>,
    pub unread_alerts: i64,
    /// Recently alerted (source|call|tags), so repeated frames alert once
    pub alert_recent: std::collections::HashMap<String, chrono::DateTime<Utc>>,
    pub alert_center_open: bool,
    /// 0 Incoming, 1 Sent, 2 Archived
    pub alert_tab: usize,
    pub alert_selected: Option<String>,
    pub alert_setup: Option<Vec<crate::varac::alerts::AlertTag>>,
    /// "Callsign history" window for this callsign
    pub history_call: Option<String>,
    pub history_input: String,
    pub history_qso: Option<String>,
    /// "QSO log: set Name/QTH/LOC" — the chat line being picked from
    pub qso_log_pick: Option<String>,
    /// CQ type for the next CQ (VarAC: STANDARD, SOS, EMRG, HELP, CHAT, INFO, DATA or up
    /// to 4 characters of your own)
    pub cq_type: String,
    /// VarAC's "VMail will be queued" note (not connected)
    pub vmail_queued_popup: bool,
    /// VarAC's "Leave a message" offer when the partner is away
    pub leave_message_popup: Option<String>,
    /// The partner answered <AWQ> (away, auto-QSY): offer "Leave a message" after the QSY
    pub partner_awq: bool,
    pub relay_notifications_open: bool,
    pub relay_notification_count: usize,

    // Away status
    pub away_active: bool,
    /// Verbose SNR running on this link (VarAC's checkbox)
    pub verbose_snr: bool,
    /// The partner invited us to verbose SNR and we must answer
    pub verbose_snr_invite: bool,
    /// "Auto log QSO" off: the finished QSO waits here for the LOG button (VarAC)
    pub unlogged_qso: Option<crate::storage::QsoRecord>,
    /// The current link's qso row, written when it ends (as in VarAC)
    pub qso_row: Option<(crate::storage::QsoRecord, crate::storage::QsoExtra)>,
    /// The row written at the last link's end: a later LOG updates it
    pub qso_row_written: Option<String>,
    /// The current link was the partner's call (as in VarAC)
    pub link_incoming: bool,
    /// VarAC's "PAY ATTENTION! EmComm on a calling frequency" question is open
    pub emcomm_confirm: bool,
    /// VarAC's "close during an activity?" question: None, Some(false) open, Some(true) yes
    pub close_confirm: Option<bool>,
    /// The Tune window (opened by TUNE) and the modem's TX drive level in dB, once known
    pub tune_window: bool,
    /// TUNE was held because the frequency is busy: the Tune window asks, no popup.
    pub tune_busy: bool,
    pub drive_level: Option<i32>,
    /// The radio's ALC meter can be read (the Tune window's AUTO), and its last reading
    pub rig_alc: bool,
    pub alc_reading: Option<(f32, std::time::Instant)>,
    pub alc_polled: Option<std::time::Instant>,
    /// AUTO in the Tune window: setting the drive level from the ALC meter
    pub auto_tune: Option<AutoTune>,
    /// How the last AUTO ended (and whether it worked)
    pub auto_tune_note: Option<(String, bool)>,
    /// VarAC's "Bandwidth not allowed!" box is open
    pub bw_refused: bool,
    /// LOG clicked: write the waiting QSO
    pub log_qso_requested: bool,
    /// VarAC's "VMail relay alert" popup: (partner, closes at)
    pub vmail_relay_alert: Option<(String, chrono::DateTime<Utc>)>,
    pub away_message: String,

    // QSO duration tracking
    pub connection_start: Option<chrono::DateTime<Utc>>,

    // SNR tracking (Last / Avg / Mine)
    pub snr_last: Option<f32>,
    pub snr_avg: Option<f32>,
    pub snr_mine: Option<f32>,
    pub snr_samples: Vec<f32>,

    // Broadcast compose
    pub broadcast_compose_open: bool,
    pub broadcast_text: String,
    /// Broadcast window: addressee (ALL or a call), digipeaters, ask for an SNR receipt,
    /// QSY announcement (kHz)
    pub broadcast_to: String,
    pub broadcast_via: String,
    pub broadcast_snr_request: bool,
    pub broadcast_qsy_khz: String,
    /// The call our last Path Finder request asked about, and when it went out
    pub path_finder_target: Option<String>,
    pub path_finder_sent: Option<chrono::DateTime<Utc>>,

    // File send dialog
    pub file_send_open: bool,
    pub file_path_input: String,

    // "Currently sending" / "In queue" display
    pub currently_sending: Option<String>,
    pub queue_count: usize,
    pub queue_lines: Vec<String>,
    pub queue_open: bool,
    /// Status strip: VMail waiting in the Outbox / parked for others
    pub outbox_count: usize,
    pub parking_count: usize,
    /// EMAIL GATE pill: None = not configured, Some(on); a click asks the app to switch it
    pub email_gate_pill: Option<(bool, bool)>,
    pub email_gate_toggle: bool,

    // CQ active
    pub cq_active: bool,
    pub auto_cq_enabled: bool,
    pub auto_cq_interval_secs: u32,
    pub last_cq_sent: Option<chrono::DateTime<Utc>>,

    // Auto log and is-typing
    pub auto_log_qso: bool,
    pub is_typing_enabled: bool,

    // Path finder
    pub path_finder_open: bool,
    pub path_finder_callsign: String,

    // Is-typing throttle (send at most once per 3 seconds)
    pub last_typing_sent: Option<chrono::DateTime<Utc>>,

    // Auto-away tracking
    pub last_activity: chrono::DateTime<Utc>,
    pub auto_away_triggered: bool,

    // Canned messages (F1–F12, VarAC's 12 slots)
    pub canned_messages: [CannedMessage; 12],
    pub canned_editor_open: bool,

    // Beacon timer
    pub beacon_auto_enabled: bool,
    /// When the periodic beacon was switched on (VarAC stops it after 72 hours)
    pub beacon_started_at: Option<chrono::DateTime<Utc>>,
    /// The dial the beacon timer last saw, when it changed, and the beacon switch's last state
    pub beacon_dial: u64,
    pub beacon_freq_changed_at: Option<chrono::DateTime<Utc>>,
    pub beacon_was_on: bool,
    pub last_beacon_sent: Option<chrono::DateTime<Utc>>,

    // Band / frequency display
    pub current_band: String,
    pub current_frequency: u64,

    // Settings dialog
    pub settings_open: bool,
    pub settings_form: SettingsForm,

    // Log viewer
    pub log_open: bool,

    // QSO log
    pub qso_log_open: bool,
    pub qso_notes_guid: Option<String>,
    pub qso_notes_text: String,

    // Heard stations
    pub heard_stations_open: bool,

    // Contacts / address book
    pub contacts_open: bool,
    pub contact_edit_callsign: String,
    pub contact_edit_name: String,
    pub contact_edit_qth: String,
    pub contact_edit_locator: String,
    pub contact_edit_notes: String,
    pub contact_search: String,
    pub contact_adding: bool,

    // Chat search filter
    pub chat_search: String,
    pub chat_search_active: bool,
    /// The broadcasts' Search box, as the chat's
    pub broadcast_search: String,
    pub broadcast_search_active: bool,

    /// The chat and message box are in their own big window (Pop out / Shrink back)
    pub chat_window_open: bool,
    /// The broadcasts are in their own window (Pop out / Shrink back)
    pub broadcasts_window_open: bool,
    /// The broadcasts' share of the space they split with the chat in the main window
    /// (the drag bar between them); None = the default height
    pub broadcasts_share: Option<f32>,
    /// The chat grows over the broadcasts and the top cards while the mouse rests on it
    /// (off with the chat's 📌 button: it stays in its place)
    pub chat_grow_on_hover: bool,

    // CQ filter
    pub cq_filter: String,

    // Connection timeout tracking
    pub connect_started: Option<chrono::DateTime<Utc>>,
    /// The modem last keyed up to call (the connect timeout counts from it)
    pub connect_last_try: Option<chrono::DateTime<Utc>>,
    pub connect_timeout_secs: u32,

    // Font size
    pub font_size: f32,

    // Calling frequencies reference
    pub freq_ref_open: bool,

    // QSO log search
    pub qso_search: String,

    // Timestamp format: false = UTC, true = local time
    pub use_local_time: bool,

    // Modem auto-reconnect
    pub modem_auto_reconnect: bool,
    pub modem_reconnect_at: Option<chrono::DateTime<Utc>>,

    // Multi-line chat input
    pub multiline_input: bool,
    // SEND (in the right-hand column) clicked; the composer sends this frame
    pub send_requested: bool,

    // About dialog
    pub about_open: bool,
    /// Help → User guide window, its section and search text
    pub help_open: bool,
    pub help_section: usize,
    pub help_search: String,

    // BBS listing dialog
    pub bbs_listing_open: bool,
    pub bbs_listing_entries: Vec<crate::varac::messages::BbsListingEntry>,

    // Partner info from opening report
    pub partner_name: Option<String>,
    pub partner_qth: Option<String>,
    pub partner_locator: Option<String>,
    pub partner_services: Option<crate::varac::messages::ServiceFlags>,

    // Is-typing indicator from partner
    pub partner_typing_at: Option<chrono::DateTime<Utc>>,

    // QSY negotiation
    pub pending_qsy: Option<PendingQsy>,
    pub qsy_dialog_open: bool,
    pub qsy_offset_input: String,

    // Callsign watchlist
    pub watchlist: Vec<String>,
    /// The operator's own frequencies (frequency dropdown), the typed frequency and the
    /// name for the next one added
    pub my_frequencies: Vec<crate::config::SavedFrequency>,
    pub freq_entry: String,
    pub freq_entry_name: String,
    pub watchlist_open: bool,
    pub watchlist_input: String,

    // Callsign tags (friendly names)
    pub callsign_tags: std::collections::HashMap<String, String>,
    pub callsign_tags_open: bool,
    pub callsign_tag_call_input: String,
    pub callsign_tag_name_input: String,

    // Auto-QSY allowed frequency ranges (Hz)
    pub auto_qsy_ranges: Vec<(u64, u64)>,
    pub auto_qsy_enabled: bool,

    // QSO statistics window
    pub qso_stats_open: bool,

    // Connection retry
    pub last_connect_callsign: Option<String>,

    // Band scanner

    // QSO duration alert (minutes, 0 = disabled)
    pub qso_alert_minutes: u32,
    pub qso_alert_fired: bool,

    // DX alert — notify on new DXCC entities
    pub dx_alert_enabled: bool,

    // Dark/light theme
    pub dark_mode: bool,
    /// VarAC's SCIFI theme
    pub scifi_theme: bool,

    // Chat bookmarks
    pub bookmarked_messages: Vec<BookmarkedMessage>,
    pub bookmarks_open: bool,

    // Rig control
    pub rig_backend: crate::rig::RigBackend,
    pub rig_connected: bool,
    pub rig_freq: Option<u64>,
    pub rig_mode: Option<crate::rig::RadioMode>,
    pub rig_settings_backend: String,
    pub rig_settings_host: String,
    pub rig_settings_port: String,
    pub rig_settings_serial: String,
    pub rig_settings_baud: String,
    pub rig_settings_model: String,
    pub rig_settings_ptt: bool,
    pub vmail_window_open: bool,
    pub listen_mode: String,
    pub rig_test_ptt: Option<bool>,
    /// The current link is a ping (CALL-T), not a QSO.
    pub ping_active: bool,
    /// "Send QSY Invitation" picker is open (opened_at for its auto-close).
    pub qsy_invite_open: Option<chrono::DateTime<Utc>>,
    /// The invitation window was opened mid-QSO (QSY button), not by the partner's <Q>
    pub qsy_invite_mid: bool,
    /// The frequency picked in it (a slot or a custom one) and the custom kHz text
    pub qsy_invite_target: Option<u64>,
    pub qsy_custom_khz: String,
    /// Frequency sniffer held down: (frequency to return to, since, we switched PTT off)
    pub qsy_sniff: Option<(u64, chrono::DateTime<Utc>, bool)>,
    /// CQ slot selector's sniffer held down: (calling frequency to return to, since, the
    /// slot was heard busy). VarAC pauses beacons meanwhile; PTT stays as it is.
    pub cq_sniff: Option<(u64, chrono::DateTime<Utc>, bool)>,
    pub rig_reconnect_requested: bool,
    pub rig_ptt: bool,
    pub rig_error: Option<String>,
    /// The Rig controls window, the radio's extra controls as last read, and what was
    /// asked of them (here or from a remote station) for the app to send to the rig
    pub rig_panel_open: bool,
    pub rig_extras: crate::rig::RigExtras,
    pub rig_actions: Vec<crate::rig::RigAction>,
    /// Remote mode: the Remote control window, and the console while controlling a station
    pub remote: super::remote_ui::RemoteForm,
    /// Settings → RIG → sharing the radio with other programs: the form, "Apply" asked
    /// for, and the server's state (port and programs connected, or why it isn't running)
    pub rig_share: crate::rig::share::ShareSettings,
    pub rig_share_port: String,
    pub rig_share_apply: bool,
    pub rig_share_status: Option<Result<(u16, usize), String>>,
    pub rig_ports: Vec<crate::rig::PortInfo>,
    /// Settings → RIG "Detect radio": asked for, then running (the result comes back here)
    pub rig_detect_requested: bool,
    pub rig_detect_rx: Option<std::sync::mpsc::Receiver<Result<crate::rig::DetectedRig, String>>>,
    pub rig_detect_status: Option<String>,
    /// The search was started by the radio not answering at startup (not the button)
    pub rig_detect_auto: bool,

    pub cmd_tx: tokio::sync::mpsc::UnboundedSender<GuiCommand>,
}

pub struct BookmarkedMessage {
    pub time: String,
    pub callsign: String,
    pub text: String,
}

#[allow(dead_code)]
pub struct PendingQsy {
    pub offset_hz: Option<i32>,
    pub freq_hz: Option<u64>,
}

impl GuiState {
    pub fn new(callsign: String, modem_connected: bool, cmd_tx: tokio::sync::mpsc::UnboundedSender<GuiCommand>) -> Self {
        let status = if modem_connected {
            "VARA modem connected. Ready.".into()
        } else {
            "Offline — use Connect Modem".into()
        };
        Self {
            link_encrypted: false,
            encryption_lock: false,
            encryption_lock_toggled: false,
            encryption_saved: (false, false, false),
            pending_signature: None,
            chat_lines: VecDeque::with_capacity(500),
            cq_entries: VecDeque::with_capacity(200),
            beacon_entries: VecDeque::with_capacity(200),
            broadcasts: VecDeque::with_capacity(200),
            vmail_list: Vec::new(),
            log_lines: VecDeque::with_capacity(500),
            connected_to: None,
            session_state: SessionState::Idle,
            ptt: false,
            busy: false,
            ptt_disabled: false,
            tx_held: None,
            tx_alert: None,
            ignore_dcd: false,
            ignore_dcd_since: None,
            unattended_links: true,
            busy_cleared_at: None,
            busy_holdoff_secs: 2 + (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos()).unwrap_or(0) % 5) as i64,
            buffer_bytes: 0,
            buffer_at: None,
            tx_speed: None,
            my_callsign: callsign,
            modem_connected,
            snr: None,
            unread_vmails: 0,
            input_text: String::new(),
            reply_to_chat: None,
            rx_chat_id: None,
            connect_callsign: String::new(),
            status_msg: status,
            selected_vmail: None,
            viewing_vmail: false,
            pending_file_offer: None,
            pending_vmail_attachment: false,
            file_accepted: false,
            rx_file: None,
            compose_attachments: Vec::new(),
            last_attachment_dir: None,
            compose_open: false,
            compose_to: String::new(),
            compose_subject: String::new(),
            compose_body: String::new(),
            compose_urgent: false,
            compose_park: false,
            compose_reply_to: String::new(),
            compose_raw: false,
            relay_offer: None,
            cq_slot_selector_open: false,
            cq_slot: 11,
            pending_slot_qsy: None,
            cq_seq: None,
            slot_wait: None,
            tune_to: None,
            pending_connect: None,
            stale_cq_confirm: None,
            qsy_return: None,
            return_cf_prompt: None,
            vmail_folder_tab: 0,
            vmail_sent_list: Vec::new(),
            vmail_outbox_list: Vec::new(),
            vmail_parking_list: Vec::new(),
            vmail_drafts_list: Vec::new(),
            compose_editing: None,
            compose_cancel_confirm: false,
            relay_prompt: None,
            qsy_popup: None,
            qso_log: QsoLogFields::default(),
            psk_reconfigure: false,
            hamplay: None,
            hamplay_last_invite: None,
            hamplay_center_open: false,
            ai_mode: false,
            ai_count: 0,
            ai_history: Vec::new(),
            ai_pending: None,
            ai_waiting: Default::default(),
            cluster_note_shown: false,
            block_list: crate::config::block_list(),
            block_confirm: None,
            ale: None,
            ale_ticked: None,
            ale_low_since: None,
            ale_declined_at: None,
            ale_suggestion: None,
            ale_request: Default::default(),
            ale_attempt: Default::default(),
            activity_bump: Default::default(),
            ale_actions: Vec::new(),
            psk_partner_spotted: false,
            attachment_ask: None,
            profiles_open: false,
            profile_new_name: String::new(),
            snr_his_series: Vec::new(),
            snr_my_series: Vec::new(),
            snr_chart: None,
            map_open: false,
            map_minutes: 1440,
            qsl: None,
            shrinker: None,
            sent_files: Vec::new(),
            gallery_open: false,
            freq_mode: crate::varac::scheduler::Mode::Inactive,
            schedule_rules: load_sched_file(crate::varac::scheduler::SCHEDULE_FILE).map(|t| crate::varac::scheduler::parse_schedule(&t)).unwrap_or_default(),
            scan_list: load_sched_file(crate::varac::scheduler::SCANNER_FILE).map(|t| crate::varac::scheduler::parse_scanner(&t)).unwrap_or_default(),
            scanner: Default::default(),
            sched_last_check: None,
            sched_queued: None,
            sched_notify_until: None,
            scan_busy_seen: false,
            sched_editor: None,
            fsr_dirty: true,
            dx_spot: None,
            dx_spot_rx: None,
            confirm_qso: None,
            confirm_qso_yes: false,
            auto_log_off_notice: None,
            alert_tags: load_alert_tags(),
            unread_alerts: 0,
            alert_recent: Default::default(),
            alert_center_open: false,
            alert_tab: 0,
            alert_selected: None,
            alert_setup: None,
            history_call: None,
            history_input: String::new(),
            history_qso: None,
            qso_log_pick: None,
            cq_type: "STANDARD".into(),
            vmail_queued_popup: false,
            leave_message_popup: None,
            partner_awq: false,
            relay_notifications_open: false,
            relay_notification_count: 0,
            away_active: false,
            verbose_snr: false,
            verbose_snr_invite: false,
            unlogged_qso: None,
            qso_row: None,
            qso_row_written: None,
            link_incoming: false,
            emcomm_confirm: false,
            close_confirm: None,
            tune_window: false,
            tune_busy: false,
            drive_level: None,
            rig_alc: false,
            alc_reading: None,
            alc_polled: None,
            auto_tune: None,
            auto_tune_note: None,
            bw_refused: false,
            help_open: false,
            help_section: 0,
            help_search: String::new(),
            log_qso_requested: false,
            vmail_relay_alert: None,
            away_message: String::new(),
            connection_start: None,
            snr_last: None,
            snr_avg: None,
            snr_mine: None,
            snr_samples: Vec::new(),
            broadcast_compose_open: false,
            broadcast_text: String::new(),
            broadcast_to: "ALL".into(),
            broadcast_via: String::new(),
            broadcast_snr_request: false,
            broadcast_qsy_khz: String::new(),
            path_finder_target: None,
            path_finder_sent: None,
            file_send_open: false,
            file_path_input: String::new(),
            currently_sending: None,
            queue_count: 0,
            queue_lines: Vec::new(),
            queue_open: false,
            outbox_count: 0,
            parking_count: 0,
            email_gate_pill: None,
            email_gate_toggle: false,
            cq_active: false,
            auto_cq_enabled: false,
            auto_cq_interval_secs: 30,
            last_cq_sent: None,
            auto_log_qso: true,
            is_typing_enabled: true,
            path_finder_open: false,
            path_finder_callsign: String::new(),
            last_typing_sent: None,
            last_activity: Utc::now(),
            auto_away_triggered: false,
            beacon_auto_enabled: false,
            beacon_started_at: None,
            beacon_dial: 0,
            beacon_freq_changed_at: None,
            beacon_was_on: false,
            last_beacon_sent: None,
            current_band: "20m".into(),
            current_frequency: 14_105_000,
            // VarAC's defaults: <SND> sends at once, <DISC> disconnects after sending
            canned_messages: {
                let m = |label: &str, text: &str| CannedMessage { label: label.into(), text: text.into(), disconnect_after: false };
                let mut c: [CannedMessage; 12] = std::array::from_fn(|_| CannedMessage::default());
                c[0] = m("MY FULL INFO", "<NAME>//n<QTH>//n<LOC>//n<EMAIL>//n<ICE><SND>");
                c[1] = m("MY SHORT INFO", "<NAME>//n<QTH>//n<LOC><SND>");
                c[2] = m("RIG", "My working conditions://n<RIG>//n<ANT>");
                c[3] = m("73s", "73's and all the best!//nDE <CALL> Disconnecting...<SND><DISC>");
                c
            },
            canned_editor_open: false,
            settings_open: false,
            settings_form: SettingsForm {
                callsign: String::new(),
                name: String::new(),
                qth: String::new(),
                locator: String::new(),
                modem_host: String::new(),
                cmd_port: String::new(),
                data_port: String::new(),
                kiss_port: String::new(),
                bandwidth: String::new(),
                vmail_enabled: false,
                bbs_enabled: false,
                settings_tab: 0,
                beacon_interval: "15".into(),
                auto_log_qso: true,
                auto_away_enabled: false,
                auto_away_minutes: "10".into(),
                allow_pings: true,
                allow_info_request: true,
                load_history: true,
                font_size: "14".into(),
                notifications_enabled: true,
                sound_alerts: true,
                use_local_time: false,
                modem_auto_reconnect: false,
                bbs_directory: String::new(),
                email_gateway: false,
                smtp_server: "smtp.gmail.com".into(),
                smtp_port: "587".into(),
                smtp_security: "starttls".into(),
                smtp_username: String::new(),
                smtp_password: String::new(),
                gateway_email: String::new(),
                email_test_status: None,
                email_test_rx: None,
                email_gateway_limit: false,
                email_gateway_allow: String::new(),
                smtp_retries: "5".into(),
                email_gateway_announce: false,
                email_gateway_auto_relay: false,
                verbose_snr_auto_accept: false,
                allow_last_heard: true,
                call_id_minutes: "10".into(),
                idle_disconnect_minutes: "5".into(),
                welcome_message: String::new(),
                welcome_always: false,
                allow_path_finder: true,
                rotator_enabled: false,
                rotator_host: "127.0.0.1".into(),
                rotator_port: "12000".into(),
                adif_log_path: String::new(),
                adif_submode: String::new(),
                default_log_comment: "Made with Openslot".into(),
                logger_app: "NONE".into(),
                logger_ip: "127.0.0.1".into(),
                logger_port: String::new(),
                logger_app2: "NONE".into(),
                logger_ip2: "127.0.0.1".into(),
                logger_port2: String::new(),
                logger_callsign_lookup: false,
                pskreporter_upload: false,
                pskreporter_self_report: true,
                callsigns_block_list: String::new(),
                unlimited_beacon_time: false,
                hamplay_enabled: true,
                ai_enabled: false,
                ai_api_key: String::new(),
                ai_model: crate::varac::ai_gateway::DEFAULT_MODEL.into(),
                ai_timeout_secs: "30".into(),
                ai_limit_chars: "200".into(),
                ai_chat_context: true,
                ai_limit_per_qso: "0".into(),
                ai_announce: false,
                ai_customization: String::new(),
                dxcluster_enabled: false,
                dxcluster_host: "k1ttt.net".into(),
                dxcluster_port: "7373".into(),
                dxcluster_username: String::new(),
                dxcluster_password: String::new(),
                schedule_start_mode: "LAST_STATE".into(),
                scanner_busy_hold_secs: "0".into(),
                scanner_announce: false,
                vara_path: String::new(),
                vara_launch: false,
                vara_launch_delay_secs: "5".into(),
                emcomm_mode: false,
                emcomm_disable_snr: false,
                emcomm_disable_call_id: false,
                emcomm_allow_last_connections: false,
                emcomm_auto_file_approval: false,
                emcomm_qsy_notify: true,
                start_beacon_on_startup: false,
                suppress_typing_low_snr: true,
                gps_manual: String::new(),
                distance_unit: "MI".into(),
                incoming_files_dir: String::new(),
                digipeat_via: String::new(),
                vara_modem_type: "VaraHF".into(),
                usb_mode: "USB-D".into(),
                ale_snr_threshold: "-18".into(),
                ale_history_days: "3".into(),
                ale_recent_minutes: "60".into(),
                ale_skip_approval: false,
                ale_enabled: false,
                send_relay_notifications: true,
                allow_parking: true,
                encrypt_link: false,
                sign_messages: false,
                use_shared_keys: false,
                use_encryption: false,
                use_aes: false,
                shared_new: Default::default(),
                confirm_new_identity: false,
                incoming_files_size_limit: "5000".into(),
                emcomm_auto_reply_checkin: true,
                email: String::new(),
                full_callsign: String::new(),
                my_rig: String::new(),
                my_power: String::new(),
                my_antenna: String::new(),
                my_age: String::new(),
                my_profession: String::new(),
                my_hobbies: String::new(),
                multi_band_last_heard: false,
                bbs_limit: false,
                bbs_allow: String::new(),
                bbs_announce: false,
                beacon_type: "standard".into(),
                cq_slot_wait_secs: crate::varac::slots::DEFAULT_CQ_SLOT_WAIT_SECS.to_string(),
                skip_cq_slot_selector: false,
                slot_step_hz: crate::varac::slots::DEFAULT_SLOT_STEP_HZ.to_string(),
                imap_enabled: false,
                imap_server: "imap.gmail.com".into(),
                imap_port: "993".into(),
                imap_security: "tls".into(),
                imap_username: String::new(),
                imap_password: String::new(),
                imap_poll_seconds: "300".into(),
                imap_test_status: None,
                imap_test_rx: None,
                qso_alert_minutes: "0".into(),
            },
            log_open: false,
            qso_log_open: false,
            qso_notes_guid: None,
            qso_notes_text: String::new(),
            heard_stations_open: false,
            contacts_open: false,
            contact_edit_callsign: String::new(),
            contact_edit_name: String::new(),
            contact_edit_qth: String::new(),
            contact_edit_locator: String::new(),
            contact_edit_notes: String::new(),
            contact_search: String::new(),
            contact_adding: false,
            chat_search: String::new(),
            chat_search_active: false,
            broadcast_search: String::new(),
            broadcast_search_active: false,
            chat_window_open: false,
            broadcasts_window_open: false,
            broadcasts_share: None,
            chat_grow_on_hover: true,
            cq_filter: String::new(),
            connect_started: None,
            connect_last_try: None,
            connect_timeout_secs: 30,
            font_size: 14.0,
            freq_ref_open: false,
            qso_search: String::new(),
            use_local_time: false,
            modem_auto_reconnect: false,
            modem_reconnect_at: None,
            multiline_input: false,
            send_requested: false,
            about_open: false,
            bbs_listing_open: false,
            bbs_listing_entries: Vec::new(),
            partner_name: None,
            partner_qth: None,
            partner_locator: None,
            partner_services: None,
            partner_typing_at: None,
            pending_qsy: None,
            qsy_dialog_open: false,
            qsy_offset_input: String::new(),
            qsy_invite_mid: false,
            qsy_invite_target: None,
            qsy_custom_khz: String::new(),
            qsy_sniff: None,
            cq_sniff: None,
            watchlist: Vec::new(),
            my_frequencies: Vec::new(),
            freq_entry: String::new(),
            freq_entry_name: String::new(),
            watchlist_open: false,
            watchlist_input: String::new(),
            callsign_tags: std::collections::HashMap::new(),
            callsign_tags_open: false,
            callsign_tag_call_input: String::new(),
            callsign_tag_name_input: String::new(),
            auto_qsy_ranges: default_auto_qsy_ranges(),
            auto_qsy_enabled: false,
            qso_stats_open: false,
            last_connect_callsign: None,
            qso_alert_minutes: 0,
            qso_alert_fired: false,
            dx_alert_enabled: false,
            dark_mode: true,
            scifi_theme: false,
            bookmarked_messages: Vec::new(),
            bookmarks_open: false,
            rig_backend: crate::rig::RigBackend::None,
            rig_connected: false,
            rig_freq: None,
            rig_mode: None,
            rig_panel_open: false,
            rig_extras: Default::default(),
            rig_actions: Vec::new(),
            remote: Default::default(),
            rig_share: crate::config::rig_share_settings(),
            rig_share_port: crate::config::rig_share_settings().port().to_string(),
            rig_share_apply: true,
            rig_share_status: None,
            rig_settings_backend: "none".into(),
            rig_settings_host: "127.0.0.1".into(),
            rig_settings_port: "4532".into(),
            rig_settings_serial: "/dev/ttyUSB0".into(),
            rig_settings_baud: "38400".into(),
            rig_settings_model: "ftdx3000".into(),
            rig_settings_ptt: true,
            vmail_window_open: false,
            listen_mode: "ON".into(),
            rig_test_ptt: None,
            ping_active: false,
            qsy_invite_open: None,
            rig_reconnect_requested: false,
            rig_ptt: false,
            rig_error: None,
            rig_ports: Vec::new(),
            rig_detect_requested: false,
            rig_detect_rx: None,
            rig_detect_status: None,
            rig_detect_auto: false,
            cmd_tx,
        }
    }

    pub fn format_time(&self) -> String {
        if self.use_local_time {
            chrono::Local::now().format("%H:%M:%S").to_string()
        } else {
            Utc::now().format("%H:%M:%S").to_string()
        }
    }

    pub fn add_log(&mut self, msg: String) {
        tracing::info!(target: "gui", "{msg}");
        let ts = self.format_time();
        self.log_lines.push_back(format!("[{ts}] {msg}"));
        if self.log_lines.len() > 500 {
            self.log_lines.pop_front();
        }
    }

    /// VarAC's "REPLIED TO" text: the first 50 characters of line `id` of this QSO.
    pub fn chat_quote(&self, id: u32) -> Option<String> {
        self.chat_lines.iter().rev().find(|l| l.chat_id == Some(id))
            .map(|l| l.text.replace('\n', " ").chars().take(50).collect())
    }

    pub fn add_chat(&mut self, line: ChatLine) {
        self.chat_lines.push_back(line);
        if self.chat_lines.len() > 500 {
            self.chat_lines.pop_front();
        }
    }

    /// On the block list (the callsign without its SSID)?
    pub fn is_blocked(&self, call: &str) -> bool {
        let base = call.split('-').next().unwrap_or(call).trim();
        self.block_list.iter().any(|b| b.eq_ignore_ascii_case(base) || b.eq_ignore_ascii_case(call.trim()))
    }

    pub fn add_system_chat(&mut self, text: String) {
        tracing::info!(target: "gui", "chat: {text}");
        let ts = self.format_time();
        self.add_chat(ChatLine {
            time: ts,
            callsign: String::new(),
            text,
            is_outgoing: false,
            is_system: true,
            reply_to: None,
            ..Default::default()
        });
    }

    /// Stop the tune carrier (and an AUTO run with it).
    pub fn stop_tune(&mut self) {
        if self.auto_tune.take().is_some() {
            self.auto_tune_note = Some(("AUTO stopped".into(), false));
        }
        self.send_cmd(GuiCommand::TuneStop);
    }

    pub fn send_cmd(&self, cmd: GuiCommand) {
        if matches!(cmd, GuiCommand::SendChat(_) | GuiCommand::Connect(_) | GuiCommand::Ping(_)
            | GuiCommand::SendCq { .. } | GuiCommand::SendTypedCq { .. })
        {
            self.activity_bump.set(true);
        }
        // ALE mode (as in VarAC): a connect opens the ALE window instead; its
        // own attempts go straight through
        if let GuiCommand::Connect(ref call) = cmd {
            if self.freq_mode == crate::varac::scheduler::Mode::Ale && self.settings_form.ale_enabled
                && !self.ale_attempt.replace(false)
            {
                if self.ale.is_none() {
                    *self.ale_request.borrow_mut() = Some(call.clone());
                }
                return;
            }
        }
        let _ = self.cmd_tx.send(cmd);
    }

    /// VarAC refuses CQ, connect, ping and beacon at 2300/2750 Hz on a calling frequency.
    /// True (with the warning shown) when that applies now.
    pub fn refuse_wide_on_calling_freq(&mut self) -> bool {
        // (VARA HF only: FM and SAT have no 500/2300/2750 Hz choice)
        if self.modem_type() != crate::vara::commands::ModemType::Hf {
            return false;
        }
        // (VarAC blocks only 2300 Hz there: 2750 passes its check)
        let wide = self.settings_form.bandwidth.trim() == "2300";
        let on_cf = crate::varac::messages::is_calling_frequency(self.dial());
        if wide && on_cf {
            // (VarAC's message box; ours also offers the switch to 500 Hz)
            self.status_msg = "Bandwidth not allowed on a calling frequency: switch to 500 Hz or change frequency".into();
            self.bw_refused = true;
            return true;
        }
        false
    }

    /// The bandwidth our CQs and beacons announce: the modem's.
    pub fn cq_bandwidth(&self) -> u16 {
        self.settings_form.bandwidth.trim().parse().unwrap_or(500)
    }

    // === VarAC slot system ===

    pub fn slot_step(&self) -> i64 {
        // (VarAC: VARA SAT slots are 2550 Hz apart)
        let default = if self.settings_form.vara_modem_type == "VaraSAT" { 2550 } else { crate::varac::slots::DEFAULT_SLOT_STEP_HZ };
        self.settings_form.slot_step_hz.trim().parse().ok().filter(|&s: &i64| s > 0).unwrap_or(default)
    }

    pub fn modem_type(&self) -> crate::vara::commands::ModemType {
        crate::vara::commands::ModemType::from_key(&self.settings_form.vara_modem_type)
    }

    pub fn slot_wait_secs(&self) -> i64 {
        self.settings_form.cq_slot_wait_secs.trim().parse().unwrap_or(crate::varac::slots::DEFAULT_CQ_SLOT_WAIT_SECS) as i64
    }

    /// VarAC's BBS file list: asking for the list, or a file, needs a link
    /// and no calling frequency; the refusal box to show otherwise.
    pub fn bbs_refusal(&self, download: bool) -> Option<(String, String)> {
        let connected = self.session_state == SessionState::Connected;
        let on_cf = crate::varac::messages::is_calling_frequency(self.dial());
        if download && !connected {
            return Some(("Operation can not be performed now".into(), "You are currently not connected.".into()));
        }
        if download && on_cf {
            return Some(("You are on a calling frequency".into(), "You cannot download BBS files on a calling frequency. Please QSY first.".into()));
        }
        (!connected || on_cf).then(|| (
            "BBS file list is not available".into(),
            "The BBS file list displays the files available for download from the station you are currently connected to. It is shown only when:\n\n1. You are actively connected to a station that operates a BBS.\n\n2. You\u{2019}re not on an official VarAC calling frequency.".into(),
        ))
    }

    /// VarAC switching the operation mode to or from EmComm: EmComm turns on unattended
    /// links and auto-logging; leaving it puts the beacon interval back to its minimum.
    pub fn set_emcomm(&mut self, on: bool) {
        self.settings_form.emcomm_mode = on;
        if on {
            self.unattended_links = true;
            self.send_cmd(GuiCommand::SetUnattended(true));
            self.settings_form.auto_log_qso = true;
            self.auto_log_qso = true;
        } else {
            self.settings_form.beacon_interval = if self.settings_form.vara_modem_type == "VaraFM" { "5" } else { "15" }.into();
        }
    }

    /// Change VARA HF's bandwidth (VarAC's 500 / 2300 / 2750 buttons).
    pub fn set_bandwidth(&mut self, hz: &str) {
        use crate::vara::commands::Bandwidth;
        let bw = match hz {
            "500" => Bandwidth::Bw500,
            "2300" => Bandwidth::Bw2300,
            "2750" => Bandwidth::Bw2750,
            _ => return,
        };
        self.send_cmd(GuiCommand::SetBandwidth(bw));
        self.settings_form.bandwidth = hz.to_string();
        self.add_log(format!("VARA bandwidth set to {hz} Hz"));
    }

    /// Keep the frequency list in the settings file right away.
    pub fn save_my_frequencies(&self) {
        let list = self.my_frequencies.clone();
        crate::config::update_config_file(|f| f.my_frequencies = Some(list));
    }

    /// The radio mode VarAC sets before a frequency change (UsbMode).
    pub fn rig_usb_mode(&self) -> crate::rig::RadioMode {
        match self.settings_form.usb_mode.as_str() {
            "USB" => crate::rig::RadioMode::Usb,
            "FM" => crate::rig::RadioMode::Fm,
            "FM-D" => crate::rig::RadioMode::FmData,
            _ => crate::rig::RadioMode::UsbData,
        }
    }

    /// Where the radio is now (CAT), else the calling frequency we selected.
    pub fn dial(&self) -> u64 {
        self.rig_freq.filter(|_| self.rig_connected).unwrap_or(self.current_frequency)
    }

    /// Call CQ naming `slot` (0 = no slot), then move there once it's on the air.
    pub fn call_cq_on_slot(&mut self, slot: u8) {
        if self.refuse_wide_on_calling_freq() {
            return;
        }
        let source = crate::varac::slots::cq_source(&self.my_callsign, slot);
        let bandwidth = self.cq_bandwidth();
        let cq_type = self.cq_type.trim().to_uppercase();
        // VarAC sends the CQ twice, waiting after each: 10 s for a standard CQ on
        // VARA HF, 6 s on FM / SAT, 9 s for a typed CQ; only then does it move to the slot
        let (kind, wait) = if cq_type.is_empty() || cq_type == "STANDARD" {
            (CqKind::Standard { source: source.clone(), bandwidth }, if self.modem_type() == crate::vara::commands::ModemType::Hf { 10 } else { 6 })
        } else {
            // (VarAC: any other type is a KISS frame that carries our locator)
            (CqKind::Typed { slot, cq_type: cq_type.clone() }, 9)
        };
        self.send_cmd(kind.command());
        self.cq_seq = Some(CqSeq { kind, wait_secs: wait, second: false, sent_at: Utc::now(), next_at: None });
        self.cq_active = true;
        self.last_cq_sent = Some(Utc::now());
        self.pending_slot_qsy = (slot != 0).then(|| (slot, self.current_frequency, Utc::now()));
        self.add_log(format!("CQ sent as {source}{}", if slot != 0 {
            format!(" — standing by on {} afterwards", crate::varac::slots::label(slot, self.slot_step()))
        } else {
            String::new()
        }));
    }

    /// The CQ is on the air: retune to its slot and wait there for a reply.
    pub fn start_slot_wait(&mut self) {
        let Some((slot, cf, _)) = self.pending_slot_qsy.take() else { return };
        let Some(f) = crate::varac::slots::frequency(cf, slot, self.slot_step()) else { return };
        self.tune_to = Some(f);
        let until = Utc::now() + chrono::Duration::seconds(self.slot_wait_secs());
        self.slot_wait = Some((slot, cf, until));
        if !self.rig_connected {
            self.status_msg = format!("No rig control: tune your radio to {:.3} MHz (slot {slot})", f as f64 / 1e6);
        }
        self.add_log(format!("QSY to slot {slot} ({:.3} MHz), waiting for replies", f as f64 / 1e6));
    }

    /// The VMails of the open folder tab (Inbox, Sent, Outbox, Parking, Drafts): the list
    /// and the reader must agree, or a row opens another folder's VMail.
    pub fn vmail_folder(&self) -> &[VMailEntry] {
        match self.vmail_folder_tab {
            1 => &self.vmail_sent_list,
            2 => &self.vmail_outbox_list,
            3 => &self.vmail_parking_list,
            4 => &self.vmail_drafts_list,
            _ => &self.vmail_list,
        }
    }

    /// Leave the slot: back to the calling frequency.
    pub fn return_to_cf(&mut self) {
        let cf = self.slot_wait.take().map(|(_, cf, _)| cf).unwrap_or(self.current_frequency);
        self.pending_slot_qsy = None;
        self.tune_to = Some(cf);
        self.cq_over();
        self.add_log(format!("Back on the calling frequency {:.3} MHz", cf as f64 / 1e6));
    }

    /// The CQ has run its course (slot wait over, or a CQ without a slot sent twice): it
    /// stays active only for Auto-CQ, which calls again from here.
    fn cq_over(&mut self) {
        if !self.auto_cq_enabled {
            self.cq_active = false;
        }
    }

    /// Answer a CQ that named a slot: retune there, then connect (as in
    /// VarAC).
    /// VarAC's "QSY to slot" in the CQ menu: tune to the slot the CQ named, without calling.
    pub fn qsy_to_slot(&mut self, call: &str, slot: u8) {
        let Some(f) = crate::varac::slots::frequency(self.current_frequency, slot, self.slot_step()) else { return };
        self.remember_cf(self.current_frequency);
        self.tune_to = Some(f);
        self.add_log(format!("QSY to slot {slot} ({:.3} MHz), where {call} waits", f as f64 / 1e6));
        if !self.rig_connected {
            self.status_msg = format!("No rig control: tune your radio to {:.3} MHz (slot {slot})", f as f64 / 1e6);
        }
    }

    pub fn connect_on_slot(&mut self, call: &str, slot: u8) {
        let Some(f) = crate::varac::slots::frequency(self.current_frequency, slot, self.slot_step()) else { return };
        self.remember_cf(self.current_frequency);
        self.tune_to = Some(f);
        self.pending_connect = Some((call.to_string(), Utc::now() + chrono::Duration::milliseconds(1500)));
        self.add_log(format!("QSY to slot {slot} ({:.3} MHz) to answer {call}", f as f64 / 1e6));
        if !self.rig_connected {
            self.status_msg = format!("No rig control: tune your radio to {:.3} MHz (slot {slot}) before the call goes out", f as f64 / 1e6);
        }
    }

    /// VarAC's "Wrong band" check before connecting a station from the CQ / beacon lists:
    /// true (and the message box shown) when it was last heard on another band.
    pub fn refuse_wrong_band(&mut self, call: &str) -> bool {
        let base = call.split('-').next().unwrap_or(call).to_uppercase();
        let heard = self.cq_entries.iter().chain(self.beacon_entries.iter())
            .filter(|e| e.callsign.split('-').next().unwrap_or(&e.callsign).eq_ignore_ascii_case(&base))
            .max_by_key(|e| e.timestamp)
            .map(|e| e.band.clone())
            .filter(|b| !b.is_empty());
        match heard {
            Some(band) if !band.eq_ignore_ascii_case(&self.current_band) => {
                self.tx_alert = Some((
                    "Wrong band".into(),
                    format!(
                        "You are trying to connect a station on {band}\nwhile you are currently on {}.\n\nSwitch bands before trying to connect.",
                        self.current_band
                    ),
                ));
                true
            }
            _ => false,
        }
    }

    /// Our profile as Settings has it (for INFO and message tags).
    pub fn profile(&self) -> crate::varac::messages::Profile {
        let f = &self.settings_form;
        crate::varac::messages::Profile {
            name: f.name.clone(),
            qth: f.qth.clone(),
            locator: f.locator.clone(),
            email: f.email.clone(),
            rig: f.my_rig.clone(),
            power: f.my_power.clone(),
            antenna: f.my_antenna.clone(),
            age: f.my_age.clone(),
            profession: f.my_profession.clone(),
            hobbies: f.my_hobbies.clone(),
        }
    }

    /// An outgoing chat line as VarAC sends it: tags expanded (messages::expand_tags).
    pub fn expand_tags(&self, text: &str) -> String {
        let me = self.profile();
        let full = self.settings_form.full_callsign.trim();
        let my_call = if full.is_empty() { self.my_callsign.as_str() } else { full };
        crate::varac::messages::expand_tags(text, &crate::varac::messages::TagContext {
            my_call,
            me: &me,
            his_call: self.connected_to.as_deref().unwrap_or(""),
            his_name: self.partner_name.as_deref().unwrap_or(""),
            his_qth: self.partner_qth.as_deref().unwrap_or(""),
            his_loc: self.partner_locator.as_deref().unwrap_or(""),
        })
        // <FS>: the frequency scheduler / scanner list (empty when off)
        .replace("<FS>", &match self.freq_mode {
            crate::varac::scheduler::Mode::Inactive => String::new(),
            m => crate::varac::scheduler::fsr_answer(m, &self.schedule_rules, &self.scan_list),
        })
    }

    /// Beacon source: CALL-9, or CALL-8 in EmComm mode (VarAC).
    pub fn beacon_source(&self) -> String {
        let base = self.my_callsign.split('-').next().unwrap_or(&self.my_callsign);
        format!("{base}-{}", if self.settings_form.emcomm_mode { 8 } else { 9 })
    }

    /// Note that the QSO is leaving `freq`, if it's a calling frequency.
    pub fn remember_cf(&mut self, freq: u64) {
        if self.qsy_return.is_none() && crate::varac::messages::is_calling_frequency(freq) {
            self.qsy_return = Some(freq);
        }
    }

    /// Retune to `freq` and make it the working (calling) frequency.
    pub fn go_to_frequency(&mut self, freq: u64) {
        self.current_frequency = freq;
        if let Some(band) = crate::gui::freq_to_band(freq) {
            self.current_band = band.to_string();
        }
        self.tune_to = Some(freq);
        self.add_log(format!("Back on the calling frequency {:.3} MHz", freq as f64 / 1e6));
    }

    /// After a QSO that moved off a calling frequency: offer to go back (VarAC).
    pub fn offer_return_to_cf(&mut self) {
        let Some(cf) = self.qsy_return.take() else { return };
        if self.dial() == cf || self.freq_mode.scanning() {
            return;
        }
        let auto = self.auto_qsy_enabled && self.rig_connected;
        let until = Utc::now() + chrono::Duration::seconds(if auto { 10 } else { 30 });
        self.return_cf_prompt = Some((cf, until, auto));
        self.add_log(format!("QSO over: asking to go back to the calling frequency {:.3} MHz ({} in {} s)",
            cf as f64 / 1e6, if auto { "yes" } else { "no" }, (until - Utc::now()).num_seconds().max(0) + 1));
    }

    /// As in VarAC: inside one of the allowed ranges, on
    /// the band we're on now (an empty list allows anything).
    /// A range counts only when it's on the band
    /// we're on; no ranges = nothing allowed.
    pub fn qsy_target_allowed(&self, freq_hz: u64) -> bool {
        let band = crate::varac::messages::band_code(self.dial());
        self.auto_qsy_ranges.iter()
            .any(|(lo, hi)| crate::varac::messages::band_code(*lo) == band && (*lo..=*hi).contains(&freq_hz))
    }

    /// Open VarAC's "QSY NOW" countdown for a move to `target_hz` (default: accept in 10 s
    /// with auto-QSY on and the target allowed; otherwise reject in 30 s).
    pub fn open_qsy_popup(&mut self, target_hz: u64, recovery: bool) {
        let auto = self.auto_qsy_enabled;
        let allowed = self.qsy_target_allowed(target_hz);
        let default_accept = auto && allowed;
        let secs = if default_accept { 10 } else { 30 };
        self.qsy_popup = Some(QsyPopup {
            target_hz,
            offset_hz: (target_hz as i64 - self.dial() as i64) as i32,
            default_accept,
            out_of_range: auto && !allowed,
            auto,
            until: Utc::now() + chrono::Duration::seconds(secs),
            recovery,
        });
    }

    pub fn is_qsy_freq_allowed(&self, freq_hz: u64) -> bool {
        self.qsy_target_allowed(freq_hz)
    }

    pub fn record_snr(&mut self, db: f32) {
        if self.session_state == SessionState::Connected {
            self.snr_his_series.push((Utc::now(), db));
        }
        self.snr = Some(db);
        self.snr_last = Some(db);
        self.snr_samples.push(db);
        let sum: f32 = self.snr_samples.iter().sum();
        self.snr_avg = Some(sum / self.snr_samples.len() as f32);
    }

    pub fn reset_snr_tracking(&mut self) {
        self.snr = None;
        self.snr_last = None;
        self.snr_avg = None;
        self.snr_mine = None;
        self.snr_samples.clear();
    }

    /// How the partner hears us (their <R±NN> reports), also for the SNR chart.
    pub fn set_snr_mine(&mut self, db: f32) {
        self.snr_mine = Some(db);
        if self.session_state == SessionState::Connected {
            self.snr_my_series.push((Utc::now(), db));
        }
    }
}

pub fn default_auto_qsy_ranges() -> Vec<(u64, u64)> {
    vec![
        (14_101_250, 14_108_750),
        (7_101_250, 7_108_750),
        (1_991_250, 1_998_000),
        (3_591_250, 3_598_000),
        (10_129_250, 10_136_750),
        (18_103_250, 18_110_750),
        (21_101_250, 21_108_750),
        (24_923_250, 24_930_750),
        (28_101_250, 28_108_750),
        (50_326_250, 50_333_750),
        (144_000_000, 146_000_000),
        (430_000_000, 440_000_000),
    ]
}

pub fn parse_qsy_ranges(ranges: &[String]) -> Vec<(u64, u64)> {
    ranges.iter().filter_map(|s| {
        let parts: Vec<&str> = s.split('-').collect();
        if parts.len() == 2 {
            let lo: u64 = parts[0].replace('.', "").parse().ok()?;
            let hi: u64 = parts[1].replace('.', "").parse().ok()?;
            Some((lo, hi))
        } else {
            None
        }
    }).collect()
}

impl GuiState {
    pub fn rig_backend_from_settings(&self) -> crate::rig::RigBackend {
        crate::rig::RigBackend::from_settings(
            &self.rig_settings_backend,
            &self.rig_settings_host,
            &self.rig_settings_port,
            &self.rig_settings_serial,
            &self.rig_settings_baud,
            &self.rig_settings_model,
        )
    }
}

// === HamPlay ===
impl GuiState {
    fn hamplay_partner(&self) -> String {
        self.connected_to.clone().unwrap_or_default()
    }

    /// Game center: invite the partner. The inviter is player 1 and moves first.
    pub fn hamplay_invite(&mut self, game: crate::varac::hamplay::Game) {
        if self.session_state != SessionState::Connected || self.connected_to.is_none() {
            self.status_msg = "You must connect to someone first.".into();
            self.add_system_chat("HamPlay: you must connect to someone first.".into());
            return;
        }
        if crate::varac::messages::is_calling_frequency(self.dial()) {
            self.add_system_chat("Playing on the calling frequency is not allowed. Please QSY to another frequency first.".into());
            return;
        }
        let opponent = self.hamplay_partner();
        self.send_cmd(GuiCommand::SendInquiry(format!("<P:{}>", game.id())));
        self.hamplay = Some(HamPlaySession {
            board: crate::varac::hamplay::Board::new(game),
            me: 1,
            active: false,
            invited: false,
            status: format!("HamPlay Invitation sent.\nWaiting for {opponent} to join the game..."),
            opponent,
            open: true,
        });
        self.add_system_chat(format!("HAMPLAY: {} invitation sent", game.name()));
    }

    /// ACCEPT (an invitation or a rematch): we are player 2.
    pub fn hamplay_accept(&mut self) {
        let Some(g) = self.hamplay.as_mut() else { return };
        if !g.invited {
            return;
        }
        g.board = crate::varac::hamplay::Board::new(g.board.game);
        (g.me, g.active, g.invited) = (2, true, false);
        g.status = format!("Invitation accepted.\nWait for {} move...", g.opponent);
        self.send_cmd(GuiCommand::SendInquiry("<PR>".into()));
    }

    /// A click on the board (row, column).
    pub fn hamplay_click(&mut self, row: usize, col: usize) {
        let Some(g) = self.hamplay.as_mut() else { return };
        if !g.my_turn() {
            return;
        }
        let Some(mv) = g.board.move_for_click(row, col) else { return };
        g.board.play(&mv);
        let send = format!("<PM:{mv}>");
        self.hamplay_after_move();
        self.send_cmd(GuiCommand::SendInquiry(send));
    }

    /// REMATCH: we ask, so we become player 1 (and move first once accepted).
    pub fn hamplay_rematch(&mut self) {
        let Some(g) = self.hamplay.as_mut() else { return };
        if g.active && g.board.outcome.is_none() {
            return;
        }
        g.board = crate::varac::hamplay::Board::new(g.board.game);
        (g.me, g.active, g.invited) = (1, false, false);
        g.status = "Rematch invitation sent...".into();
        self.send_cmd(GuiCommand::SendInquiry("<PE>".into()));
    }

    /// Closing the game while it's on tells the partner we left (`<PA>`).
    pub fn hamplay_close(&mut self) {
        if let Some(g) = self.hamplay.take() {
            if self.session_state != SessionState::Connected {
                return;
            }
            if g.active && g.board.outcome.is_none() {
                self.send_cmd(GuiCommand::SendInquiry("<PA>".into()));
            } else if g.invited {
                // closing an invitation declines it (else the inviter waits)
                self.send_cmd(GuiCommand::SendInquiry("<PJ>".into()));
            }
        }
    }

    fn hamplay_after_move(&mut self) {
        let me_call = self.my_callsign.clone();
        let Some(g) = self.hamplay.as_mut() else { return };
        use crate::varac::hamplay::Outcome;
        g.status = match g.board.outcome {
            Some(Outcome::Draw) => "It's a draw!".into(),
            Some(Outcome::Win(p)) if p == g.me => format!("{me_call} Wins!"),
            Some(Outcome::Win(_)) => format!("{} Wins!\nYou lose!", g.opponent),
            None if g.my_turn() => "Your turn.\nMake a move.".into(),
            None => format!("{} turn.\nWait for their move.", g.opponent),
        };
        crate::gui::sound::play(match g.board.outcome {
            Some(Outcome::Draw) => crate::gui::sound::Sound::GameDraw,
            Some(Outcome::Win(p)) if p == g.me => crate::gui::sound::Sound::GameWin,
            Some(Outcome::Win(_)) => crate::gui::sound::Sound::GameLose,
            None => crate::gui::sound::Sound::GameMove,
        });
        if let Some(o) = g.board.outcome {
            let text = match o {
                Outcome::Draw => "HAMPLAY: it's a draw".to_string(),
                Outcome::Win(p) if p == g.me => "HAMPLAY: you win!".to_string(),
                Outcome::Win(_) => format!("HAMPLAY: {} wins", g.opponent),
            };
            g.active = false;
            self.add_system_chat(text);
        }
    }

    /// The partner's HamPlay messages.
    pub fn on_hamplay(&mut self, m: crate::varac::messages::HamPlayMsg) {
        use crate::varac::hamplay::{Board, Game};
        use crate::varac::messages::HamPlayMsg;
        let opponent = self.hamplay_partner();
        let double = |last: Option<chrono::DateTime<Utc>>| last.is_some_and(|t| (Utc::now() - t).num_seconds() < 45);
        match m {
            HamPlayMsg::Invite(_) | HamPlayMsg::Rematch if !self.settings_form.hamplay_enabled => {
                self.send_cmd(GuiCommand::SendInquiry("<PJ>".into()));
                self.add_log("HamPlay invitation rejected: HamPlay is off".into());
            }
            HamPlayMsg::Invite(_) | HamPlayMsg::Rematch if double(self.hamplay_last_invite) => {
                self.add_system_chat("<< DOUBLE HAMPLAY GAME REQUEST IN 45 SECONDS. IGNORING. >>".into());
            }
            HamPlayMsg::Invite(id) => {
                let Some(game) = Game::from_id(id) else {
                    self.add_log(format!("Unable to find game ID: {id}"));
                    self.send_cmd(GuiCommand::SendInquiry("<PJ>".into()));
                    return;
                };
                self.hamplay_last_invite = Some(Utc::now());
                self.add_system_chat(format!("{opponent} INVITES YOU TO PLAY {} in HamPlay.", game.name().to_uppercase()));
                self.hamplay = Some(HamPlaySession {
                    board: Board::new(game),
                    me: 2,
                    active: false,
                    invited: true,
                    status: format!("{opponent} invites you to play {}.\nClick ACCEPT to join.", game.name()),
                    opponent,
                    open: true,
                });
            }
            HamPlayMsg::Rematch => {
                self.hamplay_last_invite = Some(Utc::now());
                let Some(g) = self.hamplay.as_mut() else { return };
                g.board = Board::new(g.board.game);
                (g.me, g.active, g.invited, g.open) = (2, false, true, true);
                g.status = format!("{opponent} has challenged you to a rematch.\nClick ACCEPT to play.");
                self.add_system_chat(format!("{opponent} has challenged you to a rematch"));
            }
            HamPlayMsg::Accept => {
                let Some(g) = self.hamplay.as_mut() else { return };
                if g.me == 1 && !g.active {
                    g.board = Board::new(g.board.game);
                    g.active = true;
                    g.status = format!("{opponent} accepted your invitation.\nYour move.");
                }
            }
            HamPlayMsg::Move(mv) => {
                let ok = self.hamplay.as_mut().is_some_and(|g| g.active && g.board.to_move != g.me && g.board.play(&mv));
                if ok {
                    self.hamplay_after_move();
                } else {
                    self.add_log(format!("HamPlay: move {mv} ignored"));
                }
            }
            HamPlayMsg::Reject => {
                self.add_system_chat(format!("{opponent} REJECTED YOUR HAMPLAY INVITATION."));
                self.hamplay = None;
            }
            HamPlayMsg::Abort => {
                self.add_system_chat(format!("{opponent} ABORTED THE GAME."));
                if let Some(g) = self.hamplay.as_mut() {
                    g.active = false;
                    g.invited = false;
                    g.status = format!("{opponent} has left the game.\nClick Close to exit the game.");
                }
            }
        }
    }
}


/// Where the alert tags file lives: next to the config file.
pub fn alert_tags_path() -> Option<std::path::PathBuf> {
    crate::config::config_dir().map(|d| d.join(crate::varac::alerts::FILE_NAME))
}

pub fn load_alert_tags() -> Vec<crate::varac::alerts::AlertTag> {
    alert_tags_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|t| crate::varac::alerts::parse(&t))
        .unwrap_or_default()
}

fn load_sched_file(name: &str) -> Option<String> {
    crate::config::config_dir().and_then(|d| std::fs::read_to_string(d.join(name)).ok())
}

/// The scheduler editor's rows, as typed.
#[derive(Default, Clone)]
pub struct SchedEditor {
    /// (time HH:MM, frequency)
    pub rules: Vec<(String, String)>,
    pub scan: Vec<ScanRow>,
}

#[derive(Default, Clone)]
pub struct ScanRow {
    pub freq: String,
    pub wait: u32,
    pub beacon: bool,
    pub tune: bool,
    pub enabled: bool,
    pub start: String,
    pub end: String,
}

// === Frequency scheduler / scanner (VarAC) ===
impl GuiState {
    /// Switch the scheduler mode (button, startup). Scanner mode takes over the beacons.
    pub fn set_freq_mode(&mut self, mode: crate::varac::scheduler::Mode) {
        use crate::varac::scheduler::Mode;
        if mode == self.freq_mode {
            return;
        }
        self.freq_mode = mode;
        self.sched_queued = None;
        self.sched_notify_until = None;
        self.sched_last_check = None;
        match mode {
            Mode::Scheduler => self.add_log("FREQ Schedule enabled".into()),
            Mode::Scanner | Mode::Ale => {
                self.scanner.reset(self.scan_list.len());
                self.scan_busy_seen = false;
                self.beacon_auto_enabled = false; // VarAC: beacons managed by the scanner
                self.add_log(if mode == Mode::Ale { "ALE enabled" } else { "FREQ scanner enabled" }.into());
            }
            Mode::Inactive => self.add_log("FREQ Schedule disabled".into()),
        }
        self.fsr_dirty = true;
        let key = mode.key().to_string();
        crate::config::update_config_file(|c| c.schedule_last_state = Some(key));
    }

    /// Our broadcast went out of the queue: the status line said it was waiting (it stayed
    /// saying so after it was sent). A newer status line is left alone.
    pub fn broadcast_went_out(&mut self, to: &str) {
        if self.status_msg == BROADCAST_QUEUED {
            self.status_msg = format!("Broadcast sent to {to}");
        }
    }

    /// The modem is calling a station for us (SessionMsg::Dialing): CONNECTING until it
    /// answers or the modem gives up, with the connect timeout counting from now.
    pub fn dialing(&mut self) {
        if self.session_state == SessionState::Idle {
            self.session_state = SessionState::Connecting;
            self.connect_started = Some(Utc::now());
            self.connect_last_try = self.connect_started;
        }
    }

    /// The connect timeout: a call the modem has stopped trying, still unanswered and not
    /// ended by the modem after connect_timeout_secs, is aborted and the station is idle
    /// again (the modem's DISCONNECTED, if it comes, changes nothing). The clock runs from
    /// the modem's last attempt, not from the dialing: while it keys up to call, the call is
    /// alive and the modem's own retries decide how long it lasts, as in VarAC (VARA HF's
    /// 15 retries run well past the 30 s default, and cutting them lost weak-signal calls).
    pub fn check_connect_timeout(&mut self, now: chrono::DateTime<Utc>) {
        let Some(_) = self.connect_started else { return };
        if self.session_state != SessionState::Connecting {
            // (answered, or not dialing yet: SessionMsg::Dialing starts the clock again)
            self.connect_started = None;
            self.connect_last_try = None;
            return;
        }
        if self.ptt {
            self.connect_last_try = Some(now);
        }
        let last = self.connect_last_try.unwrap_or(now);
        let timeout = self.connect_timeout_secs.clamp(10, 120) as i64;
        if (now - last).num_seconds() >= timeout {
            self.connect_started = None;
            self.connect_last_try = None;
            self.session_state = SessionState::Idle;
            self.status_msg = "Connection timed out — aborting".into();
            self.add_log(format!("Connection timeout after {timeout}s — sending abort"));
            self.send_cmd(GuiCommand::Abort);
        }
    }

    /// Something that pauses the scanner (and defers a scheduled QSY): a QSO or connect,
    /// a CQ or beacon on the air, a ping, a slot wait, a tune.
    pub fn freq_mode_busy(&self) -> bool {
        self.ale.is_some()
            || self.cq_seq.is_some()
            || self.session_state != SessionState::Idle
            || self.ptt
            || self.ping_active
            || self.pending_connect.is_some()
            || self.pending_slot_qsy.is_some()
            || self.slot_wait.is_some()
            || self.broadcast_compose_open
    }

    /// The editor, filled from the current lists.
    pub fn new_sched_editor(&self) -> SchedEditor {
        use crate::varac::scheduler::dotted;
        SchedEditor {
            rules: self.schedule_rules.iter().map(|r| (r.at.format("%H:%M").to_string(), dotted(r.freq_hz))).collect(),
            scan: self.scan_list.iter().map(|e| ScanRow {
                freq: dotted(e.freq_hz),
                wait: e.wait_secs,
                beacon: e.beacon,
                tune: e.tune,
                enabled: e.enabled,
                start: e.window.map(|w| w.0.format("%H:%M").to_string()).unwrap_or_default(),
                end: e.window.map(|w| w.1.format("%H:%M").to_string()).unwrap_or_default(),
            }).collect(),
        }
    }

    /// Save the editor's lists (VarAC's files, next to the config file).
    pub fn save_sched_editor(&mut self, ed: &SchedEditor) -> Result<(), String> {
        use crate::varac::scheduler as sch;
        let sched_text: String = ed.rules.iter()
            .filter(|(t, f)| !t.trim().is_empty() && !f.trim().is_empty())
            .map(|(t, f)| format!("{},{}\r\n", t.trim(), f.trim()))
            .collect();
        let rules = sch::parse_schedule(&sched_text);
        let scan: Vec<sch::ScanEntry> = ed.scan.iter().filter(|r| !r.freq.trim().is_empty()).filter_map(|r| {
            let freq_hz = sch::parse_freq(&r.freq)?;
            let window = match (chrono::NaiveTime::parse_from_str(r.start.trim(), "%H:%M"), chrono::NaiveTime::parse_from_str(r.end.trim(), "%H:%M")) {
                (Ok(a), Ok(b)) => Some((a, b)),
                _ => None,
            };
            let mut e = sch::ScanEntry { freq_hz, wait_secs: r.wait, beacon: r.beacon, tune: r.tune, enabled: r.enabled, window };
            e.wait_secs = e.wait_secs.max(e.min_wait());
            Some(e)
        }).take(sch::MAX_SCAN).collect();
        let dir = crate::config::config_dir().ok_or("no config folder")?;
        std::fs::write(dir.join(sch::SCHEDULE_FILE), sch::serialize_schedule(&rules)).map_err(|e| e.to_string())?;
        std::fs::write(dir.join(sch::SCANNER_FILE), sch::serialize_scanner(&scan)).map_err(|e| e.to_string())?;
        self.schedule_rules = rules;
        self.scan_list = scan;
        self.scanner.reset(self.scan_list.len());
        self.fsr_dirty = true;
        if self.freq_mode == sch::Mode::Scheduler && self.schedule_rules.is_empty()
            || self.freq_mode == sch::Mode::Scanner && !self.scan_list.iter().any(|e| e.enabled)
        {
            self.set_freq_mode(sch::Mode::Inactive);
        }
        Ok(())
    }

    /// Retune (scheduler / scanner)
    pub fn tune_for_schedule(&mut self, f: u64) {
        self.current_frequency = f;
        if let Some(b) = crate::gui::freq_to_band(f) {
            self.current_band = b.to_string();
        }
        self.tune_to = Some(f);
    }
}

/// The image shrinker's settings and its last result.
pub struct Shrinker {
    pub path: std::path::PathBuf,
    pub original: Vec<u8>,
    pub size: (u32, u32),
    /// 1–100 %
    pub pct: u32,
    /// VarAC's quality 0–50
    pub quality: u8,
    pub overlay: bool,
    /// (JPEG, width, height) for the current settings
    pub result: Option<Result<(Vec<u8>, u32, u32), String>>,
    pub dirty: bool,
}

/// The QSL card being made.
pub struct QslWindow {
    pub card: crate::varac::images::QslCard,
    pub background: String,
    pub fade: bool,
    pub texture: Option<eframe::egui::TextureHandle>,
    pub dirty: bool,
    pub error: Option<String>,
}

impl GuiState {
    /// A QSL card for a QSO: (callsign, SNR sent, SNR received, band, start, partner's name).
    pub fn open_qsl(&mut self, call: &str, rst_s: &str, rst_r: &str, band: &str, start: chrono::DateTime<Utc>, his_name: &str) {
        let cfg = crate::config::read_qsl();
        let f = &self.settings_form;
        let extra = crate::config::profile_extra();
        let my_lines: Vec<String> = [("Name: ", f.name.clone()), ("QTH: ", f.qth.clone()), ("Locator: ", f.locator.clone()),
            ("Rig: ", extra.rig.replace('\n', ", ")), ("Antenna: ", extra.antenna.clone())]
            .into_iter().filter(|(_, v)| !v.trim().is_empty()).map(|(k, v)| format!("{k}{}", v.trim())).collect();
        let full = f.full_callsign.trim();
        let card = crate::varac::images::QslCard {
            my_call: if full.is_empty() { self.my_callsign.clone() } else { full.to_uppercase() },
            my_lines,
            call: call.to_uppercase(),
            rst_sent: rst_s.to_string(),
            rst_rcvd: rst_r.to_string(),
            band: band.to_string(),
            time_utc: start.format("%b %d, %Y %H:%M:%S").to_string(),
            power: extra.power,
            message: crate::varac::images::qsl_message(&cfg.1, his_name, &f.name),
        };
        self.qsl = Some(QslWindow { card, background: cfg.0, fade: cfg.2, texture: None, dirty: true, error: None });
    }
}

/// A CQ in progress (as in VarAC): sent, a wait, sent again, a wait, done.
#[derive(Clone, Debug)]
pub struct CqSeq {
    pub kind: CqKind,
    pub wait_secs: i64,
    /// the second transmission went out
    pub second: bool,
    pub sent_at: chrono::DateTime<Utc>,
    /// after the current transmission: send again / finish at this time
    pub next_at: Option<chrono::DateTime<Utc>>,
}

#[derive(Clone, Debug)]
pub enum CqKind {
    Standard { source: String, bandwidth: u16 },
    Typed { slot: u8, cq_type: String },
}

impl CqKind {
    pub fn command(&self) -> GuiCommand {
        match self.clone() {
            CqKind::Standard { source, bandwidth } => GuiCommand::SendCq { source, bandwidth },
            CqKind::Typed { slot, cq_type } => GuiCommand::SendTypedCq { slot, cq_type },
        }
    }
}

impl GuiState {
    /// The CQ's transmission ended (PTT released): wait, then the next step.
    pub fn cq_tx_done(&mut self) {
        if let Some(c) = self.cq_seq.as_mut() {
            if c.next_at.is_none() && (Utc::now() - c.sent_at).num_milliseconds() > 800 {
                c.next_at = Some(Utc::now() + chrono::Duration::seconds(c.wait_secs));
            }
        }
    }

    /// Every tick: the second CQ, then the end (the slot stand-by begins).
    pub fn cq_seq_tick(&mut self) {
        let now = Utc::now();
        let Some(c) = self.cq_seq.as_mut() else { return };
        // (no PTT report within 20 s: count the transmission as done)
        if c.next_at.is_none() && (now - c.sent_at).num_seconds() >= 20 {
            c.next_at = Some(now);
        }
        if c.next_at.is_none_or(|t| now < t) {
            return;
        }
        if !c.second {
            c.second = true;
            c.sent_at = now;
            c.next_at = None;
            let cmd = c.kind.command();
            self.send_cmd(cmd);
            self.add_log("CQ sent again (VarAC sends each CQ twice)".into());
        } else {
            self.cq_seq = None;
            self.start_slot_wait();
            if self.slot_wait.is_none() {
                self.cq_over();
            }
        }
    }
}

/// ALC at or under this (of the meter's full scale) counts as "barely moving".
pub const AUTO_TUNE_ALC: f32 = 0.10;

/// AUTO's search for the highest drive level that keeps the ALC under [`AUTO_TUNE_ALC`]:
/// down from the start while the ALC shows, then up 1 dB at a time until it would.
pub struct AutoTune {
    pub level: i32,
    /// The highest level seen with the ALC clear, and the lowest seen over it
    pub best: Option<i32>,
    pub ceiling: Option<i32>,
    pub started: std::time::Instant,
    /// The carrier has been seen on air (since `keyed_wait` it's been asked for)
    pub keyed: bool,
    pub keyed_wait: std::time::Instant,
    pub rekeys: u8,
    /// No reading before this (the level just changed)
    pub settle_until: std::time::Instant,
    /// Readings for this level, and when the one being waited for was asked
    pub readings: Vec<f32>,
    pub asked: Option<std::time::Instant>,
}

/// What a step of AUTO decided.
#[derive(Debug, PartialEq)]
pub enum AutoTuneStep {
    /// Try this level next
    Level(i32),
    /// Done: this level keeps the ALC clear
    Done(i32),
    /// Even the lowest drive shows ALC
    TooHot,
}

impl AutoTune {
    pub const SETTLE: std::time::Duration = std::time::Duration::from_millis(500);
    /// Readings per level (the worst of them counts)
    pub const READS: usize = 2;

    pub fn new(level: i32) -> Self {
        let now = std::time::Instant::now();
        Self {
            level: level.clamp(-30, 0),
            best: None,
            ceiling: None,
            started: now,
            keyed: false,
            keyed_wait: now,
            rekeys: 0,
            settle_until: now,
            readings: Vec::new(),
            asked: None,
        }
    }

    /// The next move from the ALC at the current level.
    pub fn step(&mut self, alc: f32) -> AutoTuneStep {
        if alc <= AUTO_TUNE_ALC {
            self.best = Some(self.level);
            if self.level >= 0 || self.ceiling.is_some_and(|c| self.level + 1 >= c) {
                return AutoTuneStep::Done(self.level);
            }
            self.level += 1;
        } else {
            self.ceiling = Some(self.level);
            if let Some(b) = self.best.filter(|&b| b < self.level) {
                return AutoTuneStep::Done(b);
            }
            if self.level <= -30 {
                return AutoTuneStep::TooHot;
            }
            // Big steps while the ALC is well up, then 1 dB (and back up to the edge)
            let down = if alc > 0.5 { 3 } else if alc > 0.25 { 2 } else { 1 };
            self.level = (self.level - down).max(-30);
        }
        AutoTuneStep::Level(self.level)
    }
}

#[cfg(test)]
mod auto_tune_tests {
    use super::*;

    /// A radio whose ALC starts at `edge` dB and rises 10 % per dB above it.
    fn run(start: i32, edge: i32) -> AutoTuneStep {
        let mut a = AutoTune::new(start);
        for _ in 0..64 {
            let alc = ((a.level - edge) as f32 * 0.1).max(0.0);
            match a.step(alc) {
                AutoTuneStep::Level(_) => {}
                done => return done,
            }
        }
        panic!("AUTO didn't finish");
    }

    #[test]
    fn finds_the_alc_edge() {
        // (the ALC reads 10 % one dB over the edge: still "barely moving")
        assert_eq!(run(-5, -12), AutoTuneStep::Done(-11));
        assert_eq!(run(-20, -12), AutoTuneStep::Done(-11));
        assert_eq!(run(0, -3), AutoTuneStep::Done(-2));
        assert_eq!(run(-10, 5), AutoTuneStep::Done(0));
        assert_eq!(run(-5, -40), AutoTuneStep::TooHot);
    }

    /// Every start and edge across the slider: AUTO finishes within a few steps, on the
    /// highest level whose ALC is clear (the edge itself, or one over it at exactly 10 %),
    /// and never outside -30…0.
    #[test]
    fn auto_tune_always_lands_on_the_edge() {
        for start in -35..=5 {
            for edge in -40..=8 {
                let mut a = AutoTune::new(start);
                let mut steps = 0;
                let alc_at = |l: i32| ((l - edge) as f32 * 0.1).max(0.0);
                let end = loop {
                    assert!((-30..=0).contains(&a.level), "start {start} edge {edge}: level {}", a.level);
                    steps += 1;
                    assert!(steps < 40, "start {start} edge {edge}: no end");
                    match a.step(alc_at(a.level)) {
                        AutoTuneStep::Level(_) => {}
                        done => break done,
                    }
                };
                match end {
                    AutoTuneStep::Done(l) => {
                        assert!(alc_at(l) <= AUTO_TUNE_ALC, "start {start} edge {edge}: {l} dB isn't clear");
                        assert!(l == 0 || alc_at(l + 1) > AUTO_TUNE_ALC, "start {start} edge {edge}: {l} dB isn't the highest clear");
                    }
                    AutoTuneStep::TooHot => assert!(alc_at(-30) > AUTO_TUNE_ALC, "start {start} edge {edge}: TooHot with -30 clear"),
                    AutoTuneStep::Level(_) => unreachable!(),
                }
            }
        }
    }

    /// The download card's speed: over the last minute of deliveries, none before a second.
    #[test]
    fn rx_file_speed() {
        let t0 = std::time::Instant::now();
        let s = |secs: u64, n: usize| (t0 + std::time::Duration::from_secs(secs), n);
        let mut f = RxFile {
            name: "f".into(), from: None, file_bytes: 0, total: 10_000, received: 0,
            started: t0, samples: vec![s(0, 0)], done: None,
        };
        assert_eq!(f.bps(), None, "one sample: no speed yet");
        f.samples.push(s(10, 1000));
        assert_eq!(f.bps(), Some(800.0));
        // (an old slow start falls out of the minute)
        f.samples = vec![s(0, 0), s(100, 100), s(110, 1100)];
        assert_eq!(f.bps(), Some(800.0));
        f.samples = vec![s(0, 500), s(5, 500)];
        assert_eq!(f.bps(), None, "nothing new: no speed");
    }
}

#[cfg(test)]
mod connect_state_tests {
    use super::*;

    fn station() -> (GuiState, tokio::sync::mpsc::UnboundedReceiver<GuiCommand>) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        (GuiState::new("NR5P".into(), true, tx), rx)
    }

    fn sent(rx: &mut tokio::sync::mpsc::UnboundedReceiver<GuiCommand>) -> Vec<GuiCommand> {
        std::iter::from_fn(|| rx.try_recv().ok()).collect()
    }

    /// The modem taking our CONNECT makes the station CONNECTING (it used to stay IDLE, so
    /// the timeout never ran and a second connect or a CQ wasn't refused).
    #[test]
    fn dialing_is_connecting_until_the_timeout() {
        let (mut st, mut rx) = station();
        st.connect_timeout_secs = 30;
        // try_connect's stamp, before the modem has the call: cleared, nothing aborted
        st.connect_started = Some(Utc::now());
        st.check_connect_timeout(Utc::now());
        assert_eq!((st.session_state, st.connect_started), (SessionState::Idle, None));

        st.dialing();
        assert_eq!(st.session_state, SessionState::Connecting);
        let start = st.connect_started.expect("the timeout counts from the dialing");
        assert!(st.freq_mode_busy(), "a call in progress pauses the scanner");
        st.check_connect_timeout(start + chrono::Duration::seconds(29));
        assert_eq!(st.session_state, SessionState::Connecting);
        assert!(sent(&mut rx).is_empty());

        st.check_connect_timeout(start + chrono::Duration::seconds(30));
        assert_eq!((st.session_state, st.connect_started), (SessionState::Idle, None));
        assert!(matches!(sent(&mut rx).as_slice(), [GuiCommand::Abort]));
        assert!(st.status_msg.contains("timed out"));
    }

    /// While the modem keeps keying up to call (VARA HF's retries run past a minute), the
    /// call is left to it; the timeout only ends a call it has stopped trying.
    #[test]
    fn the_modem_retrying_keeps_the_call() {
        let (mut st, mut rx) = station();
        st.connect_timeout_secs = 30;
        st.dialing();
        let start = st.connect_started.unwrap();
        // a connect request every 5 s for 70 s
        for s in (0..=70).step_by(5) {
            st.ptt = true;
            st.check_connect_timeout(start + chrono::Duration::seconds(s));
            st.ptt = false;
            st.check_connect_timeout(start + chrono::Duration::seconds(s + 2));
        }
        assert_eq!(st.session_state, SessionState::Connecting, "still the modem's call");
        assert!(sent(&mut rx).is_empty());
        assert_eq!(st.connect_started, Some(start), "the CONNECTING counter runs on");
        // then silence: 30 s after its last try, it's aborted
        st.check_connect_timeout(start + chrono::Duration::seconds(99));
        assert_eq!(st.session_state, SessionState::Connecting);
        st.check_connect_timeout(start + chrono::Duration::seconds(100));
        assert_eq!(st.session_state, SessionState::Idle);
        assert!(matches!(sent(&mut rx).as_slice(), [GuiCommand::Abort]));
    }

    /// The queued-broadcast status line gives way once it's sent, and only that line.
    #[test]
    fn a_sent_broadcast_is_no_longer_queued() {
        let (mut st, _rx) = station();
        st.status_msg = BROADCAST_QUEUED.into();
        st.broadcast_went_out("ALL");
        assert_eq!(st.status_msg, "Broadcast sent to ALL");
        st.status_msg = "CQ stopped".into();
        st.broadcast_went_out("ALL");
        assert_eq!(st.status_msg, "CQ stopped", "a newer status line stays");
    }

    /// Only an idle station starts dialing: in a QSO (or already calling) nothing changes.
    #[test]
    fn dialing_only_from_idle() {
        let (mut st, _rx) = station();
        st.session_state = SessionState::Connected;
        st.dialing();
        assert_eq!((st.session_state, st.connect_started), (SessionState::Connected, None));
        // answered: the clock stops
        st.session_state = SessionState::Idle;
        st.dialing();
        st.session_state = SessionState::Connected;
        st.check_connect_timeout(Utc::now() + chrono::Duration::seconds(300));
        assert_eq!((st.session_state, st.connect_started), (SessionState::Connected, None));
    }
}
