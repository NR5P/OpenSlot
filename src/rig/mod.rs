use std::fmt;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

pub mod rigctld;
pub mod cat;
pub mod flrig;
pub mod rotator;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RadioMode {
    Usb,
    Lsb,
    UsbData,
    LsbData,
    Cw,
    Am,
    Fm,
    /// FM data (VarAC's "FM-D")
    FmData,
}

impl fmt::Display for RadioMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RadioMode::Usb => write!(f, "USB"),
            RadioMode::Lsb => write!(f, "LSB"),
            RadioMode::UsbData => write!(f, "USB-D"),
            RadioMode::LsbData => write!(f, "LSB-D"),
            RadioMode::Cw => write!(f, "CW"),
            RadioMode::Am => write!(f, "AM"),
            RadioMode::Fm => write!(f, "FM"),
            RadioMode::FmData => write!(f, "FM-D"),
        }
    }
}

#[derive(Debug)]
pub enum RigError {
    NotConnected,
    Io(io::Error),
    Protocol(String),
    Timeout,
}

impl RigError {
    /// Errors that mean the link to the radio is gone (vs. a rejected command).
    pub fn is_link_failure(&self) -> bool {
        matches!(self, RigError::NotConnected | RigError::Io(_) | RigError::Timeout)
    }
}

impl fmt::Display for RigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RigError::NotConnected => write!(f, "not connected to radio"),
            RigError::Io(e) => write!(f, "I/O error: {e}"),
            RigError::Protocol(msg) => write!(f, "protocol error: {msg}"),
            RigError::Timeout => write!(f, "timeout"),
        }
    }
}

impl From<io::Error> for RigError {
    fn from(e: io::Error) -> Self {
        if matches!(e.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock) {
            RigError::Timeout
        } else {
            RigError::Io(e)
        }
    }
}

pub type RigResult<T> = Result<T, RigError>;

pub trait RigControl: Send {
    fn connect(&mut self) -> RigResult<()>;
    fn disconnect(&mut self);
    fn is_connected(&self) -> bool;
    fn get_freq(&mut self) -> RigResult<u64>;
    fn set_freq(&mut self, hz: u64) -> RigResult<()>;
    fn get_mode(&mut self) -> RigResult<RadioMode>;
    fn set_mode(&mut self, mode: RadioMode) -> RigResult<()>;
    fn set_ptt(&mut self, on: bool) -> RigResult<()>;
    /// Start the antenna tuner (as in VarAC); not every rig can.
    fn tune_atu(&mut self) -> RigResult<()> {
        Err(RigError::Protocol("this rig control can't start the tuner".into()))
    }
    fn name(&self) -> &str;
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub enum RigBackend {
    #[default]
    None,
    Rigctld { host: String, port: u16 },
    CatSerial { port: String, baud: u32, model: CatModel },
    /// FLRig's XML-RPC server (VarAC [RIG_FLRIG_CONFIG], default localhost:12345)
    Flrig { host: String, port: u16 },
}

impl fmt::Display for RigBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RigBackend::None => write!(f, "None"),
            RigBackend::Rigctld { host, port } => write!(f, "rigctld ({host}:{port})"),
            RigBackend::CatSerial { port, model, .. } => write!(f, "{model} on {port}"),
            RigBackend::Flrig { host, port } => write!(f, "FLRig ({host}:{port})"),
        }
    }
}

impl RigBackend {
    /// Start a worker for this backend and ask it to connect (it keeps retrying).
    pub fn into_manager(self) -> Option<RigManager> {
        if self == RigBackend::None {
            return None;
        }
        let mgr = RigManager::new(self);
        mgr.connect();
        Some(mgr)
    }

    /// Build a backend from the string-typed settings stored in the config file / settings form.
    pub fn from_settings(backend: &str, host: &str, port: &str, serial: &str, baud: &str, model: &str) -> Self {
        match backend {
            "rigctld" => RigBackend::Rigctld {
                host: if host.trim().is_empty() { "127.0.0.1".into() } else { host.trim().to_string() },
                port: port.trim().parse().unwrap_or(4532),
            },
            "cat" => RigBackend::CatSerial {
                port: serial.trim().to_string(),
                baud: baud.trim().parse().unwrap_or(38400),
                model: CatModel::from_key(model),
            },
            "flrig" => RigBackend::Flrig {
                host: if host.trim().is_empty() { "127.0.0.1".into() } else { host.trim().to_string() },
                port: port.trim().parse().unwrap_or(12345),
            },
            _ => RigBackend::None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CatModel {
    Ftdx3000,
    YaesuGeneric,
    KenwoodGeneric,
}

impl CatModel {
    pub fn from_key(key: &str) -> Self {
        match key {
            "yaesu" => CatModel::YaesuGeneric,
            "kenwood" | "qmx" => CatModel::KenwoodGeneric,
            _ => CatModel::Ftdx3000,
        }
    }
}

impl fmt::Display for CatModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CatModel::Ftdx3000 => write!(f, "FTDX3000"),
            CatModel::YaesuGeneric => write!(f, "Yaesu (generic)"),
            CatModel::KenwoodGeneric => write!(f, "Kenwood/QMX"),
        }
    }
}

pub fn create_rig(backend: &RigBackend) -> Option<Box<dyn RigControl>> {
    match backend {
        RigBackend::None => None,
        RigBackend::Rigctld { host, port } => {
            Some(Box::new(rigctld::RigctldClient::new(host, *port)))
        }
        RigBackend::CatSerial { port, baud, model } => {
            Some(Box::new(cat::CatSerial::new(port, *baud, *model)))
        }
        RigBackend::Flrig { host, port } => Some(Box::new(flrig::FlrigClient::new(host, *port))),
    }
}

/// A serial port for the settings dropdowns: its name and, where the system says, what it is.
#[derive(Debug, Clone, PartialEq)]
pub struct PortInfo {
    pub name: String,
    /// What the system calls the device ("Silicon Labs Dual CP2105 USB to UART Bridge:
    /// Enhanced COM Port"); empty when it doesn't say.
    pub description: String,
    /// On a dual-port radio interface: Some(true) for the CAT port, Some(false) for the
    /// keying (PTT/CW) port. None when the port isn't known to be either.
    pub cat_port: Option<bool>,
}

impl PortInfo {
    /// For the dropdowns: "COM4 — Silicon Labs … Enhanced COM Port (CAT)".
    pub fn label(&self) -> String {
        let role = match self.cat_port {
            Some(true) => " (CAT)",
            Some(false) => " (PTT/keying)",
            None => "",
        };
        if self.description.is_empty() {
            format!("{}{role}", self.name)
        } else {
            format!("{} — {}{role}", self.name, self.description)
        }
    }
}

/// Silicon Labs CP2105, the dual USB-serial bridge in Yaesu radios (FTDX3000, FT-991, FT-891,
/// FT-710, FTDX10/101): interface 0 is the Enhanced (CAT) port, 1 the Standard (keying) port.
const CP2105: (u16, u16) = (0x10c4, 0xea70);

/// Serial ports present on this machine, for the settings dropdowns.
pub fn list_serial_ports() -> Vec<PortInfo> {
    serialport::available_ports()
        .map(|ports| ports.into_iter().map(port_info).collect())
        .unwrap_or_default()
}

fn port_info(p: serialport::SerialPortInfo) -> PortInfo {
    let serialport::SerialPortType::UsbPort(usb) = p.port_type else {
        return PortInfo { name: p.port_name, description: String::new(), cat_port: None };
    };
    // Windows gives the device's friendly name, which ends in "(COM4)": the name is shown already.
    let product = usb.product.unwrap_or_default();
    let product = product.trim_end_matches(&format!("({})", p.port_name)).trim().to_string();
    let description = match usb.manufacturer {
        Some(m) if !m.is_empty() && !product.contains(&m) => format!("{m} {product}").trim().to_string(),
        _ => product,
    };
    let cat_port = if (usb.vid, usb.pid) == CP2105 {
        usb.interface.map(|i| i == 0)
    } else {
        None
    }
    .or_else(|| {
        let d = description.to_ascii_lowercase();
        if d.contains("enhanced com port") {
            Some(true)
        } else if d.contains("standard com port") {
            Some(false)
        } else {
            None
        }
    });
    PortInfo { name: p.port_name, description, cat_port }
}

/// What "Detect radio" found: the settings to use and the radio's name.
#[derive(Debug, Clone, PartialEq)]
pub struct DetectedRig {
    pub port: String,
    pub baud: u32,
    /// As in the settings: "ftdx3000", "yaesu" or "kenwood"
    pub model: &'static str,
    pub radio: String,
}

/// The CAT RATE settings radios offer, most common first.
const DETECT_BAUDS: [u32; 6] = [38400, 4800, 9600, 19200, 57600, 115200];

/// Looks for a radio on the serial ports by asking each one "ID;" at each baud rate (a read-only
/// query in both the Yaesu and Kenwood CAT sets). DTR is held low throughout. RTS is raised only on
/// ports known to be a radio's CAT port, where Yaesu's "CAT RTS" needs it; a port that may key the
/// transmitter on RTS (a CP2105 Standard port, a Digirig) is never touched with it, and a
/// CP2105 Standard port is skipped altogether. Takes a few seconds per port; run it off the UI thread.
pub fn detect_cat(ports: &[PortInfo]) -> Result<DetectedRig, String> {
    let candidates: Vec<&PortInfo> = ports.iter().filter(|p| p.cat_port != Some(false)).collect();
    if candidates.is_empty() {
        return Err("no serial ports found — is the radio's USB cable plugged in and the radio on?".into());
    }
    let mut busy = Vec::new();
    for p in &candidates {
        for baud in DETECT_BAUDS {
            match probe_id(&p.name, baud, p.cat_port == Some(true)) {
                Ok(Some(id)) => {
                    let (model, radio) = model_for_id(&id);
                    return Ok(DetectedRig { port: p.name.clone(), baud, model, radio });
                }
                Ok(None) => {}
                Err(e) => {
                    busy.push(format!("{} ({e})", p.name));
                    break;
                }
            }
        }
    }
    let mut msg = "no radio answered on ".to_string() + &candidates.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", ");
    if !busy.is_empty() {
        msg += &format!("; couldn't open {} — close other programs using it", busy.join(", "));
    }
    msg += ". Check the radio is on and its CAT RATE, or set the port and baud by hand and press TEST.";
    Err(msg)
}

/// One "ID;" at one baud rate: Ok(Some(id digits)) on a reply, Ok(None) on silence or noise.
fn probe_id(port: &str, baud: u32, raise_rts: bool) -> Result<Option<String>, String> {
    use std::io::{Read, Write};
    let mut sp = serialport::new(port, baud)
        .timeout(Duration::from_millis(50))
        .stop_bits(serialport::StopBits::Two)
        .data_bits(serialport::DataBits::Eight)
        .parity(serialport::Parity::None)
        .flow_control(serialport::FlowControl::None)
        .open()
        .map_err(|e| e.to_string())?;
    let _ = sp.write_data_terminal_ready(false);
    let _ = sp.write_request_to_send(raise_rts);
    std::thread::sleep(Duration::from_millis(100));
    let _ = sp.clear(serialport::ClearBuffer::Input);
    // A "?;" means the baud rate is right but the radio got junk with it (left from probing at
    // another rate): ask once more. (A leading ';' to clear it won't do: the FTDX3000 answers
    // "?;" to the empty command and drops the rest.)
    let mut found = None;
    for _ in 0..2 {
        sp.write_all(b"ID;").map_err(|e| e.to_string())?;
        let _ = sp.flush();
        let mut reply = Vec::new();
        let deadline = Instant::now() + Duration::from_millis(350);
        let mut buf = [0u8; 256];
        while Instant::now() < deadline && found.is_none() {
            match sp.read(&mut buf) {
                Ok(n) => reply.extend_from_slice(&buf[..n]),
                Err(e) if e.kind() == io::ErrorKind::TimedOut => {}
                Err(_) => break,
            }
            found = find_id(&reply);
        }
        if found.is_some() || !String::from_utf8_lossy(&reply).contains("?;") {
            break;
        }
    }
    let _ = sp.write_request_to_send(false);
    Ok(found)
}

/// The digits of an "ID0462;" (Yaesu) or "ID020;" (Kenwood) reply anywhere in what came back;
/// radios with auto information on send other replies around it.
fn find_id(reply: &[u8]) -> Option<String> {
    let s = String::from_utf8_lossy(reply);
    s.match_indices("ID").find_map(|(i, _)| {
        let rest = &s[i + 2..];
        let end = rest.find(';')?;
        let digits = &rest[..end];
        ((digits.len() == 3 || digits.len() == 4) && digits.bytes().all(|b| b.is_ascii_digit())).then(|| digits.to_string())
    })
}

/// The settings model and a name for a radio's ID reply (Yaesu IDs as in Hamlib's newcat).
fn model_for_id(id: &str) -> (&'static str, String) {
    let yaesu = match id {
        "0462" => Some(("ftdx3000", "Yaesu FTDX3000")),
        "0310" => Some(("ftdx3000", "Yaesu FT-950")),
        "0583" => Some(("ftdx3000", "Yaesu FTDX1200")),
        "0570" => Some(("yaesu", "Yaesu FT-991/991A")),
        "0650" => Some(("yaesu", "Yaesu FT-891")),
        "0800" => Some(("yaesu", "Yaesu FT-710")),
        "0761" => Some(("yaesu", "Yaesu FTDX10")),
        "0681" => Some(("yaesu", "Yaesu FTDX101D")),
        "0682" => Some(("yaesu", "Yaesu FTDX101MP")),
        _ => None,
    };
    match yaesu {
        Some((model, name)) => (model, name.to_string()),
        None if id.len() == 4 => ("yaesu", format!("Yaesu radio (ID {id})")),
        None => ("kenwood", format!("Kenwood-compatible radio (ID {id})")),
    }
}

#[allow(dead_code)]
pub enum RigCommand {
    Connect,
    Disconnect,
    SetFreq(u64),
    SetMode(RadioMode),
    SetPtt(bool),
    /// Move the VFO by this many Hz from wherever it is (QSY agreed on the air).
    OffsetFreq(i64),
    PollFreq,
    /// Start the antenna tuner
    TuneAtu,
    /// Unkey and stop the worker thread.
    Shutdown,
}

pub enum RigEvent {
    Connected(String),
    Disconnected,
    FreqUpdate(u64),
    ModeUpdate(RadioMode),
    PttUpdate(bool),
    Error(String),
}

/// Routes VARA's `PTT ON/OFF` straight from the async session task to the rig
/// worker thread, bypassing the GUI frame loop so keying latency stays low.
#[derive(Clone, Default)]
pub struct PttRouter {
    target: Arc<Mutex<Option<mpsc::Sender<RigCommand>>>>,
    enabled: Arc<AtomicBool>,
    /// Dial frequency as last shown in the GUI (0 = unknown), for background tasks.
    dial_hz: Arc<std::sync::atomic::AtomicU64>,
}

impl PttRouter {
    pub fn set_target(&self, tx: Option<mpsc::Sender<RigCommand>>) {
        if let Ok(mut t) = self.target.lock() {
            *t = tx;
        }
    }

    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::Relaxed);
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn set_dial(&self, hz: u64) {
        self.dial_hz.store(hz, Ordering::Relaxed);
    }

    pub fn dial(&self) -> Option<u64> {
        Some(self.dial_hz.load(Ordering::Relaxed)).filter(|&f| f > 0)
    }

    /// Retune by `hz` relative to the current VFO, from any thread (e.g. a QSY agreed
    /// in the protocol task while the GUI window is hidden).
    pub fn offset_freq(&self, hz: i64) {
        // Background timers read the dial before the GUI redraws (a covered window
        // doesn't), so move it here too: the QSY'd link isn't on the CF any more
        if let Some(f) = self.dial() {
            self.set_dial((f as i64 + hz).max(0) as u64);
        }
        if let Ok(t) = self.target.lock() {
            if let Some(tx) = t.as_ref() {
                let _ = tx.send(RigCommand::OffsetFreq(hz));
            }
        }
    }

    /// Key or unkey the radio. Unkey is always forwarded, even when disabled,
    /// so toggling the setting mid-transmission can never leave the rig keyed.
    pub fn key(&self, on: bool) {
        if on && !self.is_enabled() {
            return;
        }
        if let Ok(t) = self.target.lock() {
            if let Some(tx) = t.as_ref() {
                let _ = tx.send(RigCommand::SetPtt(on));
            }
        }
    }
}

/// Every rig worker (its commands, and whether it has finished), so an exit that skips the
/// GUI's own shutdown (macOS: Quit from the menu or the Dock) can still unkey the radio.
static WORKERS: Mutex<Vec<(mpsc::Sender<RigCommand>, Arc<AtomicBool>)>> = Mutex::new(Vec::new());

/// Unkeys every radio and stops its worker, waiting up to `limit` for them.
#[allow(dead_code)] // the Mac app's exit hook
pub fn shutdown_all(limit: Duration) {
    let workers = WORKERS.lock().map(|mut w| std::mem::take(&mut *w)).unwrap_or_default();
    for (tx, _) in &workers {
        let _ = tx.send(RigCommand::Shutdown);
    }
    let start = Instant::now();
    while start.elapsed() < limit && workers.iter().any(|(_, done)| !done.load(Ordering::SeqCst)) {
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[allow(dead_code)]
pub struct RigManager {
    cmd_tx: mpsc::Sender<RigCommand>,
    event_rx: mpsc::Receiver<RigEvent>,
    pub backend: RigBackend,
    connected: bool,
    last_freq: Option<u64>,
    last_mode: Option<RadioMode>,
}

#[allow(dead_code)]
impl RigManager {
    pub fn new(backend: RigBackend) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();

        let be = backend.clone();
        let done = Arc::new(AtomicBool::new(false));
        let finished = done.clone();
        std::thread::Builder::new()
            .name("rig-worker".into())
            .spawn(move || {
                rig_worker(be, cmd_rx, event_tx);
                finished.store(true, Ordering::SeqCst);
            })
            .expect("spawn rig worker");
        if let Ok(mut w) = WORKERS.lock() {
            w.retain(|(_, d)| !d.load(Ordering::SeqCst));
            w.push((cmd_tx.clone(), done));
        }

        Self {
            cmd_tx,
            event_rx,
            backend,
            connected: false,
            last_freq: None,
            last_mode: None,
        }
    }

    /// A sender into the worker, e.g. for a [`PttRouter`].
    pub fn command_sender(&self) -> mpsc::Sender<RigCommand> {
        self.cmd_tx.clone()
    }

    pub fn send(&self, cmd: RigCommand) {
        let _ = self.cmd_tx.send(cmd);
    }

    pub fn connect(&self) {
        self.send(RigCommand::Connect);
    }

    pub fn disconnect(&self) {
        self.send(RigCommand::Disconnect);
    }

    pub fn set_freq(&self, hz: u64) {
        self.send(RigCommand::SetFreq(hz));
    }

    pub fn set_mode(&self, mode: RadioMode) {
        self.send(RigCommand::SetMode(mode));
    }

    pub fn set_ptt(&self, on: bool) {
        self.send(RigCommand::SetPtt(on));
    }

    pub fn tune_atu(&self) {
        self.send(RigCommand::TuneAtu);
    }

    pub fn poll_freq(&self) {
        self.send(RigCommand::PollFreq);
    }

    pub fn poll_events(&mut self) -> Vec<RigEvent> {
        let mut events = Vec::new();
        while let Ok(ev) = self.event_rx.try_recv() {
            match &ev {
                RigEvent::Connected(_) => self.connected = true,
                RigEvent::Disconnected => self.connected = false,
                RigEvent::FreqUpdate(f) => self.last_freq = Some(*f),
                RigEvent::ModeUpdate(m) => self.last_mode = Some(*m),
                RigEvent::PttUpdate(_) | RigEvent::Error(_) => {}
            }
            events.push(ev);
        }
        events
    }

    pub fn is_connected(&self) -> bool {
        self.connected
    }

    pub fn last_freq(&self) -> Option<u64> {
        self.last_freq
    }

    pub fn last_mode(&self) -> Option<RadioMode> {
        self.last_mode
    }
}

impl Drop for RigManager {
    fn drop(&mut self) {
        // Other senders (e.g. a PttRouter) may keep the worker alive, so stop it explicitly.
        let _ = self.cmd_tx.send(RigCommand::Shutdown);
    }
}

const POLL_INTERVAL: Duration = Duration::from_millis(1500);
const RECONNECT_INTERVAL: Duration = Duration::from_secs(5);
/// Consecutive link failures before we declare the radio gone.
const MAX_FAILURES: u32 = 3;
/// Safety net: never hold the transmitter keyed longer than this.
const PTT_WATCHDOG: Duration = Duration::from_secs(180);

struct Worker {
    rig: Box<dyn RigControl>,
    events: mpsc::Sender<RigEvent>,
    /// The user wants to be connected (auto-reconnect while true).
    want_connected: bool,
    next_reconnect: Instant,
    last_connect_error: Option<String>,
    failures: u32,
    last_poll: Instant,
    poll_suppress_until: Option<Instant>,
    ptt_since: Option<Instant>,
    /// Cleared if the radio/server rejects mode reads, so we stop asking.
    mode_readable: bool,
}

impl Worker {
    fn emit(&self, ev: RigEvent) {
        let _ = self.events.send(ev);
    }

    fn try_connect(&mut self) {
        self.next_reconnect = Instant::now() + RECONNECT_INTERVAL;
        match self.rig.connect() {
            Ok(()) => {
                self.failures = 0;
                self.last_connect_error = None;
                self.mode_readable = true;
                self.emit(RigEvent::Connected(self.rig.name().to_string()));
                self.poll(true);
            }
            Err(e) => {
                self.rig.disconnect();
                // Only report a failure once per distinct error to avoid log spam while retrying.
                let msg = format!("Connect failed: {e} (retrying every {}s)", RECONNECT_INTERVAL.as_secs());
                if self.last_connect_error.as_deref() != Some(msg.as_str()) {
                    self.emit(RigEvent::Error(msg.clone()));
                    self.last_connect_error = Some(msg);
                }
            }
        }
    }

    fn drop_link(&mut self, why: &str) {
        self.rig.disconnect();
        self.ptt_since = None;
        self.emit(RigEvent::Error(format!("Lost radio link: {why}")));
        self.emit(RigEvent::Disconnected);
        self.emit(RigEvent::PttUpdate(false));
        self.next_reconnect = Instant::now() + RECONNECT_INTERVAL;
    }

    /// Track a command result; returns the value if OK. Repeated link failures drop the connection.
    fn check<T>(&mut self, what: &str, r: RigResult<T>) -> Option<T> {
        match r {
            Ok(v) => {
                self.failures = 0;
                Some(v)
            }
            Err(e) => {
                if e.is_link_failure() {
                    self.failures += 1;
                    if self.failures >= MAX_FAILURES {
                        self.drop_link(&e.to_string());
                    }
                } else {
                    self.emit(RigEvent::Error(format!("{what}: {e}")));
                }
                None
            }
        }
    }

    fn poll(&mut self, with_mode: bool) {
        self.last_poll = Instant::now();
        let f = self.rig.get_freq();
        if let Some(f) = self.check("Get freq", f) {
            self.emit(RigEvent::FreqUpdate(f));
        }
        if with_mode && self.mode_readable && self.rig.is_connected() {
            match self.rig.get_mode() {
                Ok(m) => self.emit(RigEvent::ModeUpdate(m)),
                Err(e) if !e.is_link_failure() => {
                    self.mode_readable = false;
                    self.emit(RigEvent::Error(format!("Mode readback unsupported ({e}); continuing without it")));
                }
                Err(e) => {
                    self.check::<()>("Get mode", Err(e));
                }
            }
        }
    }

    fn set_ptt(&mut self, on: bool) {
        if !self.rig.is_connected() {
            return;
        }
        let r = self.rig.set_ptt(on);
        if self.check("PTT", r).is_some() {
            self.ptt_since = on.then(Instant::now);
            self.emit(RigEvent::PttUpdate(on));
        } else if !on {
            // Unkey failed — try once more, it matters.
            let _ = self.rig.set_ptt(false);
            self.ptt_since = None;
        }
    }

    fn handle(&mut self, cmd: RigCommand) -> bool {
        match cmd {
            RigCommand::Connect => {
                self.want_connected = true;
                if !self.rig.is_connected() {
                    self.last_connect_error = None;
                    self.try_connect();
                }
            }
            RigCommand::Disconnect => {
                self.want_connected = false;
                if self.ptt_since.is_some() {
                    let _ = self.rig.set_ptt(false);
                    self.ptt_since = None;
                }
                self.rig.disconnect();
                self.emit(RigEvent::Disconnected);
            }
            RigCommand::SetFreq(hz) if self.rig.is_connected() => {
                let r = self.rig.set_freq(hz);
                if self.check("Set freq", r).is_some() {
                    self.emit(RigEvent::FreqUpdate(hz));
                    self.poll_suppress_until = Some(Instant::now() + Duration::from_secs(2));
                }
            }
            RigCommand::SetMode(mode) if self.rig.is_connected() => {
                let r = self.rig.set_mode(mode);
                if self.check("Set mode", r).is_some() {
                    self.emit(RigEvent::ModeUpdate(mode));
                }
            }
            RigCommand::SetPtt(on) => self.set_ptt(on),
            RigCommand::TuneAtu if self.rig.is_connected() => {
                let r = self.rig.tune_atu();
                self.check("Antenna tuner", r);
            }
            RigCommand::OffsetFreq(hz) if self.rig.is_connected() => {
                let f = self.rig.get_freq();
                if let Some(f) = self.check("Get freq", f) {
                    let target = (f as i64 + hz).max(0) as u64;
                    let r = self.rig.set_freq(target);
                    if self.check("Set freq", r).is_some() {
                        self.emit(RigEvent::FreqUpdate(target));
                        self.poll_suppress_until = Some(Instant::now() + Duration::from_secs(2));
                    }
                }
            }
            RigCommand::PollFreq if self.rig.is_connected() && self.ptt_since.is_none() => {
                self.poll(false);
            }
            RigCommand::Shutdown => return false,
            _ => {}
        }
        true
    }

    fn tick(&mut self) {
        if !self.rig.is_connected() {
            if self.want_connected && Instant::now() >= self.next_reconnect {
                self.try_connect();
            }
            return;
        }

        if let Some(since) = self.ptt_since {
            if since.elapsed() > PTT_WATCHDOG {
                self.emit(RigEvent::Error(format!(
                    "PTT watchdog: transmitter keyed > {}s, unkeying",
                    PTT_WATCHDOG.as_secs()
                )));
                self.set_ptt(false);
            }
            // Don't poll while transmitting: keeps PTT-off latency low and some rigs dislike CAT traffic in TX.
            return;
        }

        let suppressed = self.poll_suppress_until.is_some_and(|t| Instant::now() < t);
        if !suppressed && self.last_poll.elapsed() >= POLL_INTERVAL {
            self.poll(true);
        }
    }

    fn shutdown(&mut self) {
        if self.ptt_since.is_some() || self.rig.is_connected() {
            let _ = self.rig.set_ptt(false);
        }
        self.rig.disconnect();
    }
}

fn rig_worker(
    backend: RigBackend,
    cmd_rx: mpsc::Receiver<RigCommand>,
    event_tx: mpsc::Sender<RigEvent>,
) {
    let Some(rig) = create_rig(&backend) else { return };
    let mut w = Worker {
        rig,
        events: event_tx,
        want_connected: false,
        next_reconnect: Instant::now(),
        last_connect_error: None,
        failures: 0,
        last_poll: Instant::now(),
        poll_suppress_until: None,
        ptt_since: None,
        mode_readable: true,
    };

    loop {
        match cmd_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(cmd) => {
                if !w.handle(cmd) {
                    break;
                }
                // Drain anything else queued (e.g. PTT behind a poll) before the next tick.
                let mut stop = false;
                while let Ok(cmd) = cmd_rx.try_recv() {
                    if !w.handle(cmd) {
                        stop = true;
                        break;
                    }
                }
                if stop {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        w.tick();
    }
    w.shutdown();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_from_settings() {
        assert_eq!(
            RigBackend::from_settings("rigctld", "localhost", "4532", "", "", ""),
            RigBackend::Rigctld { host: "localhost".into(), port: 4532 }
        );
        assert_eq!(
            RigBackend::from_settings("cat", "", "", "/dev/ttyUSB0", "38400", "qmx"),
            RigBackend::CatSerial { port: "/dev/ttyUSB0".into(), baud: 38400, model: CatModel::KenwoodGeneric }
        );
        assert_eq!(RigBackend::from_settings("none", "", "", "", "", ""), RigBackend::None);
    }

    /// The ID reply is found among auto-information output, and noise isn't taken for one.
    #[test]
    fn detect_reads_id_replies() {
        assert_eq!(find_id(b"RM0000;RM0000;ID0462;?;RM0000;"), Some("0462".into()));
        assert_eq!(find_id(b"ID020;"), Some("020".into()));
        assert_eq!(find_id(b"\x7b\xff\xff"), None);
        assert_eq!(find_id(b"ID04"), None);
        assert_eq!(find_id(b"IDX12;"), None);
        assert_eq!(model_for_id("0462"), ("ftdx3000", "Yaesu FTDX3000".into()));
        assert_eq!(model_for_id("0570").0, "yaesu");
        assert_eq!(model_for_id("9999").0, "yaesu");
        assert_eq!(model_for_id("020").0, "kenwood");
    }

    #[test]
    fn port_labels() {
        let p = PortInfo { name: "COM4".into(), description: "Silicon Labs Dual CP2105 USB to UART Bridge: Enhanced COM Port".into(), cat_port: Some(true) };
        assert_eq!(p.label(), "COM4 — Silicon Labs Dual CP2105 USB to UART Bridge: Enhanced COM Port (CAT)");
        let p = PortInfo { name: "/dev/ttyS0".into(), description: String::new(), cat_port: None };
        assert_eq!(p.label(), "/dev/ttyS0");
    }

    /// A port that may key on RTS is never probed (detect_cat filters it before opening).
    #[test]
    fn detect_skips_keying_ports() {
        let keying = PortInfo { name: "COM3".into(), description: String::new(), cat_port: Some(false) };
        assert!(detect_cat(&[keying]).unwrap_err().starts_with("no serial ports found"));
    }

    /// A QSY agreed on the air moves the VFO relative to where it is (rig worker,
    /// talking to a fake rigctld).
    #[test]
    fn offset_freq_moves_the_vfo() {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            let mut w = s.try_clone().unwrap();
            let mut freq = 14_105_000u64;
            for line in BufReader::new(s).lines() {
                let Ok(line) = line else { return };
                let reply = match line.trim() {
                    "f" => format!("{freq}\n"),
                    "m" => "PKTUSB\n500\n".to_string(),
                    l if l.starts_with("F ") => {
                        freq = l[2..].trim().parse().unwrap();
                        "RPRT 0\n".to_string()
                    }
                    _ => "RPRT 0\n".to_string(),
                };
                if w.write_all(reply.as_bytes()).is_err() {
                    return;
                }
            }
        });
        let mut mgr = RigBackend::Rigctld { host: "127.0.0.1".into(), port }.into_manager().unwrap();
        let router = PttRouter::default();
        router.set_target(Some(mgr.command_sender()));
        let deadline = Instant::now() + Duration::from_secs(5);
        while !mgr.is_connected() && Instant::now() < deadline {
            mgr.poll_events();
            std::thread::sleep(Duration::from_millis(50));
        }
        router.offset_freq(-1500);
        let mut got = None;
        while Instant::now() < deadline {
            for ev in mgr.poll_events() {
                if let RigEvent::FreqUpdate(f) = ev {
                    if f != 14_105_000 {
                        got = Some(f);
                    }
                }
            }
            if got.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(got, Some(14_103_500));
    }

    #[test]
    fn ptt_router_disabled_still_unkeys() {
        let r = PttRouter::default();
        let (tx, rx) = mpsc::channel();
        r.set_target(Some(tx));
        r.key(true);
        assert!(rx.try_recv().is_err(), "keying must be blocked while disabled");
        r.key(false);
        assert!(matches!(rx.try_recv(), Ok(RigCommand::SetPtt(false))));
        r.set_enabled(true);
        r.key(true);
        assert!(matches!(rx.try_recv(), Ok(RigCommand::SetPtt(true))));
    }
}
