//! The app engine: one VarAC session on a background tokio runtime, talking to the
//! modem (the built-in XARA, or any VARA modem on the network), keeping the QSO log,
//! chat, VMail and heard stations in the same SQLite database layout as the desktop app.
//!
//! The app sends commands as JSON and polls events as JSON; see include/openslot.h.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use chrono::Utc;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::storage::{ChatRecord, CqFrameRecord, QsoExtra, QsoRecord, Storage, VMailFolder, VMailRecord};
use crate::vara::commands::{Bandwidth, ListenMode, VaraCommand};
use crate::vara::connection::VaraConnection;
use crate::varac::bbs;
use crate::varac::messages::{BbsFileOffer, IncomingMessage, ServiceFlags, VMail};
use crate::varac::session::{SessionEvent, StationConfig, StationPolicy, VaracSession};
use crate::varac::slots;

const EVENT_LIMIT: usize = 5000;

static EVENTS: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());

/// Queues an event for the app.
pub fn emit(event: Value) {
    let mut q = EVENTS.lock().unwrap_or_else(PoisonError::into_inner);
    if q.len() == EVENT_LIMIT {
        q.pop_front();
    }
    q.push_back(event.to_string());
}

pub fn next_event() -> Option<String> {
    EVENTS.lock().unwrap_or_else(PoisonError::into_inner).pop_front()
}

fn now_hm() -> String {
    Utc::now().format("%H:%M").to_string()
}

fn system(text: impl Into<String>) {
    emit(json!({"type": "system", "text": text.into(), "time": now_hm()}));
}

fn log(text: impl Into<String>) {
    emit(json!({"type": "log", "text": text.into()}));
}

#[derive(Deserialize, Clone)]
pub struct Config {
    /// Where the database and received files go (the app's documents/files folder).
    pub data_dir: String,
    pub callsign: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub qth: Option<String>,
    #[serde(default)]
    pub locator: Option<String>,
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub cmd_port: u16,
    #[serde(default = "default_bw")]
    pub bandwidth: String,
    /// Dial frequency, for the log (no rig control on a phone).
    #[serde(default = "default_freq")]
    pub frequency: u64,
    /// Settings → QSO, as VarAC: answer info requests, answer pings, accept verbose SNR
    #[serde(default = "yes")]
    pub allow_info: bool,
    #[serde(default = "yes")]
    pub allow_pings: bool,
    #[serde(default)]
    pub verbose_snr_auto_accept: bool,
    /// Sent to stations that call us, in front of our opening ("de CALL" follows)
    #[serde(default)]
    pub welcome_message: String,
    /// ...also while we're away
    #[serde(default)]
    pub welcome_always: bool,
    /// Shared in INFO replies (<EM:>); empty = private
    #[serde(default)]
    pub email: String,
    /// Complex callsign (W/4Z1AC/QRP) for the opening (<FC:>); empty = none
    #[serde(default)]
    pub full_callsign: String,
    /// Station ID during a QSO, every N minutes (VarAC: 5-30)
    #[serde(default = "default_call_id")]
    pub call_id_minutes: u64,
    /// Tell the partner <IE> and disconnect after this long idle (0 = never)
    #[serde(default = "default_idle")]
    pub idle_disconnect_minutes: u64,
    /// Answer last-heard peeks (<LHR>, <LHC:>) from our heard list
    #[serde(default = "yes")]
    pub allow_last_heard: bool,
    /// ...for every band, not just the one we're on
    #[serde(default)]
    pub multi_band_last_heard: bool,
}

fn default_call_id() -> u64 {
    10
}
fn default_idle() -> u64 {
    5
}

/// Settings → QSO and profile changed while running; fields left out stay as they are.
#[derive(Deserialize, Default)]
#[serde(default)]
pub struct PolicyUpdate {
    allow_info: Option<bool>,
    allow_pings: Option<bool>,
    verbose_snr_auto_accept: Option<bool>,
    welcome_message: Option<String>,
    welcome_always: Option<bool>,
    email: Option<String>,
    full_callsign: Option<String>,
    call_id_minutes: Option<u64>,
    idle_disconnect_minutes: Option<u64>,
    allow_last_heard: Option<bool>,
    multi_band_last_heard: Option<bool>,
}

fn yes() -> bool {
    true
}

fn default_slot_wait() -> u64 {
    slots::DEFAULT_CQ_SLOT_WAIT_SECS
}

/// A CQ that named a slot, still going out (the move to the slot follows it).
struct SlotCq {
    slot: u8,
    cf: u64,
    wait_secs: u64,
    sent: std::time::Instant,
}

/// The QSO as VarAC conducts it: VMail waits for a QSY off the calling frequency, Call
/// ID, idle disconnect and the calling-frequency limits.
#[derive(Default)]
struct Conduct {
    since: Option<std::time::Instant>,
    started_on_cf: bool,
    qsy_done: bool,
    partner_away: bool,
    link_open: bool,
    mail_delivered: bool,
    vw_sent: bool,
    last_id: Option<std::time::Instant>,
    last_activity: Option<std::time::Instant>,
    /// Our Call ID is going out (it isn't QSO activity)
    id_on_air: bool,
    /// <IE>/<AE> is queued: disconnect once it's on the air
    bye_tag_pending: bool,
    bye_sent: bool,
    cf_abort_done: bool,
    cf_warned: bool,
    /// VMail sent and waiting for the partner's <SMR> (then filed as Sent): (guid, to)
    awaiting_ack: VecDeque<(String, String)>,
    /// The partner's details from its opening (a Call ID later carries none)
    name: Option<String>,
    qth: Option<String>,
    locator: Option<String>,
}

/// Outbox or parked VMail waiting for `call`?
fn has_mail_for(db: &Mutex<Storage>, call: &str) -> bool {
    let base = |c: &str| c.split(['-', '/']).next().unwrap_or(c).trim().to_uppercase();
    let want = base(call);
    let d = db.lock().unwrap_or_else(PoisonError::into_inner);
    !d.outbox_vmails_for(call).unwrap_or_default().is_empty()
        || d.vmails_in_folder(VMailFolder::Parking, 200).unwrap_or_default().iter().any(|vm| base(&vm.vmail_to) == want)
}

fn on_calling_freq(hz: u64) -> bool {
    crate::varac::messages::is_calling_frequency(hz)
}

/// Standing by on a slot after our CQ, until a reply or `until`.
struct SlotWait {
    cf: u64,
    until: std::time::Instant,
}

fn default_host() -> String {
    "127.0.0.1".into()
}
fn default_port() -> u16 {
    8300
}
fn default_bw() -> String {
    "500".into()
}
fn default_freq() -> u64 {
    14_105_000
}

fn bandwidth(s: &str) -> Bandwidth {
    match s {
        "500" => Bandwidth::Bw500,
        "2750" => Bandwidth::Bw2750,
        _ => Bandwidth::Bw2300,
    }
}

pub fn band_of(hz: u64) -> &'static str {
    match hz {
        1_800_000..=2_000_000 => "160m",
        3_500_000..=4_000_000 => "80m",
        5_330_000..=5_410_000 => "60m",
        7_000_000..=7_300_000 => "40m",
        10_100_000..=10_150_000 => "30m",
        14_000_000..=14_350_000 => "20m",
        18_068_000..=18_168_000 => "17m",
        21_000_000..=21_450_000 => "15m",
        24_890_000..=24_990_000 => "12m",
        28_000_000..=29_700_000 => "10m",
        50_000_000..=54_000_000 => "6m",
        144_000_000..=148_000_000 => "2m",
        420_000_000..=450_000_000 => "70cm",
        _ => "",
    }
}

/// Commands from the app (`{"cmd": "...", ...}`).
#[derive(Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    ConnectModem,
    DisconnectModem,
    /// Call a station; `slot_hz` when answering a CQ on its slot (VarAC): the operator has
    /// tuned there, and we go back to the calling frequency after the QSO
    Connect {
        call: String,
        #[serde(default)]
        slot_hz: Option<u64>,
    },
    Ping { call: String },
    Disconnect,
    Abort,
    Chat { text: String },
    Typing,
    /// Call CQ; `slot` (VarAC: 1-5 below the calling frequency, 11-15 above, 0 = none) is
    /// where we wait for replies afterwards, for `wait_secs`
    Cq {
        #[serde(default)]
        slot: u8,
        #[serde(default = "default_slot_wait")]
        wait_secs: u64,
    },
    /// Stop waiting on the slot: back to the calling frequency
    SlotReturn,
    Beacon,
    Listen { mode: String },
    Bandwidth { bw: String },
    Broadcast { text: String },
    /// Queue a VMail; it goes out now if we're in a QSO with the addressee, else on the next one.
    Vmail { to: String, subject: String, body: String, #[serde(default)] urgent: bool },
    Away { text: Option<String> },
    FileAccept,
    FileDecline,
    QsyAccept,
    QsyReject,
    /// The operator's answer in the "QSY NOW" countdown (after a qsy_invitation event)
    QsyAnswer { accept: bool },
    SnrRequest,
    InfoRequest,
    Tune,
    TuneStop,
    ClearTxBuffer,
    SetFrequency { hz: u64 },
    /// Settings → QSO and profile changed (applies at once, also to a running session)
    Policy(PolicyUpdate),
    /// Ask the partner to QSY by `offset_hz` (VarAC: e.g. to relay VMail off the CF)
    QsyRequest { offset_hz: i32 },
    /// Our QSY invitation in the opening handshake (after a qsy_invite_request event):
    /// the slot to move to, 0 = stay
    QsyInvite { slot: u8 },
    /// Ask the partner a VarAC inquiry: last_heard, last_heard_call{call}, parked, locator,
    /// gps, version, schedule
    Inquire {
        what: String,
        #[serde(default)]
        call: Option<String>,
    },
    /// The operator went back to the calling frequency (after the qsy_back prompt)
    ReturnCf { hz: u64 },
    /// Invite the partner to verbose SNR (an SNR report every minute), or stop it
    VerboseSnr { on: bool },
    /// Answer the partner's verbose SNR invitation
    VerboseSnrAnswer { accept: bool },
    /// Offer the partner a file (VarAC `<SF:>`), in a QSO off the calling frequency; the
    /// data goes when it answers. A picture with `pct` goes through the image shrinker at
    /// `pct` % and `quality` (0–50, VarAC's 50 by default), as `file_info` previewed it.
    SendFile {
        path: String,
        #[serde(default)]
        pct: Option<u32>,
        #[serde(default)]
        quality: Option<u8>,
    },
    /// Stop our file: before the partner accepts, the offer is withdrawn (VarAC `<SFAB>`);
    /// once the data is going VARA can't take it back, so the link is dropped (ABORT)
    FileCancel,
}

pub struct Engine {
    rt: tokio::runtime::Runtime,
    cmd_tx: mpsc::UnboundedSender<Command>,
    pub db: Arc<Mutex<Storage>>,
    pub config: Config,
}

impl Engine {
    pub fn start(config: Config) -> Result<Self, String> {
        let dir = PathBuf::from(&config.data_dir);
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let db = Storage::open(&dir.join("varac.db")).map_err(|e| e.to_string())?;
        let db = Arc::new(Mutex::new(db));
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("openslot")
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let bridge = Bridge::new(config.clone(), db.clone());
        rt.spawn(bridge.run(cmd_rx));
        Ok(Self { rt, cmd_tx, db, config })
    }

    pub fn command(&self, cmd: Command) {
        let _ = self.cmd_tx.send(cmd);
    }

    pub fn shutdown(self) {
        let _ = self.cmd_tx.send(Command::DisconnectModem);
        self.rt.shutdown_timeout(std::time::Duration::from_secs(2));
    }
}

/// The session and what the app shouldn't have to track itself (the QSO being logged etc).
struct Bridge {
    config: Config,
    station: StationConfig,
    db: Arc<Mutex<Storage>>,
    session: Option<VaracSession>,
    frequency: u64,
    qso_guid: Option<String>,
    /// The qso row of the link that is up, written when it ends (as the desktop does)
    qso_row: Option<(QsoRecord, QsoExtra)>,
    partner: Option<String>,
    pending_offer: Option<BbsFileOffer>,
    pending_attachment: bool,
    /// The pending offer waits for the operator (the session answered the others itself)
    offer_asked: bool,
    away: bool,
    slot_cq: Option<SlotCq>,
    slot_wait: Option<SlotWait>,
    /// A QSO that began on our slot: back to this calling frequency when it ends
    cf_after_qso: Option<u64>,
    conduct: Conduct,
    /// VARA's last SN report (CQ records, Call ID)
    last_snr: Option<f32>,
    /// The "QSY NOW" countdown on screen: when it answers by itself, and the offset (Hz)
    qsy_popup: Option<(std::time::Instant, i32)>,
    /// The link's bandwidth (Hz), for VARA HF's limits on a file's size
    link_bw: Option<u16>,
    /// Our file being sent: its name and how many bytes its data frame puts in VARA's
    /// buffer (for the progress)
    sending: Option<SendingFile>,
}

struct SendingFile {
    name: String,
    /// The data frame's size in VARA's TX buffer (`<len> <SFB>base64`)
    frame_bytes: usize,
    /// The partner said send it: the data is going out
    started: bool,
}

impl Bridge {
    fn new(config: Config, db: Arc<Mutex<Storage>>) -> Self {
        let station = StationConfig {
            callsign: config.callsign.to_uppercase(),
            name: config.name.clone().filter(|s| !s.trim().is_empty()),
            qth: config.qth.clone().filter(|s| !s.trim().is_empty()),
            locator: config.locator.clone().filter(|s| !s.trim().is_empty()),
            // VarAC always announces VMail
            services: ServiceFlags(ServiceFlags::VMAIL),
            bbs_dir: None,
            email_gate: None,
            bbs_gate: None,
            policy: StationPolicy::new(config.allow_info, config.allow_pings, config.verbose_snr_auto_accept),
        };
        let bridge = Self {
            frequency: config.frequency,
            config,
            station,
            db,
            session: None,
            qso_guid: None,
            qso_row: None,
            partner: None,
            pending_offer: None,
            pending_attachment: false,
            offer_asked: false,
            away: false,
            slot_cq: None,
            slot_wait: None,
            cf_after_qso: None,
            conduct: Conduct::default(),
            last_snr: None,
            qsy_popup: None,
            link_bw: None,
            sending: None,
        };
        bridge.sync_policy();
        // The session answers pings with <R+NN> <VW> when we hold mail for the pinger (VarAC)
        let db = bridge.db.clone();
        bridge.station.policy.mail_waiting.set(move |call| has_mail_for(&db, call));
        bridge
    }

    /// The station policy the session reads, from our settings.
    fn sync_policy(&self) {
        let (c, p) = (&self.config, &self.station.policy);
        p.set(c.allow_info, c.allow_pings, c.verbose_snr_auto_accept);
        p.set_welcome(&c.welcome_message, c.welcome_always);
        p.set_email(&c.email);
        p.set_full_callsign(&c.full_callsign);
        // The desktop offers <Q>/<AWQ> only with auto-QSY on and rig control; the phones
        // have no rig control
        p.auto_qsy.store(false, std::sync::atomic::Ordering::Relaxed);
    }

    fn apply_policy(&mut self, u: PolicyUpdate) {
        let c = &mut self.config;
        if let Some(v) = u.allow_info { c.allow_info = v }
        if let Some(v) = u.allow_pings { c.allow_pings = v }
        if let Some(v) = u.verbose_snr_auto_accept { c.verbose_snr_auto_accept = v }
        if let Some(v) = u.welcome_message { c.welcome_message = v }
        if let Some(v) = u.welcome_always { c.welcome_always = v }
        if let Some(v) = u.email { c.email = v }
        if let Some(v) = u.full_callsign { c.full_callsign = v }
        if let Some(v) = u.call_id_minutes { c.call_id_minutes = v }
        if let Some(v) = u.idle_disconnect_minutes { c.idle_disconnect_minutes = v }
        if let Some(v) = u.allow_last_heard { c.allow_last_heard = v }
        if let Some(v) = u.multi_band_last_heard { c.multi_band_last_heard = v }
        self.sync_policy();
    }

    fn call(&self) -> &str {
        &self.station.callsign
    }

    fn with_db<T>(&self, f: impl FnOnce(&Storage) -> T) -> T {
        f(&self.db.lock().unwrap_or_else(PoisonError::into_inner))
    }

    async fn run(mut self, mut cmd_rx: mpsc::UnboundedReceiver<Command>) {
        // Verbose SNR: one <R+NN> a minute while it runs (VarAC)
        // (verbose SNR keeps VarAC's own clock: checked every 2 s)
        let mut minute = tokio::time::interval(std::time::Duration::from_secs(2));
        minute.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // VarAC slots: the move to the slot after a CQ, and the wait there running out
        let mut second = tokio::time::interval(std::time::Duration::from_secs(1));
        second.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            let Some(sess) = self.session.as_mut() else {
                match cmd_rx.recv().await {
                    Some(cmd) => self.command(cmd).await,
                    None => return,
                }
                continue;
            };
            // The modem went away (stopped, or the network dropped)
            if sess.modem_closed() {
                self.modem_lost("the modem closed the connection");
                continue;
            }
            // Only waiting races the command channel; an input is always handled to the end.
            let input = tokio::select! {
                cmd = cmd_rx.recv() => {
                    match cmd {
                        Some(cmd) => self.command(cmd).await,
                        None => return,
                    }
                    continue;
                }
                _ = second.tick() => {
                    self.slot_tick();
                    // No answer in the "QSY NOW" countdown: the desktop's default without
                    // auto-QSY, reject
                    if self.qsy_popup.is_some_and(|(until, _)| std::time::Instant::now() >= until) {
                        self.answer_qsy(false).await;
                    }
                    match self.conduct_tick().await {
                        Err(e) if e.kind() != std::io::ErrorKind::NotConnected => self.modem_lost(&format!("modem error: {e}")),
                        _ => {}
                    }
                    continue;
                }
                _ = minute.tick() => {
                    match sess.verbose_snr_tick().await {
                        Err(e) if e.kind() != std::io::ErrorKind::NotConnected => self.modem_lost(&format!("modem error: {e}")),
                        _ => {}
                    }
                    continue;
                }
                input = sess.next_input() => input,
            };
            let Some(input) = input else { continue };
            // The session offers a QSY (<Q>) only when a link starts on a calling frequency
            // (the desktop keeps this current from its dial the same way)
            self.station.policy.on_calling_frequency.store(
                self.frequency == 0 || on_calling_freq(self.frequency),
                std::sync::atomic::Ordering::Relaxed,
            );
            let Some(sess) = self.session.as_mut() else { continue };
            if let Some(event) = sess.handle_input(input).await {
                self.event(event).await;
            }
        }
    }

    fn modem_lost(&mut self, why: &str) {
        self.session = None;
        self.leave_slot("offline");
        self.end_qso();
        emit(json!({"type": "modem", "connected": false, "error": why}));
    }

    async fn connect_modem(&mut self) {
        let c = &self.config;
        let result = async {
            let mut vara = VaraConnection::connect(&c.host, c.cmd_port, c.cmd_port + 1).await?;
            // As the desktop's modem connect (as in VarAC): VARA HF always
            // starts at 500 Hz; one MYCALL, with the -T ping alias only when pings are allowed
            vara.initialize_for(&self.station.callsign, Bandwidth::Bw500, crate::vara::commands::ModemType::Hf, c.allow_pings).await?;
            Ok::<_, std::io::Error>(vara)
        }
        .await;
        match result {
            Ok(vara) => {
                let mut sess = VaracSession::new(vara, self.station.clone());
                // Calling a station, we choose the slot to QSY to (VarAC's picker)
                sess.ask_qsy_invite = true;
                // Broadcasts go over the modem's KISS port (VarAC's default: command port - 200)
                if let Err(e) = sess.attach_kiss(&c.host, c.cmd_port.saturating_sub(200)).await {
                    log(format!("KISS port not available ({e}): no broadcasts"));
                }
                self.session = Some(sess);
                // (the modem is at 500 Hz now: our CQ frames and the calling-frequency checks say so)
                if self.config.bandwidth.trim() != "500" {
                    self.config.bandwidth = "500".into();
                    system("Modem bandwidth set to 500 Hz (VARA HF starts at 500)");
                    // (so the app's bandwidth buttons show it)
                    emit(json!({"type": "bandwidth", "bw": "500"}));
                }
                emit(json!({"type": "modem", "connected": true}));
            }
            Err(e) => emit(json!({"type": "modem", "connected": false, "error": e.to_string()})),
        }
    }

    async fn command(&mut self, cmd: Command) {
        if let Command::ConnectModem = cmd {
            if self.session.is_none() {
                self.connect_modem().await;
            }
            return;
        }
        if let Command::DisconnectModem = cmd {
            if self.session.take().is_some() {
                self.leave_slot("offline");
                self.end_qso();
                emit(json!({"type": "modem", "connected": false}));
            }
            return;
        }
        if let Command::SetFrequency { hz } = cmd {
            self.frequency = hz;
            return;
        }
        if let Command::Vmail { to, subject, body, urgent } = cmd {
            self.queue_vmail(to, subject, body, urgent).await;
            return;
        }
        if let Command::SlotReturn = cmd {
            self.leave_slot("returned");
            return;
        }
        if let Command::ReturnCf { hz } = cmd {
            self.return_to_cf(hz, "returned");
            return;
        }
        if let Command::Policy(u) = cmd {
            self.apply_policy(u);
            let r = match self.session.as_mut() {
                Some(sess) => sess.apply_ping_policy().await,
                None => Ok(()),
            };
            if let Err(e) = r {
                self.modem_lost(&format!("modem error: {e}"));
            }
            return;
        }
        if let Command::Connect { call, slot_hz: Some(hz) } = &cmd {
            if self.cf_after_qso.is_none() {
                self.cf_after_qso = Some(self.frequency);
            }
            self.frequency = *hz;
            system(format!("On {} MHz to answer {}", mhz(*hz), call.trim().to_uppercase()));
        }
        if matches!(cmd, Command::Connect { .. } | Command::Ping { .. } | Command::Cq { .. } | Command::Beacon) && self.wide_on_calling_freq() {
            system(
                "Bandwidth not allowed: you can't use a 2300/2750 Hz bandwidth on a calling QRG as it may \
                 cause QRM to nearby VarAC stations. Switch your bandwidth to 500 Hz or change frequency.",
            );
            // (the app shows VarAC's box, with a switch to 500 Hz)
            emit(json!({"type": "bw_refused", "bandwidth": self.config.bandwidth, "frequency": self.frequency}));
            return;
        }
        // Off the air the bandwidth is just remembered: the modem gets it when it connects
        if let Command::Bandwidth { bw } = &cmd {
            if self.session.is_none() {
                self.config.bandwidth = bw.clone();
                return;
            }
        }
        let call = self.call().to_string();
        let Some(mut session) = self.session.take() else {
            system("The modem isn't connected");
            return;
        };
        let sess = &mut session;
        let r = match cmd {
            Command::Connect { call: dest, .. } => {
                let dest = dest.trim().to_uppercase();
                system(format!("CALLING {dest}"));
                sess.vara.send_command(&VaraCommand::Connect { source: call, destination: dest }).await
            }
            Command::Ping { call: dest } => {
                // VarAC pings by linking to the station's "-T" alias; it answers with <R+NN>
                let base = dest.trim().to_uppercase().split('-').next().unwrap_or("").to_string();
                sess.vara.send_command(&VaraCommand::Connect { source: call, destination: format!("{base}-T") }).await
            }
            Command::Disconnect => sess.send_disconnect().await,
            Command::Abort => {
                // A slot CQ that's aborted never moves to the slot
                self.slot_cq = None;
                sess.vara.send_command(&VaraCommand::Abort).await
            }
            Command::Chat { text } => {
                let wire = text.replace('\n', "//n");
                let r = sess.send_chat(&wire).await;
                if r.is_ok() {
                    emit(json!({"type": "chat", "from": call, "text": text, "outgoing": true, "time": now_hm()}));
                    self.store_chat(2, &call, &text);
                }
                r
            }
            Command::Typing => sess.send_is_typing().await,
            Command::Cq { slot, wait_secs } => {
                // VarAC: the CQ's SSID names the slot we'll wait on afterwards (none = the
                // bare callsign); the move there follows once the CQ is out
                let slot = if slots::offset_hz(slot, slots::DEFAULT_SLOT_STEP_HZ).is_some() { slot } else { 0 };
                let source = slots::cq_source(&call, slot);
                let bw = self.config.bandwidth.parse().unwrap_or(500);
                let r = sess.vara.send_command(&VaraCommand::CqFrame { source: source.clone(), bandwidth: bw }).await;
                if r.is_ok() {
                    self.leave_slot("returned");
                    if slot == 0 {
                        system("CQ sent");
                    } else {
                        system(format!("CQ sent as {source}: you'll wait on {} for replies", slots::label(slot, slots::DEFAULT_SLOT_STEP_HZ)));
                        self.slot_cq = Some(SlotCq { slot, cf: self.frequency, wait_secs, sent: std::time::Instant::now() });
                    }
                }
                r
            }
            Command::Beacon => {
                let bw = self.config.bandwidth.parse().unwrap_or(500);
                let r = sess.vara.send_command(&VaraCommand::CqFrame { source: format!("{call}-9"), bandwidth: bw }).await;
                if r.is_ok() {
                    system("Beacon sent");
                }
                r
            }
            Command::Listen { mode } => {
                let mode = match mode.as_str() {
                    "off" => ListenMode::Off,
                    "cq" => ListenMode::Cq,
                    _ => ListenMode::On,
                };
                sess.vara.send_command(&VaraCommand::Listen(mode)).await
            }
            Command::Bandwidth { bw } => {
                self.config.bandwidth = bw.clone();
                sess.vara.send_command(&VaraCommand::SetBandwidth(bandwidth(&bw))).await
            }
            Command::Broadcast { text } => {
                let r = sess.send_broadcast(&text).await;
                match &r {
                    Ok(()) => emit(json!({"type": "broadcast", "from": call, "to": "ALL", "message": text, "time": now_hm()})),
                    Err(e) => system(format!("Broadcast not sent: {e}")),
                }
                r
            }
            Command::Away { text } => {
                self.away = text.is_some();
                // (VarAC announces away only when a link starts, never during a QSO)
                sess.set_away(text.clone());
                Ok(())
            }
            Command::FileAccept => {
                if self.pending_offer.is_some() && std::mem::take(&mut self.offer_asked) {
                    system("Receiving the file…");
                    if self.pending_attachment {
                        sess.answer_attachment(true).await
                    } else {
                        sess.send_file_ready().await
                    }
                } else {
                    Ok(())
                }
            }
            Command::FileDecline => {
                if !std::mem::take(&mut self.offer_asked) {
                    return;
                }
                self.pending_offer = None;
                if std::mem::take(&mut self.pending_attachment) {
                    sess.answer_attachment(false).await
                } else {
                    sess.send_file_abort().await
                }
            }
            Command::QsyAnswer { accept } => {
                self.answer_qsy(accept).await;
                Ok(())
            }
            Command::QsyAccept => sess.send_qsy_accept().await,
            Command::QsyReject => sess.send_qsy_reject().await,
            Command::SnrRequest => sess.send_snr_request().await,
            Command::InfoRequest => sess.send_info_request().await,
            Command::Tune => sess.vara.send_command(&VaraCommand::Tune(true)).await,
            Command::TuneStop => sess.vara.send_command(&VaraCommand::Tune(false)).await,
            Command::ClearTxBuffer => sess.vara.send_command(&VaraCommand::CleanTxBuffer).await,
            Command::QsyRequest { offset_hz } => {
                let r = sess.send_qsy_request(offset_hz / 10).await;
                if r.is_ok() {
                    system(format!("QSY request sent: {offset_hz:+} Hz"));
                }
                r
            }
            Command::QsyInvite { slot } => {
                let hz = slots::offset_hz(slot, slots::DEFAULT_SLOT_STEP_HZ).unwrap_or(0);
                if hz != 0 {
                    // The calling frequency to offer going back to after the QSO
                    if self.cf_after_qso.is_none() && on_calling_freq(self.frequency) {
                        self.cf_after_qso = Some(self.frequency);
                    }
                }
                sess.send_qsy_invitation((hz / 10) as i32).await
            }
            Command::Inquire { what, call } => {
                // VarAC's INQUIRE window: questions the partner's VarAC answers itself
                let tag = match what.as_str() {
                    "last_heard" => Some("<LHR>".to_string()),
                    "last_heard_call" => call
                        .map(|c| c.trim().to_uppercase())
                        .filter(|c| !c.is_empty())
                        .map(|c| format!("<LHC:{c}>")),
                    "parked" => Some("<VRP>".into()),
                    "locator" => Some("<LOCR>".into()),
                    "gps" => Some("<GPSR>".into()),
                    "version" => Some("<VER>".into()),
                    "schedule" => Some("<FSR>".into()),
                    _ => None,
                };
                match tag {
                    Some(t) => sess.send_chat(&t).await,
                    None => {
                        system(format!("Unknown inquiry: {what}"));
                        Ok(())
                    }
                }
            }
            Command::VerboseSnr { on } => {
                let r = sess.set_verbose_snr(on).await;
                if r.is_ok() {
                    system(if on { "Verbose SNR invitation sent" } else { "Verbose SNR stopped" });
                    emit(json!({"type": "verbose_snr", "on": on}));
                }
                r
            }
            Command::SendFile { path, pct, quality } => self.send_file(sess, &path, pct, quality).await,
            // Before the partner says send it, the offer is withdrawn (<SFAB>). After, VARA
            // has the data and can't take it back (its CLEANTXBUFFER only reports an empty
            // buffer, and a DISCONNECT waits for it): only ABORT stops it, ending the QSO
            Command::FileCancel => {
                if sess.file_data_going() {
                    let name = self.sending.take().map(|f| f.name).unwrap_or_default();
                    system(format!("File transfer stopped: {name}. The link was dropped (VARA can't take back what it's sending)"));
                    emit(json!({"type": "file_send", "state": "cancelled", "name": name}));
                    sess.vara.send_command(&VaraCommand::Abort).await
                } else {
                    match sess.cancel_outgoing_file().await {
                        Ok(true) => {
                            let name = self.sending.take().map(|f| f.name).unwrap_or_default();
                            system(format!("File offer withdrawn: {name}"));
                            emit(json!({"type": "file_send", "state": "cancelled", "name": name}));
                            Ok(())
                        }
                        Ok(false) => Ok(()),
                        Err(e) => Err(e),
                    }
                }
            }
            Command::VerboseSnrAnswer { accept } => {
                let r = sess.answer_verbose_snr(accept).await;
                if r.is_ok() {
                    system(if accept { "Verbose SNR accepted: SNR reports every minute" } else { "Verbose SNR rejected" });
                    emit(json!({"type": "verbose_snr", "on": accept}));
                }
                r
            }
            Command::ConnectModem
            | Command::DisconnectModem
            | Command::SetFrequency { .. }
            | Command::Vmail { .. }
            | Command::Policy(_)
            | Command::SlotReturn
            | Command::ReturnCf { .. } => Ok(()),
        };
        match r {
            Ok(()) => self.session = Some(session),
            // A QSO command without a link: the modem is fine
            Err(e) if e.kind() == std::io::ErrorKind::NotConnected => {
                system("Not in a QSO");
                self.session = Some(session);
            }
            Err(e) => self.modem_lost(&format!("modem error: {e}")),
        }
    }

    /// Once a second: a slot CQ that's out (if no PTT report came, after 15 s) moves to its
    /// slot; a wait there that ran out goes back to the calling frequency.
    /// The "QSY NOW" answer, by the operator or by the countdown.
    async fn answer_qsy(&mut self, accept: bool) {
        let Some((_, offset)) = self.qsy_popup.take() else { return };
        emit(json!({"type": "qsy_invitation_closed"}));
        if let Some(sess) = self.session.as_mut() {
            let _ = sess.answer_qsy(accept, false, offset).await;
        }
        if !accept {
            let target = (self.frequency as i64 + offset as i64).max(0) as u64;
            system(format!("QSY to {} rejected", crate::varac::messages::freq_dotted(target as i64)));
        }
    }

    fn slot_tick(&mut self) {
        if self.slot_cq.as_ref().is_some_and(|c| c.sent.elapsed().as_secs() >= 15) {
            self.start_slot_wait();
        }
        if self.slot_wait.as_ref().is_some_and(|w| std::time::Instant::now() >= w.until) {
            system("No reply on the slot");
            self.leave_slot("timeout");
        }
    }

    /// Our slot CQ is on the air: stand by on the slot (the operator retunes; no rig control).
    fn start_slot_wait(&mut self) {
        let Some(c) = self.slot_cq.take() else { return };
        let Some(hz) = slots::frequency(c.cf, c.slot, slots::DEFAULT_SLOT_STEP_HZ) else { return };
        self.frequency = hz;
        self.slot_wait = Some(SlotWait { cf: c.cf, until: std::time::Instant::now() + std::time::Duration::from_secs(c.wait_secs) });
        let label = slots::label(c.slot, slots::DEFAULT_SLOT_STEP_HZ);
        system(format!("Tune your radio to {} MHz, {label}, and wait there for a reply", mhz(hz)));
        emit(json!({"type": "slot_wait", "slot": c.slot, "label": label, "hz": hz, "cf_hz": c.cf, "seconds": c.wait_secs}));
    }

    /// Leave our slot (the wait ran out or was ended, the QSO there is over, or we went
    /// offline): back to the calling frequency.
    fn leave_slot(&mut self, why: &str) {
        self.slot_cq = None;
        let cf = match self.slot_wait.take() {
            Some(w) => {
                emit(json!({"type": "slot_wait_end", "why": why}));
                Some(w.cf)
            }
            None => self.cf_after_qso.take(),
        };
        let Some(cf) = cf.filter(|cf| *cf != self.frequency) else { return };
        if why == "qso_end" {
            // VarAC: "Your QSO is over. Do you want to switch back to the calling frequency?"
            emit(json!({"type": "qsy_back", "cf_hz": cf}));
        } else {
            self.return_to_cf(cf, why);
        }
    }

    /// Both stations agreed to QSY by `hz`: the operator retunes (no rig control).
    async fn moved_by(&mut self, hz: i64) {
        // Leaving the calling frequency: offer to go back after the QSO (VarAC)
        if self.cf_after_qso.is_none() && on_calling_freq(self.frequency) {
            self.cf_after_qso = Some(self.frequency);
        }
        self.frequency = (self.frequency as i64 + hz).max(0) as u64;
        system(format!("QSY agreed: retune your radio by {hz:+} Hz to {} MHz", mhz(self.frequency)));
        emit(json!({"type": "qsy", "offset_hz": hz}));
        // Off the calling frequency now: VMail waiting for the QSY goes out
        self.conduct.qsy_done = true;
        if self.conduct.link_open && !self.conduct.mail_delivered {
            self.offer_mail().await;
        }
    }

    fn return_to_cf(&mut self, cf: u64, why: &str) {
        self.frequency = cf;
        system(format!("Back to the calling frequency: retune your radio to {} MHz", mhz(cf)));
        emit(json!({"type": "cf_return", "cf_hz": cf, "why": why}));
    }

    /// Every second during a link: Call ID, idle disconnect and VarAC's calling-frequency
    /// limits (away 5 min → <AE>; reminder, then abort at 240 s, pings 90 s).
    async fn conduct_tick(&mut self) -> std::io::Result<()> {
        let Some(mut sess) = self.session.take() else { return Ok(()) };
        let r = self.conduct_step(&mut sess).await;
        self.session = Some(sess);
        r
    }

    async fn conduct_step(&mut self, s: &mut VaracSession) -> std::io::Result<()> {
        use std::time::{Duration, Instant};
        let Some(since) = self.conduct.since.filter(|_| s.is_connected()) else { return Ok(()) };
        let now = Instant::now();
        let age = now - since;
        let id_min = Some(self.config.call_id_minutes).filter(|m| (5..=30).contains(m)).unwrap_or(5);
        let idle_min = self.config.idle_disconnect_minutes;
        // Station ID: "de CALL <R+NN>" (VarAC)
        if !s.is_ping() && self.conduct.last_id.is_some_and(|t| now - t >= Duration::from_secs(id_min * 60)) {
            let snr = self.last_snr.map(|v| format!(" <R{}>", crate::varac::messages::snr_text(v.round() as i32))).unwrap_or_default();
            s.send_chat(&format!("de {}{snr}", self.station.callsign)).await?;
            (self.conduct.last_id, self.conduct.id_on_air) = (Some(now), true);
            log("Call ID sent");
        }
        // Idle: tell the partner <IE> and disconnect (the called side first)
        let idle_limit = Duration::from_secs(idle_min * 60 + if s.is_incoming() { 0 } else { 10 });
        if idle_min > 0
            && !self.conduct.bye_sent
            && !s.is_transferring()
            && self.conduct.last_activity.is_some_and(|t| now - t >= idle_limit)
        {
            system("IDLE TIME LIMIT EXCEEDED. DISCONNECTING...");
            s.send_chat("<IE>").await?;
            (self.conduct.bye_sent, self.conduct.bye_tag_pending) = (true, true);
        }
        if on_calling_freq(self.frequency) && !self.conduct.cf_abort_done {
            // Away on a calling frequency: 5 minutes, then <AE> and disconnect
            if s.is_away() && s.is_incoming() && !s.is_ping() && !self.conduct.bye_sent && age >= Duration::from_secs(300) {
                system("CONNECTION ON CALLING FREQUENCY IN AWAY STATUS TIME LIMIT EXCEEDED. DISCONNECTING...");
                s.send_chat("<AE>").await?;
                (self.conduct.bye_sent, self.conduct.bye_tag_pending) = (true, true);
            }
            let limit: u64 = if s.is_ping() { 90 } else { 240 };
            // ...after a reminder (caller at 150 s, called station at 210 s)
            let warn_at = if s.is_incoming() { 210 } else { 150 };
            if !s.is_ping() && !self.conduct.cf_warned && age >= Duration::from_secs(warn_at) {
                self.conduct.cf_warned = true;
                system(format!("You're on a calling Freq. PSE QSY OR LINK WILL BE CLOSED IN {} SEC.", limit.saturating_sub(age.as_secs())));
            }
            if age >= Duration::from_secs(limit) {
                self.conduct.cf_abort_done = true;
                system(if s.is_ping() {
                    "PINGING ON CALLING FREQUENCY TIME LIMIT EXCEEDED (90 SECONDS) DISCONNECTING..."
                } else {
                    "CONNECTION ON CALLING FREQUENCY TIME LIMIT EXCEEDED (240 SECONDS) DISCONNECTING..."
                });
                s.vara.send_command(&VaraCommand::Abort).await?;
            }
        }
        Ok(())
    }

    /// VarAC refuses CQ, connect, ping and beacon at 2300/2750 Hz on a calling frequency.
    fn wide_on_calling_freq(&self) -> bool {
        self.config.bandwidth.trim() == "2300" && crate::varac::messages::is_calling_frequency(self.frequency)
    }

    async fn queue_vmail(&mut self, to: String, subject: String, body: String, urgent: bool) {
        let to = to.trim().to_uppercase();
        let guid = uuid::Uuid::new_v4().to_string();
        let rec = VMailRecord {
            guid: guid.clone(),
            creation_time: Utc::now(),
            sent_time: None,
            received_time: None,
            folder_id: VMailFolder::Outbox as i32,
            vmail_to: to.clone(),
            vmail_from: self.call().to_string(),
            vmail_via: None,
            subject,
            msg: body,
            read_status: true,
            is_deleted: false,
            urgent,
            delivery_band: None,
            delivery_snr: None,
            frequency: None,
        };
        if let Err(e) = self.with_db(|d| d.insert_vmail(&rec)) {
            system(format!("VMail not saved: {e}"));
            return;
        }
        let in_qso_with_addressee = self
            .partner
            .as_deref()
            .is_some_and(|p| p.split('-').next().unwrap_or(p).eq_ignore_ascii_case(to.split('-').next().unwrap_or(&to)));
        if in_qso_with_addressee {
            self.offer_mail().await;
        } else {
            system(format!("VMail to {to} is in the Outbox; it goes out on your next QSO with {to}"));
        }
        emit(json!({"type": "vmail_changed"}));
    }

    /// VMail for the partner, once the link is open. VarAC relays it only off the calling
    /// frequency: a link that began on one waits for the QSY (or an AWAY partner), and the
    /// partner hears <VW> meanwhile, once.
    async fn offer_mail(&mut self) {
        let c = &self.conduct;
        if !c.link_open {
            return;
        }
        if !c.started_on_cf || c.qsy_done || c.partner_away {
            self.conduct.mail_delivered = true;
            self.deliver_queued().await;
        } else if !c.vw_sent && self.has_mail_for_partner() {
            self.conduct.vw_sent = true;
            let partner = self.partner.clone().unwrap_or_default();
            if let Some(sess) = self.session.as_mut() {
                if let Err(e) = sess.send_chat("<VW>").await {
                    log(format!("<VW> not sent: {e}"));
                }
            }
            system(format!("You have VMail waiting for {partner}: invite them to QSY; it is relayed automatically after the QSY"));
            emit(json!({"type": "vmail_relay_alert", "call": partner}));
        }
    }

    /// Outbox or parked VMail waiting for the partner?
    fn has_mail_for_partner(&self) -> bool {
        self.partner.as_deref().is_some_and(|p| has_mail_for(&self.db, p))
    }

    /// The partner's Outbox VMail and VMail parked here for it; each is filed as Sent when
    /// the partner acknowledges it (<SMR>).
    async fn deliver_queued(&mut self) {
        let Some(partner) = self.partner.clone() else { return };
        let base = partner.split('-').next().unwrap_or(&partner).to_uppercase();
        let jobs: Vec<(String, VMail, Vec<PathBuf>)> = self.with_db(|d| {
            let mut recs = d.outbox_vmails_for(&partner).unwrap_or_default();
            recs.extend(
                d.vmails_in_folder(VMailFolder::Parking, 200)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|vm| vm.vmail_to.split('-').next().unwrap_or("").eq_ignore_ascii_case(&base)),
            );
            recs.iter()
                .filter(|vm| !self.conduct.awaiting_ack.iter().any(|(g, _)| *g == vm.guid))
                .map(|vm| {
                    let vmail = VMail {
                        guid: vm.guid.clone(),
                        from: vm.vmail_from.clone(),
                        to: vm.vmail_to.clone(),
                        subject: vm.subject.clone(),
                        body: vm.msg.clone(),
                        compose_time: vm.creation_time.format("%Y-%m-%d %H:%M:%S").to_string(),
                        timestamp: vm.creation_time,
                        urgent: vm.urgent,
                        raw: false,
                        is_parked: false,
                        has_attachment: false,
                        reply_to: None,
                    };
                    (vm.guid.clone(), vmail, d.vmail_attachments(&vm.guid).unwrap_or_default())
                })
                .collect()
        });
        let Some(mut session) = self.session.take() else { return };
        let sess = &mut session;
        for (guid, vmail, paths) in jobs {
            let to = vmail.to.clone();
            let ok = if paths.is_empty() {
                sess.send_vmail(&vmail).await.is_ok()
            } else {
                let files: Vec<(String, Vec<u8>)> = paths
                    .iter()
                    .filter_map(|p| Some((p.file_name()?.to_string_lossy().into_owned(), std::fs::read(p).ok()?)))
                    .collect();
                sess.send_vmail_with_attachments(&VMail { has_attachment: true, ..vmail }, &files).await.is_ok()
            };
            if ok {
                system(format!("VMail to {to} sent, waiting for confirmation"));
                self.conduct.awaiting_ack.push_back((guid, to));
            }
        }
        self.session = Some(session);
    }

    /// Send a reply in the QSO (peek answers).
    async fn answer(&mut self, text: &str) {
        if let Some(sess) = self.session.as_mut() {
            if let Err(e) = sess.send_chat(text).await {
                log(format!("Reply not sent: {e}"));
            }
        }
    }

    fn store_chat(&self, entry_type: i32, callsign: &str, text: &str) {
        let Some(qso) = self.qso_guid.clone() else { return };
        self.with_db(|d| {
            d.insert_chat(&ChatRecord {
                guid: uuid::Uuid::new_v4().to_string(),
                entry_type,
                qso_guid: qso,
                callsign: callsign.to_string(),
                entry: text.to_string(),
                creation_time: Utc::now(),
                chat_id: None,
                reply_on_chat_id: None,
            })
            .ok()
        });
    }

    fn end_qso(&mut self) {
        self.qso_guid = None;
        // The link ended: write its qso row (VarAC: every link)
        if let Some((mut q, x)) = self.qso_row.take() {
            q.endtime = Some(Utc::now());
            if let Err(e) = self.with_db(|d| d.insert_qso_full(&q, &x)) {
                log(format!("Error: writing to qso table. {e}"));
            }
        }
        if self.partner.take().is_some() {
            emit(json!({"type": "disconnected"}));
        }
        self.pending_offer = None;
        self.pending_attachment = false;
        self.offer_asked = false;
        if self.qsy_popup.take().is_some() {
            emit(json!({"type": "qsy_invitation_closed"}));
        }
    }

    /// `send_file`: offer the file (or the shrunk picture) the VarAC way. Refused off a
    /// QSO, on a calling frequency (VarAC doesn't send files there), while another file
    /// of ours is going, and over VARA HF's size limit for the link.
    async fn send_file(&mut self, sess: &mut VaracSession, path: &str, pct: Option<u32>, quality: Option<u8>) -> std::io::Result<()> {
        let refuse = |why: &str| {
            system(format!("Can't send the file: {why}"));
            emit(json!({"type": "file_send", "state": "refused", "why": why}));
            Ok(())
        };
        if !sess.is_connected() {
            return refuse("not in a QSO");
        }
        if on_calling_freq(self.frequency) {
            return refuse("files aren't sent on a calling frequency. QSY first");
        }
        if self.sending.is_some() || sess.sending_file() {
            return refuse("a file is already being sent");
        }
        let shrink = pct.map(|p| (p, quality.unwrap_or(crate::files::DEFAULT_QUALITY)));
        let out = match crate::files::outgoing(std::path::Path::new(path), shrink) {
            Ok(o) => o,
            Err(e) => return refuse(&e),
        };
        let prepared = match bbs::prepare_file(&out.name, &out.data) {
            Ok(p) => p,
            Err(e) => return refuse(&e.to_string()),
        };
        // VARA HF's limits on the compressed file (VarAC's words)
        if let Some(limit) = crate::varac::session::attachment_limit(self.link_bw) {
            if prepared.compressed_size >= limit {
                let (k, bw) = if limit == 100_000 { ("100K", 500) } else { ("1M", 2300) };
                return refuse(&format!("VARA HF doesn't allow files over {k} compressed at {bw} Hz ({} bytes)", prepared.compressed_size));
            }
        }
        let frame_bytes = crate::varac::framing::encode_frame(&format!("<SFB>{}", prepared.base64_payload)).len();
        let compressed = prepared.compressed_size;
        sess.send_file(prepared).await?;
        let partner = self.partner.clone().unwrap_or_else(|| "the partner".into());
        system(format!("File offered to {partner}: {} ({} bytes, {compressed} compressed)", out.name, out.data.len()));
        emit(json!({"type": "file_send", "state": "offered", "name": out.name, "bytes": out.data.len(), "compressed": compressed}));
        self.sending = Some(SendingFile { name: out.name, frame_bytes, started: false });
        Ok(())
    }

    /// Our file didn't make it (`state`: refused or failed).
    fn file_send_ended(&mut self, state: &str, why: &str) {
        if let Some(f) = self.sending.take() {
            system(format!("File not sent: {} ({why})", f.name));
            emit(json!({"type": "file_send", "state": state, "name": f.name, "why": why}));
        }
    }

    fn received_dir(&self) -> PathBuf {
        let dir = PathBuf::from(&self.config.data_dir).join("Received");
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    async fn event(&mut self, event: SessionEvent) {
        match event {
            SessionEvent::Connected { source, destination, bandwidth } => {
                let ping = source.to_uppercase().ends_with("-T") || destination.to_uppercase().ends_with("-T");
                // VARA reports the link as source → destination; the partner is whichever isn't us
                let mine = |c: &str| c.split('-').next().unwrap_or(c).eq_ignore_ascii_case(self.call().split('-').next().unwrap_or(""));
                let partner = if mine(&destination) { source.clone() } else { destination.clone() };
                let partner = partner.trim_end_matches("-T").trim_end_matches("-t").to_string();
                let bw = bandwidth.unwrap_or_else(|| self.config.bandwidth.clone());
                self.link_bw = bw.trim().parse().ok();
                self.partner = Some(partner.clone());
                self.slot_cq = None;
                if let Some(w) = self.slot_wait.take() {
                    self.cf_after_qso = Some(w.cf);
                    emit(json!({"type": "slot_wait_end", "why": "connected"}));
                }
                let now = std::time::Instant::now();
                self.conduct = Conduct {
                    since: Some(now),
                    started_on_cf: on_calling_freq(self.frequency),
                    last_id: Some(now),
                    last_activity: Some(now),
                    ..Conduct::default()
                };
                // VarAC's outbox doesn't wait for the handshake: on a calling frequency
                // it says <VW> as soon as the link is up
                if self.conduct.started_on_cf && !ping && self.has_mail_for_partner() {
                    self.conduct.vw_sent = true;
                    if let Some(sess) = self.session.as_mut() {
                        if let Err(e) = sess.send_chat("<VW>").await {
                            log(format!("<VW> not sent: {e}"));
                        }
                    }
                    system(format!("You have VMail waiting for {partner}: invite them to QSY; it is relayed automatically after the QSY"));
                    emit(json!({"type": "vmail_relay_alert", "call": partner}));
                }
                emit(json!({"type": "connected", "call": partner, "bandwidth": bw, "ping": ping, "time": now_hm()}));
                // The qso row (VarAC's columns; pings get one too), written
                // when the link ends
                let guid = uuid::Uuid::new_v4().to_string();
                let full = self.config.full_callsign.trim().to_uppercase();
                let rec = QsoRecord {
                    guid: guid.clone(),
                    starttime: Utc::now(),
                    endtime: None,
                    frequency: self.frequency as i64,
                    band: band_of(self.frequency).to_string(),
                    bandwidth: bw.clone(),
                    callsign: partner.clone(),
                    my_callsign: if full.is_empty() { self.call().to_string() } else { full },
                    snr_received: None,
                    snr_sent: None,
                    name: None,
                    qth: None,
                    comments: Some(format!("{} using Openslot", if ping { "PING" } else { "LINK" })),
                };
                let extra = QsoExtra {
                    mode: "DYNAMIC".into(),
                    submode: crate::vara::commands::ModemType::from_key("").submode().into(),
                    me_away: self.away,
                    is_ping: ping,
                    version: env!("CARGO_PKG_VERSION").into(),
                    ..Default::default()
                };
                self.qso_row = Some((rec, extra));
                if ping {
                    system(format!("PING {partner}"));
                    return;
                }
                system(format!("CONNECTED TO {partner} ({bw} Hz)"));
                self.qso_guid = Some(guid);
            }
            SessionEvent::Disconnected => {
                system("DISCONNECTED");
                self.link_bw = None;
                self.file_send_ended("failed", "the link closed before the file was sent");
                // A file we said yes to that never all came
                if self.pending_offer.is_some() && !self.offer_asked {
                    let name = self.pending_offer.take().map(|o| bbs::strip_gzip_suffix(&o.name).to_string()).unwrap_or_default();
                    system(format!("File not received: {name} (the link closed)"));
                    emit(json!({"type": "file_receive_failed"}));
                }
                self.conduct = Conduct::default();
                self.end_qso();
                self.leave_slot("qso_end");
                emit(json!({"type": "verbose_snr", "on": false}));
            }
            SessionEvent::Ptt(on) => {
                emit(json!({"type": "ptt", "on": on}));
                // The slot CQ has gone out (PTT released): now move to the slot
                if !on && self.slot_cq.as_ref().is_some_and(|c| c.sent.elapsed().as_millis() > 800) {
                    self.start_slot_wait();
                }
            }
            SessionEvent::BufferUpdate(n) => {
                emit(json!({"type": "buffer", "bytes": n}));
                if let Some(total) = self.sending.as_ref().filter(|f| f.started).map(|f| f.frame_bytes) {
                    let sent = total.saturating_sub(n as usize);
                    emit(json!({"type": "file_progress", "outgoing": true, "done": sent, "total": total}));
                }
                if n == 0 {
                    // What we sent is out: QSO activity (but not our own Call ID)
                    if !std::mem::take(&mut self.conduct.id_on_air) {
                        self.conduct.last_activity = Some(std::time::Instant::now());
                    }
                    // <IE>/<AE> is on the air: drop the link (VarAC sends VARA DISCONNECT)
                    if std::mem::take(&mut self.conduct.bye_tag_pending) {
                        if let Some(sess) = self.session.as_mut() {
                            if let Err(e) = sess.vara.send_command(&VaraCommand::Disconnect).await {
                                log(format!("DISCONNECT failed: {e}"));
                            }
                        }
                    }
                }
            }
            SessionEvent::Busy(on) => emit(json!({"type": "busy", "on": on})),
            SessionEvent::CqFrame { source, bandwidth } => {
                let base = source.split('-').next().unwrap_or(&source).to_string();
                let beacon = source.ends_with("-9");
                // VarAC: a CQ's SSID is the slot its caller waits on (1-5 below the calling
                // frequency, 11-15 above); answering means tuning there first
                let slot = slots::slot_of_cq(&source);
                let rec = CqFrameRecord {
                    guid: uuid::Uuid::new_v4().to_string(),
                    cqframe_time: Utc::now(),
                    cqframe_type_id: if beacon { 2 } else { 1 },
                    frequency: self.frequency as i64,
                    bandwidth: bandwidth.to_string(),
                    from_callsign: base.clone(),
                    snr: self.last_snr.map(|s| s.round() as i32),
                    slot: (slot != 0).then_some(slot as i32),
                    locator: None,
                    band: Some(band_of(self.frequency).to_string()),
                };
                self.with_db(|d| {
                    d.insert_cqframe(&rec).ok();
                    d.upsert_heard_station(&base).ok();
                });
                let slot_hz = (slot != 0).then(|| slots::frequency(self.frequency, slot, slots::DEFAULT_SLOT_STEP_HZ)).flatten();
                emit(json!({
                    "type": "cq", "call": base, "source": source, "bandwidth": bandwidth, "beacon": beacon,
                    "slot": slot, "slot_label": slot_label(slot), "slot_hz": slot_hz, "time": now_hm(),
                }));
            }
            SessionEvent::TypedCq(cq) => {
                system(format!("CQ {} de {} {} (slot {})", cq.cq_type, cq.callsign, cq.locator, cq.slot));
                emit(json!({"type": "cq", "call": cq.callsign, "source": cq.callsign, "bandwidth": self.config.bandwidth,
                    "beacon": false, "locator": cq.locator, "cq_type": cq.cq_type, "slot": cq.slot, "time": now_hm()}));
            }
            SessionEvent::AdvancedBeacon(b) => {
                let rec = CqFrameRecord {
                    guid: uuid::Uuid::new_v4().to_string(),
                    cqframe_time: Utc::now(),
                    cqframe_type_id: 2,
                    frequency: self.frequency as i64,
                    bandwidth: self.config.bandwidth.clone(),
                    from_callsign: b.callsign.clone(),
                    snr: None,
                    slot: None,
                    locator: Some(b.locator.clone()).filter(|l| !l.is_empty()),
                    band: Some(band_of(self.frequency).to_string()),
                };
                self.with_db(|d| {
                    d.insert_cqframe(&rec).ok();
                    d.upsert_heard_station(&b.callsign).ok();
                });
                if b.email_gateway() {
                    log(format!("Beacon: {} is an email gateway", b.callsign));
                }
                emit(json!({"type": "cq", "call": b.callsign, "source": format!("{}-9", b.callsign), "bandwidth": self.config.bandwidth, "beacon": true, "locator": b.locator, "email_gateway": b.email_gateway(), "time": now_hm()}));
            }
            SessionEvent::Message(m) => {
                self.conduct.last_activity = Some(std::time::Instant::now());
                self.message(m).await
            }
            SessionEvent::Handshake(step) => log(format!("Handshake: {step}")),
            SessionEvent::Notice(m) => system(m),
            SessionEvent::FrameId { .. } => {}
            SessionEvent::TxStarted { .. } => {}
            SessionEvent::RxProgress { received, total } => {
                emit(json!({"type": "file_progress", "outgoing": false, "done": received, "total": total}));
            }
            SessionEvent::HandshakeComplete => {
                self.conduct.link_open = true;
                self.offer_mail().await;
            }
            SessionEvent::QsyBy(hz) => self.moved_by(hz as i64).await,
            // VarAC's "QSY NOW" countdown, as the desktop shows it: the operator says QSY or
            // REJECT; with no answer in 30 s it's rejected (no auto-QSY on the phones)
            SessionEvent::QsyInvitation { target, inviter } => {
                use crate::varac::session::QsyTarget;
                let step = slots::DEFAULT_SLOT_STEP_HZ;
                let dial = self.frequency as i64;
                let target_hz = match target {
                    QsyTarget::Offset(hz) => dial + hz as i64,
                    QsyTarget::Up => dial + step,
                    QsyTarget::Down => dial - step,
                    QsyTarget::Absolute(f) => f as i64,
                }
                .max(0) as u64;
                let offset = (target_hz as i64 - dial) as i32;
                let who = if inviter { "YOUR" } else { "A" };
                system(format!("<<{who} QSY TO {:.3} MHz INVITATION>>", target_hz as f64 / 1e6));
                const SECS: u64 = 30;
                self.qsy_popup = Some((std::time::Instant::now() + std::time::Duration::from_secs(SECS), offset));
                emit(json!({"type": "qsy_invitation", "target_hz": target_hz, "offset_hz": offset,
                    "inviter": inviter, "seconds": SECS, "default_accept": false}));
            }
            SessionEvent::QsyResolved => {
                if self.qsy_popup.take().is_some() {
                    emit(json!({"type": "qsy_invitation_closed"}));
                }
            }
            SessionEvent::QsyInvitationRequested => {
                // We called; the partner allows auto-QSY: the operator picks the slot
                system("Your QSO partner allows auto-QSY. Send a QSY invitation to clear the calling frequency.");
                emit(json!({"type": "qsy_invite_request", "cf_hz": self.frequency, "on_cf": on_calling_freq(self.frequency)}));
            }
            SessionEvent::VaraEvent(crate::vara::events::VaraEvent::SignalReport(db)) => {
                self.last_snr = Some(db);
                emit(json!({"type": "snr", "db": db}));
            }
            SessionEvent::VaraEvent(e) => log(format!("{e:?}")),
        }
    }

    async fn message(&mut self, msg: IncomingMessage) {
        let partner = self.partner.clone().unwrap_or_else(|| "?".into());
        match msg {
            IncomingMessage::Opening(mut r) => {
                let c = &mut self.conduct;
                r.name = r.name.or(c.name.take());
                r.qth = r.qth.or(c.qth.take());
                r.locator = r.locator.or(c.locator.take());
                (c.name, c.qth, c.locator) = (r.name.clone(), r.qth.clone(), r.locator.clone());
                if let Some(w) = &r.welcome {
                    let text = w.replace("//n", "\n");
                    self.store_chat(1, &partner, &text);
                    emit(json!({"type": "chat", "from": partner, "text": text, "outgoing": false, "time": now_hm()}));
                }
                if self.qso_guid.is_some() {
                    // (the desktop fills its QSO log from the opening; RPT_RCVD = how they hear us)
                    if let Some((q, x)) = self.qso_row.as_mut().filter(|(_, x)| !x.is_ping) {
                        (q.name, q.qth) = (r.name.clone(), r.qth.clone());
                        q.snr_received = Some(r.signal_report_db as i32);
                        let loc = r.locator.as_deref().unwrap_or("").trim().to_uppercase();
                        x.locator = if crate::varac::messages::is_grid_locator(&loc) { loc } else { String::new() };
                    }
                    self.with_db(|d| d.auto_save_contact_from_qso(&r.callsign, r.name.as_deref(), r.qth.as_deref(), r.locator.as_deref()).ok());
                }
                emit(json!({
                    "type": "opening", "call": r.callsign, "snr": r.signal_report_db,
                    "name": r.name, "qth": r.qth, "locator": r.locator,
                }));
            }
            IncomingMessage::Chat(c) => {
                let text = c.text.replace("//n", "\n");
                let from = if c.from.is_empty() { partner } else { c.from.clone() };
                self.store_chat(1, &from, &text);
                emit(json!({"type": "chat", "from": from, "text": text, "outgoing": false, "time": now_hm()}));
            }
            IncomingMessage::RawText(t) => {
                let text = t.replace("//n", "\n");
                self.store_chat(1, &partner, &text);
                emit(json!({"type": "chat", "from": partner, "text": text, "outgoing": false, "time": now_hm()}));
            }
            IncomingMessage::IsTyping => emit(json!({"type": "typing"})),
            IncomingMessage::VMail(vm) => {
                let folder = if vm.is_parked { VMailFolder::Parking } else { VMailFolder::Inbox };
                let rec = VMailRecord {
                    guid: uuid::Uuid::new_v4().to_string(),
                    creation_time: vm.timestamp,
                    sent_time: None,
                    received_time: Some(Utc::now()),
                    folder_id: folder as i32,
                    vmail_to: vm.to.clone(),
                    vmail_from: vm.from.clone(),
                    vmail_via: None,
                    subject: vm.subject.clone(),
                    msg: vm.body.clone(),
                    read_status: false,
                    is_deleted: false,
                    urgent: vm.urgent,
                    delivery_band: None,
                    delivery_snr: None,
                    frequency: Some(self.frequency as i64),
                };
                self.with_db(|d| d.insert_vmail(&rec).ok());
                emit(json!({"type": "vmail", "from": vm.from, "subject": vm.subject, "parked": vm.is_parked}));
                emit(json!({"type": "vmail_changed"}));
            }
            IncomingMessage::VMailAck => match self.conduct.awaiting_ack.pop_front() {
                Some((guid, to)) => {
                    self.with_db(|d| d.move_vmail_to_sent(&guid).ok());
                    system(format!("VMail to {to} delivered"));
                    emit(json!({"type": "vmail_changed"}));
                }
                None => system("VMail delivery confirmed"),
            },
            // Refused / failed on the partner's side: it stays in the Outbox (VarAC's words)
            IncomingMessage::VMailRejected => {
                self.conduct.awaiting_ack.pop_front();
                system(format!("VMAIL REJECTED AS {partner} DOES NOT ACCEPT INCOMING VMAILS."));
            }
            IncomingMessage::VMailFailed => {
                self.conduct.awaiting_ack.pop_front();
                system("VMAIL DELIVERY FAILED ON THE RECEIVING SIDE. STOPPING VMAIL FORWARDING.");
            }
            IncomingMessage::Disconnect => system("Partner requested disconnect"),
            IncomingMessage::Away(r) => {
                system(format!("Station AWAY: {}", r.as_deref().unwrap_or("(no message)")));
                // An AWAY partner gets its VMail on the calling frequency (VarAC)
                self.conduct.partner_away = true;
                if self.conduct.link_open && !self.conduct.mail_delivered {
                    self.offer_mail().await;
                }
            }
            // VarAC's last-heard peeks, answered from our heard list
            IncomingMessage::LastHeardRequest => {
                let reply = if !self.config.allow_last_heard {
                    "<LHJ>".to_string()
                } else {
                    let band = (!self.config.multi_band_last_heard).then(|| band_of(self.frequency)).filter(|b| !b.is_empty());
                    self.with_db(|d| d.last_heard(Utc::now() - chrono::Duration::hours(24), band, 10).ok())
                        .and_then(|e| crate::varac::messages::last_heard_reply(&e))
                        .unwrap_or_else(|| "<LHE>".into())
                };
                self.answer(&reply).await;
            }
            IncomingMessage::LastHeardCallsignRequest(call) => {
                let reply = if !self.config.allow_last_heard {
                    "<LHJ>".to_string()
                } else {
                    self.with_db(|d| d.last_heard_callsign(&call).ok().flatten())
                        .map(|e| crate::varac::messages::last_heard_callsign_reply(&e))
                        .unwrap_or_else(|| "<LHCE>".into())
                };
                self.answer(&reply).await;
            }
            // VarAC <VRP>: the partner's VMail parked here
            IncomingMessage::RelayPeek => {
                let entries: Vec<_> = self
                    .with_db(|d| d.parked_vmails_from(&partner).ok())
                    .unwrap_or_default()
                    .into_iter()
                    .map(|v| (v.vmail_to, v.creation_time, v.sent_time))
                    .collect();
                self.answer(&crate::varac::messages::relay_peek_reply(&entries, Utc::now())).await;
            }
            // The partner's answers to our inquiries, worded as VarAC shows them
            IncomingMessage::LastHeardReply(data) => system(format!("{partner} last heard:\n{data}")),
            IncomingMessage::LastHeardEmpty => system(format!("{partner} LAST HEARD LIST IS EMPTY (OR DID NOT HEAR THIS STATION)")),
            IncomingMessage::LastHeardRejected => system(format!("{partner} REJECTED YOUR LAST HEARD PEEKING REQUEST")),
            IncomingMessage::RelayPeekRejected => system(format!("{partner} won't show its parked VMail")),
            IncomingMessage::FreqScheduleResponse(_) => system(format!("{partner} FREQUENCY SCHEDULER IS OFF")),
            IncomingMessage::LastConnectionsRejected => system(format!("{partner} REJECTED YOUR LAST HEARD CONNECTIONS REQUEST")),
            IncomingMessage::InfoDenied => system(format!("{partner} REJECTED YOUR INFO PEEKING REQUEST")),
            IncomingMessage::SnrReport(db) => {
                system(format!("They hear you at {db:+} dB"));
                emit(json!({"type": "snr_report", "db": db}));
            }
            IncomingMessage::StationInfo { name, qth, locator, email } => {
                let parts: Vec<String> = [
                    name.as_ref().map(|n| format!("Name: {n}")),
                    qth.as_ref().map(|q| format!("QTH: {q}")),
                    locator.as_ref().map(|l| format!("Locator: {l}")),
                    email.as_ref().map(|e| format!("Email: {e}")),
                ]
                .into_iter()
                .flatten()
                .collect();
                system(format!("Station info: {}", parts.join(", ")));
            }
            IncomingMessage::RelayNotification { relay, .. } => {
                system(format!("VMAIL WAITING FOR YOU AT {relay}: connect to {relay} to collect it"));
            }
            IncomingMessage::Broadcast(bc) => {
                self.with_db(|d| {
                    d.insert_broadcast(&crate::storage::BroadcastRecord {
                        guid: uuid::Uuid::new_v4().to_string(),
                        broadcast_time: bc.timestamp,
                        frequency: self.frequency as i64,
                        from_callsign: bc.from.clone(),
                        to_callsign: bc.to.clone(),
                        via_callsign: bc.via.clone(),
                        broadcast_message: bc.message.clone(),
                        snr: bc.snr.map(|s| s as i32),
                        band: Some(band_of(self.frequency).to_string()),
                    })
                    .ok()
                });
                emit(json!({"type": "broadcast", "from": bc.from, "to": bc.to, "message": bc.message, "time": bc.timestamp.format("%H:%M").to_string()}));
            }
            // The session answered as VarAC does (small: <SFRD>; big while away: <SFAB>);
            // otherwise the operator decides
            IncomingMessage::BbsFileOffer(offer) => {
                use crate::varac::session::FileOfferDecision;
                let name = bbs::strip_gzip_suffix(&offer.name).to_string();
                system(format!("File offered: {name} ({} bytes)", offer.compressed_size));
                self.pending_attachment = false;
                self.offer_asked = false;
                match self.station.policy.file_offer_decision(offer.compressed_size as u64, self.away) {
                    FileOfferDecision::Accept => {
                        system(format!("File {name} accepted automatically"));
                        emit(json!({"type": "file_incoming", "name": name}));
                        self.pending_offer = Some(offer);
                    }
                    FileOfferDecision::Refuse => {
                        system(format!("File {name} refused: bigger than the incoming files size limit while away"));
                        self.pending_offer = None;
                    }
                    FileOfferDecision::Ask => {
                        emit(json!({"type": "file_offer", "name": name, "size": offer.compressed_size}));
                        self.offer_asked = true;
                        self.pending_offer = Some(offer);
                    }
                }
            }
            IncomingMessage::VMailAttachmentOffer(offer) => {
                use crate::varac::session::FileOfferDecision;
                self.offer_asked = false;
                match self.station.policy.file_offer_decision(offer.compressed_size as u64, self.away) {
                    FileOfferDecision::Accept => {
                        system("<< RECEIVING VMAIL WITH ATTACHMENT >>");
                        self.pending_attachment = true;
                        self.pending_offer = Some(offer);
                    }
                    FileOfferDecision::Refuse => {
                        system(format!("VMail with a {} bytes attachment refused (over the size limit while away)", offer.compressed_size));
                        self.pending_attachment = false;
                        self.pending_offer = None;
                    }
                    // Rejecting it rejects the whole VMail
                    FileOfferDecision::Ask => {
                        emit(json!({"type": "file_offer", "name": "a VMail attachment", "size": offer.compressed_size, "attachment": true}));
                        self.offer_asked = true;
                        self.pending_attachment = true;
                        self.pending_offer = Some(offer);
                    }
                }
            }
            IncomingMessage::BbsFilePayload(data) => {
                let Some(offer) = self.pending_offer.take() else { return };
                let attachment = std::mem::take(&mut self.pending_attachment);
                let name = safe_file_name(bbs::strip_gzip_suffix(&offer.name));
                match bbs::receive_file(String::from_utf8_lossy(&data).trim(), &offer.md5_hex) {
                    Ok(bytes) => {
                        let path = unique_path(self.received_dir().join(&name));
                        match std::fs::write(&path, &bytes) {
                            Ok(()) => {
                                let what = if attachment { "VMail attachment" } else { "File" };
                                system(format!("{what} saved: {} ({} bytes)", path.file_name().unwrap_or_default().to_string_lossy(), bytes.len()));
                                emit(json!({"type": "file_saved", "path": path.to_string_lossy()}));
                            }
                            Err(e) => system(format!("File save error: {e}")),
                        }
                        if !attachment {
                            if let Some(sess) = self.session.as_mut() {
                                let _ = sess.send_file_ok().await;
                            }
                        }
                    }
                    Err(e) => {
                        system(format!("File transfer error: {e}"));
                        emit(json!({"type": "file_receive_failed"}));
                        if !attachment {
                            if let Some(sess) = self.session.as_mut() {
                                let _ = sess.send_file_abort().await;
                            }
                        }
                    }
                }
            }
            // The partner said send it: the data goes out now (the session sent it)
            IncomingMessage::BbsFileReady => {
                if let Some(f) = self.sending.as_mut() {
                    system(format!("{partner} is ready: sending {}", f.name));
                    f.started = true;
                    emit(json!({"type": "file_send", "state": "sending", "name": f.name, "total": f.frame_bytes}));
                }
            }
            IncomingMessage::BbsFileOk => {
                if let Some(f) = self.sending.take() {
                    system(format!("File transfer completed: {} received it", partner));
                    emit(json!({"type": "file_send", "state": "done", "name": f.name}));
                } else {
                    system("File transfer completed");
                }
            }
            // Ours, when we're sending: the partner declined or dropped it
            IncomingMessage::BbsFileAborted | IncomingMessage::BbsFileRejected if self.sending.is_some() && self.pending_offer.is_none() => {
                let started = self.sending.as_ref().is_some_and(|f| f.started);
                let why = if started { format!("{partner} stopped the transfer") } else { format!("{partner} declined the file") };
                self.file_send_ended(if started { "failed" } else { "refused" }, &why);
            }
            IncomingMessage::BbsFileFailed if self.sending.is_some() => {
                self.file_send_ended("failed", "FILE TRANSFER FAILED AT DESTINATION! Your partner was given the reason for the failure.");
            }
            IncomingMessage::BbsFileAborted => {
                self.pending_offer = None;
                if std::mem::take(&mut self.offer_asked) {
                    emit(json!({"type": "file_offer_closed"}));
                }
                system("File transfer aborted");
            }
            IncomingMessage::QsyRequest { offset_hz } => {
                system(format!("{partner} asks to QSY by {offset_hz:+} Hz"));
                emit(json!({"type": "qsy_request", "offset_hz": offset_hz}));
            }
            IncomingMessage::QsyAbsolute { freq_hz } => {
                system(format!("{partner} asks to QSY to {} MHz", mhz(freq_hz)));
                emit(json!({"type": "qsy_request", "freq_hz": freq_hz}));
            }
            // Verbose SNR (VarAC <VSI>): the session already answered when we're away
            // (reject) or auto-accept is on; otherwise the operator answers
            IncomingMessage::ExtendedSnrRequest => {
                if self.away {
                    system(format!("{partner} invited you to verbose SNR: rejected (away)"));
                } else if self.station.policy.verbose_snr_auto_accept.load(std::sync::atomic::Ordering::Relaxed) {
                    system(format!("{partner} invited you to verbose SNR: accepted, SNR reports every minute"));
                    emit(json!({"type": "verbose_snr", "on": true}));
                } else {
                    emit(json!({"type": "verbose_snr_invite", "call": partner}));
                }
            }
            IncomingMessage::ExtendedSnrAccepted => system(format!("{partner} accepted your verbose SNR invitation")),
            IncomingMessage::ExtendedSnrRejected => {
                system(format!("{partner} rejected your verbose SNR invitation"));
                emit(json!({"type": "verbose_snr", "on": false}));
            }
            IncomingMessage::ExtendedSnrStop => {
                system(format!("{partner} stopped verbose SNR"));
                emit(json!({"type": "verbose_snr", "on": false}));
            }
            // As the desktop GUI: the partner accepted our QSY request
            IncomingMessage::QsyAccepted => system("QSY accepted by partner"),
            IncomingMessage::QsyRejected => system("QSY rejected"),
            IncomingMessage::QsyOutOfRange => system("QSY out of the partner's range"),
            IncomingMessage::VMailWaiting => {
                system(format!("{partner} HAS VMAILS TO RELAY TO YOU. INVITE {partner} TO QSY TO RECEIVE THE VMAILS"))
            }
            // The partner's VarAC closes the link (VarAC's words)
            IncomingMessage::IdleTimeExceeded => system(format!("{partner}: IDLE TIME LIMIT EXCEEDED. DISCONNECTING...")),
            IncomingMessage::AwayTimeExceeded => {
                system(format!("{partner}: CONNECTION ON CALLING FREQUENCY IN AWAY STATUS TIME LIMIT EXCEEDED. DISCONNECTING..."))
            }
            other => log(format!("{other:?}")),
        }
    }
}

/// MHz with the digits a frequency needs: 14.105, 14.1065, 14.10275 (slots are 750 Hz apart).
fn mhz(hz: u64) -> String {
    let mut s = format!("{:.5}", hz as f64 / 1e6);
    while s.ends_with('0') && s.len() - s.find('.').unwrap_or(0) > 4 {
        s.pop();
    }
    s
}

/// "slot 12 (+1500 Hz)" for a CQ that names a slot, else None.
fn slot_label(slot: u8) -> Option<String> {
    (slot != 0).then(|| slots::label(slot, slots::DEFAULT_SLOT_STEP_HZ))
}

/// A file name from another station, reduced to a plain name (no directories).
fn safe_file_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let clean: String = base.chars().filter(|c| !c.is_control()).collect();
    let clean = clean.trim().trim_start_matches('.').to_string();
    if clean.is_empty() { "received.bin".into() } else { clean }
}

/// `path`, or `name (2).ext` etc. if it exists.
fn unique_path(path: PathBuf) -> PathBuf {
    if !path.exists() {
        return path;
    }
    let stem = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
    let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    let dir = path.parent().map(PathBuf::from).unwrap_or_default();
    (2..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| !p.exists())
        .unwrap_or(path)
}

/// The frequency picker's reads, the desktop's own functions; they need no engine, so the
/// picker works offline too. None for any other query.
///   parse_frequency {text}: the frequency typed, in Hz (null if it isn't one)
///   calling_frequencies: VarAC's calling frequencies, [{hz, band}]
///   band {hz}: {band, calling} (band null outside the ham bands)
pub fn frequency_query(q: &Value) -> Option<Value> {
    use crate::varac::messages;
    Some(match q["q"].as_str()? {
        "parse_frequency" => json!(messages::parse_frequency_input(q["text"].as_str().unwrap_or(""))),
        "calling_frequencies" => Value::Array(
            messages::CALLING_FREQUENCIES_HZ.iter().map(|&(hz, band)| json!({"hz": hz, "band": band})).collect(),
        ),
        "band" => {
            let hz = q["hz"].as_u64().unwrap_or(0);
            json!({"band": messages::band_name(hz), "calling": messages::is_calling_frequency(hz)})
        }
        _ => return None,
    })
}

/// Reads for the app's lists (`{"q": "...", ...}`), answered as JSON.
pub fn query(db: &Storage, q: &Value) -> Value {
    let folder = |s: &str| match s {
        "sent" => VMailFolder::Sent,
        "outbox" => VMailFolder::Outbox,
        "parking" => VMailFolder::Parking,
        _ => VMailFolder::Inbox,
    };
    let vmail_json = |v: &VMailRecord| {
        json!({
            "guid": v.guid, "from": v.vmail_from, "to": v.vmail_to, "subject": v.subject,
            "body": v.msg, "read": v.read_status, "urgent": v.urgent,
            "time": v.received_time.unwrap_or(v.creation_time).format("%Y-%m-%d %H:%M").to_string(),
        })
    };
    let limit = q["limit"].as_u64().unwrap_or(200) as usize;
    match q["q"].as_str().unwrap_or("") {
        "vmails" => {
            let list = db.vmails_in_folder(folder(q["folder"].as_str().unwrap_or("inbox")), limit).unwrap_or_default();
            Value::Array(list.iter().map(vmail_json).collect())
        }
        "vmail_read" => json!(db.mark_vmail_read(q["guid"].as_str().unwrap_or("")).is_ok()),
        "vmail_delete" => json!(db.mark_vmail_deleted(q["guid"].as_str().unwrap_or("")).is_ok()),
        "unread" => json!(db.unread_vmail_count().unwrap_or(0)),
        "qsos" => Value::Array(
            db.recent_qsos(limit)
                .unwrap_or_default()
                .iter()
                .map(|r| {
                    json!({
                        "guid": r.guid, "call": r.callsign, "start": r.starttime.format("%Y-%m-%d %H:%M").to_string(),
                        "band": r.band, "bandwidth": r.bandwidth, "name": r.name, "qth": r.qth,
                        "snr": r.snr_received, "frequency": r.frequency,
                    })
                })
                .collect(),
        ),
        "heard" => Value::Array(
            db.recent_cqframes(limit)
                .unwrap_or_default()
                .iter()
                .map(|c| {
                    let slot = c.slot.and_then(|s| u8::try_from(s).ok()).unwrap_or(0);
                    let slot_hz = (slot != 0)
                        .then(|| slots::frequency(c.frequency as u64, slot, slots::DEFAULT_SLOT_STEP_HZ))
                        .flatten();
                    json!({
                        "call": c.from_callsign, "beacon": c.cqframe_type_id == 2, "bandwidth": c.bandwidth,
                        "time": c.cqframe_time.format("%H:%M").to_string(), "band": c.band,
                        "slot": slot, "slot_label": slot_label(slot), "slot_hz": slot_hz,
                    })
                })
                .collect(),
        ),
        "chat_history" => Value::Array(
            db.chat_history_with_callsign(q["call"].as_str().unwrap_or(""), limit)
                .unwrap_or_default()
                .iter()
                .map(|c| {
                    json!({
                        "from": c.callsign, "text": c.entry, "outgoing": c.entry_type == 2,
                        "time": c.creation_time.format("%m-%d %H:%M").to_string(),
                    })
                })
                .collect(),
        ),
        "adif" => json!(db.export_adif().unwrap_or_default()),
        _ => Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::mhz;

    #[test]
    fn mhz_keeps_the_digits_a_slot_needs() {
        assert_eq!(mhz(14_105_000), "14.105");
        assert_eq!(mhz(14_106_500), "14.1065");
        assert_eq!(mhz(14_102_750), "14.10275");
        assert_eq!(mhz(7_100_000), "7.100");
    }
}
