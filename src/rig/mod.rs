use std::fmt;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

pub mod rigctld;
pub mod cat;
pub mod flrig;
pub mod rotator;
pub mod extras;
pub mod share;

pub use extras::{Agc, Item, Level, Meter, RigAction, RigExtras, Switch};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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

impl RadioMode {
    /// The modes the Rig controls window offers, in its order.
    pub const ALL: [RadioMode; 8] = [
        RadioMode::UsbData, RadioMode::Usb, RadioMode::LsbData, RadioMode::Lsb,
        RadioMode::Cw, RadioMode::Am, RadioMode::Fm, RadioMode::FmData,
    ];
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
    /// The radio's ALC meter, 0.0 (no ALC action) … 1.0 (full scale); not every rig can.
    fn get_alc(&mut self) -> RigResult<f32> {
        Err(RigError::Protocol("this rig control can't read the ALC meter".into()))
    }
    // The Rig controls window (see extras.rs). Each defaults to "not offered"; a backend
    // answers for what its radio can do, and the worker hides whatever errors.
    fn get_level(&mut self, _level: Level) -> RigResult<f32> {
        Err(unsupported())
    }
    fn set_level(&mut self, _level: Level, _value: f32) -> RigResult<()> {
        Err(unsupported())
    }
    fn get_switch(&mut self, _switch: Switch) -> RigResult<bool> {
        Err(unsupported())
    }
    fn set_switch(&mut self, _switch: Switch, _on: bool) -> RigResult<()> {
        Err(unsupported())
    }
    fn get_agc(&mut self) -> RigResult<Agc> {
        Err(unsupported())
    }
    fn set_agc(&mut self, _agc: Agc) -> RigResult<()> {
        Err(unsupported())
    }
    fn get_meter(&mut self, _meter: Meter) -> RigResult<f32> {
        Err(unsupported())
    }
    fn name(&self) -> &str;
}

/// Failed reads in a row before a control is taken as not offered after all.
const MAX_MISSES: u8 = 3;

fn unsupported() -> RigError {
    RigError::Protocol("not offered by this rig control".into())
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
    /// Icom CI-V (and radios that speak it: Xiegu and others), at this CI-V address;
    /// 0 = find the radio's address when connecting.
    Icom(u8),
}

impl CatModel {
    pub fn from_key(key: &str) -> Self {
        match key {
            "yaesu" => CatModel::YaesuGeneric,
            "kenwood" | "qmx" => CatModel::KenwoodGeneric,
            // "icom", or "icom:94" with the radio's CI-V address in hex
            k if k.starts_with("icom") => {
                CatModel::Icom(k.strip_prefix("icom:").and_then(|a| u8::from_str_radix(a, 16).ok()).unwrap_or(0))
            }
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
            CatModel::Icom(0) => write!(f, "Icom CI-V"),
            CatModel::Icom(a) => write!(f, "Icom CI-V ({a:02X}h)"),
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
    /// A USB device (a radio's own port, a USB-serial cable), not Bluetooth or a built-in port
    pub usb: bool,
}

impl PortInfo {
    /// For the dropdowns: "COM4 — Silicon Labs … Enhanced COM Port (CAT)".
    pub fn label(&self) -> String {
        let role = match self.cat_port {
            Some(true) => " (CAT)",
            Some(false) => " (PTT/keying)",
            None => "",
        };
        // A /dev/serial/by-id name is long and says what the description says: show the device it
        // points at now instead (only Linux has these).
        let shown = match self.name.strip_prefix("/dev/serial/by-id/") {
            Some(_) => std::fs::canonicalize(&self.name)
                .map_or_else(|_| self.name.clone(), |p| p.to_string_lossy().into_owned()),
            None => self.name.clone(),
        };
        if self.description.is_empty() {
            format!("{shown}{role}")
        } else {
            format!("{shown} — {}{role}", self.description)
        }
    }
}

/// Silicon Labs CP2105, the dual USB-serial bridge in Yaesu radios (FTDX3000, FT-991, FT-891,
/// FT-710, FTDX10/101): interface 0 is the Enhanced (CAT) port, 1 the Standard (keying) port.
const CP2105: (u16, u16) = (0x10c4, 0xea70);

/// Serial ports present on this machine, for the settings dropdowns. On Linux USB ports are
/// named by their /dev/serial/by-id link, which survives the radio being unplugged or power cycled.
pub fn list_serial_ports() -> Vec<PortInfo> {
    serialport::available_ports()
        .map(|ports| {
            ports
                .into_iter()
                .map(port_info)
                .map(|p| PortInfo { name: stable_port_name(&p.name), ..p })
                .collect()
        })
        .unwrap_or_default()
}

/// Linux numbers USB serial devices in the order they appear, so a radio that drops off the bus
/// can come back as /dev/ttyUSB2 instead of /dev/ttyUSB0. The /dev/serial/by-id link (USB serial
/// number + interface) stays the same: return it for a port that has one, else the name as given.
#[cfg(target_os = "linux")]
pub fn stable_port_name(name: &str) -> String {
    const BY_ID: &str = "/dev/serial/by-id";
    if name.starts_with(BY_ID) {
        return name.to_string();
    }
    let Ok(target) = std::fs::canonicalize(name) else { return name.to_string() };
    std::fs::read_dir(BY_ID)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|link| std::fs::canonicalize(link).is_ok_and(|t| t == target))
        .map_or_else(|| name.to_string(), |link| link.to_string_lossy().into_owned())
}

/// COM ports (Windows) and /dev/cu.* names (macOS) don't get renumbered this way.
#[cfg(not(target_os = "linux"))]
pub fn stable_port_name(name: &str) -> String {
    name.to_string()
}

/// The saved CAT port is gone (Linux, a bare /dev/ttyUSBn the radio no longer has): the one radio
/// CAT port that is plugged in, if there is exactly one. Only ports known to be a radio's CAT port
/// (a CP2105 interface 0, an "Enhanced COM Port") qualify, never a keying port or an unknown one,
/// since connecting raises RTS.
#[cfg(target_os = "linux")]
pub fn replacement_cat_port(saved: &str) -> Option<String> {
    if saved.is_empty() || std::path::Path::new(saved).exists() {
        return None;
    }
    let mut cat: Vec<String> =
        list_serial_ports().into_iter().filter(|p| p.cat_port == Some(true)).map(|p| p.name).collect();
    (cat.len() == 1).then(|| cat.remove(0))
}

#[cfg(not(target_os = "linux"))]
pub fn replacement_cat_port(_saved: &str) -> Option<String> {
    None
}

fn port_info(p: serialport::SerialPortInfo) -> PortInfo {
    let serialport::SerialPortType::UsbPort(usb) = p.port_type else {
        return PortInfo { name: p.port_name, description: String::new(), cat_port: None, usb: false };
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
    PortInfo { name: p.port_name, description, cat_port, usb: true }
}

/// What "Detect radio" found: the settings to use and the radio's name.
#[derive(Debug, Clone, PartialEq)]
pub struct DetectedRig {
    pub port: String,
    pub baud: u32,
    /// As in the settings: "ftdx3000", "yaesu", "kenwood" or "icom:94"
    pub model: String,
    pub radio: String,
}

/// The CAT RATE settings radios offer, most common first.
const DETECT_BAUDS: [u32; 6] = [38400, 4800, 9600, 19200, 57600, 115200];

/// Looks for a radio on the serial ports by asking each one "ID;" at each baud rate (a read-only
/// query in both the Yaesu and Kenwood CAT sets) and, on ports that aren't a Yaesu's CAT port, for
/// the frequency in Icom's CI-V. DTR is held low throughout. RTS is raised only on ports known to
/// be a radio's CAT port, where Yaesu's "CAT RTS" needs it; a port that may key the transmitter on
/// RTS (a CP2105 Standard port, a Digirig) is never touched with it, and a CP2105 Standard port is
/// skipped altogether. `usb_only` leaves out Bluetooth and built-in ports (looking at startup).
/// Takes a few seconds per port; run it off the UI thread.
pub fn detect_cat(ports: &[PortInfo], usb_only: bool) -> Result<DetectedRig, String> {
    let mut candidates: Vec<&PortInfo> = ports
        .iter()
        .filter(|p| p.cat_port != Some(false) && (p.usb || !usb_only))
        // (macOS lists each port twice: /dev/tty.* waits for carrier, /dev/cu.* is for calling out)
        .filter(|p| !p.name.starts_with("/dev/tty."))
        .collect();
    // A known CAT port first, then the other USB ones
    candidates.sort_by_key(|p| (p.cat_port != Some(true), !p.usb));
    if candidates.is_empty() {
        return Err("no serial ports found — is the radio's USB cable plugged in and the radio on?".into());
    }
    let mut busy = Vec::new();
    for p in &candidates {
        for baud in DETECT_BAUDS {
            match probe_id(&p.name, baud, p.cat_port == Some(true)) {
                Ok(Some(id)) => {
                    let (model, radio) = model_for_id(&id);
                    return Ok(DetectedRig { port: p.name.clone(), baud, model: model.to_string(), radio });
                }
                Ok(None) => {}
                Err(e) => {
                    busy.push(format!("{} ({e})", p.name));
                    break;
                }
            }
            if p.cat_port == Some(true) {
                continue;
            }
            if let Ok(Some(addr)) = probe_civ(&p.name, baud) {
                return Ok(DetectedRig { port: p.name.clone(), baud, model: format!("icom:{addr:02x}"), radio: icom_name(addr) });
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

/// The CI-V addresses radios come set to (Hamlib's defaults), for finding one that doesn't
/// answer the broadcast address.
const CIV_ADDRESSES: [u8; 17] =
    [0x94, 0xA4, 0x98, 0xA2, 0x88, 0x8E, 0x70, 0x7C, 0x76, 0x80, 0x7A, 0x5E, 0x66, 0x74, 0x6A, 0x6E, 0xAC];

/// Asks for the frequency in CI-V (read-only), at the broadcast address and each usual one, at one
/// baud rate: Ok(Some(the radio's address)) when one answers. RTS and DTR stay low (an Icom can
/// key on either, by its USB SEND / USB keying settings).
fn probe_civ(port: &str, baud: u32) -> Result<Option<u8>, String> {
    use std::io::{Read, Write};
    let mut sp = serialport::new(port, baud)
        .timeout(Duration::from_millis(50))
        .stop_bits(serialport::StopBits::One)
        .data_bits(serialport::DataBits::Eight)
        .parity(serialport::Parity::None)
        .flow_control(serialport::FlowControl::None)
        .open()
        .map_err(|e| e.to_string())?;
    let _ = sp.write_data_terminal_ready(false);
    let _ = sp.write_request_to_send(false);
    std::thread::sleep(Duration::from_millis(100));
    let _ = sp.clear(serialport::ClearBuffer::Input);
    let mut out = cat::civ_frame(0x00, &[0x03]);
    for a in CIV_ADDRESSES {
        out.extend(cat::civ_frame(a, &[0x03]));
    }
    sp.write_all(&out).map_err(|e| e.to_string())?;
    let _ = sp.flush();
    let mut reply = Vec::new();
    let deadline = Instant::now() + Duration::from_millis(400);
    let mut buf = [0u8; 256];
    while Instant::now() < deadline {
        match sp.read(&mut buf) {
            Ok(n) => reply.extend_from_slice(&buf[..n]),
            Err(e) if e.kind() == io::ErrorKind::TimedOut => {}
            Err(_) => break,
        }
        if let Some(a) = cat::civ_answer(&reply) {
            return Ok(Some(a));
        }
    }
    Ok(None)
}

/// A name for an Icom by its CI-V address (Hamlib's defaults; a radio can be set to another).
fn icom_name(addr: u8) -> String {
    let name = match addr {
        0x94 => "Icom IC-7300",
        0xA4 => "Icom IC-705",
        0x98 => "Icom IC-7610",
        0xA2 => "Icom IC-9700",
        0x88 => "Icom IC-7100",
        0x8E => "Icom IC-7851",
        0x70 => "Icom IC-7000",
        0x7C => "Icom IC-9100",
        0x76 => "Icom IC-7200",
        0x80 => "Icom IC-7410",
        0x7A => "Icom IC-7600",
        0x5E => "Icom IC-718",
        0x66 => "Icom IC-746PRO",
        0x74 => "Icom IC-7700",
        0x6A => "Icom IC-7800",
        0x6E => "Icom IC-756PROIII",
        0xAC => "Icom IC-905",
        _ => return format!("Icom-compatible radio (CI-V {addr:02X}h)"),
    };
    format!("{name} (CI-V {addr:02X}h)")
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
    /// Read the ALC meter once (answered with [`RigEvent::Alc`]), also while transmitting
    ReadAlc,
    /// Read the extra controls and meters (the Rig controls window is open, or a remote
    /// station is watching); off, nothing but the frequency and mode is asked for.
    WatchExtras(bool),
    /// The modem is transmitting (keyed by VOX or the modem's own PTT, not through us):
    /// read the transmit meters meanwhile.
    TxHint(bool),
    SetLevel(Level, f32),
    SetSwitch(Switch, bool),
    SetAgc(Agc),
    /// Unkey and stop the worker thread.
    Shutdown,
}

#[derive(Debug)]
pub enum RigEvent {
    Connected(String),
    Disconnected,
    FreqUpdate(u64),
    ModeUpdate(RadioMode),
    PttUpdate(bool),
    /// Whether this radio's ALC meter can be read (asked once per connect)
    AlcSupported(bool),
    /// An ALC reading, 0.0 … 1.0
    Alc(f32),
    /// The extra controls' values (while watched)
    Extras(RigExtras),
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
    /// WatchExtras and TxHint as last sent
    watching: bool,
    tx_hint: bool,
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
            watching: false,
            tx_hint: false,
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

    /// Read the extra controls (or stop); sent only when it changes.
    pub fn watch_extras(&mut self, on: bool) {
        if on != self.watching {
            self.watching = on;
            self.send(RigCommand::WatchExtras(on));
        }
    }

    /// The modem is transmitting (sent only when it changes).
    pub fn tx_hint(&mut self, on: bool) {
        if on != self.tx_hint {
            self.tx_hint = on;
            self.send(RigCommand::TxHint(on));
        }
    }

    pub fn apply(&self, action: RigAction) {
        self.send(match action {
            RigAction::Mode { mode } => RigCommand::SetMode(mode),
            RigAction::Level { level, value } => RigCommand::SetLevel(level, value),
            RigAction::Switch { switch, on } => RigCommand::SetSwitch(switch, on),
            RigAction::Agc { agc } => RigCommand::SetAgc(agc),
            RigAction::Tune => RigCommand::TuneAtu,
        });
    }

    pub fn poll_events(&mut self) -> Vec<RigEvent> {
        let mut events = Vec::new();
        while let Ok(ev) = self.event_rx.try_recv() {
            match &ev {
                RigEvent::Connected(_) => self.connected = true,
                RigEvent::Disconnected => self.connected = false,
                RigEvent::FreqUpdate(f) => self.last_freq = Some(*f),
                RigEvent::ModeUpdate(m) => self.last_mode = Some(*m),
                RigEvent::PttUpdate(_) | RigEvent::AlcSupported(_) | RigEvent::Alc(_) | RigEvent::Extras(_) | RigEvent::Error(_) => {}
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
/// While the extra controls are watched: one of them is read at most this often, between
/// frequency polls, so a PTT command never waits behind more than one read.
const EXTRA_INTERVAL: Duration = Duration::from_millis(150);
/// Transmitting, the power/SWR/ALC meters are read no more often than this: some rigs
/// dislike CAT traffic in TX, and unkeying must stay quick.
const TX_METER_INTERVAL: Duration = Duration::from_millis(600);

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
    /// The ALC meter answered at connect
    alc_readable: bool,
    watch_extras: bool,
    extras: RigExtras,
    /// What the radio answers to, once asked (None: not yet)
    offered: Option<Vec<Item>>,
    /// Still to ask about, and where the round of reads is
    to_probe: Vec<Item>,
    /// Reads of an offered control that failed in a row (a busy radio answers some with an
    /// error): only a few in a row mean it's really gone
    misses: std::collections::HashMap<Item, u8>,
    next_read: usize,
    last_extra: Instant,
    extras_sent: Option<RigExtras>,
    extras_sent_at: Instant,
    tx_hint: bool,
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
                // (another radio may answer now: ask about its controls again)
                self.offered = None;
                self.to_probe = Item::all();
                self.misses.clear();
                self.extras = RigExtras::default();
                self.extras_sent = None;
                self.emit(RigEvent::Connected(self.rig.name().to_string()));
                self.poll(true);
                // (a refusal or no answer here only means no ALC: it doesn't count against the link)
                self.alc_readable = self.rig.is_connected() && self.rig.get_alc().is_ok();
                self.emit(RigEvent::AlcSupported(self.alc_readable));
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
            RigCommand::ReadAlc if self.rig.is_connected() && self.alc_readable => match self.rig.get_alc() {
                Ok(v) => self.emit(RigEvent::Alc(v.clamp(0.0, 1.0))),
                Err(e) if !e.is_link_failure() => {
                    self.alc_readable = false;
                    self.emit(RigEvent::AlcSupported(false));
                    self.emit(RigEvent::Error(format!("ALC meter: {e}")));
                }
                Err(e) => {
                    self.check::<()>("ALC meter", Err(e));
                }
            },
            RigCommand::PollFreq if self.rig.is_connected() && self.ptt_since.is_none() => {
                self.poll(false);
            }
            RigCommand::WatchExtras(on) => {
                self.watch_extras = on;
                if on {
                    // (the last values at once, even before the next read)
                    self.extras_sent = None;
                    self.send_extras();
                }
            }
            RigCommand::TxHint(on) => self.tx_hint = on,
            // (NaN isn't a level: a program's "L RFPOWER nan" would be 5 W on CAT)
            RigCommand::SetLevel(level, v) if self.rig.is_connected() && v.is_finite() => {
                let v = v.clamp(0.0, 1.0);
                match self.rig.set_level(level, v) {
                    Ok(()) => {
                        self.extras.levels.insert(level, v);
                        self.send_extras();
                    }
                    Err(e) => self.emit(RigEvent::Error(format!("{}: {e}", level.label()))),
                }
            }
            RigCommand::SetSwitch(sw, on) if self.rig.is_connected() => match self.rig.set_switch(sw, on) {
                Ok(()) => {
                    self.extras.switches.insert(sw, on);
                    self.send_extras();
                }
                Err(e) => self.emit(RigEvent::Error(format!("{}: {e}", sw.tip()))),
            },
            RigCommand::SetAgc(agc) if self.rig.is_connected() => match self.rig.set_agc(agc) {
                Ok(()) => {
                    self.extras.agc = Some(agc);
                    self.send_extras();
                }
                Err(e) => self.emit(RigEvent::Error(format!("AGC: {e}"))),
            },
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
            // Don't poll while transmitting: keeps PTT-off latency low and some rigs dislike
            // CAT traffic in TX. (Only the TX meters, slowly, while someone is watching them.)
            if self.watch_extras && self.last_extra.elapsed() >= TX_METER_INTERVAL {
                self.read_extra(true);
            }
            return;
        }

        let suppressed = self.poll_suppress_until.is_some_and(|t| Instant::now() < t);
        if !suppressed && self.last_poll.elapsed() >= POLL_INTERVAL {
            self.poll(true);
        } else if self.watch_extras {
            // (keyed by VOX or the modem: the transmit meters, as slowly as when we key it)
            let every = if self.tx_hint { TX_METER_INTERVAL } else { EXTRA_INTERVAL };
            if self.last_extra.elapsed() >= every {
                self.read_extra(self.tx_hint);
            }
        }
    }

    /// Reads one extra control: first each in turn to learn which the radio offers, then the
    /// offered ones round and round (the S-meter every other read, so it moves).
    fn read_extra(&mut self, transmitting: bool) {
        self.last_extra = Instant::now();
        // (transmitting, only the transmit meters are asked about: the rest after it, while
        // those already found are read meanwhile)
        let next = self.to_probe.iter().rposition(|&i| !transmitting || matches!(i, Item::Meter(m) if m.transmit()));
        if let Some(at) = next {
            let item = self.to_probe.remove(at);
            if self.read_item(item) {
                self.offered.get_or_insert_with(Vec::new).push(item);
            }
            if self.to_probe.is_empty() {
                self.offered.get_or_insert_with(Vec::new);
                self.extras.probed = true;
            }
            self.send_extras();
            return;
        }
        // Every other read: the S-meter receiving, the ALC transmitting (Tune → AUTO SET)
        let signal = Item::Meter(if transmitting { Meter::Alc } else { Meter::Signal });
        let round: Vec<Item> = self.offered.iter().flatten().copied()
            .filter(|&i| matches!(i, Item::Meter(m) if m.transmit()) == transmitting && i != signal)
            .collect();
        self.next_read = self.next_read.wrapping_add(1);
        let has_signal = self.offered.iter().flatten().any(|&i| i == signal);
        let item = match (has_signal, round.is_empty()) {
            (false, true) => return,
            (true, true) => signal,
            (true, false) if self.next_read % 2 == 0 => signal,
            _ => round[(self.next_read / 2) % round.len()],
        };
        if self.read_item(item) {
            self.misses.remove(&item);
        } else {
            // (rejected a few times in a row: stop asking, as with the mode; once could be
            // the radio busy, and its last reading stays meanwhile)
            let n = self.misses.entry(item).or_insert(0);
            *n += 1;
            if *n >= MAX_MISSES {
                self.misses.remove(&item);
                if let Some(o) = self.offered.as_mut() {
                    o.retain(|&i| i != item);
                }
                self.extras.forget(item);
            }
        }
        self.send_extras();
    }

    /// Reads `item` into `extras`; false when the radio didn't answer for it (the last
    /// reading, if any, stays). A timeout or I/O error counts the same, but never as a lost
    /// link: the frequency poll decides that.
    fn read_item(&mut self, item: Item) -> bool {
        let x = &mut self.extras;
        match item {
            Item::Level(l) => match self.rig.get_level(l) {
                Ok(v) if v.is_finite() => {
                    x.levels.insert(l, v.clamp(0.0, 1.0));
                    true
                }
                _ => false,
            },
            Item::Switch(s) => match self.rig.get_switch(s) {
                Ok(v) => {
                    x.switches.insert(s, v);
                    true
                }
                Err(_) => false,
            },
            Item::Agc => match self.rig.get_agc() {
                Ok(a) => {
                    x.agc = Some(a);
                    true
                }
                Err(_) => false,
            },
            Item::Meter(m) => match self.rig.get_meter(m) {
                Ok(v) if v.is_finite() => {
                    x.meters.insert(m, v);
                    if m == Meter::Alc {
                        x.alc_reads = x.alc_reads.wrapping_add(1);
                    }
                    true
                }
                _ => false,
            },
        }
    }

    fn send_extras(&mut self) {
        // (unchanged, still once a second: a steady meter is still a fresh reading)
        let same = self.extras_sent.as_ref() == Some(&self.extras);
        if !self.watch_extras || (same && self.extras_sent_at.elapsed() < Duration::from_secs(1)) {
            return;
        }
        self.extras_sent = Some(self.extras.clone());
        self.extras_sent_at = Instant::now();
        self.emit(RigEvent::Extras(self.extras.clone()));
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
        alc_readable: false,
        watch_extras: false,
        extras: RigExtras::default(),
        offered: None,
        to_probe: Item::all(),
        misses: std::collections::HashMap::new(),
        next_read: 0,
        last_extra: Instant::now(),
        extras_sent: None,
        extras_sent_at: Instant::now(),
        tx_hint: false,
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
        assert_eq!(CatModel::from_key("icom:94"), CatModel::Icom(0x94));
        assert_eq!(CatModel::from_key("icom"), CatModel::Icom(0));
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
        let p = PortInfo { name: "COM4".into(), description: "Silicon Labs Dual CP2105 USB to UART Bridge: Enhanced COM Port".into(), cat_port: Some(true), usb: true };
        assert_eq!(p.label(), "COM4 — Silicon Labs Dual CP2105 USB to UART Bridge: Enhanced COM Port (CAT)");
        let p = PortInfo { name: "/dev/ttyS0".into(), description: String::new(), cat_port: None, usb: false };
        assert_eq!(p.label(), "/dev/ttyS0");
    }

    /// Names without a by-id link come back as given; a by-id name is already stable; a port
    /// that exists (or no port at all) is never swapped for another.
    #[test]
    fn stable_port_names() {
        assert_eq!(stable_port_name("/dev/ttyUSB99"), "/dev/ttyUSB99");
        assert_eq!(stable_port_name("COM4"), "COM4");
        assert_eq!(stable_port_name(""), "");
        let by_id = "/dev/serial/by-id/usb-Silicon_Labs_CP2105-if00-port0";
        assert_eq!(stable_port_name(by_id), by_id);
        assert_eq!(replacement_cat_port(""), None);
        assert_eq!(replacement_cat_port("/dev/null"), None);
    }

    /// A port that may key on RTS is never probed (detect_cat filters it before opening).
    #[test]
    fn detect_skips_keying_ports() {
        let keying = PortInfo { name: "COM3".into(), description: String::new(), cat_port: Some(false), usb: true };
        assert!(detect_cat(&[keying], false).unwrap_err().starts_with("no serial ports found"));
        // Looking at startup leaves out Bluetooth and built-in ports, and macOS's tty.* twins
        let bt = PortInfo { name: "/dev/cu.Bluetooth-Incoming-Port".into(), description: String::new(), cat_port: None, usb: false };
        let twin = PortInfo { name: "/dev/tty.usbserial-1".into(), description: String::new(), cat_port: None, usb: true };
        assert!(detect_cat(&[bt, twin], true).unwrap_err().starts_with("no serial ports found"));
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

    /// A radio busy for a moment (Hamlib's RPRT -5 for a couple of reads) keeps the control:
    /// only a few failures in a row hide it.
    #[test]
    fn a_busy_moment_keeps_a_control() {
        use std::io::{BufRead, BufReader, Write};
        use std::sync::atomic::AtomicUsize;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let power_reads = Arc::new(AtomicUsize::new(0));
        let reads = power_reads.clone();
        std::thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            let mut w = s.try_clone().unwrap();
            for line in BufReader::new(s).lines() {
                let Ok(line) = line else { return };
                let reply = match line.trim() {
                    "f" => "14105000\n".to_string(),
                    "m" => "PKTUSB\n3000\n".to_string(),
                    "l RFPOWER" => {
                        // (the first read is the probe; the 2nd and 3rd find the radio busy)
                        let n = reads.fetch_add(1, Ordering::SeqCst);
                        if (1..=2).contains(&n) { "RPRT -5\n".to_string() } else { "0.5\n".to_string() }
                    }
                    l if l.starts_with("l ") || l.starts_with("u ") => "RPRT -11\n".to_string(),
                    _ => "RPRT 0\n".to_string(),
                };
                if w.write_all(reply.as_bytes()).is_err() {
                    return;
                }
            }
        });
        let mut mgr = RigBackend::Rigctld { host: "127.0.0.1".into(), port }.into_manager().unwrap();
        while !mgr.is_connected() {
            mgr.poll_events();
            std::thread::sleep(Duration::from_millis(20));
        }
        mgr.watch_extras(true);
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut last = None;
        while power_reads.load(Ordering::SeqCst) < 6 {
            assert!(Instant::now() < deadline, "power stopped being read after {} reads", power_reads.load(Ordering::SeqCst));
            for ev in mgr.poll_events() {
                if let RigEvent::Extras(x) = ev {
                    last = Some(x);
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        std::thread::sleep(Duration::from_millis(300));
        for ev in mgr.poll_events() {
            if let RigEvent::Extras(x) = ev {
                last = Some(x);
            }
        }
        assert_eq!(last.and_then(|x| x.levels.get(&Level::RfPower).copied()), Some(0.5), "still shown");
    }

    /// The extra controls: read only while watched, only those the radio answers to, and a
    /// change reads back at once.
    #[test]
    fn extras_are_learned_and_set() {
        use std::io::{BufRead, BufReader, Write};
        use std::sync::atomic::AtomicUsize;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let level_reads = Arc::new(AtomicUsize::new(0));
        let reads = level_reads.clone();
        std::thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            let mut w = s.try_clone().unwrap();
            let mut power = 0.5f32;
            for line in BufReader::new(s).lines() {
                let Ok(line) = line else { return };
                let l = line.trim();
                // (the one "l ALC" at connect is the Tune window's AUTO asking, not an extra)
                if (l.starts_with("l ") || l.starts_with("u ")) && l != "l ALC" {
                    reads.fetch_add(1, Ordering::SeqCst);
                }
                let reply = match l {
                    "f" => "14105000\n".to_string(),
                    "m" => "PKTUSB\n3000\n".to_string(),
                    "l RFPOWER" => format!("{power}\n"),
                    "l STRENGTH" => "-6\n".to_string(),
                    "u NB" => "1\n".to_string(),
                    "l ALC" => "0.25\n".to_string(),
                    l if l.starts_with("L RFPOWER ") => {
                        power = l[10..].parse().unwrap();
                        "RPRT 0\n".to_string()
                    }
                    l if l.starts_with("l ") || l.starts_with("u ") => "RPRT -11\n".to_string(),
                    _ => "RPRT 0\n".to_string(),
                };
                if w.write_all(reply.as_bytes()).is_err() {
                    return;
                }
            }
        });
        let mut mgr = RigBackend::Rigctld { host: "127.0.0.1".into(), port }.into_manager().unwrap();
        let wait = |mgr: &mut RigManager, what: &str, f: &dyn Fn(&RigExtras) -> bool| -> RigExtras {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                for ev in mgr.poll_events() {
                    if let RigEvent::Extras(x) = ev {
                        if f(&x) {
                            return x;
                        }
                    }
                }
                assert!(Instant::now() < deadline, "timed out waiting for {what}");
                std::thread::sleep(Duration::from_millis(20));
            }
        };
        while !mgr.is_connected() {
            mgr.poll_events();
            std::thread::sleep(Duration::from_millis(20));
        }
        std::thread::sleep(Duration::from_millis(600));
        assert_eq!(level_reads.load(Ordering::SeqCst), 0, "nothing extra is read unless watched");

        mgr.watch_extras(true);
        let x = wait(&mut mgr, "the controls learned", &|x| x.probed);
        assert_eq!(x.levels.keys().copied().collect::<Vec<_>>(), vec![Level::RfPower]);
        assert_eq!(x.switches.get(&Switch::NoiseBlanker), Some(&true));
        assert_eq!(x.meters.get(&Meter::Signal), Some(&-6.0));
        assert_eq!(x.agc, None);

        mgr.apply(RigAction::Level { level: Level::RfPower, value: 0.8 });
        wait(&mut mgr, "the new power", &|x| x.levels.get(&Level::RfPower) == Some(&0.8));

        // Keyed by VOX (not through us): the transmit meters are read meanwhile
        mgr.tx_hint(true);
        wait(&mut mgr, "an ALC reading in transmit", &|x| x.alc_reads > 0 && x.meters.get(&Meter::Alc) == Some(&0.25));
        mgr.tx_hint(false);

        mgr.watch_extras(false);
        std::thread::sleep(Duration::from_millis(300));
        let before = level_reads.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(800));
        assert_eq!(level_reads.load(Ordering::SeqCst), before, "unwatched, the reads stop");
    }

    /// Keyed straight away (Tune → AUTO SET before the controls were ever learned): the
    /// ALC is still read again and again, the rest learned after the transmission.
    #[test]
    fn transmit_meters_before_the_rest_is_learned() {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            let mut w = s.try_clone().unwrap();
            for line in BufReader::new(s).lines() {
                let Ok(line) = line else { return };
                let reply = match line.trim() {
                    "f" => "7105000\n",
                    "m" => "PKTUSB\n3000\n",
                    "l ALC" => "0.1\n",
                    "l AF" => "0.3\n",
                    l if l.starts_with("l ") || l.starts_with("u ") => "RPRT -11\n",
                    _ => "RPRT 0\n",
                };
                if w.write_all(reply.as_bytes()).is_err() {
                    return;
                }
            }
        });
        let mut mgr = RigBackend::Rigctld { host: "127.0.0.1".into(), port }.into_manager().unwrap();
        while !mgr.is_connected() {
            mgr.poll_events();
            std::thread::sleep(Duration::from_millis(20));
        }
        mgr.watch_extras(true);
        mgr.tx_hint(true);
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut last = RigExtras::default();
        while last.alc_reads < 4 {
            assert!(Instant::now() < deadline, "the ALC wasn't read repeatedly: {last:?}");
            for ev in mgr.poll_events() {
                if let RigEvent::Extras(x) = ev {
                    last = x;
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!last.probed && last.levels.is_empty(), "the receive controls wait for the transmission to end");
        mgr.tx_hint(false);
        let deadline = Instant::now() + Duration::from_secs(10);
        while !last.probed {
            assert!(Instant::now() < deadline, "not learned after the transmission");
            for ev in mgr.poll_events() {
                if let RigEvent::Extras(x) = ev {
                    last = x;
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(last.levels.get(&Level::AfGain), Some(&0.3));
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

    /// A rigctld that answers the frequency and mode, and the ALC level only if `alc`.
    fn fake_rigctld(alc: Option<&'static str>) -> u16 {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let Ok((mut s, _)) = listener.accept() else { return };
            let (mut buf, mut pending) = ([0u8; 256], String::new());
            loop {
                let n = match s.read(&mut buf) {
                    Ok(0) | Err(_) => return,
                    Ok(n) => n,
                };
                pending.push_str(&String::from_utf8_lossy(&buf[..n]));
                while let Some(i) = pending.find('\n') {
                    let line: String = pending.drain(..=i).collect();
                    let reply = match line.trim() {
                        "f" => "14105000\n".to_string(),
                        "m" => "PKTUSB\n3000\n".to_string(),
                        "l ALC" => alc.map_or("RPRT -11\n".to_string(), |v| format!("{v}\n")),
                        _ => "RPRT 0\n".to_string(),
                    };
                    if s.write_all(reply.as_bytes()).is_err() {
                        return;
                    }
                }
            }
        });
        port
    }

    /// Events from the worker until `until` matches one (or 3 s pass).
    fn events_until(m: &mut RigManager, until: impl Fn(&RigEvent) -> bool) -> Vec<RigEvent> {
        let end = Instant::now() + Duration::from_secs(3);
        let mut seen = Vec::new();
        while Instant::now() < end {
            for ev in m.poll_events() {
                let hit = until(&ev);
                seen.push(ev);
                if hit {
                    return seen;
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("no matching rig event in {seen:?}");
    }

    /// A radio whose ALC can be read says so at connect, and ReadAlc answers with it.
    #[test]
    fn worker_reads_the_alc_meter() {
        let mut m = RigManager::new(RigBackend::Rigctld { host: "127.0.0.1".into(), port: fake_rigctld(Some("0.400000")) });
        m.connect();
        let ev = events_until(&mut m, |e| matches!(e, RigEvent::AlcSupported(_)));
        assert!(ev.iter().any(|e| matches!(e, RigEvent::Connected(_))), "{ev:?}");
        assert!(matches!(ev.last(), Some(RigEvent::AlcSupported(true))), "{ev:?}");
        m.send(RigCommand::ReadAlc);
        let ev = events_until(&mut m, |e| matches!(e, RigEvent::Alc(_)));
        assert!(matches!(ev.last(), Some(RigEvent::Alc(v)) if (*v - 0.4).abs() < 1e-6), "{ev:?}");
        m.send(RigCommand::Shutdown);
    }

    /// A radio without an ALC reading stays connected: no ALC, and ReadAlc asks nothing.
    #[test]
    fn worker_without_alc_stays_connected() {
        let mut m = RigManager::new(RigBackend::Rigctld { host: "127.0.0.1".into(), port: fake_rigctld(None) });
        m.connect();
        let ev = events_until(&mut m, |e| matches!(e, RigEvent::AlcSupported(_)));
        assert!(matches!(ev.last(), Some(RigEvent::AlcSupported(false))), "{ev:?}");
        m.send(RigCommand::ReadAlc);
        m.send(RigCommand::PollFreq);
        std::thread::sleep(Duration::from_millis(300));
        let ev = m.poll_events();
        assert!(!ev.iter().any(|e| matches!(e, RigEvent::Alc(_) | RigEvent::Disconnected | RigEvent::Error(_))), "{ev:?}");
        assert!(m.connected);
        m.send(RigCommand::Shutdown);
    }
}
