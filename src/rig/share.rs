//! Sharing the radio with other programs: a Hamlib `rigctld`-compatible server ("Hamlib NET
//! rigctl", model 2) on the Openslot connected to the radio, as RigHelm has. Loggers, WSJT-X,
//! fldigi, GridTracker… point at it (default port 4532) and see the radio Openslot controls,
//! on this computer or, when allowed, anywhere on the LAN.
//!
//! Reads come from what Openslot last read (the frequency every 1.5 s), so other programs
//! add no CAT traffic; changes go through Openslot's own rig worker. Transmitting is off
//! unless allowed: then a program may key the radio, but only unkey what it keyed itself, so
//! a logger's "PTT off" can never cut a VARA transmission short.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::{Agc, Level, Meter, RadioMode, RigCommand, RigExtras, Switch};

pub const DEFAULT_PORT: u16 = 4532;

/// Settings → RIG → "Share the radio with other programs" ([rig_share] in the config file).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShareSettings {
    pub enabled: bool,
    pub port: Option<u16>,
    /// Other computers on the LAN may connect (else this computer only)
    pub lan: bool,
    /// Programs may key the radio (PTT, the tuner)
    pub allow_tx: bool,
}

impl ShareSettings {
    pub fn port(&self) -> u16 {
        self.port.unwrap_or(DEFAULT_PORT)
    }
}

/// The radio as Openslot last saw it.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub connected: bool,
    pub freq: Option<u64>,
    pub mode: Option<RadioMode>,
    pub ptt: bool,
    pub extras: RigExtras,
    /// PTT isn't disabled in Openslot
    pub may_transmit: bool,
}

struct Inner {
    snap: Mutex<Snapshot>,
    target: Mutex<Option<mpsc::Sender<RigCommand>>>,
    clients: AtomicUsize,
    stop: AtomicBool,
    allow_tx: bool,
    /// A program asked for a level or meter: Openslot reads them for a while
    extras_wanted: Mutex<Option<Instant>>,
    notes: Mutex<Vec<String>>,
}

impl Inner {
    fn send(&self, cmd: RigCommand) -> bool {
        self.target.lock().ok().and_then(|t| t.as_ref().map(|tx| tx.send(cmd).is_ok())).unwrap_or(false)
    }

    fn note(&self, text: String) {
        if let Ok(mut n) = self.notes.lock() {
            n.push(text);
        }
    }
}

/// A running server; stops when dropped.
pub struct ShareServer {
    inner: Arc<Inner>,
    /// The port it listens on
    pub port: u16,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ShareServer {
    pub fn start(settings: &ShareSettings) -> std::io::Result<Self> {
        let bind = if settings.lan { "0.0.0.0" } else { "127.0.0.1" };
        let listener = TcpListener::bind((bind, settings.port()))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let inner = Arc::new(Inner {
            snap: Mutex::new(Snapshot::default()),
            target: Mutex::new(None),
            clients: AtomicUsize::new(0),
            stop: AtomicBool::new(false),
            allow_tx: settings.allow_tx,
            extras_wanted: Mutex::new(None),
            notes: Mutex::new(Vec::new()),
        });
        let i2 = inner.clone();
        let thread = std::thread::Builder::new().name("rig-share".into()).spawn(move || {
            while !i2.stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, peer)) => {
                        let _ = stream.set_nonblocking(false);
                        let _ = stream.set_nodelay(true);
                        let inner = i2.clone();
                        let _ = std::thread::Builder::new().name("rig-share-client".into()).spawn(move || {
                            inner.clients.fetch_add(1, Ordering::SeqCst);
                            inner.note(format!("Rig sharing: a program connected from {peer}"));
                            serve(stream, &inner);
                            inner.clients.fetch_sub(1, Ordering::SeqCst);
                            inner.note(format!("Rig sharing: the program at {peer} disconnected"));
                        });
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(100)),
                    Err(_) => std::thread::sleep(Duration::from_millis(250)),
                }
            }
        })?;
        Ok(Self { inner, port, thread: Some(thread) })
    }

    /// The radio now, and where its commands go (Openslot's rig worker; None without one).
    pub fn update(&self, snap: Snapshot, target: Option<mpsc::Sender<RigCommand>>) {
        if let Ok(mut s) = self.inner.snap.lock() {
            *s = snap;
        }
        if let Ok(mut t) = self.inner.target.lock() {
            *t = target;
        }
    }

    pub fn clients(&self) -> usize {
        self.inner.clients.load(Ordering::Relaxed)
    }

    /// A program has asked for levels or meters in the last half minute (they're read only
    /// while wanted; a program that connects for each question finds them read the next time).
    pub fn wants_extras(&self) -> bool {
        self.inner.extras_wanted.lock().ok().and_then(|w| *w).is_some_and(|t| t.elapsed() < Duration::from_secs(30))
    }

    /// What happened, for the log.
    pub fn notes(&self) -> Vec<String> {
        self.inner.notes.lock().map(|mut n| std::mem::take(&mut *n)).unwrap_or_default()
    }
}

impl Drop for ShareServer {
    fn drop(&mut self) {
        self.inner.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

// Hamlib error codes (negated in RPRT lines)
const EINVAL: i32 = 1;
const ENIMPL: i32 = 4;
const ETIMEOUT: i32 = 5;
const ENAVAIL: i32 = 11;
const ERJCTED: i32 = 9;

// Hamlib's bits (rig.h)
const MODE_AM: u64 = 1 << 0;
const MODE_CW: u64 = 1 << 1;
const MODE_USB: u64 = 1 << 2;
const MODE_LSB: u64 = 1 << 3;
const MODE_FM: u64 = 1 << 5;
const MODE_PKTLSB: u64 = 1 << 10;
const MODE_PKTUSB: u64 = 1 << 11;
const MODE_PKTFM: u64 = 1 << 12;
const ALL_MODES: u64 = MODE_AM | MODE_CW | MODE_USB | MODE_LSB | MODE_FM | MODE_PKTLSB | MODE_PKTUSB | MODE_PKTFM;
/// Levels: (name, bit, settable)
const LEVELS: &[(&str, u32, bool)] = &[
    ("PREAMP", 0, true), ("ATT", 1, true), ("AF", 3, true), ("RF", 4, true), ("SQL", 5, true),
    ("RFPOWER", 12, true), ("MICGAIN", 13, true), ("AGC", 17, true), ("SWR", 28, false),
    ("ALC", 29, false), ("STRENGTH", 30, false), ("RFPOWER_METER", 32, false),
];
const FUNCS: &[(&str, u32)] = &[("NB", 1), ("NR", 9)];

enum Out {
    Ok,
    Values(Vec<(&'static str, String)>),
    Raw(String),
    Err(i32),
    Quit,
}

struct Session<'a> {
    inner: &'a Inner,
    /// This program keyed the radio (and so may unkey it)
    keyed: bool,
}

fn serve(stream: TcpStream, inner: &Inner) {
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    let Ok(mut w) = stream.try_clone() else { return };
    let mut r = BufReader::new(stream);
    let mut s = Session { inner, keyed: false };
    let mut line = String::new();
    while !inner.stop.load(Ordering::Relaxed) {
        match r.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {
                let cmd = std::mem::take(&mut line);
                let cmd = cmd.trim_end_matches(['\r', '\n']);
                if cmd.trim().is_empty() {
                    continue;
                }
                match s.execute(cmd) {
                    Some(reply) => {
                        if w.write_all(reply.as_bytes()).is_err() {
                            break;
                        }
                    }
                    None => break,
                }
            }
            // (the timeout only lets us notice a stop; a partial line stays in `line`)
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(_) => break,
        }
    }
    if s.keyed {
        inner.send(RigCommand::SetPtt(false));
        inner.note("Rig sharing: a program left with the radio keyed: unkeyed".into());
    }
}

fn short_name(c: char) -> Option<&'static str> {
    Some(match c {
        'f' => "get_freq",
        'F' => "set_freq",
        'm' => "get_mode",
        'M' => "set_mode",
        'v' => "get_vfo",
        'V' => "set_vfo",
        't' => "get_ptt",
        'T' => "set_ptt",
        's' => "get_split_vfo",
        'S' => "set_split_vfo",
        'i' => "get_split_freq",
        'I' => "set_split_freq",
        'x' => "get_split_mode",
        'l' => "get_level",
        'L' => "set_level",
        'u' => "get_func",
        'U' => "set_func",
        'G' => "vfo_op",
        '_' => "get_info",
        'q' | 'Q' => "quit",
        _ => return None,
    })
}

fn is_vfo_name(s: &str) -> bool {
    matches!(s.to_ascii_uppercase().as_str(), "VFOA" | "VFOB" | "CURRVFO" | "MAIN" | "SUB" | "TX" | "RX" | "VFO" | "NONE")
}

fn hamlib_mode(m: RadioMode) -> &'static str {
    match m {
        RadioMode::Usb => "USB",
        RadioMode::Lsb => "LSB",
        RadioMode::UsbData => "PKTUSB",
        RadioMode::LsbData => "PKTLSB",
        RadioMode::Cw => "CW",
        RadioMode::Am => "AM",
        RadioMode::Fm => "FM",
        RadioMode::FmData => "PKTFM",
    }
}

fn from_hamlib(m: &str) -> Option<RadioMode> {
    Some(match m.to_ascii_uppercase().as_str() {
        "USB" => RadioMode::Usb,
        "LSB" => RadioMode::Lsb,
        "PKTUSB" => RadioMode::UsbData,
        "PKTLSB" => RadioMode::LsbData,
        "CW" | "CWR" => RadioMode::Cw,
        "AM" => RadioMode::Am,
        "FM" | "FMN" => RadioMode::Fm,
        "PKTFM" | "PKTFMN" => RadioMode::FmData,
        _ => return None,
    })
}

/// The passband reported with the mode (Openslot leaves the radio's filter alone).
fn passband(m: RadioMode) -> u32 {
    match m {
        RadioMode::Cw => 500,
        RadioMode::Am => 6000,
        RadioMode::Fm | RadioMode::FmData => 12000,
        _ => 3000,
    }
}

impl Session<'_> {
    /// One command line; None closes the connection.
    fn execute(&mut self, line: &str) -> Option<String> {
        // Extended responses: a leading '+', ';', '|' or ','
        let (ext, sep, body) = match line.chars().next() {
            Some('+') => (true, '\n', &line[1..]),
            Some(c @ (';' | '|' | ',')) => (true, c, &line[1..]),
            _ => (false, '\n', line),
        };
        let body = body.trim();
        let (name, rest) = match body.strip_prefix('\\') {
            Some(long) => {
                let mut it = long.splitn(2, char::is_whitespace);
                (it.next().unwrap_or("").to_string(), it.next().unwrap_or("").trim().to_string())
            }
            None => {
                let mut chars = body.chars();
                let Some(c) = chars.next() else { return Some(format!("RPRT -{EINVAL}\n")) };
                (short_name(c).unwrap_or("?").to_string(), chars.as_str().trim().to_string())
            }
        };
        let mut args: Vec<&str> = rest.split_whitespace().collect();
        // (we report VFO mode off, but take a VFO argument from those that send one anyway)
        let keeps_vfo = name == "set_vfo" || (name == "set_split_vfo" && args.len() < 3);
        if !keeps_vfo && args.first().is_some_and(|a| is_vfo_name(a)) {
            args.remove(0);
        }
        let out = self.dispatch(&name, &args);
        Some(match out {
            Out::Quit => return None,
            Out::Raw(s) => s,
            Out::Ok if ext => format!("{name}: {rest}{sep}RPRT 0\n"),
            Out::Ok => "RPRT 0\n".into(),
            Out::Err(code) if ext => format!("{name}: {rest}{sep}RPRT -{code}\n"),
            Out::Err(code) => format!("RPRT -{code}\n"),
            Out::Values(vals) => {
                let mut s = String::new();
                if ext {
                    s.push_str(&format!("{name}:{sep}"));
                    for (label, v) in &vals {
                        s.push_str(&format!("{label}: {v}{sep}"));
                    }
                    s.push_str("RPRT 0\n");
                } else {
                    for (_, v) in &vals {
                        s.push_str(v);
                        s.push('\n');
                    }
                }
                s
            }
        })
    }

    fn snap(&self) -> Snapshot {
        self.inner.snap.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// A change for the radio, through Openslot's rig worker. What it sets reads back as
    /// set at once (the radio's own reading follows).
    fn change(&self, cmd: RigCommand) -> Out {
        if !self.snap().connected {
            return Out::Err(ETIMEOUT);
        }
        let shown = |snap: &mut Snapshot| match &cmd {
            RigCommand::SetFreq(f) => snap.freq = Some(*f),
            RigCommand::SetMode(m) => snap.mode = Some(*m),
            RigCommand::SetLevel(l, v) => {
                snap.extras.levels.insert(*l, *v);
            }
            RigCommand::SetSwitch(sw, on) => {
                snap.extras.switches.insert(*sw, *on);
            }
            RigCommand::SetAgc(a) => snap.extras.agc = Some(*a),
            _ => {}
        };
        if let Ok(mut snap) = self.inner.snap.lock() {
            shown(&mut snap);
        }
        if self.inner.send(cmd) { Out::Ok } else { Out::Err(ETIMEOUT) }
    }

    fn may_transmit(&self) -> bool {
        self.inner.allow_tx && self.snap().may_transmit
    }

    fn want_extras(&self) {
        if let Ok(mut w) = self.inner.extras_wanted.lock() {
            *w = Some(Instant::now());
        }
    }

    fn dispatch(&mut self, name: &str, args: &[&str]) -> Out {
        let s = self.snap();
        let num = |i: usize| -> Option<f64> { args.get(i).and_then(|a| a.parse::<f64>().ok()) };
        match name {
            "get_freq" | "get_split_freq" => match (s.connected, s.freq) {
                (true, Some(f)) => Out::Values(vec![(if name == "get_freq" { "Frequency" } else { "TX Frequency" }, f.to_string())]),
                _ => Out::Err(ETIMEOUT),
            },
            "set_freq" => {
                let Some(f) = num(0).filter(|f| (30_000.0..=1.0e9).contains(f)) else { return Out::Err(EINVAL) };
                self.change(RigCommand::SetFreq(f.round() as u64))
            }
            // (no split: TX is on the receive frequency)
            "set_split_freq" => match (num(0), s.freq) {
                (Some(f), Some(cur)) if f.round() as u64 == cur => Out::Ok,
                _ => Out::Err(ERJCTED),
            },
            "get_mode" | "get_split_mode" => match (s.connected, s.mode) {
                (true, Some(m)) => Out::Values(vec![("Mode", hamlib_mode(m).into()), ("Passband", passband(m).to_string())]),
                (true, None) => Out::Values(vec![("Mode", "PKTUSB".into()), ("Passband", "3000".into())]),
                _ => Out::Err(ETIMEOUT),
            },
            "set_mode" => {
                let Some(m) = args.first().and_then(|m| from_hamlib(m)) else { return Out::Err(EINVAL) };
                if s.mode == Some(m) {
                    return Out::Ok;
                }
                self.change(RigCommand::SetMode(m))
            }
            "get_vfo" => Out::Values(vec![("VFO", "VFOA".into())]),
            "set_vfo" => match args.first().map(|v| v.to_ascii_uppercase()).as_deref() {
                Some("VFOA" | "MAIN" | "CURRVFO" | "RX" | "TX" | "VFO") => Out::Ok,
                _ => Out::Err(ERJCTED),
            },
            "get_ptt" => Out::Values(vec![("PTT", (s.ptt as u8).to_string())]),
            "set_ptt" => match args.first() {
                Some(&"0") => {
                    // Only what this program keyed: never a VARA transmission
                    if self.keyed {
                        self.keyed = false;
                        self.change(RigCommand::SetPtt(false))
                    } else {
                        Out::Ok
                    }
                }
                Some(&("1" | "2" | "3")) => {
                    if !self.may_transmit() {
                        self.inner.note("Rig sharing: a program's PTT was refused (transmitting isn't allowed)".into());
                        return Out::Err(ERJCTED);
                    }
                    if s.ptt && !self.keyed {
                        // (Openslot is transmitting: don't take its transmission over)
                        return Out::Err(ERJCTED);
                    }
                    let r = self.change(RigCommand::SetPtt(true));
                    if matches!(r, Out::Ok) {
                        self.keyed = true;
                    }
                    r
                }
                _ => Out::Err(EINVAL),
            },
            "get_split_vfo" => Out::Values(vec![("Split", "0".into()), ("TX VFO", "VFOA".into())]),
            "set_split_vfo" => match args.first() {
                Some(&"0") => Out::Ok,
                _ => Out::Err(ERJCTED),
            },
            "get_level" => {
                self.want_extras();
                match args.first().map(|l| l.to_ascii_uppercase()) {
                    Some(l) => self.get_level(&l, &s.extras),
                    None => Out::Err(EINVAL),
                }
            }
            "set_level" => {
                let (Some(l), Some(v)) = (args.first().map(|l| l.to_ascii_uppercase()), num(1)) else { return Out::Err(EINVAL) };
                let v = v as f32;
                match l.as_str() {
                    "PREAMP" => self.change(RigCommand::SetSwitch(Switch::Preamp, v > 0.0)),
                    "ATT" => self.change(RigCommand::SetSwitch(Switch::Attenuator, v > 0.0)),
                    "AGC" => self.change(RigCommand::SetAgc(match v as i32 {
                        0 => Agc::Off,
                        1 | 2 => Agc::Fast,
                        3 => Agc::Slow,
                        5 => Agc::Medium,
                        _ => Agc::Auto,
                    })),
                    other => match level_of(other) {
                        Some(level) => self.change(RigCommand::SetLevel(level, v.clamp(0.0, 1.0))),
                        None => Out::Err(ENIMPL),
                    },
                }
            }
            "get_func" => {
                self.want_extras();
                let sw = match args.first().map(|f| f.to_ascii_uppercase()).as_deref() {
                    Some("NB") => Switch::NoiseBlanker,
                    Some("NR") => Switch::NoiseReduction,
                    _ => return Out::Err(ENIMPL),
                };
                match s.extras.switches.get(&sw) {
                    Some(&on) => Out::Values(vec![("Func Status", (on as u8).to_string())]),
                    None => Out::Err(ENAVAIL),
                }
            }
            "set_func" => {
                let sw = match args.first().map(|f| f.to_ascii_uppercase()).as_deref() {
                    Some("NB") => Switch::NoiseBlanker,
                    Some("NR") => Switch::NoiseReduction,
                    _ => return Out::Err(ENIMPL),
                };
                self.change(RigCommand::SetSwitch(sw, args.get(1) == Some(&"1")))
            }
            "vfo_op" => match args.first().map(|o| o.to_ascii_uppercase()).as_deref() {
                Some("TUNE") if self.may_transmit() => self.change(RigCommand::TuneAtu),
                Some("TUNE") => Out::Err(ERJCTED),
                _ => Out::Err(ENIMPL),
            },
            "chk_vfo" => Out::Raw("0\n".into()),
            "dump_state" => Out::Raw(dump_state()),
            "get_powerstat" => Out::Values(vec![("Power Status", "1".into())]),
            "get_lock_mode" => Out::Values(vec![("Locked", "0".into())]),
            "set_lock_mode" | "set_vfo_opt" => Out::Ok,
            "get_info" => Out::Values(vec![("Info", format!("Openslot {} rig sharing", env!("CARGO_PKG_VERSION")))]),
            "quit" => Out::Quit,
            _ => Out::Err(ENIMPL),
        }
    }

    fn get_level(&self, level: &str, x: &RigExtras) -> Out {
        let f = |v: f32| Out::Values(vec![("Level Value", format!("{v:.6}"))]);
        let i = |v: i32| Out::Values(vec![("Level Value", v.to_string())]);
        let missing = Out::Err(ENAVAIL);
        match level {
            "STRENGTH" => x.meters.get(&Meter::Signal).map_or(missing, |&v| i(v.round() as i32)),
            "RFPOWER_METER" => x.meters.get(&Meter::Power).map_or(missing, |&v| f(v)),
            "SWR" => x.meters.get(&Meter::Swr).map_or(missing, |&v| f(v)),
            "ALC" => x.meters.get(&Meter::Alc).map_or(missing, |&v| f(v)),
            // (Hamlib wants dB: the usual first step)
            "PREAMP" => x.switches.get(&Switch::Preamp).map_or(missing, |&on| i(if on { 10 } else { 0 })),
            "ATT" => x.switches.get(&Switch::Attenuator).map_or(missing, |&on| i(if on { 6 } else { 0 })),
            "AGC" => x.agc.map_or(missing, |a| i(match a {
                Agc::Off => 0,
                Agc::Fast => 2,
                Agc::Medium => 5,
                Agc::Slow => 3,
                Agc::Auto => 6,
            })),
            other => match level_of(other) {
                Some(l) => x.levels.get(&l).map_or(missing, |&v| f(v)),
                None => Out::Err(ENIMPL),
            },
        }
    }
}

fn level_of(name: &str) -> Option<Level> {
    Some(match name {
        "RFPOWER" => Level::RfPower,
        "AF" => Level::AfGain,
        "RF" => Level::RfGain,
        "SQL" => Level::Squelch,
        "MICGAIN" => Level::MicGain,
        _ => return None,
    })
}

/// What Hamlib's NET rigctl backend reads when a program opens the radio: the ranges,
/// modes, steps, filters and what can be read and set.
fn dump_state() -> String {
    let mut s = String::new();
    let mut l = |t: &str| {
        s.push_str(t);
        s.push('\n');
    };
    let vfos = 0x1; // VFOA
    l("1"); // protocol version
    l("2"); // the model: NET rigctl
    l("0"); // ITU region (unused)
    l(&format!("30000 470000000 0x{ALL_MODES:x} -1 -1 0x{vfos:x} 0x0"));
    l("0 0 0 0 0 0 0");
    for (lo, hi) in [(1_800_000u64, 2_000_000u64), (3_500_000, 4_000_000), (5_330_000, 5_410_000), (7_000_000, 7_300_000),
        (10_100_000, 10_150_000), (14_000_000, 14_350_000), (18_068_000, 18_168_000), (21_000_000, 21_450_000),
        (24_890_000, 24_990_000), (28_000_000, 29_700_000), (50_000_000, 54_000_000)]
    {
        l(&format!("{lo} {hi} 0x{:x} 5000 100000 0x{vfos:x} 0x0", ALL_MODES & !MODE_AM));
        l(&format!("{lo} {hi} 0x{MODE_AM:x} 5000 25000 0x{vfos:x} 0x0"));
    }
    l("0 0 0 0 0 0 0");
    l(&format!("0x{ALL_MODES:x} 1"));
    l(&format!("0x{ALL_MODES:x} 10"));
    l(&format!("0x{ALL_MODES:x} 100"));
    l(&format!("0x{ALL_MODES:x} 1000"));
    l("0 0");
    let ssb = MODE_USB | MODE_LSB | MODE_PKTUSB | MODE_PKTLSB;
    for (m, w) in [(ssb, 3000), (ssb, 2400), (MODE_CW, 500), (MODE_AM, 6000), (MODE_FM | MODE_PKTFM, 12000)] {
        l(&format!("0x{m:x} {w}"));
    }
    l("0 0");
    l("0"); // max RIT
    l("0"); // max XIT
    l("0"); // max IF shift
    l("0"); // announces
    l("10 ");
    l("6 ");
    let funcs: u64 = FUNCS.iter().fold(0, |a, f| a | (1u64 << f.1));
    let get_levels: u64 = LEVELS.iter().fold(0, |a, f| a | (1u64 << f.1));
    let set_levels: u64 = LEVELS.iter().filter(|f| f.2).fold(0, |a, f| a | (1u64 << f.1));
    l(&format!("0x{funcs:x}"));
    l(&format!("0x{funcs:x}"));
    l(&format!("0x{get_levels:x}"));
    l(&format!("0x{set_levels:x}"));
    l("0x0");
    l("0x0");
    l("vfo_ops=0x800"); // TUNE
    l("ptt_type=0x1");
    l("targetable_vfo=0x0");
    l("has_set_vfo=1");
    l("has_get_vfo=1");
    l("has_set_freq=1");
    l("has_get_freq=1");
    l("has_set_conf=0");
    l("has_get_conf=0");
    l("has_power2mW=0");
    l("has_mW2power=0");
    l("has_get_ant=0");
    l("has_set_ant=0");
    l("timeout=2000");
    l("rig_model=2");
    l(&format!("rigctld_version=Openslot {}", env!("CARGO_PKG_VERSION")));
    l("agc_levels=0=OFF 2=FAST 5=MEDIUM 3=SLOW 6=AUTO");
    l("done");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(allow_tx: bool) -> (Arc<Inner>, mpsc::Receiver<RigCommand>) {
        let (tx, rx) = mpsc::channel();
        let inner = Arc::new(Inner {
            snap: Mutex::new(Snapshot { connected: true, freq: Some(14_105_000), mode: Some(RadioMode::UsbData), may_transmit: true, ..Default::default() }),
            target: Mutex::new(Some(tx)),
            clients: AtomicUsize::new(0),
            stop: AtomicBool::new(false),
            allow_tx,
            extras_wanted: Mutex::new(None),
            notes: Mutex::new(Vec::new()),
        });
        (inner, rx)
    }

    #[test]
    fn answers_like_rigctld() {
        let (inner, rx) = session(false);
        let mut s = Session { inner: &inner, keyed: false };
        assert_eq!(s.execute("f").unwrap(), "14105000\n");
        assert_eq!(s.execute("m").unwrap(), "PKTUSB\n3000\n");
        assert_eq!(s.execute("+f").unwrap(), "get_freq:\nFrequency: 14105000\nRPRT 0\n");
        assert_eq!(s.execute("\\chk_vfo").unwrap(), "0\n");
        assert!(s.execute("\\dump_state").unwrap().ends_with("done\n"));
        assert_eq!(s.execute("F 7105000").unwrap(), "RPRT 0\n");
        assert!(matches!(rx.try_recv(), Ok(RigCommand::SetFreq(7_105_000))));
        assert_eq!(s.execute("f").unwrap(), "7105000\n");
        assert_eq!(s.execute("M VFOA USB 2400").unwrap(), "RPRT 0\n");
        assert!(matches!(rx.try_recv(), Ok(RigCommand::SetMode(RadioMode::Usb))));
        assert_eq!(s.execute("s").unwrap(), "0\nVFOA\n");
        assert_eq!(s.execute("l RFPOWER").unwrap(), "RPRT -11\n", "not read yet");
        assert!(inner.extras_wanted.lock().unwrap().is_some(), "and now wanted");
        assert_eq!(s.execute("\\unknown_thing").unwrap(), "RPRT -4\n");
        assert_eq!(s.execute("+").unwrap(), "RPRT -1\n", "answered, not hung up on");
        assert!(s.execute("q").is_none());
    }

    /// No transmitting unless allowed; and a program unkeys only what it keyed.
    #[test]
    fn transmit_rules() {
        let (inner, rx) = session(false);
        let mut s = Session { inner: &inner, keyed: false };
        assert_eq!(s.execute("T 1").unwrap(), "RPRT -9\n");
        assert_eq!(s.execute("G TUNE").unwrap(), "RPRT -9\n");
        assert!(rx.try_recv().is_err());

        let (inner, rx) = session(true);
        let mut s = Session { inner: &inner, keyed: false };
        // Openslot (VARA) is transmitting: a logger's PTT off doesn't touch it
        inner.snap.lock().unwrap().ptt = true;
        assert_eq!(s.execute("T 0").unwrap(), "RPRT 0\n");
        assert!(rx.try_recv().is_err());
        assert_eq!(s.execute("T 1").unwrap(), "RPRT -9\n", "nor take it over");
        inner.snap.lock().unwrap().ptt = false;
        assert_eq!(s.execute("T 1").unwrap(), "RPRT 0\n");
        assert!(matches!(rx.try_recv(), Ok(RigCommand::SetPtt(true))));
        assert_eq!(s.execute("T 0").unwrap(), "RPRT 0\n");
        assert!(matches!(rx.try_recv(), Ok(RigCommand::SetPtt(false))));
    }

    /// Over TCP, as a Hamlib program connects (and leaving keyed unkeys).
    #[test]
    fn over_tcp() {
        let (tx, rx) = mpsc::channel();
        let settings = ShareSettings { enabled: true, port: Some(0), lan: false, allow_tx: true };
        let srv = ShareServer::start(&settings).unwrap();
        srv.update(Snapshot { connected: true, freq: Some(14_105_000), may_transmit: true, ..Default::default() }, Some(tx));
        let c = TcpStream::connect(("127.0.0.1", srv.port)).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut w = c.try_clone().unwrap();
        let mut r = BufReader::new(c);
        let mut ask = |q: &str| {
            w.write_all(format!("{q}\n").as_bytes()).unwrap();
            let mut line = String::new();
            r.read_line(&mut line).unwrap();
            line
        };
        assert_eq!(ask("f"), "14105000\n");
        assert_eq!(ask("T 1"), "RPRT 0\n");
        assert!(matches!(rx.recv_timeout(Duration::from_secs(2)), Ok(RigCommand::SetPtt(true))));
        let deadline = Instant::now() + Duration::from_secs(2);
        while srv.clients() != 1 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(srv.clients(), 1);
        drop(ask);
        drop(r);
        drop(w);
        assert!(matches!(rx.recv_timeout(Duration::from_secs(3)), Ok(RigCommand::SetPtt(false))), "unkeyed when it left");
    }
}
