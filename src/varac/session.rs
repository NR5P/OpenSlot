use crate::vara::connection::VaraConnection;
use crate::vara::events::VaraEvent;
use crate::varac::framing;
use crate::varac::messages::{
    IncomingMessage, OpeningReport, ServiceFlags, VMail,
};
use crate::varac::tags::{self, Tag};
use bytes::BytesMut;
use chrono::Utc;

/// Configuration for a VarAC station.
#[derive(Clone)]
pub struct StationConfig {
    pub callsign: String,
    pub name: Option<String>,
    pub qth: Option<String>,
    pub locator: Option<String>,
    pub services: ServiceFlags,
    /// Folder our BBS serves (None = BBS requests are rejected).
    pub bbs_dir: Option<std::path::PathBuf>,
    /// Email gateway policy (None = we're not a gateway: email VMails are refused).
    pub email_gate: Option<crate::varac::email_gateway::EmailGate>,
    /// BBS access (VarAC [BBS] LimitAccessToCallsigns); None = open to everyone.
    pub bbs_gate: Option<crate::varac::email_gateway::EmailGate>,
    /// Live-changeable answers to the partner's requests (Settings → QSO).
    pub policy: StationPolicy,
}

/// What we answer, as set in Settings → QSO (VarAC: "Allow inquire request", "Allow
/// incoming pings", "Auto accept verbose SNR"). Shared, so Settings apply at once.
#[derive(Debug, Clone)]
pub struct StationPolicy {
    pub allow_info: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub allow_pings: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub verbose_snr_auto_accept: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Welcome message (VarAC `CannedMsgWelcome`) and its "Send always" box.
    pub welcome: std::sync::Arc<std::sync::RwLock<(String, bool)>>,
    /// Own email, sent in INFO replies as `<EM:..>`.
    pub email: std::sync::Arc<std::sync::RwLock<String>>,
    /// Rig, power, antenna, age, profession, hobbies (VarAC [MY_INFO]) for INFO replies
    pub profile: std::sync::Arc<std::sync::RwLock<crate::varac::messages::Profile>>,
    /// Complex callsign (W/4Z1AC/QRP), sent in the opening as `<FC:..>`; empty = none.
    pub full_callsign: std::sync::Arc<std::sync::RwLock<String>>,
    /// "Do we hold VMail for this station?" (the app's mail store), for `<R+NN> <VW>`.
    pub mail_waiting: MailCheck,
    /// We're on a calling frequency (the app keeps this current; true when unknown).
    /// VarAC offers the caller a QSY (<Q>) only when the link starts on one.
    pub on_calling_frequency: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// EmComm: no `<R+NN>` in our opening (VarAC DisableSNRSending)
    pub emcomm_no_snr: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// EmComm: `<LCR>` is answered by the app (it has the QSO log) instead of `<LCJ>`
    pub emcomm_last_connections: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Blocked callsigns (VarAC CallsignsBlockList): their connects are aborted
    pub blocked: std::sync::Arc<std::sync::RwLock<Vec<String>>>,
    /// Our AI gateway is on and announced: `A` in the opening's `<S:..>`
    pub ai_gateway: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// VarAC AllowAutoQSY with rig control: offer `<Q>` / `<AWQ>` to a caller on a CF
    pub auto_qsy: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// EmComm DisableCallIDSending: no "de CALL" in the opening or the Call ID
    pub emcomm_no_call_id: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// VarAC [VMAIL] AllowParking: hold VMail for other stations (else `<SMFP>`)
    pub allow_parking: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// VarAC [FILE_TRANSFER] IncomingFilesSizeLimit (5000 bytes): smaller offers are taken
    /// at once; bigger ones are refused while away, else the operator decides
    pub file_size_limit: std::sync::Arc<std::sync::atomic::AtomicU64>,
    /// EmComm "accept incoming files automatically"
    pub auto_file: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Our answer to a `<FSR>` peek (the frequency scheduler / scanner list, or `<FSO>`)
    pub freq_schedule: std::sync::Arc<std::sync::RwLock<String>>,
    /// The scanner runs with "announce" on (advanced beacons say so)
    pub scanning: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

/// May a VMail with attachment be sent now (see `attachment_gate`)?
#[derive(Debug, Clone, PartialEq)]
pub enum AttachmentGate {
    Send,
    /// not yet (file transfer running, or waiting for the services answer)
    Wait,
    /// not on this link, with VarAC's reason
    Skip(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileOfferDecision {
    /// `<SFRD>` at once
    Accept,
    /// `<SFAB>` at once (away, too big)
    Refuse,
    /// the operator decides
    Ask,
}

/// A lookup the app installs so the session can ask about queued mail.
#[derive(Clone, Default)]
pub struct MailCheck(std::sync::Arc<std::sync::RwLock<Option<MailCheckFn>>>);
type MailCheckFn = std::sync::Arc<dyn Fn(&str) -> bool + Send + Sync>;

impl MailCheck {
    pub fn set(&self, f: impl Fn(&str) -> bool + Send + Sync + 'static) {
        if let Ok(mut c) = self.0.write() {
            *c = Some(std::sync::Arc::new(f));
        }
    }

    pub fn has_mail_for(&self, call: &str) -> bool {
        let f = self.0.read().ok().and_then(|c| c.clone());
        f.is_some_and(|f| f(call))
    }
}

impl std::fmt::Debug for MailCheck {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MailCheck")
    }
}

impl Default for StationPolicy {
    fn default() -> Self {
        Self::new(true, true, false)
    }
}

impl StationPolicy {
    pub fn new(allow_info: bool, allow_pings: bool, verbose_snr_auto_accept: bool) -> Self {
        use std::sync::{atomic::AtomicBool, Arc};
        Self {
            allow_info: Arc::new(AtomicBool::new(allow_info)),
            allow_pings: Arc::new(AtomicBool::new(allow_pings)),
            verbose_snr_auto_accept: Arc::new(AtomicBool::new(verbose_snr_auto_accept)),
            welcome: Arc::default(),
            email: Arc::default(),
            profile: Arc::default(),
            full_callsign: Arc::default(),
            mail_waiting: MailCheck::default(),
            on_calling_frequency: Arc::new(AtomicBool::new(true)),
            emcomm_no_snr: Arc::new(AtomicBool::new(false)),
            emcomm_last_connections: Arc::new(AtomicBool::new(false)),
            blocked: Arc::default(),
            ai_gateway: Arc::new(AtomicBool::new(false)),
            freq_schedule: Arc::default(),
            file_size_limit: Arc::new(std::sync::atomic::AtomicU64::new(5000)),
            allow_parking: Arc::new(AtomicBool::new(true)),
            auto_qsy: Arc::new(AtomicBool::new(true)),
            emcomm_no_call_id: Arc::new(AtomicBool::new(false)),
            auto_file: Arc::new(AtomicBool::new(false)),
            scanning: Arc::new(AtomicBool::new(false)),
        }
    }

    /// What VarAC does with a file offer of `size` bytes (b2 IL_438e…IL_4476).
    pub fn file_offer_decision(&self, size: u64, away: bool) -> FileOfferDecision {
        use std::sync::atomic::Ordering::Relaxed;
        if self.auto_file.load(Relaxed) {
            return FileOfferDecision::Accept;
        }
        let over = size > self.file_size_limit.load(Relaxed);
        match (over, away) {
            (false, _) => FileOfferDecision::Accept,
            (true, true) => FileOfferDecision::Refuse,
            (true, false) => FileOfferDecision::Ask,
        }
    }

    pub fn set_blocked(&self, calls: Vec<String>) {
        if let Ok(mut b) = self.blocked.write() {
            *b = calls;
        }
    }

    /// On the block list: the callsign, or its base call without the SSID (we ignore case).
    pub fn is_blocked(&self, call: &str) -> bool {
        let base = call.trim().split('-').next().unwrap_or("");
        self.blocked.read().is_ok_and(|b| b.iter().any(|c| c.eq_ignore_ascii_case(call.trim()) || c.eq_ignore_ascii_case(base)))
    }

    pub fn set_emcomm(&self, no_snr: bool, last_connections: bool) {
        use std::sync::atomic::Ordering::Relaxed;
        self.emcomm_no_snr.store(no_snr, Relaxed);
        self.emcomm_last_connections.store(last_connections, Relaxed);
    }

    pub fn set_full_callsign(&self, call: &str) {
        if let Ok(mut c) = self.full_callsign.write() {
            *c = call.trim().to_uppercase();
        }
    }

    /// The extra profile fields (name, QTH and locator come from the station config).
    pub fn set_profile(&self, p: crate::varac::messages::Profile) {
        if let Ok(mut cur) = self.profile.write() {
            *cur = p;
        }
    }

    pub fn set_email(&self, email: &str) {
        if let Ok(mut e) = self.email.write() {
            *e = email.trim().to_string();
        }
    }

    pub fn set_welcome(&self, text: &str, always: bool) {
        if let Ok(mut w) = self.welcome.write() {
            *w = (text.trim().to_string(), always);
        }
    }

    pub fn set(&self, allow_info: bool, allow_pings: bool, verbose_snr_auto_accept: bool) {
        use std::sync::atomic::Ordering::Relaxed;
        self.allow_info.store(allow_info, Relaxed);
        self.allow_pings.store(allow_pings, Relaxed);
        self.verbose_snr_auto_accept.store(verbose_snr_auto_accept, Relaxed);
    }

    fn get(flag: &std::sync::atomic::AtomicBool) -> bool {
        flag.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// QSY invitations closer together than this are ignored, as in VarAC.
const DOUBLE_QSY_SECS: u64 = 60;

/// Where a QSY invitation goes: an offset from the current frequency (`<Q:n>`), one slot
/// up / down (`<QSYU>` / `<QSYD>`), or a frequency (`<QSYF>012345678901</QSYF>`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum QsyTarget {
    Offset(i32),
    Up,
    Down,
    Absolute(u64),
}

/// What goes out once we've retuned for a QSY: the invited station's <QSYR> (VarAC sends
/// it from the new frequency), or the inviter's <TL>; plus our opening in the handshake.
#[derive(Debug, Clone, Copy, PartialEq)]
enum AfterQsy {
    Qsyr { opening: bool },
    TestLink { opening: bool },
}

/// Session state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    Idle,
    Connecting,
    Connected,
    AwaitingDisconnect,
}

/// Manages a VarAC session on top of a VARA modem connection.
pub struct VaracSession {
    pub vara: VaraConnection,
    pub config: StationConfig,
    pub state: SessionState,
    pub connected_to: Option<String>,
    pub last_signal_report: Option<f32>,
    /// The first SNR measured on this link (as in VarAC): the opening's report
    first_signal_report: Option<f32>,
    /// When the last SN came (VARA reports the connect request's SN just before CONNECTED)
    last_signal_at: Option<tokio::time::Instant>,
    /// Modem version string from `VERSION` (for `<VER>` replies).
    pub modem_version: Option<String>,
    /// QSY offset (tens of Hz) to invite the partner to when they offer auto-QSY:
    /// `<Q:n>` moves both stations by n×10 Hz (VarAC turns the chosen slot into an
    /// offset; captured `<Q:3>` → 14.105.000 became 14.105.030). 0 = stay.
    pub qsy_offset_tens_hz: i32,
    /// Ask the operator which slot to QSY to (SessionEvent::QsyInvitationRequested)
    /// instead of sending `qsy_offset_tens_hz` automatically.
    pub ask_qsy_invite: bool,
    /// Verbose SNR running on this link: an <R+NN> report every minute (VarAC <VSI>)
    pub verbose_snr: bool,
    /// The link started on a calling frequency, and a QSY was done since: openings wait for the QSY
    started_on_cf: bool,
    qsy_done: bool,
    /// Our opening went out on this link (VarAC sends one per link)
    opening_sent: bool,
    /// A late `<Q>` was taken on this link
    late_q_seen: bool,
    /// We sent our `<S:..>` on this link (then `<SQ>` goes unanswered, as in VarAC)
    services_announced: bool,
    /// An event for when this frame has gone out (our QSY countdown)
    after_sent: Option<(String, SessionEvent)>,
    /// A file is on its way to us (we answered <SFRD>)
    file_incoming: bool,
    /// Services the partner announced with `<S:..>` (as in VarAC)
    partner_services: Option<ServiceFlags>,
    /// When we asked the partner `<SQ>` before a VMail with attachment
    services_inquiry_at: Option<tokio::time::Instant>,
    /// Not before this: VarAC's 1 s after our own frame
    tx_resume_at: Option<tokio::time::Instant>,
    /// The minute of our last verbose SNR report
    vsnr_minute: Option<i64>,
    data_buf: BytesMut,
    plain_text_mode: bool,
    handshake: Handshake,
    handshake_deadline: Option<tokio::time::Instant>,
    /// Outgoing frames. VarAC sends one message per transmission and waits for the
    /// modem to report `BUFFER 0` before the next: when several frames arrive in one
    /// VARA packet, real VarAC fails with "UNABLE TO DECODE DATA" and aborts the link.
    tx_queue: std::collections::VecDeque<(String, Vec<u8>)>,
    /// Text of the frame the modem is sending now (numbered once it's out)
    tx_inflight_text: Option<String>,
    /// Chat frame number: frames numbered in both directions, per QSO (as VarAC numbers them)
    chat_id: u32,
    /// When the frame currently in the modem was written (None = modem idle).
    tx_inflight_since: Option<tokio::time::Instant>,
    /// Offset (Hz) we invited the partner to during the handshake.
    handshake_qsy_hz: i32,
    /// We asked for a QSY mid-QSO; on <QSYR> we send <TL> from the new frequency.
    qsy_requested: bool,
    /// This link is a ping (connect to `CALL-T`): reports are swapped, no handshake.
    ping: bool,
    /// Base64 payload of a file we offered, sent when the partner replies `<SFRD>`.
    outgoing_file: Option<String>,
    /// A file/attachment transfer is running (offer sent, waiting for `<SFOK>`).
    file_in_progress: bool,
    /// Further transfers `(offer frame, data frame body)`, started one at a time.
    file_jobs: std::collections::VecDeque<(String, String)>,
    /// We accepted a VMail-attachment offer; verify and confirm its data here.
    expecting_attachment: Option<crate::varac::messages::BbsFileOffer>,
    /// The transfer under way carries a VMail (`<SFM:`)
    outgoing_is_vmail: bool,
    /// An attachment offer waiting for the operator (bigger than the size limit)
    offered_attachment: Option<crate::varac::messages::BbsFileOffer>,
    /// We're away: answer callers with `<AWQ>` instead of `<Q>` (VarAC, captured).
    away: Option<String>,
    /// The partner called us (as in VarAC)
    incoming: bool,
    /// We sent CONNECT and no link or DISCONNECTED has come back yet.
    dialing: bool,
    /// The link's bandwidth (CONNECTED … 500), for VarAC's attachment limits
    link_bandwidth: Option<u16>,
    /// Partner of the previous QSO: no welcome twice (as in VarAC).
    last_partner: Option<String>,
    /// When we last moved for a QSY invitation.
    last_qsy_invite: Option<tokio::time::Instant>,
    /// An invitation we got, waiting for the countdown (true = during the opening handshake)
    qsy_pending: Option<(QsyTarget, bool)>,
    /// An invitation we sent, waiting for our own countdown (offset in Hz)
    qsy_outgoing: Option<i32>,
    /// After our retune: what to send, and when (VarAC waits ~0.5 s and retunes twice)
    qsy_after: Option<(tokio::time::Instant, AfterQsy)>,
    /// Reply to VarAC's services inquiry `<SQ>` (sent before a VMail with attachment;
    /// without an answer VarAC keeps it in the Outbox). Default: `<S:letters>`.
    pub services_reply: Option<String>,
    /// Events produced but not yet returned (one received packet can hold several).
    pending: std::collections::VecDeque<SessionEvent>,
    /// Raw text of the frame most recently decoded.
    last_raw: String,
    /// Modem KISS port: broadcasts are AX.25 UI frames sent here, no link needed.
    kiss: Option<crate::vara::kiss::KissConnection>,
}

async fn recv_kiss(k: Option<&mut crate::vara::kiss::KissConnection>) -> Option<(u8, Vec<u8>)> {
    match k {
        Some(k) => k.frame_rx.recv().await,
        None => std::future::pending().await,
    }
}

/// VarAC's link-opening exchange, captured from two real VarAC 16 stations:
///
/// ```text
/// callee: <Q>                      ("I allow auto-QSY": VarAC's caller pops a
///                                   "Send QSY Invitation" slot picker)
/// caller: <Q:n>                    (QSY invitation to slot n; 0 = stay put)
/// callee: <QSYR>  de CALLEE <R+20> <S:V>
/// caller: <TL>    de CALLER <R+21>
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Handshake {
    Idle,
    /// We called: waiting for the callee's `<Q>`.
    CallerAwaitQ,
    /// Callee offered auto-QSY; the operator is picking a slot (VarAC's
    /// "Send QSY Invitation" dialog). Auto-stays on frequency after a timeout.
    CallerChoosingSlot,
    /// We answered `<Q:n>`: waiting for `<QSYR>`.
    CallerAwaitQsyr,
    /// We were called and sent `<Q>`: waiting for `<Q:n>`.
    CalleeAwaitSlot,
    Done,
}

/// Split a frame into the messages VarAC joined with CRLF, but only at the top
/// level: a tag may itself contain CRLF (the BBS list `<BL:a|date|size\r\n…>`).
/// A QSY invitation in a frame: `<Q:n>` (n in tens of Hz), `<QSYU>`, `<QSYD>`, or
/// `<QSYF>` + 12 digits + `</QSYF>` (VarAC; 'Ø' read as '0').
fn qsy_invite_in(raw: &str, slot: Option<i32>) -> Option<QsyTarget> {
    if let Some(n) = slot {
        return Some(QsyTarget::Offset(n * 10));
    }
    if raw.contains("<QSYU>") {
        return Some(QsyTarget::Up);
    }
    if raw.contains("<QSYD>") {
        return Some(QsyTarget::Down);
    }
    let start = raw.find("<QSYF>")? + 6;
    let end = raw[start..].find("</QSYF>")? + start;
    let digits = raw[start..end].replace('Ø', "0");
    (digits.chars().count() == 12).then(|| digits.trim_start_matches('0').parse().ok().map(QsyTarget::Absolute)).flatten()
}

/// VarAC answers a ping from a station it holds mail for with `<R+NN> <VW>` in one
/// frame: read it as the report and the VMail-waiting notice.
fn split_ping_answer(part: String) -> Vec<String> {
    if let Some(r) = part.trim().strip_suffix("<VW>").map(str::trim) {
        if r.starts_with("<R") && r.ends_with('>') && !r[1..].contains('<') {
            return vec![r.to_string(), "<VW>".to_string()];
        }
    }
    vec![part]
}

fn split_messages(raw: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut depth = 0i32;
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '<' => depth += 1,
            '>' => depth = (depth - 1).max(0),
            '\r' if depth == 0 && chars.peek() == Some(&'\n') => {
                chars.next();
                if !cur.trim().is_empty() {
                    parts.push(std::mem::take(&mut cur));
                } else {
                    cur.clear();
                }
                continue;
            }
            _ => {}
        }
        cur.push(c);
    }
    if !cur.trim().is_empty() {
        parts.push(cur);
    }
    parts
}

/// Base callsign without SSID, upper-cased.
fn base_call(call: &str) -> String {
    call.split('-').next().unwrap_or(call).trim().to_uppercase()
}

impl VaracSession {
    pub fn new(vara: VaraConnection, config: StationConfig) -> Self {
        Self {
            vara,
            config,
            state: SessionState::Idle,
            connected_to: None,
            last_signal_report: None,
            first_signal_report: None,
            last_signal_at: None,
            qsy_offset_tens_hz: 0,
            ask_qsy_invite: false,
            verbose_snr: false,
            modem_version: None,
            data_buf: BytesMut::with_capacity(8192),
            plain_text_mode: false,
            handshake: Handshake::Idle,
            handshake_deadline: None,
            tx_queue: std::collections::VecDeque::new(),
            tx_inflight_text: None,
            chat_id: 0,
            tx_inflight_since: None,
            handshake_qsy_hz: 0,
            qsy_requested: false,
            ping: false,
            outgoing_file: None,
            file_in_progress: false,
            file_jobs: std::collections::VecDeque::new(),
            away: None,
            incoming: false,
            dialing: false,
            link_bandwidth: None,
            last_partner: None,
            last_qsy_invite: None,
            qsy_pending: None,
            qsy_outgoing: None,
            qsy_after: None,
            expecting_attachment: None,
            offered_attachment: None,
            started_on_cf: false,
            qsy_done: false,
            opening_sent: false,
            late_q_seen: false,
            services_announced: false,
            partner_services: None,
            services_inquiry_at: None,
            after_sent: None,
            file_incoming: false,
            tx_resume_at: None,
            vsnr_minute: None,
            outgoing_is_vmail: false,
            services_reply: None,
            pending: std::collections::VecDeque::new(),
            last_raw: String::new(),
            kiss: None,
        }
    }

    /// Main event loop driver. Call repeatedly.
    /// Wait for the next event and handle it. Not cancel-safe when used inside
    /// `tokio::select!` next to other branches: use `next_input` + `handle_input`.
    #[allow(dead_code)]
    pub async fn poll(&mut self) -> Option<SessionEvent> {
        let input = self.next_input().await?;
        self.handle_input(input).await
    }

    /// Wait for the next input. Cancel-safe: it only receives from channels or waits
    /// on timers, so dropping it (e.g. when another `select!` branch wins) loses nothing.
    pub async fn next_input(&mut self) -> Option<SessionInput> {
        if !self.pending.is_empty() {
            return Some(SessionInput::Pending);
        }
        let deadline = self.handshake_deadline;
        let qsy_after = self.qsy_after.map(|(t, _)| t);
        // Safety net if the modem never reports BUFFER 0 for a frame
        let tx_stall = self.tx_inflight_since.map(|t| t + std::time::Duration::from_secs(120));
        let choosing = self.handshake == Handshake::CallerChoosingSlot;
        let resume = self.tx_resume_at.filter(|_| !self.tx_queue.is_empty());
        tokio::select! {
            // Modem events first: CONNECTED must be handled before the partner's first frame
            biased;
            // (None once the modem connection has closed: see modem_closed())
            event = self.vara.event_rx.recv() => event.map(SessionInput::Vara),
            _ = async { tokio::time::sleep_until(tx_stall.unwrap()).await }, if tx_stall.is_some() => Some(SessionInput::TxStall),
            Some((cmd, raw)) = recv_kiss(self.kiss.as_mut()) => Some(SessionInput::Kiss(cmd, raw)),
            Some(data) = self.vara.data_rx.recv() => Some(SessionInput::Data(data)),
            _ = async { tokio::time::sleep_until(qsy_after.unwrap()).await }, if qsy_after.is_some() => Some(SessionInput::QsyAfter),
            _ = async { tokio::time::sleep_until(resume.unwrap()).await }, if resume.is_some() => Some(SessionInput::TxResume),
            _ = async { tokio::time::sleep_until(deadline.unwrap()).await }, if deadline.is_some() => {
                Some(if choosing { SessionInput::SlotTimeout } else { SessionInput::HandshakeTimeout })
            }
        }
    }

    /// True once the modem's connection has closed (it quit, or the network dropped) and
    /// every event from it has been handled: nothing more will come, so drop the session.
    pub fn modem_closed(&self) -> bool {
        self.vara.event_rx.is_closed() && self.vara.event_rx.is_empty()
    }

    /// Process one input to completion (always await this; don't race it).
    pub async fn handle_input(&mut self, input: SessionInput) -> Option<SessionEvent> {
        match input {
            SessionInput::Pending => self.pending.pop_front(),
            SessionInput::Vara(event) => Some(self.handle_vara_event(event).await),
            SessionInput::TxStall => {
                self.tx_inflight_since = None;
                let _ = self.pump_tx().await;
                Some(SessionEvent::Handshake("TX buffer report timed out; sending next frame".into()))
            }
            SessionInput::Kiss(crate::vara::kiss::VARAC_BEACON_CMD, raw) => {
                // "2…" = advanced beacon, "1…" = a typed CQ (CHAT, INFO, SOS …)
                if let Some(cq) = crate::vara::kiss::TypedCq::parse(&raw) {
                    return Some(SessionEvent::TypedCq(cq));
                }
                match crate::vara::kiss::AdvancedBeacon::parse(&raw) {
                    Some(b) => Some(SessionEvent::AdvancedBeacon(b)),
                    None => Some(SessionEvent::Handshake(format!("ignored KISS 0x02 frame ({} bytes)", raw.len()))),
                }
            }
            SessionInput::Kiss(cmd, raw) => {
                match crate::vara::kiss::UiFrame::decode(cmd, &raw) {
                    // VarAC relay notification (captured): UI frame RELAY -> TARGET, info "<SMP>"
                    // ("<SMU>" when some of the waiting mail is urgent)
                    Some(f) if f.pid == 0xF0
                        && crate::varac::messages::is_relay_notification(&String::from_utf8_lossy(&f.info))
                        && base_call(&f.dest) == base_call(&self.config.callsign) =>
                    {
                        let urgent = String::from_utf8_lossy(&f.info).trim() == "<SMU>";
                        Some(SessionEvent::Message(IncomingMessage::RelayNotification { relay: f.source, urgent }))
                    }
                    // Another station's relay notification (VMail parked at RELAY for TARGET):
                    // only noted, as VarAC does. It isn't a broadcast, so it isn't listed as
                    // one (it showed as a broadcast reading "<SMP>")
                    Some(f) if f.pid == 0xF0 && crate::varac::messages::is_relay_notification(&String::from_utf8_lossy(&f.info)) => {
                        let urgent = if String::from_utf8_lossy(&f.info).trim() == "<SMU>" { "urgent " } else { "" };
                        Some(SessionEvent::Handshake(format!("Heard a VMail relay notification from {} to {}: {urgent}VMail is waiting there for {}", f.source, f.dest, f.dest)))
                    }
                    // (as in VarAC: our own frame coming back, e.g. through a
                    // digipeater, isn't a broadcast heard)
                    Some(f) if f.pid == 0xF0 && base_call(&f.source).eq_ignore_ascii_case(&base_call(&self.config.callsign)) => {
                        Some(SessionEvent::Handshake(format!("own broadcast to {} came back, ignored", f.dest)))
                    }
                    Some(f) if f.pid == 0xF0 => Some(SessionEvent::Message(IncomingMessage::Broadcast(
                        crate::varac::messages::Broadcast {
                            from: f.source,
                            to: f.dest,
                            // (VarAC joins them with a space and replies through the same)
                            via: (!f.digis.is_empty()).then(|| f.digis.join(" ")),
                            message: crate::varac::messages::desanitize(String::from_utf8_lossy(&f.info).trim_end()),
                            band: None,
                            timestamp: Utc::now(),
                            snr: self.last_signal_report,
                        },
                    ))),
                    _ => Some(SessionEvent::Handshake(format!("ignored KISS frame ({} bytes)", raw.len()))),
                }
            }
            SessionInput::Data(data) => {
                // (the partner spoke: VarAC's pause applies only between our own frames)
                self.tx_resume_at = None;
                // The command and data sockets are read by separate tasks; give a
                // CONNECTED that is already on its way a moment to land first.
                if self.state != SessionState::Connected {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
                while let Ok(event) = self.vara.event_rx.try_recv() {
                    let ev = self.handle_vara_event(event).await;
                    self.pending.push_back(ev);
                }
                self.data_buf.extend_from_slice(&data);
                // Decode every complete frame in the buffer, not just the first
                while let Some(ev) = self.try_decode_message() {
                    match ev {
                        SessionEvent::Message(msg) => {
                            // VarAC packs several messages into one frame separated by CRLF
                            // (`<QSYR>\r\n<SM><SBJ:..><MSG:..>`, `de CALL <R+20>\r\n<Q>`).
                            let raw = std::mem::take(&mut self.last_raw);
                            let parts: Vec<String> = split_messages(&raw).into_iter().flat_map(split_ping_answer).collect();
                            let parts: Vec<&str> = parts.iter().map(|p| p.as_str()).collect();
                            if parts.len() > 1 {
                                // (lines of plain text in one frame are one message, as VarAC
                                // shows them; tags on their own lines are handled one by one)
                                let mut i = 0;
                                while i < parts.len() {
                                    let part = parts[i];
                                    let mut m = self.complete(parse_application_message(part));
                                    let mut raw_part = part.to_string();
                                    if let IncomingMessage::RawText(ref mut t) = m {
                                        while i + 1 < parts.len() {
                                            match parse_application_message(parts[i + 1]) {
                                                IncomingMessage::RawText(next) => {
                                                    t.push('\n');
                                                    t.push_str(&next);
                                                    raw_part.push_str("\r\n");
                                                    raw_part.push_str(parts[i + 1]);
                                                    i += 1;
                                                }
                                                _ => break,
                                            }
                                        }
                                    }
                                    self.last_raw = raw_part.clone();
                                    self.side_info(&m, &raw_part);
                                    self.handshake_step(m).await;
                                    i += 1;
                                }
                            } else {
                                let m = self.complete(msg);
                                self.side_info(&m, &raw);
                                self.last_raw = raw;
                                self.handshake_step(m).await;
                            }
                        }
                        other => self.pending.push_back(other),
                    }
                }
                // (with the pause dropped no timer will release what waits: send it now)
                let _ = self.pump_tx().await;
                // A file on its way in: how much of its frame is here
                if self.file_incoming {
                    if let Some((received, total)) = partial_file_frame(&self.data_buf) {
                        self.pending.push_back(SessionEvent::RxProgress { received, total });
                    }
                }
                self.pending.pop_front()
            }
            SessionInput::SlotTimeout => {
                // Nobody picked a slot: VarAC's picker has no timer and sends nothing when
                // closed — the link stays here, silent
                self.handshake = Handshake::Done;
                self.handshake_deadline = None;
                self.pending.push_back(SessionEvent::HandshakeComplete);
                Some(SessionEvent::Handshake("no slot chosen: nothing sent".into()))
            }
            SessionInput::TxResume => {
                self.tx_resume_at = None;
                let _ = self.pump_tx().await;
                self.pending.pop_front()
            }
            SessionInput::QsyAfter => {
                let (_, what) = self.qsy_after.take()?;
                let opening = match what {
                    AfterQsy::Qsyr { opening } => {
                        let _ = self.send_framed_top("<QSYR>").await;
                        opening
                    }
                    AfterQsy::TestLink { opening } => {
                        let _ = self.send_framed("<TL>").await;
                        opening
                    }
                };
                if opening {
                    self.open_now().await;
                } else if !self.opening_sent && self.state == SessionState::Connected {
                    // The link went on without our opening (the caller talked before picking
                    // a slot, and on a calling frequency the opening waits for a QSY): this
                    // QSY is when it goes
                    let _ = self.send_opening().await;
                }
                self.pending.pop_front()
            }
            // Nobody decided the countdown in time (shouldn't happen: the app does): stay
            SessionInput::HandshakeTimeout if self.qsy_pending.is_some() => {
                let _ = self.answer_qsy(false, false, 0).await;
                self.pending.pop_front()
            }
            SessionInput::HandshakeTimeout => {
                // Peer didn't run the VarAC handshake (plain VARA terminal or old client):
                // just send our opening so the QSO can proceed.
                self.handshake_deadline = None;
                // (VarAC stays silent on a calling frequency without a QSY)
                if !self.started_on_cf || self.qsy_done {
                    let _ = self.send_opening().await; // (before Done: callee adds <S:..>)
                }
                self.handshake = Handshake::Done;
                Some(SessionEvent::HandshakeComplete)
            }
        }
    }

    /// Fill in what VarAC leaves implicit inside a QSO: a direct VMail is just
    /// `<SM><SBJ:..><MSG:..>` (sender = partner, addressee = us, time = now).
    fn complete(&self, msg: IncomingMessage) -> IncomingMessage {
        match msg {
            IncomingMessage::VMail(mut vm) => {
                if vm.from.is_empty() {
                    vm.from = self.connected_to.clone().unwrap_or_default();
                }
                if vm.to.is_empty() {
                    vm.to = self.config.callsign.clone();
                }
                if vm.compose_time.is_empty() {
                    vm.compose_time = vm.timestamp.format("%Y-%m-%d %H:%M:%S").to_string();
                }
                // Addressed to someone else: we're the relay, so it goes to Parking and is
                // delivered when the addressee connects (VarAC files these in Parking too)
                // (an email for our gateway isn't relay mail)
                // (VarAC's "for me": TO is my call, my full call, or contains "MYCALL/")
                let to = vm.to.trim().to_uppercase();
                let me = base_call(&self.config.callsign).to_uppercase();
                let full = self.config.policy.full_callsign.read().map(|f| f.clone()).unwrap_or_default();
                let for_me = base_call(&to) == me || (!full.is_empty() && to == full) || to.contains(&format!("{me}/"));
                if !vm.is_email() && !for_me {
                    vm.is_parked = true;
                }
                IncomingMessage::VMail(vm)
            }
            other => other,
        }
    }

    /// VarAC reads `<NAME:>`, `<QTH:>`, `<LOC:>`, `<EM:>` and `<R±NN>` from any chat line too
    /// (e.g. a canned "my info" message): pass them on as station info / SNR report.
    fn side_info(&mut self, m: &IncomingMessage, raw: &str) {
        if !matches!(m, IncomingMessage::RawText(_) | IncomingMessage::Chat(_)) {
            return;
        }
        let (tag_list, _) = tags::extract_tags(raw);
        let get = |f: fn(&Tag) -> Option<String>| tag_list.iter().find_map(f);
        let name = get(|t| if let Tag::Name(v) = t { Some(v.clone()) } else { None });
        let qth = get(|t| if let Tag::Qth(v) = t { Some(v.clone()) } else { None });
        let locator = get(|t| if let Tag::Locator(v) = t { Some(v.clone()) } else { None });
        let email = get(|t| if let Tag::Email(v) = t { Some(v.clone()) } else { None });
        if name.is_some() || qth.is_some() || locator.is_some() || email.is_some() {
            self.pending.push_back(SessionEvent::Message(IncomingMessage::StationInfo { name, qth, locator, email }));
        }
        if let Some(db) = tag_list.iter().find_map(|t| if let Tag::SignalReport(db) = t { Some(*db as i32) } else { None }) {
            self.pending.push_back(SessionEvent::Message(IncomingMessage::SnrReport(db)));
        }
    }

    /// Our services as announced to the partner: as in VarAC,
    /// the email gateway (E) only when it's running and this station may use it.
    fn offered_services(&self) -> ServiceFlags {
        let mut f = self.config.services;
        let partner = self.connected_to.clone().unwrap_or_default();
        let gateway_ok = self.config.email_gate.as_ref().is_some_and(|g| g.check(&partner).is_ok());
        if f.has_email() && !gateway_ok {
            f.0 &= !ServiceFlags::EMAIL;
        }
        if f.has_bbs() && !self.bbs_allowed() {
            f.0 &= !ServiceFlags::BBS;
        }
        if StationPolicy::get(&self.config.policy.ai_gateway) {
            f.0 |= ServiceFlags::AI;
        }
        f
    }

    fn bbs_allowed(&self) -> bool {
        let partner = self.connected_to.clone().unwrap_or_default();
        self.config.bbs_gate.as_ref().is_none_or(|g| g.allows(&partner))
    }

    /// The partner called us.
    pub fn is_incoming(&self) -> bool {
        self.incoming
    }

    /// This link is a ping (CALL-T).
    /// The welcome message to put in front of our opening, if VarAC would send one:
    /// we were called, it isn't a ping, we aren't away (unless "Send always") and the
    /// partner isn't the station of our previous QSO.
    fn welcome_text(&self) -> Option<String> {
        let (text, always) = self.config.policy.welcome.read().ok()?.clone();
        let partner = self.connected_to.as_deref().map(base_call);
        if text.is_empty() || !self.incoming || self.ping || (self.away.is_some() && !always)
            || partner.is_none() || partner == self.last_partner.as_deref().map(base_call)
        {
            return None;
        }
        // "//n" is VarAC's line break; like everything VarAC sends, its tags are expanded
        // (<LOC> → <LOC:EM10>, <NAME> → <NAME:Joe>, <CALL>, <UTCT>…)
        let mut me = self.config.policy.profile.read().map(|p| p.clone()).unwrap_or_default();
        me.name = self.config.name.clone().unwrap_or_default();
        me.qth = self.config.qth.clone().unwrap_or_default();
        me.locator = self.config.locator.clone().unwrap_or_default();
        me.email = self.config.policy.email.read().map(|e| e.clone()).unwrap_or_default();
        let full = self.config.policy.full_callsign.read().map(|f| f.clone()).unwrap_or_default();
        let my_call = if full.is_empty() { self.config.callsign.clone() } else { full };
        let his = self.connected_to.clone().unwrap_or_default();
        let ctx = crate::varac::messages::TagContext { my_call: &my_call, me: &me, his_call: &his, his_name: "", his_qth: "", his_loc: "" };
        Some(crate::varac::messages::expand_tags(&text.replace("//n", "\r\n"), &ctx))
    }

    /// Note that we just sent CONNECT (VarAC doesn't send broadcasts while it dials).
    pub fn mark_dialing(&mut self) {
        self.dialing = true;
    }

    pub fn is_dialing(&self) -> bool {
        self.dialing
    }

    pub fn is_ping(&self) -> bool {
        self.ping
    }

    /// We're in away mode.
    pub fn is_away(&self) -> bool {
        self.away.is_some()
    }

    /// A file or VMail attachment is going in or out (VarAC doesn't idle-disconnect then).
    pub fn is_transferring(&self) -> bool {
        self.file_in_progress || self.outgoing_file.is_some() || self.expecting_attachment.is_some() || !self.file_jobs.is_empty()
    }

    pub fn is_connected(&self) -> bool {
        self.state == SessionState::Connected
    }

    /// Answer a verbose-SNR invitation: <VSIR> and start the minute reports, or <VSIJ>.
    pub async fn answer_verbose_snr(&mut self, accept: bool) -> Result<(), std::io::Error> {
        self.verbose_snr = accept;
        self.send_framed(if accept { "<VSIR>" } else { "<VSIJ>" }).await
    }

    /// Our "Verbose SNR" checkbox: invite the partner (<VSI>) and report every minute,
    /// or stop (<VSS>).
    pub async fn set_verbose_snr(&mut self, on: bool) -> Result<(), std::io::Error> {
        self.verbose_snr = on;
        self.send_framed(if on { "<VSI>" } else { "<VSS>" }).await
    }

    /// The minute report while verbose SNR runs (VarAC sends <R+NN>).
    /// Verbose SNR, VarAC's clock: the caller reports at second :00 of each minute, the
    /// called station at :30 (so they don't collide); not while a report is still queued.
    /// Call it every couple of seconds.
    pub async fn verbose_snr_tick(&mut self) -> Result<(), std::io::Error> {
        use chrono::Timelike;
        if !(self.verbose_snr && self.state == SessionState::Connected && self.last_signal_report.is_some()) {
            return Ok(());
        }
        let now = Utc::now();
        let slot = if self.incoming { 30 } else { 0 };
        if now.second() < slot || now.second() >= slot + 10 {
            return Ok(());
        }
        let minute = now.timestamp() / 60;
        let queued = self.tx_inflight_text.iter().chain(self.tx_queue.iter().map(|(t, _)| t)).any(|t| t.starts_with("<R"));
        if self.vsnr_minute == Some(minute) || queued {
            return Ok(());
        }
        self.vsnr_minute = Some(minute);
        self.send_snr_response().await
    }

    /// "Allow incoming pings": VarAC registers the -T alias (MYCALL CALL CALL-T) only
    /// when pings are allowed; without it VARA never answers a ping.
    pub async fn apply_ping_policy(&mut self) -> Result<(), std::io::Error> {
        let call = self.config.callsign.clone();
        let mut calls = vec![call.clone()];
        if StationPolicy::get(&self.config.policy.allow_pings) {
            calls.push(format!("{}-T", base_call(&call)));
        }
        self.vara.send_command(&crate::vara::commands::VaraCommand::MyCall(calls)).await
    }

    /// An email VMail for us as gateway: `<SMR>` if we take it, else `<EJ>` (VarAC: gateway
    /// off, or the sender's full callsign isn't on the allow list) and the refusal event.
    async fn answer_email_vmail(&mut self, vm: &VMail) -> Option<IncomingMessage> {
        let partner = self.connected_to.clone().unwrap_or_default();
        let verdict = match &self.config.email_gate {
            Some(gate) => gate.check(&partner),
            None => Err("EMAIL GATEWAY IS DISABLED"),
        };
        match verdict {
            Ok(()) => {
                let _ = self.send_vmail_ack().await;
                None
            }
            Err(why) => {
                let _ = self.send_framed("<EJ>").await;
                Some(IncomingMessage::EmailRefused(partner, vm.clone(), why.to_string()))
            }
        }
    }

    /// Drive the link-opening exchange. Handshake tags may come alone (`<Q>`) or
    /// riding on other text (VarAC sends `de CALL <R+20> <S:V>\r\n<Q>` to stations
    /// it has worked before), so match on the raw frame and still pass on the rest.
    async fn handshake_step(&mut self, msg: IncomingMessage) {
        let raw = std::mem::take(&mut self.last_raw);
        // any message carrying <S:..> tells us the partner's services (VarAC: Contains)
        if let Some(i) = raw.find("<S:") {
            let v = &raw[i + 3..];
            self.partner_services = Some(ServiceFlags::from_letters(&v[..v.find('>').unwrap_or(v.len())]));
        }
        let slot = raw.find("<Q:").and_then(|i| {
            let rest = &raw[i + 3..];
            rest[..rest.find('>')?].trim().parse::<i32>().ok()
        });
        // VMail with attachment: VarAC appends the VMail header to the file data frame,
        // `<SFB>base64…<SM><TME:..><SBJ:..><MSG:..>`; deliver both.
        if matches!(msg, IncomingMessage::BbsFilePayload(_)) {
            if let Some(i) = raw.find("<SM") {
                let vmail_part = raw[i..].to_string();
                let data_part = raw[..i].to_string();
                let payload = self.complete(parse_application_message(&data_part));
                let mut attachment_ok = true;
                if let (Some(offer), IncomingMessage::BbsFilePayload(data)) = (self.expecting_attachment.take(), &payload) {
                    let ok = crate::varac::bbs::receive_file(String::from_utf8_lossy(data).trim(), &offer.md5_hex).is_ok();
                    // (VarAC: <SFFA> when the data doesn't check out)
                    let _ = if ok { self.send_file_ok().await } else { self.send_file_failed().await };
                    attachment_ok = ok;
                }
                // VarAC: the VMail riding on a broken attachment isn't stored; <SMF> tells the
                // sender (so it stays in its Outbox)
                if !attachment_ok {
                    let _ = self.send_framed("<SMF>").await;
                    self.pending.push_back(SessionEvent::Notice("ERROR PROCESSING INCOMING VMAIL: the attachment was not received".into()));
                    return;
                }
                self.pending.push_back(SessionEvent::Message(payload));
                if let IncomingMessage::VMail(mut vm) = self.complete(parse_application_message(&vmail_part)) {
                    vm.has_attachment = true;
                    if vm.is_email() {
                        if let Some(refused) = self.answer_email_vmail(&vm).await {
                            self.pending.push_back(SessionEvent::Message(refused));
                            return;
                        }
                    } else {
                        let _ = self.send_vmail_ack().await;
                    }
                    self.pending.push_back(SessionEvent::Message(IncomingMessage::VMail(vm)));
                }
                return;
            }
        }
        // VarAC answers a services inquiry only if it hasn't announced its services yet
        if raw.contains("<SQ>") && self.services_announced {
            return;
        }
        if raw.contains("<SQ>") {
            let reply = self.services_reply.clone().unwrap_or_else(|| {
                format!("<S:{}>", self.offered_services().to_letters())
            });
            let _ = self.send_framed(&reply).await;
            self.pending.push_back(SessionEvent::Handshake(format!("services inquiry <SQ> → {reply}")));
            return;
        }
        let only_tag = matches!(
            msg,
            IncomingMessage::QsyReady | IncomingMessage::QsyAccepted | IncomingMessage::QsyRequest { .. } | IncomingMessage::AwayQsy
        );
        match self.handshake {
            Handshake::CallerAwaitQ if (raw.contains("<Q>") || raw.contains("<AWQ>")) && self.ask_qsy_invite => {
                if raw.contains("<AWQ>") {
                    self.pending.push_back(SessionEvent::Message(IncomingMessage::AwayQsy));
                }
                self.handshake = Handshake::CallerChoosingSlot;
                self.handshake_deadline = Some(tokio::time::Instant::now() + std::time::Duration::from_secs(90));
                self.pending.push_back(SessionEvent::QsyInvitationRequested);
            }
            Handshake::CallerAwaitQ if raw.contains("<Q>") || raw.contains("<AWQ>") => {
                if raw.contains("<AWQ>") {
                    // Partner is away (but allows auto-QSY): tell the UI, then invite as usual
                    self.pending.push_back(SessionEvent::Message(IncomingMessage::AwayQsy));
                }
                let invite = format!("<Q:{}>", self.qsy_offset_tens_hz);
                self.handshake = Handshake::CallerChoosingSlot;
                // (same as the operator picking it: our countdown follows a non-zero offset)
                let _ = self.send_qsy_invitation(self.qsy_offset_tens_hz).await;
                self.pending.push_back(SessionEvent::Handshake(format!("<Q> → {invite}")));
            }
            // A <Q>/<AWQ> later in the QSO (VarAC takes it at any time, once per link)
            Handshake::Done if (raw.contains("<Q>") || raw.contains("<AWQ>")) && !self.late_q_seen && self.state == SessionState::Connected => {
                self.late_q_seen = true;
                if raw.contains("<AWQ>") {
                    self.pending.push_back(SessionEvent::Message(IncomingMessage::AwayQsy));
                }
                self.pending.push_back(SessionEvent::Notice(
                    "Your QSO partner allows auto-QSY: send a QSY invitation (QSY) to clear the frequency.".into()));
                return;
            }
            // The callee opened without offering a QSY (captured: original VarAC with its email
            // gateway on sends just `de CALL <R+18> <S:VEB>`). Its <Q> would have come in the same
            // frame, so don't wait long: our opening follows when this short deadline runs out.
            Handshake::CallerAwaitQ if matches!(msg, IncomingMessage::Opening(_)) => {
                self.handshake_deadline = Some(tokio::time::Instant::now() + std::time::Duration::from_secs(2));
                self.pending.push_back(SessionEvent::Message(msg));
                return;
            }
            Handshake::CallerAwaitQsyr if raw.contains("<QSYR>") => {
                self.qsy_done = true;
                if let Some(offset) = self.qsy_outgoing.take() {
                    // Accepted while our countdown still ran: they've moved, so go now
                    self.pending.push_back(SessionEvent::QsyResolved);
                    self.pending.push_back(SessionEvent::QsyBy(offset));
                    self.qsy_after = Some((tokio::time::Instant::now() + std::time::Duration::from_millis(1000),
                        AfterQsy::TestLink { opening: true }));
                } else if self.handshake_qsy_hz == 0 {
                    // <Q:0> (stay): <TL> and our opening right here
                    let _ = self.send_framed("<TL>").await;
                    self.open_now().await;
                }
                // (otherwise we already moved after our countdown; <TL> and the opening went
                // out from the new frequency)
                self.pending.push_back(SessionEvent::Message(msg));
                return;
            }
            Handshake::CallerAwaitQsyr if raw.contains("<QSYJ>") || raw.contains("<QJO>") => {
                // Refused: stay where we are and open
                if self.qsy_outgoing.take().is_some() {
                    self.pending.push_back(SessionEvent::QsyResolved);
                }
                self.open_now().await;
                self.pending.push_back(SessionEvent::Message(msg));
                return;
            }
            // <Q:0>: stay here, nothing to count down (VarAC's target = its own frequency)
            Handshake::CalleeAwaitSlot if slot == Some(0) => {
                self.qsy_done = true;
                self.last_qsy_invite = Some(tokio::time::Instant::now());
                let _ = self.send_framed("<QSYR>").await;
                self.open_now().await;
            }
            // VarAC shows its "QSY NOW" countdown; the app answers (answer_qsy) and we move,
            // then say <QSYR> from the new frequency, then open
            Handshake::CalleeAwaitSlot if slot.is_some() => {
                let offset = slot.unwrap_or(0) * 10;
                self.qsy_pending = Some((QsyTarget::Offset(offset), true));
                self.handshake_deadline = Some(tokio::time::Instant::now() + std::time::Duration::from_secs(60));
                self.pending.push_back(SessionEvent::QsyInvitation { target: QsyTarget::Offset(offset), inviter: false });
            }
            // Original VarAC as caller sends its opening as soon as it hears <Q>, and
            // <Q:n> only once its operator picks a slot (captured): keep waiting.
            Handshake::CalleeAwaitSlot if matches!(msg, IncomingMessage::Opening(_)) => {
                // The operator is choosing in VarAC's QSY picker: give them time (captured:
                // a slot picked ~2 min after the opening)
                self.handshake_deadline = Some(tokio::time::Instant::now() + std::time::Duration::from_secs(300));
                self.pending.push_back(SessionEvent::Message(msg));
                return;
            }
            // Caller skipped the QSY invitation and just started talking (VarAC: no opening
            // on the calling frequency without a QSY)
            Handshake::CalleeAwaitSlot if !only_tag => {
                if !self.started_on_cf || self.qsy_done {
                    let _ = self.send_opening().await;
                }
                self.handshake = Handshake::Done;
                self.handshake_deadline = None;
                self.pending.push_back(SessionEvent::HandshakeComplete);
            }
            // A QSY invitation after we had already opened: accept and move
            // As in VarAC: a second invitation within 60 s is ignored
            Handshake::Done if qsy_invite_in(&raw, slot).is_some() && self.state == SessionState::Connected
                && self.last_qsy_invite.is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(DOUBLE_QSY_SECS)) =>
            {
                self.pending.push_back(SessionEvent::Notice(format!("<< DOUBLE QSY REQUEST IN {DOUBLE_QSY_SECS} SECONDS. IGNORING. >>")));
            }
            // A QSY invitation during the QSO (<Q:n>, <QSYU>, <QSYD>, <QSYF>…</QSYF>): VarAC's
            // countdown, as above
            Handshake::Done if self.state == SessionState::Connected && qsy_invite_in(&raw, slot).is_some() => {
                if let Some(target) = qsy_invite_in(&raw, slot) {
                    self.qsy_pending = Some((target, false));
                    self.pending.push_back(SessionEvent::QsyInvitation { target, inviter: false });
                }
            }
            // The partner's answer to our mid-QSO invitation
            Handshake::Done if self.qsy_outgoing.is_some() && raw.contains("<QSYR>") => {
                if let Some(offset) = self.qsy_outgoing.take() {
                    self.pending.push_back(SessionEvent::QsyResolved);
                    self.pending.push_back(SessionEvent::QsyBy(offset));
                    self.qsy_after = Some((tokio::time::Instant::now() + std::time::Duration::from_millis(1000),
                        AfterQsy::TestLink { opening: false }));
                }
                self.pending.push_back(SessionEvent::Message(msg));
                return;
            }
            Handshake::Done if self.qsy_outgoing.is_some() && (raw.contains("<QSYJ>") || raw.contains("<QJO>")) => {
                self.qsy_outgoing = None;
                self.pending.push_back(SessionEvent::QsyResolved);
                self.pending.push_back(SessionEvent::Message(msg));
                return;
            }
            // Late handshake tags after we already fell back: nothing to do
            Handshake::Done | Handshake::Idle if raw.trim() == "<Q>" => {}
            _ => {
                // VarAC answers these automatically; so do we (the event still goes to the UI log)
                match msg {
                    // "Allow inquire request" off: VarAC answers <INFOJ>
                    IncomingMessage::InfoRequest
                    | IncomingMessage::EmailGatewayAddressRequest
                    | IncomingMessage::LocatorRequest
                    | IncomingMessage::GpsRequest
                        if !StationPolicy::get(&self.config.policy.allow_info) =>
                    {
                        let _ = self.send_framed("<INFOJ>").await;
                    }
                    IncomingMessage::InfoRequest => {
                        let _ = self.send_info_response().await;
                    }
                    // Peeks we can't serve yet get VarAC's own "not available" answers
                    // (never invented formats): last heard → <LHJ>, parked VMail → <VRPJ>,
                    // frequency schedule → <FSO> (scheduler off)
                    // VarAC: <LOC:..> or "NO LOCATOR"; we have no GPS: "NO GPS DATA"
                    IncomingMessage::LocatorRequest => {
                        let reply = match self.config.locator.as_deref().map(str::trim).filter(|l| !l.is_empty()) {
                            Some(l) => format!("<LOC:{l}>"),
                            None => "NO LOCATOR".into(),
                        };
                        let _ = self.send_framed(&reply).await;
                    }
                    IncomingMessage::GpsRequest => {
                        let gps = crate::config::gps_answer();
                        let _ = self.send_framed(&gps).await;
                    }
                    // VarAC answers last-connections peeks only in EmComm mode with peeking
                    // allowed (then the app answers from its QSO log), otherwise <LCJ>
                    IncomingMessage::LastConnectionsRequest
                        if !StationPolicy::get(&self.config.policy.emcomm_last_connections) =>
                    {
                        let _ = self.send_framed("<LCJ>").await;
                    }
                    IncomingMessage::FreqScheduleRequest => {
                        let answer = self.config.policy.freq_schedule.read().map(|a| a.clone()).unwrap_or_default();
                        if answer != "-" {
                            let _ = self.send_framed(if answer.is_empty() { "<FSO>" } else { &answer }).await;
                        }
                    }
                    // A single VarAC (no cluster database): no connected stations, no other
                    // instances (VarAC's empty answers)
                    IncomingMessage::ClusterStationsRequest => {
                        let _ = self.send_framed("<CCE>").await;
                    }
                    IncomingMessage::ClusterInstancesRequest => {
                        let _ = self.send_framed("NO OTHER INSTANCES").await;
                    }
                    // VarAC approves a speed level change (AcceptMidLinkPactorLevelChange is ON
                    // by default, whatever the modem; lab: original VarAC on VARA answered <SPR>),
                    // and refuses one it can't read
                    IncomingMessage::PactorLevelRequest(ref n) => {
                        let ok = n.trim().parse::<u32>().is_ok();
                        let _ = self.send_framed(if ok { "<SPR>" } else { "<SPJ>" }).await;
                    }
                    // Verbose SNR (VarAC): accept when auto-accept is on (even while away),
                    // reject while away, otherwise the operator answers (answer_verbose_snr)
                    IncomingMessage::ExtendedSnrRequest if StationPolicy::get(&self.config.policy.verbose_snr_auto_accept) => {
                        let _ = self.answer_verbose_snr(true).await;
                    }
                    IncomingMessage::ExtendedSnrRequest if self.away.is_some() => {
                        let _ = self.send_framed("<VSIJ>").await;
                    }
                    // (only <VSS> stops it, as in VarAC)
                    IncomingMessage::ExtendedSnrStop => {
                        self.verbose_snr = false;
                    }
                    IncomingMessage::SnrRequest => {
                        let _ = self.send_snr_response().await;
                    }
                    IncomingMessage::VersionRequest => {
                        let _ = self.send_version_response().await;
                    }
                    IncomingMessage::SnrReport(_) if self.ping && self.state == SessionState::Connected => {
                        // Our ping got its report. VarAC (captured) just tells the modem to
                        // DISCONNECT here: no report back, no <DISC> frame.
                        self.state = SessionState::AwaitingDisconnect;
                        let _ = self.vara.send_command(&crate::vara::commands::VaraCommand::Disconnect).await;
                    }
                    // These are answered here, not in the UI: a hidden GUI window may not be
                    // redrawn (Wayland), and VarAC on the other end would just wait.
                    // BBS limited to listed stations: others get VarAC's rejections
                    IncomingMessage::BbsListRequest if !self.bbs_allowed() => {
                        let _ = self.send_bbs_list_rejected().await;
                    }
                    IncomingMessage::BbsFileRequest(_) if !self.bbs_allowed() => {
                        let _ = self.send_bbs_file_rejected().await;
                    }
                    // VarAC: no BBS on a calling frequency (both requests get <BLJ>), and no
                    // second file while one is on its way (<BGJ>)
                    IncomingMessage::BbsListRequest | IncomingMessage::BbsFileRequest(_)
                        if StationPolicy::get(&self.config.policy.on_calling_frequency) =>
                    {
                        let _ = self.send_bbs_list_rejected().await;
                    }
                    IncomingMessage::BbsFileRequest(_) if self.file_in_progress || self.file_incoming || self.expecting_attachment.is_some() => {
                        let _ = self.send_bbs_file_rejected().await;
                    }
                    // VarAC answers the gateway address as plain text ("N/A" if none)
                    IncomingMessage::EmailGatewayAddressRequest => {
                        let reply = self.config.email_gate.as_ref().map(|g| g.address_reply()).unwrap_or_else(|| "N/A".into());
                        let _ = self.send_framed(&reply).await;
                    }
                    IncomingMessage::BbsListRequest => {
                        let entries = self.config.bbs_dir.as_ref()
                            .filter(|d| d.is_dir())
                            .and_then(|d| d.to_str())
                            .map(crate::varac::bbs::scan_directory);
                        let _ = match entries {
                            Some(e) => self.send_bbs_listing(&e).await,
                            None => self.send_bbs_list_rejected().await,
                        };
                    }
                    IncomingMessage::BbsFileRequest(ref name) => {
                        let path = self.config.bbs_dir.as_ref()
                            .and_then(|d| d.to_str())
                            .and_then(|d| crate::varac::bbs::resolve_request(d, name));
                        let prepared = path.and_then(|p| {
                            let data = std::fs::read(&p).ok()?;
                            let fname = p.file_name()?.to_string_lossy().into_owned();
                            crate::varac::bbs::prepare_file(&fname, &data).ok()
                        });
                        let _ = match prepared {
                            Some(p) => self.send_file(p).await,
                            None => self.send_bbs_file_rejected().await,
                        };
                    }
                    // (VarAC handles email inside its parking branch: parking off → <SMFP>)
                    IncomingMessage::VMail(ref vm) if vm.is_email() && !StationPolicy::get(&self.config.policy.allow_parking) => {
                        let _ = self.send_framed("<SMFP>").await;
                        self.pending.push_back(SessionEvent::Notice(format!("Email VMail to {} refused: parking is not allowed here", vm.to)));
                        return;
                    }
                    IncomingMessage::VMail(ref vm) if vm.is_email() => {
                        if let Some(refused) = self.answer_email_vmail(vm).await {
                            self.pending.push_back(SessionEvent::Message(refused));
                            return;
                        }
                    }
                    // [VMAIL] AllowParking off: mail for someone else is refused (<SMFP>)
                    IncomingMessage::VMail(ref vm) if vm.is_parked && !StationPolicy::get(&self.config.policy.allow_parking) => {
                        let _ = self.send_framed("<SMFP>").await;
                        self.pending.push_back(SessionEvent::Notice(format!("VMail for {} refused: parking is not allowed here", vm.to)));
                        return;
                    }
                    IncomingMessage::VMail(_) => {
                        // VarAC acknowledges every VMail automatically
                        let _ = self.send_vmail_ack().await;
                    }
                    // VarAC treats a VMail attachment offer like any file offer: small ones
                    // are taken, big ones refused while away, else the operator decides
                    IncomingMessage::VMailAttachmentOffer(ref offer) => {
                        match self.config.policy.file_offer_decision(offer.compressed_size as u64, self.away.is_some()) {
                            FileOfferDecision::Accept => {
                                self.expecting_attachment = Some(offer.clone());
                                let _ = self.send_file_ready().await;
                            }
                            FileOfferDecision::Refuse => {
                                let _ = self.send_file_abort().await;
                            }
                            FileOfferDecision::Ask => self.offered_attachment = Some(offer.clone()),
                        }
                    }
                    IncomingMessage::BbsFileOffer(ref offer) => {
                        match self.config.policy.file_offer_decision(offer.compressed_size as u64, self.away.is_some()) {
                            FileOfferDecision::Accept => {
                                let _ = self.send_file_ready().await;
                            }
                            FileOfferDecision::Refuse => {
                                let _ = self.send_file_abort().await;
                            }
                            FileOfferDecision::Ask => {}
                        }
                    }
                    IncomingMessage::BbsFileReady if self.outgoing_file.is_some() => {
                        if let Some(b64) = self.outgoing_file.take() {
                            let _ = self.send_file_payload(&b64).await;
                        }
                    }
                    IncomingMessage::BbsFileOk | IncomingMessage::BbsFileAborted | IncomingMessage::BbsFileRejected | IncomingMessage::BbsFileFailed
                        if self.file_in_progress =>
                    {
                        // a VMail's attachment declined or broken: that VMail stays in the
                        // Outbox (as in VarAC); no receipt will come for it
                        if self.outgoing_is_vmail && !matches!(msg, IncomingMessage::BbsFileOk) {
                            self.pending.push_back(SessionEvent::Message(IncomingMessage::AttachmentRefused));
                        }
                        self.outgoing_is_vmail = false;
                        self.outgoing_file = None;
                        self.file_in_progress = false;
                        let _ = self.start_next_file_job().await;
                    }
                    // The sender withdrew a file we had said yes to: nothing more is coming
                    IncomingMessage::BbsFileAborted if self.file_incoming => {
                        self.file_incoming = false;
                    }
                    IncomingMessage::QsyAccepted if self.qsy_requested => {
                        // Like VarAC: test the link once both sides have moved
                        self.qsy_requested = false;
                        let _ = self.send_framed("<TL>").await;
                    }
                    _ => {}
                }
                self.pending.push_back(SessionEvent::Message(msg));
                return;
            }
        }
        if !only_tag {
            self.pending.push_back(SessionEvent::Message(msg));
        }
    }

    async fn handle_vara_event(&mut self, event: VaraEvent) -> SessionEvent {
        match event {
            VaraEvent::Connected { source, destination, bandwidth } => {
                self.state = SessionState::Connected;
                self.link_bandwidth = bandwidth.as_deref().and_then(|b| b.trim().parse().ok());
                self.dialing = false;
                (self.chat_id, self.tx_inflight_text) = (0, None);
                (self.services_announced, self.late_q_seen, self.qsy_done, self.file_incoming) = (false, false, false, false);
                self.opening_sent = false;
                (self.partner_services, self.services_inquiry_at) = (None, None);
                self.first_signal_report = self.last_signal_report
                    .filter(|_| self.last_signal_at.is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(5)));
                self.started_on_cf = StationPolicy::get(&self.config.policy.on_calling_frequency);
                self.data_buf.clear();
                self.plain_text_mode = false;
                // VARA reports CONNECTED <caller> <callee>; work out who the partner is.
                let we_called = base_call(&source) == base_call(&self.config.callsign);
                let (me, partner) = if we_called { (source, destination) } else { (destination, source) };
                // VarAC: a blocked station's connect is aborted, nothing is sent to it
                if !we_called && self.config.policy.is_blocked(&partner) {
                    let _ = self.vara.send_command(&crate::vara::commands::VaraCommand::Abort).await;
                    self.state = SessionState::Idle;
                    return SessionEvent::Notice(format!("CONNECTED CALLSIGN {partner} IS ON YOUR BLOCK LIST. ABORTING..."));
                }
                self.connected_to = Some(partner.clone());
                self.incoming = !we_called;
                // Ping = link to the `-T` alias VarAC registers with MYCALL
                let my_test = format!("{}-T", base_call(&self.config.callsign));
                self.ping = if we_called { partner.to_uppercase().ends_with("-T") } else { me.to_uppercase() == my_test };
                if self.ping {
                    self.handshake = Handshake::Done;
                    self.handshake_deadline = None;
                    if !we_called {
                        // VarAC answers a ping straight away with its signal report, and
                        // adds <VW> when it holds VMail for the pinger
                        // (nothing measured yet: VarAC sends nothing)
                        if let Some(snr) = self.last_signal_report {
                            let snr = snr.round().clamp(-99.0, 99.0) as i32;
                            let vw = if self.config.policy.mail_waiting.has_mail_for(&partner) { " <VW>" } else { "" };
                            let _ = self.send_framed(&format!("<R{}>{vw}", crate::varac::messages::snr_text(snr))).await;
                        }
                    }
                    return SessionEvent::Connected { source: me, destination: partner, bandwidth };
                }
                if we_called {
                    self.handshake = Handshake::CallerAwaitQ;
                    self.handshake_deadline = Some(tokio::time::Instant::now() + std::time::Duration::from_secs(45));
                } else {
                    self.handshake = Handshake::CalleeAwaitSlot;
                    // VarAC: on a calling frequency it
                    // offers a QSY — <Q> when auto-QSY is allowed (or a welcome goes out),
                    // <AWQ> when away with auto-QSY — and its opening waits for the QSY.
                    // Otherwise the opening goes at once, "<AWAY> de CALL…" when away.
                    let auto_qsy = StationPolicy::get(&self.config.policy.auto_qsy);
                    let offer = match (self.started_on_cf, self.away.is_some()) {
                        (true, true) if auto_qsy => Some("<AWQ>"),
                        (true, false) if auto_qsy || self.welcome_text().is_some() => Some("<Q>"),
                        _ => None,
                    };
                    match offer {
                        Some(o) => {
                            let _ = self.send_framed(o).await;
                            self.handshake_deadline = Some(tokio::time::Instant::now() + std::time::Duration::from_secs(300));
                        }
                        None => {
                            let _ = self.send_opening().await;
                            self.handshake = Handshake::Done;
                            self.handshake_deadline = None;
                            self.pending.push_back(SessionEvent::HandshakeComplete);
                        }
                    }
                }
                // Always `source` = us, `destination` = the other station.
                SessionEvent::Connected { source: me, destination: partner, bandwidth }
            }
            VaraEvent::Disconnected => {
                self.dialing = false;
                if !self.ping && self.connected_to.is_some() {
                    self.last_partner = self.connected_to.clone();
                }
                self.verbose_snr = false;
                self.ping = false;
                self.outgoing_file = None;
                self.file_in_progress = false;
                self.file_jobs.clear();
                self.tx_queue.clear();
                self.tx_inflight_since = None;
                (self.chat_id, self.tx_inflight_text) = (0, None);
                self.handshake = Handshake::Idle;
                self.handshake_deadline = None;
                self.state = SessionState::Idle;
                self.connected_to = None;
                self.data_buf.clear();
                // Drop anything still queued for the old link so it can't leak into the next one
                let _ = self.vara.send_command(&crate::vara::commands::VaraCommand::CleanTxBuffer).await;
                SessionEvent::Disconnected
            }
            VaraEvent::SignalReport(db) => {
                self.last_signal_report = Some(db);
                self.last_signal_at = Some(tokio::time::Instant::now());
                if self.state == SessionState::Connected && self.first_signal_report.is_none() {
                    self.first_signal_report = Some(db);
                }
                SessionEvent::VaraEvent(event)
            }
            VaraEvent::Ptt(on) => SessionEvent::Ptt(on),
            VaraEvent::Version(ref v) => {
                self.modem_version = Some(v.clone());
                SessionEvent::VaraEvent(event)
            }
            VaraEvent::Buffer(n) => {
                if n == 0 {
                    self.tx_done().await;
                }
                SessionEvent::BufferUpdate(n)
            }
            VaraEvent::Busy(on) => SessionEvent::Busy(on),
            VaraEvent::Pending => {
                self.state = SessionState::Connecting;
                SessionEvent::VaraEvent(event)
            }
            // VARA follows the PENDING of a CQ/broadcast with CANCELPENDING
            VaraEvent::CancelPending => {
                if self.state == SessionState::Connecting {
                    self.state = SessionState::Idle;
                }
                self.dialing = false;
                SessionEvent::VaraEvent(event)
            }
            VaraEvent::CqFrame { source, bandwidth } => {
                SessionEvent::CqFrame { source, bandwidth }
            }
            // VARA FM through digipeaters (VarAC: "BEACON <call> VIA <digis>"); our own
            // frame coming back through a digipeater is ignored
            VaraEvent::CqFrameVia { source, via } => {
                if base_call(&source).eq_ignore_ascii_case(&base_call(&self.config.callsign)) {
                    return SessionEvent::VaraEvent(VaraEvent::Unknown(format!("own CQFRAME back via {via}")));
                }
                self.pending.push_back(SessionEvent::Notice(format!("{} {} VIA {via}",
                    if source.ends_with("-9") || source.ends_with("-8") { "BEACON" } else { "CQ" }, source)));
                SessionEvent::CqFrame { source, bandwidth: 0 }
            }
            VaraEvent::BufferEmpty => {
                self.tx_done().await;
                SessionEvent::BufferUpdate(0)
            }
            VaraEvent::DisconnectPending => SessionEvent::VaraEvent(event),
            _ => SessionEvent::VaraEvent(event),
        }
    }

    fn try_decode_message(&mut self) -> Option<SessionEvent> {
        if self.plain_text_mode {
            if self.data_buf.is_empty() {
                return None;
            }
            let text = String::from_utf8_lossy(&self.data_buf).into_owned();
            self.data_buf.clear();
            return Some(SessionEvent::Message(IncomingMessage::RawText(text)));
        }

        if !self.data_buf.is_empty() {
            let first = self.data_buf[0];
            if !(first as char).is_ascii_digit() {
                self.plain_text_mode = true;
                tracing::info!("Switching to plain-text passthrough mode");
                let text = String::from_utf8_lossy(&self.data_buf).into_owned();
                self.data_buf.clear();
                return Some(SessionEvent::Message(IncomingMessage::RawText(text)));
            }
        }

        match framing::decode_frame(&self.data_buf) {
            Ok((message, consumed)) => {
                let _ = self.data_buf.split_to(consumed);

                // Handle VMail trailing '>' quirk: declared length may omit closing >
                if message.contains("<SM") && !self.data_buf.is_empty() {
                    if self.data_buf[0] == b'>' {
                        let _ = self.data_buf.split_to(1);
                    }
                }

                self.number_frame(false, message.clone());
                // (VarAC's sender numbers a file frame that carries a VMail twice)
                if message.starts_with("<SFB") && message.contains("<SM") {
                    self.chat_id += 1;
                }
                if message.starts_with("<SFB") {
                    self.file_incoming = false;
                }
                let parsed = parse_application_message(&message);
                self.last_raw = message;
                Some(SessionEvent::Message(parsed))
            }
            Err(framing::FrameError::Incomplete) => None,
            Err(e) => {
                tracing::warn!("frame decode error: {e}");
                None
            }
        }
    }

    // === Sending ===

    /// Send the opening report, formatted like VarAC 16: `de CALL <R+20>`, and the
    /// answering station adds its services: `de CALL <R+20> <S:V>`.
    pub async fn send_opening(&mut self) -> Result<(), std::io::Error> {
        // (as in VarAC: the report as first measured on the link, not the latest)
        let snr = self.first_signal_report;
        let we_answered = matches!(self.handshake, Handshake::CalleeAwaitSlot) || self.incoming;
        self.opening_sent = true;
        // VarAC: "{welcome} de CALL <R+NN> <S:..>" (the welcome waits for this, i.e. for
        // the QSY when the link came in on the calling frequency)
        let welcome = if we_answered { self.welcome_text().map(|w| w + " ") } else { None };
        // A complex callsign replaces the call: "de <FC:W/4Z1AC/QRP> <R+NN>" (VarAC)
        let full = self.config.policy.full_callsign.read().map(|c| c.clone()).unwrap_or_default();
        let me = if full.is_empty() || full == self.config.callsign.to_uppercase() {
            self.config.callsign.clone()
        } else {
            format!("<FC:{full}>")
        };
        // VarAC: called while away (no auto-QSY) → "<AWAY> de CALL…"; EmComm without
        // Call ID leaves "de CALL" out (" <R+05>…"); no report when nothing was measured
        let away = if we_answered && self.away.is_some() { "<AWAY> " } else { "" };
        let id = if StationPolicy::get(&self.config.policy.emcomm_no_call_id) { String::new() } else { format!("de {me}") };
        let mut msg = format!("{}{away}{id}", welcome.unwrap_or_default());
        // (EmComm stations may leave the report out, as in VarAC)
        if let Some(snr) = snr.filter(|_| !StationPolicy::get(&self.config.policy.emcomm_no_snr)) {
            let snr_i = snr.round().clamp(-99.0, 99.0) as i32;
            msg.push_str(&format!(" <R{}>", crate::varac::messages::snr_text(snr_i)));
        }
        let svc = self.offered_services().to_letters();
        if we_answered && !svc.is_empty() {
            msg.push_str(&format!(" <S:{svc}>"));
        }
        self.send_framed(&msg).await
    }

    /// Send a chat message.
    pub async fn send_chat(&mut self, text: &str) -> Result<(), std::io::Error> {
        self.send_framed(text).await
    }

    /// Send a VMail.
    pub async fn send_vmail(&mut self, vmail: &VMail) -> Result<(), std::io::Error> {
        let wire = self.vmail_wire(vmail);
        self.send_framed(&wire).await
    }

    /// VMail as VarAC sends it: `<SM>`, `<TME:>` only for mail written over
    /// 10 minutes ago, `<TO:>` only when it isn't for the link partner, `<FRM:>` only when it
    /// isn't from us (both upper-cased), `<SBJ:><MSG:>`, `<U>` when urgent, then for email
    /// `<EG>`, `<RA>`, `<RE:…>`.
    pub fn vmail_wire(&self, vmail: &VMail) -> String {
        use crate::varac::messages::sanitize;
        let mut w = String::from("<SM>");
        if (Utc::now() - vmail.timestamp).num_seconds() > 600 {
            w.push_str(&format!("<TME:{}>", vmail.compose_time));
        }
        let partner = self.connected_to.clone().unwrap_or_default().to_uppercase();
        let to = vmail.to.trim().to_uppercase();
        if !to.is_empty() && to != partner && to != base_call(&partner) {
            w.push_str(&format!("<TO:{}>", sanitize(&to)));
        }
        let full = self.config.policy.full_callsign.read().map(|f| f.clone()).unwrap_or_default();
        let from = vmail.from.trim().to_uppercase();
        if !from.is_empty() && from != self.config.callsign.to_uppercase() && from != full {
            w.push_str(&format!("<FRM:{}>", sanitize(&from)));
        }
        w.push_str(&format!("<SBJ:{}><MSG:{}>", sanitize(&vmail.subject), sanitize(&vmail.body)));
        if vmail.urgent {
            w.push_str("<U>");
        }
        if vmail.is_email() {
            w.push_str("<EG>");
            if vmail.raw {
                w.push_str("<RA>");
            }
            if let Some(r) = vmail.reply_to.as_deref() {
                w.push_str(&format!("<RE:{}>", sanitize(r)));
            }
        }
        w
    }

    /// DISCONNECT (as VarAC's DISCONNECT button does it): the modem's
    /// DISCONNECT at once; nothing is sent to the partner first.
    pub async fn send_disconnect(&mut self) -> Result<(), std::io::Error> {
        match self.state {
            SessionState::Connected | SessionState::AwaitingDisconnect => {
                self.state = SessionState::AwaitingDisconnect;
                self.vara.send_command(&crate::vara::commands::VaraCommand::Disconnect).await?;
            }
            SessionState::Connecting => {
                self.vara.send_command(&crate::vara::commands::VaraCommand::Abort).await?;
            }
            SessionState::Idle => {}
        }
        Ok(())
    }

    /// A message with VarAC's <DISC> canned tag: disconnect once what is queued (that
    /// message) has gone out of the modem.
    pub async fn disconnect_after_sent(&mut self) -> Result<(), std::io::Error> {
        if self.state != SessionState::Connected {
            return self.send_disconnect().await;
        }
        self.state = SessionState::AwaitingDisconnect;
        if self.tx_inflight_since.is_none() && self.tx_queue.is_empty() {
            self.vara.send_command(&crate::vara::commands::VaraCommand::Disconnect).await?;
        }
        Ok(())
    }

    /// Send away status.
    #[allow(dead_code)]
    pub async fn send_away(&mut self, message: Option<&str>) -> Result<(), std::io::Error> {
        match message {
            Some(msg) => self.send_framed(&format!("<AWAY>{msg}")).await,
            None => self.send_framed("<AWAY>").await,
        }
    }

    /// Set or clear away mode. While away, incoming links get `<AWQ>` ("away, auto-QSY
    /// allowed") instead of `<Q>`, as VarAC does. Nothing is transmitted here.
    pub fn set_away(&mut self, message: Option<String>) {
        self.away = message;
    }

    /// Acknowledge VMail receipt.
    pub async fn send_vmail_ack(&mut self) -> Result<(), std::io::Error> {
        self.send_framed("<SMR>").await
    }

    /// Answer `<INFO>` like VarAC (messages::info_reply): the filled-in fields as tags,
    /// then RIG / PWR / ANT / AGE / PROFESSION / HOBBIES lines, or "NO INFO".
    pub async fn send_info_response(&mut self) -> Result<(), std::io::Error> {
        let mut p = self.config.policy.profile.read().map(|p| p.clone()).unwrap_or_default();
        p.name = self.config.name.clone().unwrap_or_default();
        p.qth = self.config.qth.clone().unwrap_or_default();
        p.locator = self.config.locator.clone().unwrap_or_default();
        p.email = self.config.policy.email.read().map(|e| e.clone()).unwrap_or_default();
        let msg = crate::varac::messages::info_reply(&p);
        self.send_framed(&msg).await
    }

    /// Answer `<SNRR>` like VarAC: `<R+NN>`, how we hear the partner.
    pub async fn send_snr_response(&mut self) -> Result<(), std::io::Error> {
        // nothing measured yet: VarAC says so rather than inventing +00
        let Some(snr) = self.last_signal_report else {
            return self.send_framed("<NO REPORT>").await;
        };
        let snr = snr.round().clamp(-99.0, 99.0) as i32;
        let sign = if snr >= 0 { "+" } else { "-" };
        self.send_framed(&format!("<R{sign}{:02}>", snr.abs())).await
    }

    /// Answer `<VER>` like VarAC ("VarAC V16.1.8 / VARA HF v4.9.0").
    pub async fn send_version_response(&mut self) -> Result<(), std::io::Error> {
        let text = self.version_text();
        self.send_framed(&text).await
    }

    /// "Openslot V0.1.0 / VARA HF v4.9.0" (VarAC: software + version / modem version).
    pub fn version_text(&self) -> String {
        let modem = self.modem_version.clone().unwrap_or_else(|| "VARA".into());
        format!("Openslot V{} / {modem}", env!("CARGO_PKG_VERSION"))
    }


    /// File data frame, as VarAC sends it: `<SFB>` + base64 of the gzip.
    pub async fn send_file_payload(&mut self, base64_data: &str) -> Result<(), std::io::Error> {
        self.send_framed(&format!("<SFB>{base64_data}")).await
    }

    /// Offer a file the VarAC way: `<SF:name.VarAC_gzip|size|MD5>`, then send the data
    /// only after the receiver answers `<SFRD>`. (VarAC refuses files on the calling
    /// frequency, so QSY first.)
    pub async fn send_file(&mut self, prepared: crate::varac::bbs::PreparedFile) -> Result<(), std::io::Error> {
        // VARA HF's limits on a compressed file: under 100K at 500 Hz, 1M at 2300 Hz (VarAC)
        if let Some(limit) = attachment_limit(self.link_bandwidth) {
            if prepared.compressed_size >= limit {
                let msg = match limit {
                    100_000 => "does not allow to send files over 100K compressed using VaraHF 500HZ",
                    _ => "does not allow to send files over 1M compressed using VaraHF 2300HZ",
                };
                return Err(std::io::Error::other(msg));
            }
        }
        self.file_jobs.push_back((format!("<SF:{}>", prepared.offer_tag_value()), prepared.base64_payload));
        self.start_next_file_job().await
    }

    /// Start the next queued transfer if none is running (one at a time, like VarAC).
    async fn start_next_file_job(&mut self) -> Result<(), std::io::Error> {
        if self.file_in_progress {
            return Ok(());
        }
        if let Some((offer, data)) = self.file_jobs.pop_front() {
            self.file_in_progress = true;
            self.outgoing_is_vmail = offer.starts_with("<SFM:");
            self.outgoing_file = Some(data);
            self.send_framed(&offer).await?;
        }
        Ok(())
    }

    pub async fn send_file_ready(&mut self) -> Result<(), std::io::Error> {
        self.file_incoming = true;
        self.send_framed("<SFRD>").await
    }

    /// Confirm BBS file received OK.
    pub async fn send_file_ok(&mut self) -> Result<(), std::io::Error> {
        self.send_framed("<SFOK>").await
    }

    /// Abort a BBS file transfer.
    pub async fn send_file_abort(&mut self) -> Result<(), std::io::Error> {
        self.send_framed("<SFAB>").await
    }

    /// Withdraw our file offer before the partner answers `<SFRD>`: the partner is told
    /// `<SFAB>` and a late `<SFRD>` sends nothing. Queued files are dropped too. False when
    /// there's no offer to withdraw: once the data is going it can't be taken back (VARA's
    /// CLEANTXBUFFER only reports whether its buffer is empty), only dropping the link
    /// stops it.
    #[allow(dead_code)] // the phones
    pub async fn cancel_outgoing_file(&mut self) -> Result<bool, std::io::Error> {
        if !self.file_in_progress || self.outgoing_file.is_none() {
            return Ok(false);
        }
        self.outgoing_file = None;
        self.outgoing_is_vmail = false;
        self.file_in_progress = false;
        self.file_jobs.clear();
        self.send_file_abort().await?;
        Ok(true)
    }

    /// Our file's data is on its way (the partner answered `<SFRD>`).
    #[allow(dead_code)] // the phones
    pub fn file_data_going(&self) -> bool {
        self.file_in_progress && self.outgoing_file.is_none()
    }

    /// Whether a file of ours is offered or on its way.
    #[allow(dead_code)] // the phones
    pub fn sending_file(&self) -> bool {
        self.file_in_progress
    }

    /// The file arrived broken (VarAC `<SFFA>`: "FILE TRANSFER FAILED AT DESTINATION").
    pub async fn send_file_failed(&mut self) -> Result<(), std::io::Error> {
        self.send_framed("<SFFA>").await
    }

    /// The operator's answer to a big VMail attachment ("Rejecting it rejects the whole VMail").
    pub async fn answer_attachment(&mut self, accept: bool) -> Result<(), std::io::Error> {
        let Some(offer) = self.offered_attachment.take() else { return Ok(()) };
        if accept {
            self.expecting_attachment = Some(offer);
            self.send_file_ready().await
        } else {
            self.send_file_abort().await
        }
    }

    /// Send an info request to the connected station.
    pub async fn send_info_request(&mut self) -> Result<(), std::io::Error> {
        self.send_framed("<INFO>").await
    }

    /// Send an SNR request to the connected station.
    pub async fn send_snr_request(&mut self) -> Result<(), std::io::Error> {
        // VarAC's current SNR request (<SNRR> is its old one; both are answered)
        self.send_framed("<SR>").await
    }

    /// Send the typing indicator to the connected station.
    pub async fn send_is_typing(&mut self) -> Result<(), std::io::Error> {
        // VarAC's is-typing marker (it shows "<TYP>" as chat text)
        self.send_framed("&IT&").await
    }

    /// Send a broadcast message while connected to a station.
    #[allow(dead_code)] // the phones and the interop peer
    pub async fn send_broadcast(&mut self, text: &str) -> Result<(), std::io::Error> {
        self.send_broadcast_to("ALL", text).await
    }

    /// Send a VarAC broadcast: an AX.25 UI frame on the modem's KISS port (verified
    /// byte-for-byte against VarAC 16.1.8, and received by real VarAC). Works with or
    /// without a link.
    #[allow(dead_code)]
    pub async fn send_broadcast_to(&mut self, to: &str, text: &str) -> Result<(), std::io::Error> {
        let frame = crate::varac::messages::broadcast_frame(&self.config.callsign, to, text);
        match self.kiss.as_mut() {
            Some(k) => k.send_ui(&frame).await,
            None => Err(std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                "modem KISS port not connected (enable KISS in VARA, default port 8100)",
            )),
        }
    }

    /// VarAC's typed CQ (any type but STANDARD): KISS 0x02 with slot, locator and type.
    pub async fn send_typed_cq(&mut self, slot: u8, cq_type: &str) -> Result<(), std::io::Error> {
        let locator = self.config.locator.clone().unwrap_or_default();
        // VarAC skips a typed CQ whose fields don't fit rather than cutting them
        if locator.chars().count() > 6 || base_call(&self.config.callsign).chars().count() > 7 || cq_type.chars().count() > 4 {
            return Err(std::io::Error::other("One of the CQ elements is too long. Skipping CQ"));
        }
        let cq = crate::vara::kiss::TypedCq {
            slot,
            locator,
            callsign: base_call(&self.config.callsign),
            cq_type: cq_type.to_string(),
        };
        match self.kiss.as_mut() {
            Some(k) => k.send_typed_cq(&cq).await,
            None => Err(std::io::Error::new(std::io::ErrorKind::NotConnected, "modem KISS port not connected")),
        }
    }

    /// Send VarAC's ADVANCED beacon over KISS: callsign, locator and announced services
    /// (mask of `AdvancedBeacon::EMAIL_GATEWAY` etc.), instead of a `CQFRAME CALL-9`.
    pub async fn send_advanced_beacon(&mut self, services: u8) -> Result<(), std::io::Error> {
        let b = crate::vara::kiss::AdvancedBeacon::new(
            &self.config.callsign,
            self.config.locator.as_deref().unwrap_or(""),
            services,
        );
        // VarAC skips a beacon whose fields don't fit (locator 6, callsign 7)
        if b.locator.chars().count() > 6 || b.callsign.chars().count() > 7 {
            return Err(std::io::Error::other("One of the BEACON elements is too long. Skipping BEACON"));
        }
        match self.kiss.as_mut() {
            Some(k) => k.send_beacon(&b).await,
            None => Err(std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                "modem KISS port not connected (enable KISS in VARA, default port 8100)",
            )),
        }
    }

    /// Send a VMail with attachments the way VarAC 16 does: the files are zipped and
    /// offered as `<SFM:subject_YYYYMMDD_HHMMSS.zip.VarAC_gzip|size|MD5>`; after `<SFRD>`
    /// one frame carries `<SFB>` + base64(gzip(zip)) + the VMail header
    /// `<SM><TME:..><SBJ:..><MSG:..>`. The partner answers `<SFOK>` then `<SMR>`.
    /// May a VMail with attachment go out now? VarAC: not
    /// during a file transfer; it asks `<SQ>` when the partner hasn't announced its
    /// services and waits up to 60 s for the answer; the partner must offer `V`.
    pub async fn attachment_gate(&mut self) -> AttachmentGate {
        if self.file_in_progress || self.file_incoming || !self.file_jobs.is_empty() {
            return AttachmentGate::Wait;
        }
        let partner = self.connected_to.clone().unwrap_or_default().to_uppercase();
        match self.partner_services {
            None => {
                let now = tokio::time::Instant::now();
                let asked = *self.services_inquiry_at.get_or_insert(now);
                if asked == now {
                    let _ = self.send_framed("<SQ>").await;
                }
                if now.duration_since(asked) < std::time::Duration::from_secs(60) {
                    AttachmentGate::Wait
                } else {
                    AttachmentGate::Skip(format!("{partner} DID NOT ANSWER THE SERVICES INQUIRY (OLDER VERSION?)"))
                }
            }
            Some(s) if !s.has_vmail() => AttachmentGate::Skip(format!("{partner} DOES NOT SUPPORT VMAIL ATTACHMENTS")),
            Some(_) => AttachmentGate::Send,
        }
    }

    pub async fn send_vmail_with_attachments(
        &mut self,
        vmail: &VMail,
        files: &[(String, Vec<u8>)],
    ) -> Result<(), std::io::Error> {
        // (as in VarAC: a single .zip goes as it is)
        let (zip_name, zip_bytes) = match files {
            [(name, bytes)] if name.to_uppercase().ends_with(".ZIP") => (name.clone(), bytes.clone()),
            _ => (vmail_package_name(&vmail.subject, Utc::now()), vmail_package(files)?),
        };
        let prepared = crate::varac::bbs::prepare_file(&zip_name, &zip_bytes)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        // VarAC's limits on a compressed attachment: under 100K at 500 Hz, 1M at 2300 Hz
        if let Some(limit) = attachment_limit(self.link_bandwidth) {
            if prepared.compressed_size >= limit {
                return Err(std::io::Error::other("ATTACHMENT IS TOO BIG FOR THIS BANDWIDTH"));
            }
        }
        // Like VarAC: TO only when it isn't the link
        // partner, FRM only when it isn't us (a relayed or emailed VMail), fields sanitized
        // (the same VMail header VarAC sends, email tags included)
        let header = self.vmail_wire(vmail);
        // Data goes out on <SFRD> (BbsFileReady handler) with the VMail header appended
        self.file_jobs.push_back((
            format!("<SFM:{}>", prepared.offer_tag_value()),
            format!("{}{header}", prepared.base64_payload),
        ));
        self.start_next_file_job().await
    }

    /// Tell `target` (whose beacon we just heard) that VMail is parked for it here,
    /// as VarAC does: a KISS UI frame to the target whose text is `<SMP>`.
    #[allow(dead_code)] // (the interop peer)
    pub async fn send_relay_notification(&mut self, target: &str) -> Result<(), std::io::Error> {
        self.send_relay_notification_as(target, false).await
    }

    /// `<SMP>`, or `<SMU>` when some of the mail waiting for `target` is urgent (VarAC).
    pub async fn send_relay_notification_as(&mut self, target: &str, urgent: bool) -> Result<(), std::io::Error> {
        let tag = if urgent { "<SMU>" } else { "<SMP>" };
        let frame = crate::varac::messages::ui_frame(&self.config.callsign, &base_call(target), tag);
        match self.kiss.as_mut() {
            Some(k) => k.send_ui(&frame).await,
            None => Err(std::io::Error::new(std::io::ErrorKind::NotConnected, "modem KISS port not connected")),
        }
    }

    /// Connect the modem's KISS port (VarAC uses it for broadcasts).
    pub async fn attach_kiss(&mut self, host: &str, port: u16) -> Result<(), std::io::Error> {
        self.kiss = Some(crate::vara::kiss::KissConnection::connect(host, port).await?);
        Ok(())
    }


    /// Send a BBS directory listing to the connected station.
    pub async fn send_bbs_listing(&mut self, entries: &[crate::varac::messages::BbsListingEntry]) -> Result<(), std::io::Error> {
        if entries.is_empty() {
            // (VarAC: an empty BBS folder lists "No file"; <BLJ> is for a refusal)
            self.send_framed("<BL:No file>").await
        } else {
            // VarAC (captured): every entry ends with CRLF, e.g. `<BL:a.txt|2026-10-28|56\r\n>`
            let listing: String = entries.iter()
                .map(|e| format!("{}|{}|{}\r\n", e.name, e.date, e.size))
                .collect();
            self.send_framed(&format!("<BL:{listing}>")).await
        }
    }

    /// Reject a BBS listing request.
    pub async fn send_bbs_list_rejected(&mut self) -> Result<(), std::io::Error> {
        self.send_framed("<BLJ>").await
    }

    /// Reject a BBS file request.
    pub async fn send_bbs_file_rejected(&mut self) -> Result<(), std::io::Error> {
        self.send_framed("<BGJ>").await
    }

    /// Request a BBS directory listing from the connected station.
    pub async fn send_bbs_list_request(&mut self) -> Result<(), std::io::Error> {
        self.send_framed("<BLR>").await
    }

    /// Request a specific file from the connected station's BBS.
    pub async fn send_bbs_file_request(&mut self, filename: &str) -> Result<(), std::io::Error> {
        self.send_framed(&format!("<BG:{filename}>")).await
    }

    /// Send a path finder request for a target callsign.
    /// Send a ready-made UI frame on the KISS port.
    pub async fn send_ui(&mut self, frame: &crate::vara::kiss::UiFrame) -> Result<(), std::io::Error> {
        match self.kiss.as_mut() {
            Some(k) => k.send_ui(frame).await,
            None => Err(std::io::Error::new(std::io::ErrorKind::NotConnected, "modem KISS port not connected")),
        }
    }


    /// Send our opening and finish the handshake (callee adds its services).
    /// (VarAC sends its opening only after a QSY when the link started on a calling
    /// frequency: refused or never invited, both sides stay silent there.)
    async fn open_now(&mut self) {
        if !matches!(self.handshake, Handshake::Done | Handshake::Idle) {
            if !self.started_on_cf || self.qsy_done {
                let _ = self.send_opening().await;
            }
            self.handshake = Handshake::Done;
            self.handshake_deadline = None;
            self.pending.push_back(SessionEvent::HandshakeComplete);
        }
    }

    /// The countdown's verdict on the pending QSY: accept = retune by
    /// `offset_hz` (QsyBy) and, a moment later, <QSYR> from the new frequency (we were
    /// invited) or <TL> (we invited); reject = <QSYJ>, or <QJO> when out of our range
    /// (nothing is sent when it was our own invitation). In the handshake, our opening
    /// follows either way.
    pub async fn answer_qsy(&mut self, accept: bool, out_of_range: bool, offset_hz: i32) -> Result<(), std::io::Error> {
        let later = tokio::time::Instant::now() + std::time::Duration::from_millis(1000);
        if let Some((_, in_handshake)) = self.qsy_pending.take() {
            if accept {
                self.qsy_done = true;
                self.last_qsy_invite = Some(tokio::time::Instant::now());
                if offset_hz != 0 {
                    self.pending.push_back(SessionEvent::QsyBy(offset_hz));
                }
                self.qsy_after = Some((later, AfterQsy::Qsyr { opening: in_handshake }));
            } else {
                self.send_framed_top(if out_of_range { "<QJO>" } else { "<QSYJ>" }).await?;
                if in_handshake {
                    self.open_now().await;
                }
            }
        } else if let Some(offset) = self.qsy_outgoing.take() {
            let in_handshake = self.handshake == Handshake::CallerAwaitQsyr;
            if accept {
                self.qsy_done = true;
                self.pending.push_back(SessionEvent::QsyBy(if offset_hz != 0 { offset_hz } else { offset }));
                self.qsy_after = Some((later, AfterQsy::TestLink { opening: in_handshake }));
            } else if in_handshake {
                self.open_now().await;
            }
        }
        Ok(())
    }

    /// Mid-QSO one-slot QSY (VarAC's QSY up / down buttons): `<QSYU>` / `<QSYD>`; both sides
    /// then count down to moving by `step_hz`.
    pub async fn send_qsy_step(&mut self, up: bool, step_hz: i32) -> Result<(), std::io::Error> {
        let tag = if up { "<QSYU>" } else { "<QSYD>" };
        let offset = if up { step_hz } else { -step_hz };
        self.qsy_outgoing = Some(offset);
        self.after_sent = Some((tag.to_string(), SessionEvent::QsyInvitation { target: if up { QsyTarget::Up } else { QsyTarget::Down }, inviter: true }));
        self.send_framed_top(tag).await
    }

    /// As in VarAC: QSY tags go ahead of queued chat.
    async fn send_framed_top(&mut self, text: &str) -> Result<(), std::io::Error> {
        if !matches!(self.state, SessionState::Connected | SessionState::AwaitingDisconnect) {
            return Err(std::io::Error::new(std::io::ErrorKind::NotConnected, "no VARA link"));
        }
        self.tx_queue.push_front((text.to_string(), framing::encode_frame(text)));
        self.pump_tx().await
    }

    /// Answer the partner's auto-QSY offer with the operator's choice (offset in tens of Hz).
    pub async fn send_qsy_invitation(&mut self, offset_tens_hz: i32) -> Result<(), std::io::Error> {
        if self.handshake != Handshake::CallerChoosingSlot {
            return Ok(());
        }
        let tag = format!("<Q:{offset_tens_hz}>");
        self.handshake_qsy_hz = offset_tens_hz * 10;
        self.handshake = Handshake::CallerAwaitQsyr;
        self.handshake_deadline = Some(tokio::time::Instant::now() + std::time::Duration::from_secs(120));
        // VarAC counts down on the inviting side too, and moves by itself (its partner
        // answers <QSYR> only from the new frequency) — the countdown starts once the
        // invitation is on the air
        if offset_tens_hz != 0 {
            self.qsy_outgoing = Some(offset_tens_hz * 10);
            self.after_sent = Some((tag.clone(), SessionEvent::QsyInvitation { target: QsyTarget::Offset(offset_tens_hz * 10), inviter: true }));
        }
        self.send_framed_top(&tag).await
    }

    /// Mid-QSO QSY invitation (VarAC's Send QSY Invitation window): `<Q:n>`, n = the
    /// offset from our current frequency in tens of Hz; then our own countdown.
    pub async fn send_qsy_request(&mut self, offset_tens_hz: i32) -> Result<(), std::io::Error> {
        let tag = format!("<Q:{offset_tens_hz}>");
        if offset_tens_hz != 0 {
            self.qsy_outgoing = Some(offset_tens_hz * 10);
            self.after_sent = Some((tag.clone(), SessionEvent::QsyInvitation { target: QsyTarget::Offset(offset_tens_hz * 10), inviter: true }));
        }
        self.send_framed_top(&tag).await
    }

    pub async fn send_qsy_absolute(&mut self, freq_hz: u64) -> Result<(), std::io::Error> {
        self.qsy_requested = true;
        // (VarAC 16 understands <QSYF>+12 digits, not <QF:>)
        self.send_framed(&crate::varac::ale::qsyf(freq_hz)).await
    }

    pub async fn send_qsy_accept(&mut self) -> Result<(), std::io::Error> {
        self.send_framed("<QSYR>").await
    }

    pub async fn send_qsy_reject(&mut self) -> Result<(), std::io::Error> {
        self.send_framed("<QSYJ>").await
    }

    #[allow(dead_code)]
    pub async fn send_test_link(&mut self) -> Result<(), std::io::Error> {
        self.send_framed("<TL>").await
    }

    /// The modem has sent everything we gave it: release the next queued frame, or
    /// finish a pending disconnect once the queue has drained.
    async fn tx_done(&mut self) {
        self.tx_inflight_since = None;
        // VarAC waits a second before sending again after its own frame (a timer, so the
        // session keeps reading meanwhile)
        self.tx_resume_at = Some(tokio::time::Instant::now() + std::time::Duration::from_secs(1));
        // VarAC numbers a frame it sent once the modem's buffer has drained
        if let Some(text) = self.tx_inflight_text.take() {
            // (our QSY invitation is on the air: now our own countdown starts)
            if self.after_sent.as_ref().is_some_and(|(tag, _)| text.starts_with(tag.as_str())) {
                if let Some((_, ev)) = self.after_sent.take() {
                    self.pending.push_back(ev);
                }
            }
            self.number_frame(true, text);
        }
        let _ = self.pump_tx().await;
        if self.tx_inflight_since.is_none() && self.state == SessionState::AwaitingDisconnect {
            let _ = self.vara.send_command(&crate::vara::commands::VaraCommand::Disconnect).await;
        }
    }

    /// What waits to be sent (VarAC's "In queue"): the frame in the modem first, then the queue.
    pub fn queued_texts(&self) -> Vec<String> {
        self.tx_inflight_text.iter().chain(self.tx_queue.iter().map(|(t, _)| t)).cloned().collect()
    }

    /// CLR / EDIT: drop what hasn't been handed to the modem yet, returning its texts.
    pub fn take_queue(&mut self) -> Vec<String> {
        self.tx_queue.drain(..).map(|(t, _)| t).collect()
    }

    async fn pump_tx(&mut self) -> Result<(), std::io::Error> {
        if self.tx_inflight_since.is_some() || self.tx_resume_at.is_some_and(|t| tokio::time::Instant::now() < t) {
            return Ok(());
        }
        self.tx_resume_at = None;
        // VarAC drops a waiting "is typing" when real text is queued behind it
        if self.tx_queue.len() > 1 && self.tx_queue.front().is_some_and(|(t, _)| t.starts_with("&IT&")) {
            self.tx_queue.pop_front();
        }
        if let Some((mut text, mut frame)) = self.tx_queue.pop_front() {
            // ...and sends up to three queued messages as one frame, joined by a line break —
            // not when the first is typing, a file, a QSY invitation, a reply, a like, a BBS
            // list or an AI question, and the next can't be a file frame, a reply, a like,
            // typing or an AI question (as VarAC does)
            let head_ok = |t: &str| !["&IT&", "<SF:", "<SFM:", "<QSYU>", "<QSYD>", "<QSYF>", "<Q:", "<QF:", "<C:", "&L", "<BL:"]
                .iter().any(|p| t.starts_with(p)) && !t.to_ascii_uppercase().starts_with("AI:");
            let next_ok = |t: &str| !["<SF", "<C:", "&L", "&IT&"].iter().any(|p| t.starts_with(p))
                && !t.to_ascii_uppercase().starts_with("AI:");
            if head_ok(&text) && !text.starts_with("<SF") {
                let mut joined = 1;
                while joined < 3 && self.tx_queue.front().is_some_and(|(t, _)| next_ok(t)) {
                    let (next, _) = self.tx_queue.pop_front().unwrap_or_default();
                    text = format!("{text}\r\n{next}");
                    joined += 1;
                }
                if joined > 1 {
                    frame = framing::encode_frame(&text);
                }
            }
            self.tx_inflight_since = Some(tokio::time::Instant::now());
            // (the app shows the line on the air, filling as the modem's buffer empties)
            self.pending.push_back(SessionEvent::TxStarted { text: text.clone(), bytes: frame.len() });
            self.tx_inflight_text = Some(text);
            self.vara.send_data(&frame).await?;
        }
        Ok(())
    }

    async fn send_framed(&mut self, text: &str) -> Result<(), std::io::Error> {
        if text.contains("<S:") {
            self.services_announced = true;
        }
        // The modem buffers data written without a link and transmits it on the next
        // one, so never queue application data unless we are linked.
        if !matches!(self.state, SessionState::Connected | SessionState::AwaitingDisconnect) {
            return Err(std::io::Error::new(std::io::ErrorKind::NotConnected, "no VARA link"));
        }
        self.tx_queue.push_back((text.to_string(), framing::encode_frame(text)));
        self.pump_tx().await
    }

    /// VarAC's chat ids: every frame of the QSO, both ways, gets the next number,
    /// except is-typing (`&IT&`) and reactions (`&L…`). Never sent: both sides count.
    fn number_frame(&mut self, outgoing: bool, text: String) {
        if text.starts_with("&IT&") || text.starts_with("&L") {
            return;
        }
        self.chat_id += 1;
        self.pending.push_back(SessionEvent::FrameId { outgoing, id: self.chat_id, text });
    }

    /// Like (`&L1n`) or unlike (`&L0n`) the partner's line number `id`.
    pub async fn send_reaction(&mut self, id: u32, like: bool) -> Result<(), std::io::Error> {
        self.send_framed(&format!("&L{}{id}", u8::from(like))).await
    }
}

/// VarAC's HamPlay, AI gateway, cluster and PACTOR messages, matched as VarAC does
/// (`AI:` at the start of the upper-cased text; the other tags anywhere in it).
fn parse_feature_message(message: &str) -> Option<IncomingMessage> {
    use crate::varac::messages::HamPlayMsg;
    let m = message.trim();
    if m.get(..3).is_some_and(|p| p.eq_ignore_ascii_case("AI:")) {
        return Some(IncomingMessage::AiPrompt(m[3..].to_string()));
    }
    for prefix in ["<ACIE>", "<SAFIE>"] {
        if let Some(i) = m.find(prefix) {
            return Some(IncomingMessage::AiResponse(m[i + prefix.len()..].trim_end().to_string()));
        }
    }
    // text between `tag` and the next '>' (as in VarAC)
    let decode = |tag: &str| m.find(tag).map(|i| {
        let rest = &m[i + tag.len()..];
        rest[..rest.find('>').unwrap_or(rest.len())].to_string()
    });
    if let Some(n) = decode("<P:") {
        return Some(IncomingMessage::HamPlay(HamPlayMsg::Invite(n.trim().parse().unwrap_or(0))));
    }
    if let Some(mv) = decode("<PM:") {
        return Some(IncomingMessage::HamPlay(HamPlayMsg::Move(mv)));
    }
    for (tag, msg) in [("<PR>", HamPlayMsg::Accept), ("<PJ>", HamPlayMsg::Reject), ("<PA>", HamPlayMsg::Abort), ("<PE>", HamPlayMsg::Rematch)] {
        if m.contains(tag) {
            return Some(IncomingMessage::HamPlay(msg));
        }
    }
    // VarAC's older service announcements (b2 IL_534c…IL_55e2)
    for (tag, text) in [
        ("<EA>", "THIS STATION SERVES BOTH EMAIL AND AI GATEWAYS.\nTYPE 'AI:' FOLLOWED BY YOUR QUESTION TO ASK THE AI. FOR EXAMPLE: 'AI:how to make fire?'"),
        ("<E>", "THIS STATION SERVES AN EMAIL GATEWAY."),
        ("<A>", "THIS STATION SERVES AN AI GATEWAYS.\nTYPE 'AI:' FOLLOWED BY YOUR QUESTION TO ASK THE AI. FOR EXAMPLE: 'AI:how to make fire?'"),
    ] {
        if m.contains(tag) {
            return Some(IncomingMessage::ServiceNotice(text.into()));
        }
    }
    if m.contains("<CIR>") {
        return Some(IncomingMessage::ClusterInstancesRequest);
    }
    if let Some(body) = m.strip_suffix("<CM>") {
        return Some(IncomingMessage::ClusterRelayed(body.trim_end().to_string()));
    }
    if let Some(n) = decode("<SP:") {
        return Some(IncomingMessage::PactorLevelRequest(n));
    }
    if m.contains("<SPR>") {
        return Some(IncomingMessage::PactorLevelAnswer(true));
    }
    if m.contains("<SPJ>") {
        return Some(IncomingMessage::PactorLevelAnswer(false));
    }
    None
}

/// Parse a decoded VarAC application-layer message into a typed variant.
pub(crate) fn parse_application_message(message: &str) -> IncomingMessage {
    let (tag_list, text) = tags::extract_tags(message);

    let trimmed = text.trim();

    // Opening report: "de CALLSIGN <R+NN>"
    // VarAC's is-typing marker is the bare text "&IT&" (not a <tag>)
    if message.trim_start().starts_with("&IT&") {
        return IncomingMessage::IsTyping;
    }
    // Reaction: "&L1" + id = like the line, "&L0" + id = take it back (VarAC)
    if let Some(r) = message.trim().strip_prefix("&L") {
        let mut chars = r.chars();
        if let (Some(kind @ ('0' | '1')), Ok(id)) = (chars.next(), chars.as_str().parse::<u32>()) {
            return IncomingMessage::Reaction { like: kind == '1', id };
        }
    }
    if let Some(m) = parse_feature_message(message) {
        return m;
    }
    // Tag-only replies to <INFO> and <SNRR>
    if trimmed.is_empty() && !tag_list.is_empty() {
        if tag_list.iter().all(|t| matches!(t, Tag::Name(_) | Tag::Qth(_) | Tag::Locator(_) | Tag::Email(_))) {
            let get = |f: fn(&Tag) -> Option<String>| tag_list.iter().find_map(f);
            return IncomingMessage::StationInfo {
                name: get(|t| if let Tag::Name(v) = t { Some(v.clone()) } else { None }),
                qth: get(|t| if let Tag::Qth(v) = t { Some(v.clone()) } else { None }),
                locator: get(|t| if let Tag::Locator(v) = t { Some(v.clone()) } else { None }),
                email: get(|t| if let Tag::Email(v) = t { Some(v.clone()) } else { None }),
            };
        }
        if let [Tag::SignalReport(db)] = tag_list.as_slice() {
            return IncomingMessage::SnrReport(*db as i32);
        }
    }
    // VarAC: "[welcome ]de CALL <R+NN>[ <S:..>]"; with a complex callsign the call is
    // the tag itself: "de <FC:W/4Z1AC/QRP> <R+NN>" (no text after "de")
    // (EmComm stations may send no report: "de CALL <S:V>" or just "de CALL")
    let has_report = tag_list.iter().any(|t| matches!(t, Tag::SignalReport(_)));
    let has_id_tag = tag_list.iter().any(|t| matches!(t, Tag::Services(_) | Tag::FullCallsign(_)));
    let words: Vec<&str> = trimmed.split_whitespace().collect();
    let opening = match words.as_slice() {
        [rest @ .., "de", call]
            if has_report || has_id_tag || (rest.is_empty() && crate::varac::messages::is_valid_callsign(call)) =>
        {
            Some((rest.len(), call.to_string()))
        }
        [rest @ .., "de"] if has_report || has_id_tag => Some((rest.len(), String::new())),
        _ => None,
    };
    if let Some((welcome_words, call)) = opening {
        let callsign = if call.is_empty() {
            tag_list.iter().find_map(|t| if let Tag::FullCallsign(fc) = t { Some(fc.clone()) } else { None }).unwrap_or_default()
        } else {
            call
        };
        // The welcome is the words before the final "de"
        let welcome = (welcome_words > 0)
            .then(|| trimmed.split_whitespace().take(welcome_words).collect::<Vec<_>>().join(" "));
        let snr = tag_list.iter().find_map(|t| {
            if let Tag::SignalReport(db) = t { Some(*db) } else { None }
        }).unwrap_or(0);
        let name = tag_list.iter().find_map(|t| {
            if let Tag::Name(n) = t { Some(n.clone()) } else { None }
        });
        let qth = tag_list.iter().find_map(|t| {
            if let Tag::Qth(q) = t { Some(q.clone()) } else { None }
        });
        let locator = tag_list.iter().find_map(|t| {
            if let Tag::Locator(l) = t { Some(l.clone()) } else { None }
        });
        let full_call = tag_list.iter().find_map(|t| {
            if let Tag::FullCallsign(fc) = t { Some(fc.clone()) } else { None }
        });
        let services = tag_list.iter().find_map(|t| {
            if let Tag::Services(s) = t { Some(ServiceFlags::from_letters(s)) } else { None }
        }).unwrap_or(ServiceFlags(0));

        return IncomingMessage::Opening(OpeningReport {
            callsign,
            signal_report_db: snr,
            name,
            qth,
            locator,
            full_callsign: full_call,
            services,
            welcome,
            has_report,
        });
    }

    // VMail: starts with <SM>, <SMU>, or <SMP>
    if matches!(tag_list.first(), Some(Tag::Sm) | Some(Tag::Smu) | Some(Tag::Smp)) {
        let is_parked = tag_list.first() == Some(&Tag::Smp);
        // <SM>…<U> (VarAC); <SMU> still accepted
        let is_urgent = tag_list.first() == Some(&Tag::Smu) || tag_list.contains(&Tag::Urgent);
        let to = tag_list.iter().find_map(|t| {
            if let Tag::VmailTo(v) = t { Some(v.clone()) } else { None }
        }).unwrap_or_default();
        let from = tag_list.iter().find_map(|t| {
            if let Tag::VmailFrom(v) = t { Some(v.clone()) } else { None }
        }).unwrap_or_default();
        let subject = tag_list.iter().find_map(|t| {
            if let Tag::Subject(v) = t { Some(v.clone()) } else { None }
        }).unwrap_or_default();
        let body = tag_list.iter().find_map(|t| {
            if let Tag::Message(v) = t { Some(v.clone()) } else { None }
        }).unwrap_or_default();
        let compose_time = tag_list.iter().find_map(|t| {
            if let Tag::ComposeTime(v) = t { Some(v.clone()) } else { None }
        }).unwrap_or_default();

        // VarAC sends «» for <> inside VMail fields
        use crate::varac::messages::desanitize;
        return IncomingMessage::VMail(VMail {
            guid: String::new(),
            from: desanitize(&from),
            to: desanitize(&to),
            subject: desanitize(&subject),
            body: desanitize(&body),
            compose_time,
            timestamp: Utc::now(),
            urgent: is_urgent,
            is_parked,
            has_attachment: false,
            reply_to: tag_list.iter().find_map(|t| {
                if let Tag::EmailReplyTo(v) = t { Some(desanitize(v)) } else { None }
            }),
            raw: tag_list.contains(&Tag::EmailRaw),
        });
    }

    // VMail acknowledgement
    if tag_list.contains(&Tag::Smr) {
        return IncomingMessage::VMailAck;
    }
    if tag_list.contains(&Tag::VmailRejected) {
        return IncomingMessage::VMailRejected;
    }
    if tag_list.contains(&Tag::VmailFailed) {
        return IncomingMessage::VMailFailed;
    }
    if let Some(Tag::LastHeardCallsign(c)) = tag_list.iter().find(|t| matches!(t, Tag::LastHeardCallsign(_))) {
        return IncomingMessage::LastHeardCallsignRequest(c.clone());
    }
    if tag_list.contains(&Tag::LocatorRequest) {
        return IncomingMessage::LocatorRequest;
    }
    if tag_list.contains(&Tag::GpsRequest) {
        return IncomingMessage::GpsRequest;
    }
    if tag_list.contains(&Tag::LastConnectionsRequest) {
        return IncomingMessage::LastConnectionsRequest;
    }
    if tag_list.contains(&Tag::LastConnectionsRejected) {
        return IncomingMessage::LastConnectionsRejected;
    }

    // Email gateway answers (VarAC 16)
    if tag_list.contains(&Tag::EmailGatewayAddressRequest) {
        return IncomingMessage::EmailGatewayAddressRequest;
    }
    if tag_list.contains(&Tag::EmailGatewayRejected) {
        return IncomingMessage::EmailGatewayRejected;
    }
    if tag_list.contains(&Tag::EmailSent) {
        return IncomingMessage::EmailSent;
    }
    if tag_list.contains(&Tag::EmailFailed) {
        return IncomingMessage::EmailFailed;
    }

    // Disconnect
    if tag_list.contains(&Tag::Disc) {
        return IncomingMessage::Disconnect;
    }

    // Away variants
    if tag_list.contains(&Tag::Away) {
        let away_msg = if trimmed.is_empty() { None } else { Some(trimmed.to_string()) };
        return IncomingMessage::Away(away_msg);
    }
    if tag_list.contains(&Tag::Awq) {
        return IncomingMessage::AwayQsy;
    }
    if tag_list.contains(&Tag::Ae) {
        return IncomingMessage::AwayTimeExceeded;
    }
    if tag_list.contains(&Tag::Ie) {
        return IncomingMessage::IdleTimeExceeded;
    }

    // VMail waiting (partner has VMail for us, needs QSY)
    if tag_list.contains(&Tag::Vw) {
        return IncomingMessage::VMailWaiting;
    }

    // Parking not allowed
    if tag_list.contains(&Tag::Smfp) {
        return IncomingMessage::ParkingNotAllowed;
    }

    // BBS listing response: <BL:name|date|size\r\nname|date|size\r\n...>
    if let Some(Tag::BbsListing(data)) = tag_list.iter().find(|t| matches!(t, Tag::BbsListing(_))) {
        let entries: Vec<crate::varac::messages::BbsListingEntry> = data
            .split('\n')
            .filter(|line| !line.trim().is_empty())
            .filter_map(|line| {
                let parts: Vec<&str> = line.trim().split('|').collect();
                if parts.len() >= 3 {
                    Some(crate::varac::messages::BbsListingEntry {
                        name: parts[0].to_string(),
                        date: parts[1].to_string(),
                        size: parts[2].parse().unwrap_or(0),
                    })
                } else {
                    None
                }
            })
            .collect();
        return IncomingMessage::BbsListing(entries);
    }

    // BBS requests
    if tag_list.contains(&Tag::BbsListRequest) {
        return IncomingMessage::BbsListRequest;
    }
    if let Some(Tag::BbsFileRequest(name)) = tag_list.iter().find(|t| matches!(t, Tag::BbsFileRequest(_))) {
        return IncomingMessage::BbsFileRequest(name.clone());
    }
    if tag_list.contains(&Tag::FileReady) {
        return IncomingMessage::BbsFileReady;
    }
    if tag_list.contains(&Tag::FileOk) {
        return IncomingMessage::BbsFileOk;
    }
    if tag_list.contains(&Tag::FileAborted) {
        return IncomingMessage::BbsFileAborted;
    }
    if tag_list.contains(&Tag::FileFailed) {
        return IncomingMessage::BbsFileFailed;
    }
    if tag_list.contains(&Tag::BbsListRejected) {
        return IncomingMessage::BbsListRejected;
    }
    if tag_list.contains(&Tag::BbsFileRejected) {
        return IncomingMessage::BbsFileRejected;
    }

    // File payload: <SFB>base64data or <SFB:base64data>
    if tag_list.contains(&Tag::Sfb) {
        let payload = trimmed.to_string();
        return IncomingMessage::BbsFilePayload(payload.into_bytes());
    }
    if let Some(Tag::FilePayload(data)) = tag_list.iter().find(|t| matches!(t, Tag::FilePayload(_))) {
        return IncomingMessage::BbsFilePayload(data.clone().into_bytes());
    }

    // File offer
    if let Some(Tag::FileOffer(offer_str)) = tag_list.iter().find(|t| matches!(t, Tag::FileOffer(_))) {
        if let Some(offer) = crate::varac::messages::BbsFileOffer::parse(offer_str) {
            return IncomingMessage::BbsFileOffer(offer);
        }
    }
    if let Some(Tag::VmailFileOffer(offer_str)) = tag_list.iter().find(|t| matches!(t, Tag::VmailFileOffer(_))) {
        if let Some(offer) = crate::varac::messages::BbsFileOffer::parse(offer_str) {
            return IncomingMessage::VMailAttachmentOffer(offer);
        }
    }

    // QSY
    if let Some(Tag::QsyRelative(offset)) = tag_list.iter().find(|t| matches!(t, Tag::QsyRelative(_))) {
        return IncomingMessage::QsyRequest { offset_hz: offset * 10 };
    }
    if let Some(Tag::QsyAbsolute(freq)) = tag_list.iter().find(|t| matches!(t, Tag::QsyAbsolute(_))) {
        return IncomingMessage::QsyAbsolute { freq_hz: *freq };
    }
    if tag_list.contains(&Tag::QsyAccepted) {
        return IncomingMessage::QsyAccepted;
    }
    if tag_list.contains(&Tag::QsyRejected) {
        return IncomingMessage::QsyRejected;
    }
    if tag_list.contains(&Tag::QsyOutOfRange) {
        return IncomingMessage::QsyOutOfRange;
    }
    if tag_list.contains(&Tag::TestLink) {
        return IncomingMessage::TestLink;
    }
    if tag_list.contains(&Tag::QsyReady) {
        return IncomingMessage::QsyReady;
    }
    if tag_list.contains(&Tag::QsyAuto) {
        return IncomingMessage::QsyAuto;
    }
    if tag_list.contains(&Tag::QsyLock) {
        return IncomingMessage::QsyLock;
    }

    // Is typing indicator
    if tag_list.contains(&Tag::IsTyping) {
        return IncomingMessage::IsTyping;
    }

    // SNR request
    if tag_list.contains(&Tag::Snrr) {
        return IncomingMessage::SnrRequest;
    }

    // Extended SNR (VSI)
    if tag_list.contains(&Tag::Vsi) {
        return IncomingMessage::ExtendedSnrRequest;
    }
    if tag_list.contains(&Tag::VsiR) {
        return IncomingMessage::ExtendedSnrAccepted;
    }
    if tag_list.contains(&Tag::VsiJ) {
        return IncomingMessage::ExtendedSnrRejected;
    }
    if tag_list.contains(&Tag::VsiStop) {
        return IncomingMessage::ExtendedSnrStop;
    }

    // Info/version requests
    if tag_list.contains(&Tag::Info) {
        return IncomingMessage::InfoRequest;
    }
    if tag_list.contains(&Tag::InfoJ) {
        return IncomingMessage::InfoDenied;
    }
    if tag_list.contains(&Tag::Ver) {
        return IncomingMessage::VersionRequest;
    }

    // Service info in a standalone message
    if let Some(Tag::Services(s)) = tag_list.iter().find(|t| matches!(t, Tag::Services(_))) {
        return IncomingMessage::ServiceInfo(ServiceFlags::from_letters(s));
    }

    // Cluster
    if tag_list.contains(&Tag::ClusterStationsRequest) {
        return IncomingMessage::ClusterStationsRequest;
    }
    if let Some(Tag::ClusterStationsReply(data)) = tag_list.iter().find(|t| matches!(t, Tag::ClusterStationsReply(_))) {
        return IncomingMessage::ClusterStationsReply(data.clone());
    }
    if tag_list.contains(&Tag::ClusterStationsEmpty) {
        return IncomingMessage::ClusterStationsEmpty;
    }
    if let Some(Tag::ClusterMessage(msg)) = tag_list.iter().find(|t| matches!(t, Tag::ClusterMessage(_))) {
        return IncomingMessage::ClusterMessage(msg.clone());
    }

    // Last-heard peeking
    if tag_list.contains(&Tag::Lhp) {
        return IncomingMessage::LastHeardRequest;
    }
    if let Some(Tag::Lhr) = tag_list.iter().find(|t| matches!(t, Tag::Lhr)) {
        let reply_text = if trimmed.is_empty() { String::new() } else { trimmed.to_string() };
        return IncomingMessage::LastHeardReply(reply_text);
    }
    if tag_list.contains(&Tag::Lhe) {
        return IncomingMessage::LastHeardEmpty;
    }
    if tag_list.contains(&Tag::Lhj) {
        return IncomingMessage::LastHeardRejected;
    }

    // Relay/peeking
    if tag_list.contains(&Tag::RelayPeek) {
        return IncomingMessage::RelayPeek;
    }
    if tag_list.contains(&Tag::RelayPeekRejected) {
        return IncomingMessage::RelayPeekRejected;
    }
    if tag_list.contains(&Tag::FreqSchedule) {
        return IncomingMessage::FreqScheduleRequest;
    }
    if tag_list.contains(&Tag::FreqScheduleResponse) {
        let data = if trimmed.is_empty() { String::new() } else { trimmed.to_string() };
        return IncomingMessage::FreqScheduleResponse(data);
    }

    // AI gateway (questions and answers are matched in parse_feature_message)
    if tag_list.contains(&Tag::AiPromptError) {
        return IncomingMessage::AiError;
    }
    if tag_list.contains(&Tag::AiLimitExceeded) {
        return IncomingMessage::AiRateLimited;
    }
    if tag_list.contains(&Tag::AiPromptRejected) {
        return IncomingMessage::AiRejected;
    }
    if tag_list.contains(&Tag::AiDisable) {
        return IncomingMessage::AiDisable;
    }

    // Default: chat text (with optional reply-to)
    let reply_to = tag_list.iter().find_map(|t| {
        if let Tag::ReplyChat(id) = t { Some(id.clone()) } else { None }
    });
    if !trimmed.is_empty() {
        if reply_to.is_some() {
            return IncomingMessage::Chat(crate::varac::messages::ChatMessage {
                from: String::new(),
                to: String::new(),
                text: crate::varac::messages::desanitize(trimmed),
                timestamp: Utc::now(),
                disconnect_after: tag_list.contains(&Tag::Disc),
                reply_to,
            });
        }
        // Chat goes out raw (VarAC doesn't sanitize it), but VarAC shows «» as <>
        return IncomingMessage::RawText(crate::varac::messages::desanitize(trimmed));
    }

    IncomingMessage::RawText(message.to_string())
}

/// One raw input for the session (see `VaracSession::next_input`).
#[derive(Debug)]
pub enum SessionInput {
    Pending,
    Vara(VaraEvent),
    Data(Vec<u8>),
    Kiss(u8, Vec<u8>),
    TxStall,
    SlotTimeout,
    HandshakeTimeout,
    /// Time to send what follows our QSY (<QSYR> / <TL>)
    QsyAfter,
    /// VarAC's pause after our own frame is over: send the next
    TxResume,
}

/// Events emitted by the VarAC session.
#[derive(Debug)]
pub enum SessionEvent {
    Connected {
        source: String,
        destination: String,
        bandwidth: Option<String>,
    },
    Disconnected,
    Ptt(bool),
    BufferUpdate(u32),
    Busy(bool),
    CqFrame { source: String, bandwidth: u16 },
    /// VarAC ADVANCED beacon heard on the KISS port (callsign, locator, services).
    AdvancedBeacon(crate::vara::kiss::AdvancedBeacon),
    Message(IncomingMessage),
    /// Progress of the link-opening exchange (for the log).
    Handshake(String),
    /// A VarAC typed CQ heard (CHAT, INFO, SOS … with the caller's locator and slot)
    TypedCq(crate::vara::kiss::TypedCq),
    /// A QSO frame got VarAC's chat number (`outgoing`: ours, once transmitted).
    FrameId { outgoing: bool, id: u32, text: String },
    /// A frame of ours went to the modem: its text (lines joined by CRLF) and size in bytes.
    TxStarted { text: String, bytes: usize },
    /// Part of an incoming file frame (`<SFB>`) is here: bytes so far of the frame's total.
    RxProgress { received: usize, total: usize },
    /// A status line VarAC shows in the chat (e.g. a QSY request it ignored).
    Notice(String),
    /// Opening reports exchanged; normal traffic (queued VMail etc.) may start.
    HandshakeComplete,
    /// Handshake QSY accepted: both stations retune by this many Hz.
    QsyBy(i32),
    /// A QSY invitation to decide (VarAC's "QSY NOW" countdown, on both sides):
    /// `inviter` = we sent it. Answer with `answer_qsy`.
    QsyInvitation { target: QsyTarget, inviter: bool },
    /// The pending QSY was settled by the partner (accepted / rejected): close the countdown.
    QsyResolved,
    /// Partner allows auto-QSY: ask the operator for a slot, then call
    /// `send_qsy_invitation` (VarAC's "Send QSY Invitation" dialog).
    QsyInvitationRequested,
    VaraEvent(VaraEvent),
}

/// The incoming `<SFB>` file frame at the head of `buf`, not complete yet: (bytes of it
/// here, its declared length). The payload is base64, so one character is one byte.
pub fn partial_file_frame(buf: &[u8]) -> Option<(usize, usize)> {
    let space = buf.iter().take(12).position(|&b| b == b' ')?;
    let total: usize = std::str::from_utf8(&buf[..space]).ok()?.parse().ok()?;
    let payload = &buf[space + 1..];
    if !payload.starts_with(b"<SFB") || total == 0 {
        return None;
    }
    Some((payload.len().min(total), total))
}

/// Zip a VMail's attachments the way VarAC packages them (`<SFM:name.zip…>`).
/// VarAC's ceiling for a compressed VMail attachment on a link of this bandwidth (none
/// at 2750 Hz or unknown).
pub fn attachment_limit(bandwidth: Option<u16>) -> Option<usize> {
    match bandwidth? {
        500 => Some(100_000),
        2300 => Some(1_000_000),
        _ => None,
    }
}

/// VarAC's name for a VMail attachment package: the subject without characters Windows
/// forbids in file names, '|' or '>', trimmed, spaces to '_', at most 30 characters
/// ("VMail" when nothing is left), then `_yyyyMMdd_HHmmss.zip` (UTC).
pub fn vmail_package_name(subject: &str, now: chrono::DateTime<Utc>) -> String {
    let kept: String = subject.chars()
        .filter(|&c| !(c < ' ' || "\"<>|:*?\\/".contains(c)))
        .collect();
    let mut stem: String = kept.trim().replace(' ', "_");
    if stem.encode_utf16().count() > 30 {
        stem = String::from_utf16_lossy(&stem.encode_utf16().take(30).collect::<Vec<u16>>());
    }
    if stem.is_empty() {
        stem = "VMail".into();
    }
    format!("{stem}_{}.zip", now.format("%Y%m%d_%H%M%S"))
}

pub fn vmail_package(files: &[(String, Vec<u8>)]) -> Result<Vec<u8>, std::io::Error> {
    use std::io::Write;
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (name, data) in files {
        zip.start_file(name.as_str(), opts).map_err(std::io::Error::other)?;
        zip.write_all(data)?;
    }
    Ok(zip.finish().map_err(std::io::Error::other)?.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_file_frame_progress() {
        assert_eq!(partial_file_frame(b"105 <SFB>H4sIAAAA"), Some((13, 105)));
        // more than the frame (the next one started): capped at its length
        assert_eq!(partial_file_frame(b"9 <SFB>QUJD12 <Q>"), Some((9, 9)));
        assert_eq!(partial_file_frame(b"12 <Q>hello"), None);
        assert_eq!(partial_file_frame(b"<SFB>"), None);
        assert_eq!(partial_file_frame(b""), None);
    }

    #[test]
    fn parse_opening_report() {
        let msg = "de W1ABC <R+05><NAME:John><QTH:Boston><LOC:FN42><S:VEB>";
        let result = parse_application_message(msg);
        match result {
            IncomingMessage::Opening(report) => {
                assert_eq!(report.callsign, "W1ABC");
                assert_eq!(report.signal_report_db, 5);
                assert_eq!(report.name.as_deref(), Some("John"));
                assert_eq!(report.qth.as_deref(), Some("Boston"));
                assert_eq!(report.locator.as_deref(), Some("FN42"));
                assert!(report.services.has_vmail());
                assert!(report.services.has_email());
                assert!(!report.services.has_ai());
                assert!(report.services.has_bbs());
            }
            other => panic!("Expected Opening, got {other:?}"),
        }
    }

    /// Openings VarAC sends with a welcome in front, or with a complex callsign; a Call ID
    /// later in the QSO looks like a bare opening.
    #[test]
    fn parse_opening_with_welcome_or_full_callsign() {
        match parse_application_message("Welcome to my station de NR5P <R+18> <S:VE>") {
            IncomingMessage::Opening(r) => {
                assert_eq!(r.callsign, "NR5P");
                assert_eq!(r.welcome.as_deref(), Some("Welcome to my station"));
                assert!(r.services.has_email());
            }
            other => panic!("{other:?}"),
        }
        match parse_application_message("de <FC:W/4Z1AC/QRP> <R-03>") {
            IncomingMessage::Opening(r) => {
                assert_eq!(r.callsign, "W/4Z1AC/QRP");
                assert_eq!(r.full_callsign.as_deref(), Some("W/4Z1AC/QRP"));
                assert_eq!(r.signal_report_db, -3);
                assert_eq!(r.welcome, None);
            }
            other => panic!("{other:?}"),
        }
        match parse_application_message("Hi de <FC:W/4Z1AC/QRP> <R+01>") {
            IncomingMessage::Opening(r) => assert_eq!((r.callsign.as_str(), r.welcome.as_deref()), ("W/4Z1AC/QRP", Some("Hi"))),
            other => panic!("{other:?}"),
        }
        match parse_application_message("de NR5P <R+07>") {
            IncomingMessage::Opening(r) => assert_eq!((r.callsign.as_str(), r.welcome), ("NR5P", None)),
            other => panic!("{other:?}"),
        }
        assert!(!matches!(parse_application_message("the dog de bark"), IncomingMessage::Opening(_)));
        match parse_application_message("<NAME:Bob><EM:bob@example.com>") {
            IncomingMessage::StationInfo { email, .. } => assert_eq!(email.as_deref(), Some("bob@example.com")),
            other => panic!("{other:?}"),
        }
    }

    /// EmComm stations may open without a report: "de CALL <S:V>" or plain "de CALL".
    #[test]
    fn parse_opening_without_report() {
        match parse_application_message("de NR5P <S:V>") {
            IncomingMessage::Opening(r) => assert_eq!((r.callsign.as_str(), r.has_report), ("NR5P", false)),
            other => panic!("{other:?}"),
        }
        match parse_application_message("de NR5P") {
            IncomingMessage::Opening(r) => assert_eq!(r.callsign, "NR5P"),
            other => panic!("{other:?}"),
        }
        assert!(matches!(parse_application_message("de NR5P <R+05>"), IncomingMessage::Opening(r) if r.has_report));
        assert!(!matches!(parse_application_message("see you de nowhere"), IncomingMessage::Opening(_)));
        assert!(!matches!(parse_application_message("de"), IncomingMessage::Opening(_)));
    }

    #[test]
    fn parse_opening_negative_snr() {
        let msg = "de 4Z1AC <R-12>";
        let result = parse_application_message(msg);
        match result {
            IncomingMessage::Opening(report) => {
                assert_eq!(report.callsign, "4Z1AC");
                assert_eq!(report.signal_report_db, -12);
            }
            other => panic!("Expected Opening, got {other:?}"),
        }
    }

    #[test]
    fn parse_vmail() {
        let msg = "<SM><TME:2026-10-28 12:00:00><TO 4Z1AC><FRM:W1ABC><SBJ:Test><MSG:Hello world>";
        let result = parse_application_message(msg);
        match result {
            IncomingMessage::VMail(vmail) => {
                assert_eq!(vmail.to, "4Z1AC");
                assert_eq!(vmail.from, "W1ABC");
                assert_eq!(vmail.subject, "Test");
                assert_eq!(vmail.body, "Hello world");
                assert_eq!(vmail.compose_time, "2026-10-28 12:00:00");
                assert!(!vmail.is_parked);
            }
            other => panic!("Expected VMail, got {other:?}"),
        }
    }

    #[test]
    fn parse_parked_vmail() {
        let msg = "<SMP><TME:2026-10-28 12:00:00><TO 4Z1AC><FRM:W1ABC><SBJ:Hi><MSG:Parked>";
        let result = parse_application_message(msg);
        match result {
            IncomingMessage::VMail(vmail) => {
                assert!(vmail.is_parked);
            }
            other => panic!("Expected VMail, got {other:?}"),
        }
    }

    #[test]
    fn parse_urgent_vmail() {
        let msg = "<SMU><TME:2026-10-28 12:00:00><TO 4Z1AC><FRM:W1ABC><SBJ:Urgent!><MSG:Important>";
        let result = parse_application_message(msg);
        match result {
            IncomingMessage::VMail(vmail) => {
                assert!(vmail.urgent);
                assert!(!vmail.is_parked);
                assert_eq!(vmail.subject, "Urgent!");
            }
            other => panic!("Expected VMail, got {other:?}"),
        }
    }

    #[test]
    fn parse_vmail_ack() {
        assert!(matches!(parse_application_message("<SMR>"), IncomingMessage::VMailAck));
    }

    #[test]
    fn parse_disconnect() {
        assert!(matches!(parse_application_message("<DISC>"), IncomingMessage::Disconnect));
    }

    #[test]
    fn parse_away() {
        match parse_application_message("<AWAY>") {
            IncomingMessage::Away(None) => {}
            other => panic!("Expected Away(None), got {other:?}"),
        }
        match parse_application_message("Back in 10 <AWAY>") {
            IncomingMessage::Away(Some(msg)) => assert_eq!(msg, "Back in 10"),
            other => panic!("Expected Away(Some), got {other:?}"),
        }
    }

    #[test]
    fn parse_bbs_list_request() {
        assert!(matches!(parse_application_message("<BLR>"), IncomingMessage::BbsListRequest));
    }

    #[test]
    fn parse_file_offer() {
        let msg = "<SF:notes.txt.VarAC_gzip|1024|ABCDEF1234567890ABCDEF1234567890>";
        let result = parse_application_message(msg);
        match result {
            IncomingMessage::BbsFileOffer(offer) => {
                assert_eq!(offer.name, "notes.txt.VarAC_gzip");
                assert_eq!(offer.compressed_size, 1024);
                assert_eq!(offer.md5_hex, "ABCDEF1234567890ABCDEF1234567890");
            }
            other => panic!("Expected BbsFileOffer, got {other:?}"),
        }
    }

    #[test]
    fn parse_qsy_relative() {
        match parse_application_message("<Q:75>") {
            IncomingMessage::QsyRequest { offset_hz } => assert_eq!(offset_hz, 750),
            other => panic!("Expected QsyRequest, got {other:?}"),
        }
    }

    #[test]
    fn parse_chat_text() {
        match parse_application_message("Hello 73!") {
            IncomingMessage::RawText(t) => assert_eq!(t, "Hello 73!"),
            other => panic!("Expected RawText, got {other:?}"),
        }
    }

    #[test]
    fn parse_services_standalone() {
        match parse_application_message("<S:VA>") {
            IncomingMessage::ServiceInfo(flags) => {
                assert!(flags.has_vmail());
                assert!(flags.has_ai());
                assert!(!flags.has_email());
            }
            other => panic!("Expected ServiceInfo, got {other:?}"),
        }
    }

    #[test]
    fn parse_file_payload_bare_tag() {
        let msg = "<SFB>SGVsbG8gV29ybGQ=";
        let result = parse_application_message(msg);
        match result {
            IncomingMessage::BbsFilePayload(data) => {
                assert_eq!(String::from_utf8_lossy(&data), "SGVsbG8gV29ybGQ=");
            }
            other => panic!("Expected BbsFilePayload, got {other:?}"),
        }
    }

    #[test]
    fn parse_file_payload_value_tag() {
        let msg = "<SFB:SGVsbG8gV29ybGQ=>";
        let result = parse_application_message(msg);
        match result {
            IncomingMessage::BbsFilePayload(data) => {
                assert_eq!(String::from_utf8_lossy(&data), "SGVsbG8gV29ybGQ=");
            }
            other => panic!("Expected BbsFilePayload, got {other:?}"),
        }
    }

    #[test]
    fn parse_bbs_listing() {
        let msg = "<BL:notes.txt|2026-10-28|512\r\nphoto.jpg|2026-10-27|4096>";
        let result = parse_application_message(msg);
        match result {
            IncomingMessage::BbsListing(entries) => {
                assert_eq!(entries.len(), 2);
                assert_eq!(entries[0].name, "notes.txt");
                assert_eq!(entries[0].date, "2026-10-28");
                assert_eq!(entries[0].size, 512);
                assert_eq!(entries[1].name, "photo.jpg");
                assert_eq!(entries[1].size, 4096);
            }
            other => panic!("Expected BbsListing, got {other:?}"),
        }
    }

    #[test]
    fn parse_file_ready() {
        assert!(matches!(parse_application_message("<SFRD>"), IncomingMessage::BbsFileReady));
    }

    #[test]
    fn parse_file_ok() {
        assert!(matches!(parse_application_message("<SFOK>"), IncomingMessage::BbsFileOk));
    }

    #[test]
    fn parse_file_aborted() {
        assert!(matches!(parse_application_message("<SFAB>"), IncomingMessage::BbsFileAborted));
    }

    #[test]
    fn parse_info_request() {
        assert!(matches!(parse_application_message("<INFO>"), IncomingMessage::InfoRequest));
    }

    #[test]
    fn parse_version_request() {
        assert!(matches!(parse_application_message("<VER>"), IncomingMessage::VersionRequest));
    }

    #[test]
    fn parse_test_link() {
        assert!(matches!(parse_application_message("<TL>"), IncomingMessage::TestLink));
    }

    #[test]
    fn parse_qsy_accepted_rejected() {
        assert!(matches!(parse_application_message("<QSYR>"), IncomingMessage::QsyAccepted));
        assert!(matches!(parse_application_message("<QSYJ>"), IncomingMessage::QsyRejected));
        assert!(matches!(parse_application_message("<QJO>"), IncomingMessage::QsyOutOfRange));
    }

    #[test]
    fn parse_cluster() {
        // Cluster stations: request / empty / reply
        assert!(matches!(parse_application_message("<CCR>"), IncomingMessage::ClusterStationsRequest));
        assert!(matches!(parse_application_message("<CCE>"), IncomingMessage::ClusterStationsEmpty));
        match parse_application_message("<CC:W1ABC,NR5P>") {
            IncomingMessage::ClusterStationsReply(s) => assert_eq!(s, "W1ABC,NR5P"),
            other => panic!("Expected ClusterStationsReply, got {other:?}"),
        }
    }

    #[test]
    fn parse_last_heard() {
        // <LHR> is VarAC's last-heard *request*; <LHC:CALL> asks about one station
        assert!(matches!(parse_application_message("<LHR>"), IncomingMessage::LastHeardRequest));
        assert!(matches!(parse_application_message("<LHC:w5tst>"), IncomingMessage::LastHeardCallsignRequest(c) if c == "W5TST"));
        assert!(matches!(parse_application_message("<LOCR>"), IncomingMessage::LocatorRequest));
        assert!(matches!(parse_application_message("<GPSR>"), IncomingMessage::GpsRequest));
        assert!(matches!(parse_application_message("<LHCE>"), IncomingMessage::LastHeardEmpty));
        assert!(matches!(parse_application_message("<LHE>"), IncomingMessage::LastHeardEmpty));
        assert!(matches!(parse_application_message("<LHJ>"), IncomingMessage::LastHeardRejected));
    }

    #[test]
    fn parse_hamplay_cluster_pactor() {
        use crate::varac::messages::HamPlayMsg;
        assert!(matches!(parse_application_message("<P:2>"), IncomingMessage::HamPlay(HamPlayMsg::Invite(2))));
        assert!(matches!(parse_application_message("<PM:12>"), IncomingMessage::HamPlay(HamPlayMsg::Move(ref m)) if m == "12"));
        assert!(matches!(parse_application_message("<PR>"), IncomingMessage::HamPlay(HamPlayMsg::Accept)));
        assert!(matches!(parse_application_message("<PJ>"), IncomingMessage::HamPlay(HamPlayMsg::Reject)));
        assert!(matches!(parse_application_message("<PA>"), IncomingMessage::HamPlay(HamPlayMsg::Abort)));
        assert!(matches!(parse_application_message("<PE>"), IncomingMessage::HamPlay(HamPlayMsg::Rematch)));
        assert!(matches!(parse_application_message("<CIR>"), IncomingMessage::ClusterInstancesRequest));
        assert!(matches!(parse_application_message("K1ABC>hello all <CM>"), IncomingMessage::ClusterRelayed(ref t) if t == "K1ABC>hello all"));
        assert!(matches!(parse_application_message("<SP:3>"), IncomingMessage::PactorLevelRequest(ref n) if n == "3"));
    }

    #[test]
    fn parse_ai_gateway() {
        assert!(matches!(parse_application_message("<SAFIE>"), IncomingMessage::AiResponse(ref a) if a.is_empty()));
        assert!(matches!(parse_application_message("<ACIE>Sunny today, 73"), IncomingMessage::AiResponse(ref a) if a == "Sunny today, 73"));
        assert!(matches!(parse_application_message("ai:what is VARA?"), IncomingMessage::AiPrompt(ref q) if q == "what is VARA?"));
        // non-ASCII at the start must not panic (a cp1252 "Sí" arrives as "S\u{FFFD}")
        assert!(!matches!(parse_application_message("S\u{FFFD}"), IncomingMessage::AiPrompt(_)));
        assert!(!matches!(parse_application_message("Já"), IncomingMessage::AiPrompt(_)));
        // VarAC's own reply-with-AI quirk: <C:id>AI:… isn't a question (it doesn't start with AI:)
        assert!(!matches!(parse_application_message("<C:3>AI:hi"), IncomingMessage::AiPrompt(_)));
        assert!(matches!(parse_application_message("<AIE>"), IncomingMessage::AiError));
        assert!(matches!(parse_application_message("<AIL>"), IncomingMessage::AiRateLimited));
        assert!(matches!(parse_application_message("<AIJ>"), IncomingMessage::AiRejected));
        assert!(matches!(parse_application_message("<DISAI>"), IncomingMessage::AiDisable));
    }

    #[test]
    fn parse_relay_freq_schedule() {
        // VarAC: <VRP>/<VRPJ> parked-VMail peek; <FSR> schedule request, <FSO> scheduler off
        assert!(matches!(parse_application_message("<VRP>"), IncomingMessage::RelayPeek));
        assert!(matches!(parse_application_message("<VRPJ>"), IncomingMessage::RelayPeekRejected));
        assert!(matches!(parse_application_message("<FSR>"), IncomingMessage::FreqScheduleRequest));
        assert!(matches!(parse_application_message("<FSO>"), IncomingMessage::FreqScheduleResponse(_)));
    }

    #[test]
    fn parse_session_control_tags() {
        assert!(matches!(parse_application_message("<AWQ>"), IncomingMessage::AwayQsy));
        assert!(matches!(parse_application_message("<AE>"), IncomingMessage::AwayTimeExceeded));
        assert!(matches!(parse_application_message("<IE>"), IncomingMessage::IdleTimeExceeded));
        assert!(matches!(parse_application_message("<VW>"), IncomingMessage::VMailWaiting));
        assert!(matches!(parse_application_message("<SMFP>"), IncomingMessage::ParkingNotAllowed));
        // <ER> is VarAC's "email delivery succeeded", not an error
        assert!(matches!(parse_application_message("<ER>"), IncomingMessage::EmailSent));
        assert!(matches!(parse_application_message("<EF>"), IncomingMessage::EmailFailed));
        assert!(matches!(parse_application_message("<EJ>"), IncomingMessage::EmailGatewayRejected));
    }

    #[test]
    fn parse_extended_snr() {
        assert!(matches!(parse_application_message("<VSI>"), IncomingMessage::ExtendedSnrRequest));
        assert!(matches!(parse_application_message("<VSIR>"), IncomingMessage::ExtendedSnrAccepted));
        assert!(matches!(parse_application_message("<VSIJ>"), IncomingMessage::ExtendedSnrRejected));
        assert!(matches!(parse_application_message("<VSS>"), IncomingMessage::ExtendedSnrStop));
        // <SR> is VarAC's current SNR request, <SNRR> its old one
        assert!(matches!(parse_application_message("<SR>"), IncomingMessage::SnrRequest));
        assert!(matches!(parse_application_message("<SNRR>"), IncomingMessage::SnrRequest));
    }

    #[test]
    fn urgent_vmail_is_sm_with_u() {
        // VarAC: <SM>…<MSG:…><U>; <SMU> (urgent parked) still accepted on receipt
        match parse_application_message("<SM><TME:2026-10-30 12:00:00><SBJ:x><MSG:y><U>") {
            IncomingMessage::VMail(vm) => assert!(vm.urgent),
            other => panic!("{other:?}"),
        }
        let vm = VMail {
            guid: String::new(), from: "W5TST".into(), to: "NR5P".into(), subject: "x".into(), body: "y".into(),
            compose_time: "2026-10-30 12:00:00".into(), timestamp: chrono::Utc::now(), urgent: true,
            is_parked: false, has_attachment: false, reply_to: None, raw: false,
        };
        assert_eq!(vm.to_wire(), "<SM><TME:2026-10-30 12:00:00><TO:NR5P><FRM:W5TST><SBJ:x><MSG:y><U>");
    }

    #[test]
    fn parse_reply_to_chat() {
        // VarAC's reply-to-chat is <C:id>
        match parse_application_message("<C:42>Hello reply") {
            IncomingMessage::Chat(msg) => {
                assert_eq!(msg.text, "Hello reply");
                assert_eq!(msg.reply_to, Some("42".to_string()));
            }
            other => panic!("Expected Chat with reply_to, got {other:?}"),
        }
    }

    #[test]
    fn parse_qsy_ready_auto_lock() {
        assert!(matches!(parse_application_message("<Q>"), IncomingMessage::QsyReady));

    }

    #[test]
    fn parse_info_denied() {
        assert!(matches!(parse_application_message("<INFOJ>"), IncomingMessage::InfoDenied));
    }
}

#[cfg(test)]
mod handshake_tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    async fn read_frame(d: &mut tokio::net::TcpStream) -> String {
        let mut buf = vec![0u8; 512];
        let n = tokio::time::timeout(std::time::Duration::from_secs(3), d.read(&mut buf)).await.unwrap().unwrap();
        String::from_utf8_lossy(&buf[..n]).into_owned()
    }

    /// Relay notifications heard on the KISS port (as captured: K6HN -> JI1BQW "<SMP>"): one
    /// for us says VMail is waiting; one for another station is only logged, never listed as
    /// a broadcast; an ordinary broadcast still is.
    #[tokio::test]
    async fn relay_notifications_for_others_arent_broadcasts() {
        use crate::vara::kiss::VARAC_KISS_CMD;
        use crate::varac::messages::ui_frame;
        let cmd = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let data = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (cp, dp) = (cmd.local_addr().unwrap().port(), data.local_addr().unwrap().port());
        let modem = tokio::spawn(async move { (cmd.accept().await.unwrap(), data.accept().await.unwrap()) });
        let vara = VaraConnection::connect("127.0.0.1", cp, dp).await.unwrap();
        let _modem = modem.await.unwrap();
        let cfg = StationConfig {
            callsign: "NR5P".into(), name: None, qth: None, locator: None,
            services: ServiceFlags(ServiceFlags::VMAIL), bbs_dir: None, email_gate: None, bbs_gate: None, policy: Default::default(),
        };
        let mut s = VaracSession::new(vara, cfg);
        async fn heard(s: &mut VaracSession, from: &str, to: &str, text: &str) -> Option<SessionEvent> {
            let raw = ui_frame(from, to, text).encode(VARAC_KISS_CMD);
            s.handle_input(SessionInput::Kiss(VARAC_KISS_CMD, raw)).await
        }
        // for another station: a note in the log, naming both
        for text in ["<SMP>", "<SMU>"] {
            match heard(&mut s, "K6HN", "JI1BQW", text).await {
                Some(SessionEvent::Handshake(note)) => assert!(note.contains("K6HN") && note.contains("JI1BQW"), "{note}"),
                other => panic!("{text} for another station: {other:?}"),
            }
        }
        // for us: VMail waiting
        assert!(matches!(heard(&mut s, "K6HN", "NR5P", "<SMU>").await,
            Some(SessionEvent::Message(IncomingMessage::RelayNotification { ref relay, urgent: true })) if relay == "K6HN"));
        // a real broadcast is still one
        assert!(matches!(heard(&mut s, "K6HN", "ALL", "QRV on 40m").await,
            Some(SessionEvent::Message(IncomingMessage::Broadcast(ref b))) if b.message == "QRV on 40m"));
    }

    /// Captured: original VarAC with its email gateway on answers our call with just its
    /// opening (no <Q>). We open a few seconds later instead of waiting out the 45 s.
    #[tokio::test]
    async fn caller_opens_soon_when_callee_offers_no_qsy() {
        let cmd = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let data = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (cp, dp) = (cmd.local_addr().unwrap().port(), data.local_addr().unwrap().port());
        let modem = tokio::spawn(async move {
            let (mut c, _) = cmd.accept().await.unwrap();
            let (mut d, _) = data.accept().await.unwrap();
            c.write_all(b"CONNECTED W5TST NR5P 500\rSN 18.0\r").await.unwrap();
            d.write_all(b"22 de NR5P <R+18> <S:VEB>").await.unwrap();
            let t = std::time::Instant::now();
            let f = read_frame(&mut d).await;
            (f, t.elapsed())
        });
        let vara = VaraConnection::connect("127.0.0.1", cp, dp).await.unwrap();
        let cfg = StationConfig {
            callsign: "W5TST".into(), name: None, qth: None, locator: None,
            services: ServiceFlags(ServiceFlags::VMAIL), bbs_dir: None, email_gate: None, bbs_gate: None, policy: Default::default(),
        };
        // (off a calling frequency: on one, VarAC's caller stays silent without a QSY)
        cfg.policy.on_calling_frequency.store(false, std::sync::atomic::Ordering::Relaxed);
        let mut s = VaracSession::new(vara, cfg);
        let mut done = false;
        let _ = tokio::time::timeout(std::time::Duration::from_secs(8), async {
            loop {
                if let Some(SessionEvent::HandshakeComplete) = s.poll().await {
                    done = true;
                    break;
                }
            }
        }).await;
        let (frame, took) = modem.await.unwrap();
        assert!(done, "handshake must complete");
        assert_eq!(frame, "15 de W5TST <R+18>");
        assert!(took < std::time::Duration::from_secs(7), "{took:?}");
    }

    /// Captured from original VarAC calling us: it answers our <Q> with its opening right
    /// away and sends <Q:n> only after its operator picks a slot. We must wait for the slot,
    /// then answer <QSYR> and our opening with services (it's how VarAC learns we're a gateway).
    #[tokio::test]
    async fn callee_waits_for_slot_after_callers_opening() {
        let cmd = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let data = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (cp, dp) = (cmd.local_addr().unwrap().port(), data.local_addr().unwrap().port());
        let modem = tokio::spawn(async move {
            let (mut c, _) = cmd.accept().await.unwrap();
            let (mut d, _) = data.accept().await.unwrap();
            c.write_all(b"SN 18.0\rCONNECTED NR5P W5TST 500\r").await.unwrap();
            let mut sent = vec![read_frame(&mut d).await];
            c.write_all(b"BUFFER 0\r").await.unwrap();
            d.write_all(b"14 de NR5P <R+18>").await.unwrap();
            // nothing may go out until the slot arrives
            let mut buf = [0u8; 64];
            assert!(tokio::time::timeout(std::time::Duration::from_millis(400), d.read(&mut buf)).await.is_err());
            d.write_all(b"7 <Q:225>").await.unwrap();
            sent.push(read_frame(&mut d).await);
            c.write_all(b"BUFFER 0\r").await.unwrap();
            sent.push(read_frame(&mut d).await);
            sent
        });
        let vara = VaraConnection::connect("127.0.0.1", cp, dp).await.unwrap();
        let cfg = StationConfig {
            callsign: "W5TST".into(),
            name: None,
            qth: None,
            locator: None,
            services: ServiceFlags(ServiceFlags::VMAIL | ServiceFlags::EMAIL),
            bbs_dir: None,
            email_gate: Some(crate::varac::email_gateway::EmailGate::new(true, None)),
            bbs_gate: None,
            policy: Default::default(),
        };
        let mut s = VaracSession::new(vara, cfg);
        let mut events = Vec::new();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(4), async {
            loop {
                if let Some(ev) = s.poll().await {
                    accept_qsy_for_test(&mut s, &ev).await;
                    events.push(format!("{ev:?}"));
                }
            }
        })
        .await;
        let sent = modem.await.unwrap();
        assert_eq!(sent, vec!["3 <Q>", "6 <QSYR>", "22 de W5TST <R+18> <S:VE>"]);
        assert!(events.iter().any(|e| e.contains("QsyBy(2250)")), "{events:?}");
        assert!(events.iter().any(|e| e.contains("Opening")), "{events:?}");
    }

    /// Captured with VarAC 16.1.8 on Windows: VarAC's opening came joined with a chat it
    /// had queued, before its operator picked a slot. Our opening still goes once, after
    /// the QSY, with our services.
    #[tokio::test]
    async fn callee_opens_after_late_slot_when_caller_talked_first() {
        let cmd = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let data = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (cp, dp) = (cmd.local_addr().unwrap().port(), data.local_addr().unwrap().port());
        let modem = tokio::spawn(async move {
            let (mut c, _) = cmd.accept().await.unwrap();
            let (mut d, _) = data.accept().await.unwrap();
            c.write_all(b"SN 18.0\rCONNECTED NR5P W5TST 500\r").await.unwrap();
            let mut sent = vec![read_frame(&mut d).await];
            c.write_all(b"BUFFER 0\r").await.unwrap();
            d.write_all(b"32 de NR5P <R+18>\r\nhello from varac").await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            d.write_all(b"7 <Q:-75>").await.unwrap();
            sent.push(read_frame(&mut d).await);
            c.write_all(b"BUFFER 0\r").await.unwrap();
            sent.push(read_frame(&mut d).await);
            sent
        });
        let vara = VaraConnection::connect("127.0.0.1", cp, dp).await.unwrap();
        let cfg = StationConfig {
            callsign: "W5TST".into(),
            name: None,
            qth: None,
            locator: None,
            services: ServiceFlags(ServiceFlags::VMAIL),
            bbs_dir: None,
            email_gate: None,
            bbs_gate: None,
            policy: Default::default(),
        };
        let mut s = VaracSession::new(vara, cfg);
        let mut events = Vec::new();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(4), async {
            loop {
                if let Some(ev) = s.poll().await {
                    accept_qsy_for_test(&mut s, &ev).await;
                    events.push(format!("{ev:?}"));
                }
            }
        })
        .await;
        let sent = modem.await.unwrap();
        assert_eq!(sent, vec!["3 <Q>", "6 <QSYR>", "21 de W5TST <R+18> <S:V>"]);
        assert!(events.iter().any(|e| e.contains("QsyBy(-750)")), "{events:?}");
    }

    /// The called station's opening carries the welcome and the complex callsign, and a
    /// second QSY invitation within 60 s is ignored (as in VarAC).
    #[tokio::test]
    async fn callee_welcome_full_callsign_and_double_qsy() {
        let cmd = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let data = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (cp, dp) = (cmd.local_addr().unwrap().port(), data.local_addr().unwrap().port());
        let modem = tokio::spawn(async move {
            let (mut c, _) = cmd.accept().await.unwrap();
            let (mut d, _) = data.accept().await.unwrap();
            c.write_all(b"SN 18.0\rCONNECTED NR5P W5TST 500\r").await.unwrap();
            let mut sent = vec![read_frame(&mut d).await];
            c.write_all(b"BUFFER 0\r").await.unwrap();
            d.write_all(b"7 <Q:150>").await.unwrap();
            sent.push(read_frame(&mut d).await);
            c.write_all(b"BUFFER 0\r").await.unwrap();
            sent.push(read_frame(&mut d).await);
            c.write_all(b"BUFFER 0\r").await.unwrap();
            // a second invitation right away: no <QSYR>
            d.write_all(b"6 <Q:75>").await.unwrap();
            let mut buf = [0u8; 64];
            assert!(tokio::time::timeout(std::time::Duration::from_millis(500), d.read(&mut buf)).await.is_err());
            sent
        });
        let vara = VaraConnection::connect("127.0.0.1", cp, dp).await.unwrap();
        let cfg = StationConfig {
            callsign: "W5TST".into(), name: None, qth: None, locator: None,
            services: ServiceFlags(ServiceFlags::VMAIL), bbs_dir: None, email_gate: None, bbs_gate: None,
            policy: Default::default(),
        };
        cfg.policy.set_welcome("Welcome!", false);
        cfg.policy.set_full_callsign("w5tst/p");
        let mut s = VaracSession::new(vara, cfg);
        let mut events = Vec::new();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                if let Some(ev) = s.poll().await {
                    accept_qsy_for_test(&mut s, &ev).await;
                    events.push(format!("{ev:?}"));
                }
            }
        })
        .await;
        let sent = modem.await.unwrap();
        assert_eq!(sent, vec!["3 <Q>", "6 <QSYR>", "37 Welcome! de <FC:W5TST/P> <R+18> <S:V>"]);
        assert!(events.iter().any(|e| e.contains("QsyBy(1500)")), "{events:?}");
        assert!(events.iter().any(|e| e.contains("DOUBLE QSY REQUEST IN 60 SECONDS")), "{events:?}");
        assert!(!events.iter().any(|e| e.contains("QsyBy(750)")), "{events:?}");
    }

    /// VarAC's chat numbers: every frame of the QSO in both directions, ours once
    /// transmitted; is-typing and reactions don't count. Reactions are "&L1n"/"&L0n".
    #[tokio::test]
    async fn frames_are_numbered_like_varac() {
        let cmd = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let data = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (cp, dp) = (cmd.local_addr().unwrap().port(), data.local_addr().unwrap().port());
        let modem = tokio::spawn(async move {
            let (mut c, _) = cmd.accept().await.unwrap();
            let (mut d, _) = data.accept().await.unwrap();
            c.write_all(b"CONNECTED NR5P W5TST 500\r").await.unwrap();
            let q = read_frame(&mut d).await;
            c.write_all(b"BUFFER 0\r").await.unwrap();
            for f in ["14 de NR5P <R+18>", "5 hello", "4 &IT&", "4 &L11", "5 again"] {
                d.write_all(f.as_bytes()).await.unwrap();
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            q
        });
        let vara = VaraConnection::connect("127.0.0.1", cp, dp).await.unwrap();
        let cfg = StationConfig {
            callsign: "W5TST".into(), name: None, qth: None, locator: None,
            services: ServiceFlags(ServiceFlags::VMAIL), bbs_dir: None, email_gate: None, bbs_gate: None,
            policy: Default::default(),
        };
        let mut s = VaracSession::new(vara, cfg);
        let mut events = Vec::new();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if let Some(ev) = s.poll().await {
                    events.push(ev);
                }
            }
        })
        .await;
        assert_eq!(modem.await.unwrap(), "3 <Q>");
        let ids: Vec<(bool, u32, String)> = events.iter().filter_map(|e| match e {
            SessionEvent::FrameId { outgoing, id, text } => Some((*outgoing, *id, text.clone())),
            _ => None,
        }).collect();
        assert_eq!(ids, vec![
            (true, 1, "<Q>".to_string()),
            (false, 2, "de NR5P <R+18>".to_string()),
            (false, 3, "hello".to_string()),
            (false, 4, "again".to_string()),
        ]);
        assert!(events.iter().any(|e| matches!(e, SessionEvent::Message(IncomingMessage::Reaction { like: true, id: 1 }))), "{events:?}");
        assert!(matches!(parse_application_message("&L012"), IncomingMessage::Reaction { like: false, id: 12 }));
        assert!(!matches!(parse_application_message("&Lx"), IncomingMessage::Reaction { .. }));
    }

    /// VarAC's QSY: the inviter counts down too and moves by itself — it doesn't wait
    /// for <QSYR> (its partner sends that only from the new frequency): <Q:150>, our
    /// countdown says yes, QsyBy(1500), then <TL> and our opening.
    #[tokio::test]
    async fn caller_moves_after_its_own_countdown() {
        let cmd = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let data = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (cp, dp) = (cmd.local_addr().unwrap().port(), data.local_addr().unwrap().port());
        let modem = tokio::spawn(async move {
            let (mut c, _) = cmd.accept().await.unwrap();
            let (mut d, _) = data.accept().await.unwrap();
            c.write_all(b"SN 12.0\rCONNECTED W5TST NR5P 500\r").await.unwrap();
            d.write_all(b"3 <Q>").await.unwrap();
            let mut sent = vec![read_frame(&mut d).await];
            c.write_all(b"BUFFER 0\r").await.unwrap();
            sent.push(read_frame(&mut d).await);
            c.write_all(b"BUFFER 0\r").await.unwrap();
            sent.push(read_frame(&mut d).await);
            sent
        });
        let vara = VaraConnection::connect("127.0.0.1", cp, dp).await.unwrap();
        let cfg = StationConfig {
            callsign: "W5TST".into(), name: None, qth: None, locator: None,
            services: ServiceFlags(ServiceFlags::VMAIL), bbs_dir: None, email_gate: None, bbs_gate: None,
            policy: Default::default(),
        };
        let mut s = VaracSession::new(vara, cfg);
        s.qsy_offset_tens_hz = 150;
        let mut events = Vec::new();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(4), async {
            loop {
                if let Some(ev) = s.poll().await {
                    accept_qsy_for_test(&mut s, &ev).await;
                    events.push(format!("{ev:?}"));
                }
            }
        })
        .await;
        assert_eq!(modem.await.unwrap(), vec!["7 <Q:150>", "4 <TL>", "15 de W5TST <R+12>"]);
        assert!(events.iter().any(|e| e.contains("QsyInvitation { target: Offset(1500), inviter: true }")), "{events:?}");
        assert!(events.iter().any(|e| e.contains("QsyBy(1500)")), "{events:?}");
    }

    /// Called on a slot (not a calling frequency): VarAC offers no QSY, it opens at once.
    #[tokio::test]
    async fn callee_off_the_calling_frequency_opens_at_once() {
        let cmd = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let data = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (cp, dp) = (cmd.local_addr().unwrap().port(), data.local_addr().unwrap().port());
        let modem = tokio::spawn(async move {
            let (mut c, _) = cmd.accept().await.unwrap();
            let (mut d, _) = data.accept().await.unwrap();
            c.write_all(b"SN 9.0\rCONNECTED NR5P W5TST 500\r").await.unwrap();
            read_frame(&mut d).await
        });
        let vara = VaraConnection::connect("127.0.0.1", cp, dp).await.unwrap();
        let cfg = StationConfig {
            callsign: "W5TST".into(), name: None, qth: None, locator: None,
            services: ServiceFlags(ServiceFlags::VMAIL), bbs_dir: None, email_gate: None, bbs_gate: None,
            policy: Default::default(),
        };
        cfg.policy.on_calling_frequency.store(false, std::sync::atomic::Ordering::Relaxed);
        let mut s = VaracSession::new(vara, cfg);
        let mut events = Vec::new();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if let Some(ev) = s.poll().await {
                    events.push(format!("{ev:?}"));
                }
            }
        })
        .await;
        assert_eq!(modem.await.unwrap(), "21 de W5TST <R+09> <S:V>");
        assert!(events.iter().any(|e| e == "HandshakeComplete"), "{events:?}");
    }

    /// Rejecting an invitation: <QSYJ> (or <QJO> out of range), then we open right here.
    #[tokio::test]
    async fn callee_rejects_qsy() {
        let cmd = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let data = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (cp, dp) = (cmd.local_addr().unwrap().port(), data.local_addr().unwrap().port());
        let modem = tokio::spawn(async move {
            let (mut c, _) = cmd.accept().await.unwrap();
            let (mut d, _) = data.accept().await.unwrap();
            c.write_all(b"SN 9.0\rCONNECTED NR5P W5TST 500\r").await.unwrap();
            let mut sent = vec![read_frame(&mut d).await];
            c.write_all(b"BUFFER 0\r").await.unwrap();
            d.write_all(b"8 <Q:-300>").await.unwrap();
            sent.push(read_frame(&mut d).await);
            c.write_all(b"BUFFER 0\r").await.unwrap();
            // VarAC: after a refused QSY on the calling frequency nothing more is sent
            let more = tokio::time::timeout(std::time::Duration::from_millis(1500), read_frame(&mut d)).await;
            assert!(more.is_err(), "no opening after the refusal: {more:?}");
            sent
        });
        let vara = VaraConnection::connect("127.0.0.1", cp, dp).await.unwrap();
        let cfg = StationConfig {
            callsign: "W5TST".into(), name: None, qth: None, locator: None,
            services: ServiceFlags(ServiceFlags::VMAIL), bbs_dir: None, email_gate: None, bbs_gate: None,
            policy: Default::default(),
        };
        let mut s = VaracSession::new(vara, cfg);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                if let Some(SessionEvent::QsyInvitation { .. }) = s.poll().await {
                    let _ = s.answer_qsy(false, true, 0).await;
                }
            }
        })
        .await;
        assert_eq!(modem.await.unwrap(), vec!["3 <Q>", "5 <QJO>"]);
        assert_eq!(qsy_invite_in("<QSYU>", None), Some(QsyTarget::Up));
        assert_eq!(qsy_invite_in("<QSYD>", None), Some(QsyTarget::Down));
        assert_eq!(qsy_invite_in("<QSYF>000014108250</QSYF>", None), Some(QsyTarget::Absolute(14_108_250)));
        assert_eq!(qsy_invite_in("<QSYF>14108250</QSYF>", None), None, "VarAC wants 12 digits");
        assert_eq!(qsy_invite_in("x", Some(-75)), Some(QsyTarget::Offset(-750)));
    }

    /// VarAC's answers, as Settings → QSO set them: inquiries refused with <INFOJ>, peeks we
    /// can't serve get VarAC's own "not available" tags, <SR> answered, verbose SNR
    /// auto-accepted, and no -T alias when pings are off.
    #[tokio::test]
    async fn answers_follow_station_policy() {
        let cmd = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let data = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (cp, dp) = (cmd.local_addr().unwrap().port(), data.local_addr().unwrap().port());
        let modem = tokio::spawn(async move {
            let (mut c, _) = cmd.accept().await.unwrap();
            let (mut d, _) = data.accept().await.unwrap();
            let mut buf = vec![0u8; 256];
            let n = tokio::time::timeout(std::time::Duration::from_secs(3), c.read(&mut buf)).await.unwrap().unwrap();
            let mycall = String::from_utf8_lossy(&buf[..n]).into_owned();
            c.write_all(b"SN 12.0\r").await.unwrap();
            let mut got = Vec::new();
            for req in ["<INFO>", "<LOCR>", "<FSR>", "<SR>", "<VSI>"] {
                d.write_all(format!("{} {req}", req.len()).as_bytes()).await.unwrap();
                got.push(read_frame(&mut d).await);
                c.write_all(b"BUFFER 0\r").await.unwrap();
            }
            (mycall, got)
        });
        let vara = VaraConnection::connect("127.0.0.1", cp, dp).await.unwrap();
        let cfg = StationConfig {
            callsign: "W5TST".into(), name: None, qth: None, locator: None,
            services: ServiceFlags(ServiceFlags::VMAIL), bbs_dir: None, email_gate: None, bbs_gate: None,
            policy: StationPolicy::new(false, false, true),
        };
        let mut s = VaracSession::new(vara, cfg);
        s.connected_to = Some("NR5P".into());
        s.handshake = Handshake::Done;
        s.state = SessionState::Connected;
        s.apply_ping_policy().await.unwrap();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(4), async {
            loop {
                let _ = s.poll().await;
            }
        }).await;
        let (mycall, got) = modem.await.unwrap();
        assert_eq!(mycall, "MYCALL W5TST\r", "no -T alias when pings are off");
        assert_eq!(got, vec!["7 <INFOJ>", "7 <INFOJ>", "5 <FSO>", "6 <R+12>", "6 <VSIR>"]);
        assert!(s.verbose_snr, "verbose SNR runs after accepting");
    }

    /// A limited BBS: hidden from the opening and <BLJ>/<BGJ> for others; <EGA> answered.
    #[tokio::test]
    async fn bbs_limit_and_gateway_address() {
        let cmd = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let data = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (cp, dp) = (cmd.local_addr().unwrap().port(), data.local_addr().unwrap().port());
        let modem = tokio::spawn(async move {
            let (mut c, _) = cmd.accept().await.unwrap();
            let (mut d, _) = data.accept().await.unwrap();
            let mut got = Vec::new();
            for req in ["<BLR>", "<BG:x.txt>", "<EGA>"] {
                d.write_all(format!("{} {req}", req.len()).as_bytes()).await.unwrap();
                got.push(read_frame(&mut d).await);
                c.write_all(b"BUFFER 0\r").await.unwrap();
            }
            got
        });
        let vara = VaraConnection::connect("127.0.0.1", cp, dp).await.unwrap();
        let gate = crate::varac::email_gateway::EmailGate::new(true, None);
        gate.update(|g| g.address = "gw@example.com".into());
        let cfg = StationConfig {
            callsign: "W5TST".into(), name: None, qth: None, locator: None,
            services: ServiceFlags(ServiceFlags::VMAIL | ServiceFlags::BBS),
            bbs_dir: Some(std::env::temp_dir()),
            email_gate: Some(gate),
            bbs_gate: Some(crate::varac::email_gateway::EmailGate::new(true, Some(vec!["K5XYZ".into()]))),
            policy: Default::default(),
        };
        let mut s = VaracSession::new(vara, cfg);
        s.connected_to = Some("NR5P".into());
        s.handshake = Handshake::Done;
        s.state = SessionState::Connected;
        assert_eq!(s.offered_services().to_letters(), "V", "BBS hidden from a station not on the list");
        let _ = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let _ = s.poll().await;
            }
        }).await;
        assert_eq!(modem.await.unwrap(), vec!["5 <BLJ>", "5 <BGJ>", "14 gw@example.com"]);
    }

    /// VarAC announces its email gateway only to stations allowed to use it.
    #[tokio::test]
    async fn gateway_announced_only_to_allowed_stations() {
        let cmd = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let data = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (cp, dp) = (cmd.local_addr().unwrap().port(), data.local_addr().unwrap().port());
        let accept = tokio::spawn(async move { (cmd.accept().await.unwrap(), data.accept().await.unwrap()) });
        let vara = VaraConnection::connect("127.0.0.1", cp, dp).await.unwrap();
        let _conns = accept.await.unwrap();
        let gate = crate::varac::email_gateway::EmailGate::new(true, Some(vec!["K5XYZ".into()]));
        let cfg = StationConfig {
            callsign: "W5TST".into(), name: None, qth: None, locator: None,
            services: ServiceFlags(ServiceFlags::VMAIL | ServiceFlags::EMAIL),
            bbs_dir: None, email_gate: Some(gate.clone()), bbs_gate: None, policy: Default::default(),
        };
        let mut s = VaracSession::new(vara, cfg);
        s.connected_to = Some("NR5P".into());
        assert_eq!(s.offered_services().to_letters(), "V");
        s.connected_to = Some("K5XYZ".into());
        assert_eq!(s.offered_services().to_letters(), "VE");
        gate.update(|g| g.suspended = true);
        assert_eq!(s.offered_services().to_letters(), "V", "not while shut down");
    }

    /// VarAC's welcome: only to stations that called us, not on pings, not twice to the
    /// same partner, not while away unless "Send always"; "//n" breaks the line.
    #[tokio::test]
    async fn welcome_message_rules() {
        let cmd = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let data = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (cp, dp) = (cmd.local_addr().unwrap().port(), data.local_addr().unwrap().port());
        let accept = tokio::spawn(async move { (cmd.accept().await.unwrap(), data.accept().await.unwrap()) });
        let vara = VaraConnection::connect("127.0.0.1", cp, dp).await.unwrap();
        let _conns = accept.await.unwrap();
        let cfg = StationConfig {
            callsign: "W5TST".into(), name: None, qth: None, locator: Some("EM10".into()),
            services: ServiceFlags(ServiceFlags::VMAIL), bbs_dir: None, email_gate: None, bbs_gate: None,
            policy: Default::default(),
        };
        let policy = cfg.policy.clone();
        let mut s = VaracSession::new(vara, cfg);
        s.connected_to = Some("NR5P".into());
        s.incoming = true;
        assert_eq!(s.welcome_text(), None, "no welcome set");
        policy.set_welcome("Hi from <LOC>//nGL", false);
        // (VarAC expands its tags in the welcome too: <LOC> → <LOC:EM10>)
        assert_eq!(s.welcome_text().as_deref(), Some("Hi from <LOC:EM10>\r\nGL"));
        s.incoming = false;
        assert_eq!(s.welcome_text(), None, "we called");
        s.incoming = true;
        s.ping = true;
        assert_eq!(s.welcome_text(), None, "ping");
        s.ping = false;
        s.away = Some(String::new());
        assert_eq!(s.welcome_text(), None, "away");
        policy.set_welcome("Hi", true);
        assert_eq!(s.welcome_text().as_deref(), Some("Hi"), "send always");
        s.away = None;
        s.last_partner = Some("NR5P".into());
        assert_eq!(s.welcome_text(), None, "same partner as last time");
    }

    /// As gateway we answer an email VMail <SMR> when the sender may use us, else <EJ>
    /// (VarAC: gateway off, or not on the allow list) and report the refusal.
    #[tokio::test]
    async fn email_vmail_gets_smr_or_ej() {
        for (gate, want) in [
            (Some(crate::varac::email_gateway::EmailGate::new(true, None)), "5 <SMR>"),
            (Some(crate::varac::email_gateway::EmailGate::new(true, Some(vec!["K5XYZ".into()]))), "4 <EJ>"),
            (None, "4 <EJ>"),
        ] {
            let cmd = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let data = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let (cp, dp) = (cmd.local_addr().unwrap().port(), data.local_addr().unwrap().port());
            let modem = tokio::spawn(async move {
                let (_c, _) = cmd.accept().await.unwrap();
                let (mut d, _) = data.accept().await.unwrap();
                let wire = "<SM><TME:2026-10-30 10:00:00><TO:test@example.com><SBJ:hi><MSG:hello><EG>";
                d.write_all(format!("{} {wire}", wire.len()).as_bytes()).await.unwrap();
                read_frame(&mut d).await
            });
            let vara = VaraConnection::connect("127.0.0.1", cp, dp).await.unwrap();
            let cfg = StationConfig {
                callsign: "NR5P".into(), name: None, qth: None, locator: None,
                services: ServiceFlags(ServiceFlags::VMAIL | ServiceFlags::EMAIL),
                bbs_dir: None, email_gate: gate, bbs_gate: None, policy: Default::default(),
            };
            let mut s = VaracSession::new(vara, cfg);
            s.connected_to = Some("W5TST".into());
            s.handshake = Handshake::Done;
            s.state = SessionState::Connected;
            let mut events = Vec::new();
            let _ = tokio::time::timeout(std::time::Duration::from_secs(2), async {
                loop {
                    if let Some(ev) = s.poll().await {
                        events.push(format!("{ev:?}"));
                    }
                }
            }).await;
            assert_eq!(modem.await.unwrap(), want);
            if want.contains("EJ") {
                assert!(events.iter().any(|e| e.contains("EmailRefused")), "{events:?}");
            } else {
                assert!(events.iter().any(|e| e.contains("VMail(")), "{events:?}");
            }
        }
    }

    /// The modem quitting must end the session cleanly: next_input() used to panic here
    /// (a select! with every branch disabled), killing the GUI's session task.
    #[tokio::test]
    async fn modem_closing_is_reported_not_a_panic() {
        let cmd = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let data = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (cp, dp) = (cmd.local_addr().unwrap().port(), data.local_addr().unwrap().port());
        let modem = tokio::spawn(async move {
            let (mut c, _) = cmd.accept().await.unwrap();
            let (d, _) = data.accept().await.unwrap();
            c.write_all(b"BUSY OFF\r").await.unwrap();
            drop((c, d));
        });
        let vara = VaraConnection::connect("127.0.0.1", cp, dp).await.unwrap();
        let cfg = StationConfig {
            callsign: "W5TST".into(),
            name: None,
            qth: None,
            locator: None,
            services: ServiceFlags(ServiceFlags::VMAIL),
            bbs_dir: None,
            email_gate: None,
            bbs_gate: None,
            policy: Default::default(),
        };
        let mut s = VaracSession::new(vara, cfg);
        modem.await.unwrap();
        let mut events = 0;
        let closed = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                if s.modem_closed() {
                    return true;
                }
                if let Some(input) = s.next_input().await {
                    events += 1;
                    s.handle_input(input).await;
                }
            }
        })
        .await;
        assert_eq!(closed, Ok(true));
        assert_eq!(events, 1, "BUSY OFF is still delivered before the close");
        // and a wait doesn't hang (or panic, as it did with the KISS port attached)
        let next = tokio::time::timeout(std::time::Duration::from_secs(1), s.next_input()).await;
        assert!(matches!(next, Ok(None)));
    }

    /// An email VMail (`<EG>`, TO = address) is for us as gateway, not relay mail.
    #[tokio::test]
    async fn queued_chat_goes_out_joined_like_varac() {
        use tokio::io::AsyncReadExt;
        let cmd = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let data = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (cp, dp) = (cmd.local_addr().unwrap().port(), data.local_addr().unwrap().port());
        let accept = tokio::spawn(async move { (cmd.accept().await.unwrap(), data.accept().await.unwrap()) });
        let vara = VaraConnection::connect("127.0.0.1", cp, dp).await.unwrap();
        let (_c, (mut modem_data, _)) = accept.await.unwrap();
        let cfg = StationConfig {
            callsign: "W5TST".into(), name: None, qth: None, locator: None,
            services: ServiceFlags(ServiceFlags::VMAIL), bbs_dir: None, email_gate: None, bbs_gate: None,
            policy: Default::default(),
        };
        let mut s = VaracSession::new(vara, cfg);
        s.state = SessionState::Connected;
        // the first goes at once; the rest wait for the modem's BUFFER 0
        for t in ["one", "&IT&", "two", "three", "four", "<SR>", "five", "<C:2>x", "&L13"] {
            s.send_framed(t).await.unwrap();
        }
        let mut sent = String::new();
        let mut buf = [0u8; 512];
        let n = modem_data.read(&mut buf).await.unwrap();
        sent.push_str(&String::from_utf8_lossy(&buf[..n]));
        assert_eq!(sent, "3 one");
        for _ in 0..3 {
            s.tx_done().await;
            s.tx_resume_at = None; // (VarAC's 1 s pause, skipped here)
            s.pump_tx().await.unwrap();
        }
        let mut rest = String::new();
        // (three frames, each "length text": however the reads split them)
        while rest.matches(' ').count() < 3 {
            let n = tokio::time::timeout(std::time::Duration::from_secs(2), modem_data.read(&mut buf)).await.unwrap().unwrap();
            rest.push_str(&String::from_utf8_lossy(&buf[..n]));
        }
        // the stale &IT& is dropped, three go together, then <SR> with the next line (VarAC
        // joins tags too; only files, replies, likes, typing, QSY invitations and AI: not)
        assert_eq!(rest, "16 two\r\nthree\r\nfour10 <SR>\r\nfive6 <C:2>x", "a reply goes alone");
    }

    /// The partner speaking during VarAC's 1 s pause after our frame ends the pause: what
    /// waits must go out then, not stay "In queue" until something else happens.
    #[tokio::test]
    async fn partner_frame_in_the_pause_releases_the_queue() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let cmd = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let data = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (cp, dp) = (cmd.local_addr().unwrap().port(), data.local_addr().unwrap().port());
        let accept = tokio::spawn(async move { (cmd.accept().await.unwrap(), data.accept().await.unwrap()) });
        let vara = VaraConnection::connect("127.0.0.1", cp, dp).await.unwrap();
        let (_c, (mut modem_data, _)) = accept.await.unwrap();
        let cfg = StationConfig {
            callsign: "W5TST".into(), name: None, qth: None, locator: None,
            services: ServiceFlags(ServiceFlags::VMAIL), bbs_dir: None, email_gate: None, bbs_gate: None,
            policy: Default::default(),
        };
        let mut s = VaracSession::new(vara, cfg);
        s.state = SessionState::Connected;
        s.send_framed("<TL>").await.unwrap();
        s.send_framed("second").await.unwrap();
        let mut buf = [0u8; 512];
        let n = modem_data.read(&mut buf).await.unwrap();
        assert_eq!(&buf[..n], b"4 <TL>");
        // BUFFER 0: the pause starts, "second" waits
        s.tx_done().await;
        assert!(s.tx_resume_at.is_some());
        s.pending.clear();
        // the partner's frame lands inside the pause
        modem_data.write_all(b"5 hello").await.unwrap();
        let input = tokio::time::timeout(std::time::Duration::from_millis(500), s.next_input()).await.unwrap().unwrap();
        assert!(matches!(input, SessionInput::Data(_)));
        s.handle_input(input).await;
        let n = tokio::time::timeout(std::time::Duration::from_millis(500), modem_data.read(&mut buf)).await
            .expect("the queued frame was not sent").unwrap();
        assert_eq!(&buf[..n], b"6 second");
    }

    /// A file coming in over the modem in pieces: each piece reports how much of the
    /// `<SFB>` frame is here, the last one delivers the file, and nothing reports after it.
    #[tokio::test]
    async fn incoming_file_reports_progress_then_arrives() {
        use tokio::io::AsyncWriteExt;
        let cmd = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let data = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (cp, dp) = (cmd.local_addr().unwrap().port(), data.local_addr().unwrap().port());
        let accept = tokio::spawn(async move { (cmd.accept().await.unwrap(), data.accept().await.unwrap()) });
        let vara = VaraConnection::connect("127.0.0.1", cp, dp).await.unwrap();
        let (_c, (mut modem_data, _)) = accept.await.unwrap();
        let cfg = StationConfig {
            callsign: "W5TST".into(), name: None, qth: None, locator: None,
            services: ServiceFlags(ServiceFlags::VMAIL), bbs_dir: None, email_gate: None, bbs_gate: None,
            policy: Default::default(),
        };
        let mut s = VaracSession::new(vara, cfg);
        s.state = SessionState::Connected;
        s.send_file_ready().await.unwrap();
        s.pending.clear();

        let body = format!("<SFB>{}", "QUJD".repeat(25)); // 105 bytes
        let frame = format!("{} {body}", body.len());
        let (a, rest) = frame.split_at(30);
        let (b, c) = rest.split_at(40);
        let mut events = Vec::new();
        for piece in [a, b, c] {
            modem_data.write_all(piece.as_bytes()).await.unwrap();
            let input = tokio::time::timeout(std::time::Duration::from_millis(500), s.next_input()).await.unwrap().unwrap();
            if let Some(ev) = s.handle_input(input).await {
                events.push(ev);
            }
            while let Some(ev) = s.pending.pop_front() {
                events.push(ev);
            }
        }
        let progress: Vec<_> = events.iter().filter_map(|e| match e {
            SessionEvent::RxProgress { received, total } => Some((*received, *total)),
            _ => None,
        }).collect();
        assert_eq!(progress, vec![(26, 105), (66, 105)], "{events:?}");
        assert!(
            events.iter().any(|e| matches!(e, SessionEvent::Message(IncomingMessage::BbsFilePayload(d)) if d.len() == 100)),
            "the file arrived: {events:?}"
        );
        assert!(!s.file_incoming, "no more progress once the file is here");
    }

    #[tokio::test]
    async fn email_vmail_is_not_parked() {
        let cmd = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let data = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (cp, dp) = (cmd.local_addr().unwrap().port(), data.local_addr().unwrap().port());
        let accept = tokio::spawn(async move { (cmd.accept().await.unwrap(), data.accept().await.unwrap()) });
        let vara = VaraConnection::connect("127.0.0.1", cp, dp).await.unwrap();
        let _conns = accept.await.unwrap();
        let cfg = StationConfig {
            callsign: "W5TST".into(),
            name: None,
            qth: None,
            locator: None,
            services: ServiceFlags(ServiceFlags::VMAIL | ServiceFlags::EMAIL),
            bbs_dir: None,
            email_gate: None,
            bbs_gate: None,
            policy: Default::default(),
        };
        let mut s = VaracSession::new(vara, cfg);
        s.connected_to = Some("NR5P".into());
        let wire = "<SM><TME:2026-10-30 10:00:00><TO:test@example.com><SBJ:hi><MSG:hello><EG>";
        match s.complete(parse_application_message(wire)) {
            IncomingMessage::VMail(vm) => {
                assert!(vm.is_email());
                assert!(!vm.is_parked);
                assert_eq!(vm.from, "NR5P");
                assert_eq!(vm.to, "test@example.com");
            }
            other => panic!("Expected VMail, got {other:?}"),
        }
        // ...while mail for another station still goes to Parking
        match s.complete(parse_application_message("<SM><TME:2026-10-30 10:00:00><TO:K5XYZ><FRM:NR5P><SBJ:x><MSG:y>")) {
            IncomingMessage::VMail(vm) => assert!(vm.is_parked),
            other => panic!("Expected VMail, got {other:?}"),
        }
    }

    /// Replays what real VarAC 16 sent when we called it (captured through XARA):
    /// opening+<Q> in one frame, then <QSYR>. We must answer <Q:0>, then <TL> and
    /// our opening as *separate* transmissions (one per BUFFER 0).
    #[tokio::test]
    async fn caller_handshake_matches_varac() {
        let cmd = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let data = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (cp, dp) = (cmd.local_addr().unwrap().port(), data.local_addr().unwrap().port());
        let modem = tokio::spawn(async move {
            let (mut c, _) = cmd.accept().await.unwrap();
            let (mut d, _) = data.accept().await.unwrap();
            c.write_all(b"CONNECTED W5TST NR5P 500\rSN 18.0\r").await.unwrap();
            d.write_all(b"25 de NR5P <R+20> <S:V>\r\n<Q>").await.unwrap();
            let mut sent = vec![read_frame(&mut d).await];
            c.write_all(b"BUFFER 0\r").await.unwrap();
            // Captured: VarAC delivers a waiting VMail in the same frame as <QSYR>
            d.write_all(b"43 <QSYR>\r\n<SM><SBJ:here you go><MSG:test 123>").await.unwrap();
            sent.push(read_frame(&mut d).await);
            // Nothing else may be sent until the modem reports the buffer drained
            let mut buf = [0u8; 64];
            assert!(tokio::time::timeout(std::time::Duration::from_millis(300), d.read(&mut buf)).await.is_err());
            c.write_all(b"BUFFER 0\r").await.unwrap();
            sent.push(read_frame(&mut d).await);
            sent
        });
        let vara = VaraConnection::connect("127.0.0.1", cp, dp).await.unwrap();
        let cfg = StationConfig {
            callsign: "W5TST".into(),
            name: None,
            qth: None,
            locator: None,
            services: ServiceFlags(ServiceFlags::VMAIL),
            bbs_dir: None,
            email_gate: None,
            bbs_gate: None,
            policy: Default::default(),
        };
        let mut s = VaracSession::new(vara, cfg);
        let mut events = Vec::new();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(4), async {
            loop {
                if let Some(ev) = s.poll().await {
                    let done = matches!(ev, SessionEvent::HandshakeComplete);
                    events.push(format!("{ev:?}"));
                    if done {
                        // let the last queued frame go out
                        for _ in 0..8 {
                            if let Ok(Some(ev)) = tokio::time::timeout(std::time::Duration::from_millis(500), s.poll()).await {
                                events.push(format!("{ev:?}"));
                            }
                        }
                        break;
                    }
                }
            }
        })
        .await;
        let sent = modem.await.unwrap();
        // (our opening and the receipt for the VMail that rode on <QSYR> share a frame, as
        // VarAC joins queued messages)
        assert_eq!(sent, vec!["5 <Q:0>", "4 <TL>", "22 de W5TST <R+18>\r\n<SMR>"]);
        assert!(events.iter().any(|e| e.contains("destination: \"NR5P\"")), "{events:?}");
        assert!(events.iter().any(|e| e.contains("Opening")), "partner opening must still reach the app: {events:?}");
        assert!(events.iter().any(|e| e == "HandshakeComplete"), "{events:?}");
        assert!(
            events.iter().any(|e| e.contains("VMail(") && e.contains("from: \"NR5P\"") && e.contains("to: \"W5TST\"") && e.contains("here you go")),
            "VMail riding on <QSYR> must be delivered with implied from/to: {events:?}"
        );
    }
}

#[cfg(test)]
mod reply_format_tests {
    use super::*;

    /// Replies captured from VarAC 16.1.8
    #[test]
    fn parses_varac_replies() {
        assert!(matches!(parse_application_message("&IT&"), IncomingMessage::IsTyping));
        match parse_application_message("<LOC:EM10HW>") {
            IncomingMessage::StationInfo { name: None, qth: None, locator: Some(l), .. } => assert_eq!(l, "EM10HW"),
            other => panic!("{other:?}"),
        }
        match parse_application_message("<NAME:Bob><QTH:Austin><LOC:EM10>") {
            IncomingMessage::StationInfo { name: Some(n), qth: Some(q), .. } => assert_eq!((n.as_str(), q.as_str()), ("Bob", "Austin")),
            other => panic!("{other:?}"),
        }
        assert!(matches!(parse_application_message("<R+20>"), IncomingMessage::SnrReport(20)));
        assert!(matches!(parse_application_message("<R-07>"), IncomingMessage::SnrReport(-7)));
    }
}

#[cfg(test)]
mod split_tests {
    use super::*;

    #[test]
    fn crlf_splits_only_outside_tags() {
        assert_eq!(split_messages("<QSYR>\r\n<SQ>"), vec!["<QSYR>", "<SQ>"]);
        assert_eq!(split_messages("de NR5P <R+20> <S:V>\r\n<Q>"), vec!["de NR5P <R+20> <S:V>", "<Q>"]);
        // Captured BBS list from VarAC 16.1.8: CRLF inside the tag stays
        let bl = "<BL:bbs_test.txt|2026-10-28|56\r\n>";
        assert_eq!(split_messages(bl), vec![bl]);
        match parse_application_message(bl) {
            IncomingMessage::BbsListing(e) => {
                assert_eq!(e.len(), 1);
                assert_eq!((e[0].name.as_str(), e[0].date.as_str(), e[0].size), ("bbs_test.txt", "2026-10-28", 56));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(split_messages("yes\r\n"), vec!["yes"]);
        // VarAC's ping answer when it holds mail for us
        assert_eq!(split_ping_answer("<R+22> <VW>".into()), vec!["<R+22>", "<VW>"]);
        assert_eq!(split_ping_answer("<VW>".into()), vec!["<VW>"]);
        assert_eq!(split_ping_answer("hi <R+22> <VW>".into()), vec!["hi <R+22> <VW>"]);
    }
}

#[cfg(test)]
mod sfm_flow_tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    async fn read_one(d: &mut tokio::net::TcpStream) -> String {
        let mut buf = vec![0u8; 512];
        match tokio::time::timeout(std::time::Duration::from_secs(2), d.read(&mut buf)).await {
            Ok(Ok(n)) => String::from_utf8_lossy(&buf[..n]).into_owned(),
            _ => String::new(),
        }
    }

    /// Withdrawing our file offer before the partner answers: the partner gets `<SFAB>`, a
    /// late `<SFRD>` sends nothing, and the next file can go. Once the data is going it
    /// can't be withdrawn (VARA can't take it back).
    #[tokio::test]
    async fn outgoing_file_offer_can_be_withdrawn() {
        use tokio::io::AsyncWriteExt;
        let cmd = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let data = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (cp, dp) = (cmd.local_addr().unwrap().port(), data.local_addr().unwrap().port());
        let accept = tokio::spawn(async move { (cmd.accept().await.unwrap(), data.accept().await.unwrap()) });
        let vara = VaraConnection::connect("127.0.0.1", cp, dp).await.unwrap();
        let (_c, (mut modem_data, _)) = accept.await.unwrap();
        let cfg = StationConfig {
            callsign: "W5TST".into(), name: None, qth: None, locator: None,
            services: ServiceFlags(ServiceFlags::VMAIL), bbs_dir: None, email_gate: None, bbs_gate: None,
            policy: Default::default(),
        };
        let mut s = VaracSession::new(vara, cfg);
        s.state = SessionState::Connected;
        assert!(!s.cancel_outgoing_file().await.unwrap(), "nothing to withdraw yet");

        let file = crate::varac::bbs::prepare_file("a.txt", b"hello file").unwrap();
        s.send_file(file).await.unwrap();
        assert!(s.sending_file() && !s.file_data_going());
        let offer = read_one(&mut modem_data).await;
        assert!(offer.contains("<SF:a.txt.VarAC_gzip|"), "{offer}");
        s.tx_done().await; // (the offer is out)

        assert!(s.cancel_outgoing_file().await.unwrap());
        assert!(!s.sending_file());
        let mut sent = String::new();
        for _ in 0..3 {
            sent.push_str(&read_one(&mut modem_data).await);
            if sent.contains("<SFAB>") {
                break;
            }
            if let Some(at) = s.tx_resume_at {
                tokio::time::sleep_until(at).await;
                let _ = s.pump_tx().await;
            }
        }
        assert!(sent.contains("<SFAB>"), "{sent:?}");
        s.tx_done().await;

        // the partner's <SFRD> crossed it: no data goes out
        modem_data.write_all(b"6 <SFRD>").await.unwrap();
        take_data(&mut s).await;
        let late = read_one(&mut modem_data).await;
        assert!(!late.contains("<SFB>"), "{late:?}");

        // the next file goes, and once its data is going it can't be withdrawn
        let file = crate::varac::bbs::prepare_file("b.txt", b"second").unwrap();
        s.send_file(file).await.unwrap();
        let mut offer = read_one(&mut modem_data).await;
        if !offer.contains("<SF:b.txt") {
            if let Some(at) = s.tx_resume_at {
                tokio::time::sleep_until(at).await;
                let _ = s.pump_tx().await;
            }
            offer.push_str(&read_one(&mut modem_data).await);
        }
        assert!(offer.contains("<SF:b.txt"), "{offer:?}");
        s.tx_done().await;
        modem_data.write_all(b"6 <SFRD>").await.unwrap();
        take_data(&mut s).await;
        assert!(s.file_data_going());
        let data = read_one(&mut modem_data).await;
        assert!(data.contains("<SFB>"), "the data went: {data:?}");
        assert!(!s.cancel_outgoing_file().await.unwrap());
        assert!(s.sending_file(), "still sending: VARA has the data");
    }

    /// Run the session until it has read what the modem sent (its own queued events first).
    async fn take_data(s: &mut VaracSession) {
        for _ in 0..20 {
            let input = tokio::time::timeout(std::time::Duration::from_millis(500), s.next_input()).await
                .expect("the modem's data never came").unwrap();
            let data = matches!(input, SessionInput::Data(_));
            s.handle_input(input).await;
            if data {
                return;
            }
        }
        panic!("no data from the modem");
    }

    /// VMail with attachment, as in VarAC: `<SQ>` once, wait up to 60 s, partner needs `V`.
    #[tokio::test]
    async fn attachment_needs_partner_v() {
        let cmd = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let data = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (cp, dp) = (cmd.local_addr().unwrap().port(), data.local_addr().unwrap().port());
        let modem = tokio::spawn(async move {
            let (_c, _) = cmd.accept().await.unwrap();
            let (mut d, _) = data.accept().await.unwrap();
            read_one(&mut d).await
        });
        let vara = VaraConnection::connect("127.0.0.1", cp, dp).await.unwrap();
        let cfg = StationConfig {
            callsign: "W5TST".into(), name: None, qth: None, locator: None,
            services: ServiceFlags(ServiceFlags::VMAIL), bbs_dir: None, email_gate: None, bbs_gate: None, policy: Default::default(),
        };
        let mut s = VaracSession::new(vara, cfg);
        (s.connected_to, s.state) = (Some("nr5p".into()), SessionState::Connected);
        assert_eq!(s.attachment_gate().await, AttachmentGate::Wait);
        assert_eq!(s.attachment_gate().await, AttachmentGate::Wait);
        assert_eq!(modem.await.unwrap(), "4 <SQ>");
        s.services_inquiry_at = tokio::time::Instant::now().checked_sub(std::time::Duration::from_secs(61));
        assert_eq!(s.attachment_gate().await, AttachmentGate::Skip("NR5P DID NOT ANSWER THE SERVICES INQUIRY (OLDER VERSION?)".into()));
        s.partner_services = Some(ServiceFlags::from_letters("EB"));
        assert_eq!(s.attachment_gate().await, AttachmentGate::Skip("NR5P DOES NOT SUPPORT VMAIL ATTACHMENTS".into()));
        s.partner_services = Some(ServiceFlags::from_letters("V"));
        assert_eq!(s.attachment_gate().await, AttachmentGate::Send);
    }

    #[test]
    fn vmail_package_names() {
        let t = chrono::TimeZone::with_ymd_and_hms(&Utc, 2026, 10, 30, 12, 9, 56).unwrap();
        assert_eq!(vmail_package_name(" Field day: photos? ", t), "Field_day_photos_20261030_120956.zip");
        assert_eq!(vmail_package_name("a|b>c<d", t), "abcd_20261030_120956.zip");
        assert_eq!(vmail_package_name("??", t), "VMail_20261030_120956.zip");
        assert_eq!(vmail_package_name(&"x".repeat(40), t), format!("{}_20261030_120956.zip", "x".repeat(30)));
    }

    /// Replays the live QSO where original VarAC (caller) sent Openslot a VMail with an
    /// attachment: we must answer the `<SFM:>` offer with `<SFRD>`.
    #[tokio::test]
    async fn callee_answers_attachment_offer() {
        let cmd = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let data = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (cp, dp) = (cmd.local_addr().unwrap().port(), data.local_addr().unwrap().port());
        let modem = tokio::spawn(async move {
            let (mut c, _) = cmd.accept().await.unwrap();
            let (mut d, _) = data.accept().await.unwrap();
            let mut sent = Vec::new();
            c.write_all(b"CONNECTED NR5P W5TST 500\r").await.unwrap();
            sent.push(read_one(&mut d).await); // <Q>
            c.write_all(b"BUFFER 0\r").await.unwrap();
            d.write_all(b"8 <Q:-150>").await.unwrap();
            sent.push(read_one(&mut d).await); // <QSYR>
            c.write_all(b"BUFFER 0\r").await.unwrap();
            sent.push(read_one(&mut d).await); // opening
            c.write_all(b"BUFFER 0\r").await.unwrap();
            d.write_all(b"81 <SFM:vmail_2_20261030_120956.zip.VarAC_g").await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            d.write_all(b"zip|180|1B900E0DA6E4A740463899EC923A1088>").await.unwrap();
            sent.push(read_one(&mut d).await); // must be <SFRD>
            sent
        });
        let vara = VaraConnection::connect("127.0.0.1", cp, dp).await.unwrap();
        let cfg = StationConfig {
            callsign: "W5TST".into(), name: None, qth: None, locator: None,
            services: ServiceFlags(ServiceFlags::VMAIL), bbs_dir: None, email_gate: None, bbs_gate: None, policy: Default::default(),
        };
        let mut s = VaracSession::new(vara, cfg);
        s.ask_qsy_invite = true;
        // Interrupt the wait every millisecond, like a busy GUI command channel: inputs
        // must still be processed completely (regression: lost <SFRD>).
        let _ = tokio::time::timeout(std::time::Duration::from_secs(8), async {
            loop {
                let input = tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_millis(1)) => None,
                    i = s.next_input() => i,
                };
                if let Some(i) = input {
                    if let Some(ev) = s.handle_input(i).await {
                        accept_qsy_for_test(&mut s, &ev).await;
                    }
                }
            }
        })
        .await;
        let sent = modem.await.unwrap();
        assert_eq!(sent.last().map(|s| s.as_str()), Some("6 <SFRD>"), "{sent:?}");
    }
}

#[cfg(test)]
mod sfm_debug_tests {
    use super::*;
    #[test]
    fn parses_captured_sfm_offer() {
        let m = parse_application_message("<SFM:vmail_2_20261030_120956.zip.VarAC_gzip|180|1B900E0DA6E4A740463899EC923A1088>");
        assert!(matches!(m, IncomingMessage::VMailAttachmentOffer(_)), "{m:?}");
    }
}

/// Tests: accept a QSY countdown at once, as the app does when auto-QSY says ACCEPT.
#[cfg(test)]
pub(crate) async fn accept_qsy_for_test(s: &mut VaracSession, ev: &SessionEvent) {
    if let SessionEvent::QsyInvitation { target: QsyTarget::Offset(hz), .. } = ev {
        let _ = s.answer_qsy(true, false, *hz).await;
    }
}
