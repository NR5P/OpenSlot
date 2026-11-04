//! The XARA modem built into the desktop apps (Linux, macOS and Windows; the plain root build
//! doesn't compile this): its settings (xara.toml next to config.toml), starting and stopping
//! it, and its tab.
//!
//! A bar above the GUI's own menu bar holds the app's two tabs, **Openslot** (the shared GUI)
//! and **Modem**, with the modem's state and Start/Stop. The Modem tab is the NR5P XARA app's
//! window inside Openslot: the link's live status, levels, Tune and the waterfall, every
//! setting the XARA app has (Registered, the VARA licence switch, among them), and its
//! Received, Log and Web UI pages. The station keeps working while it shows (GuiShell's tick).
//!
//! XARA opens the usual VARA ports (command 8300, data 8301, KISS 8100) on this computer, so the
//! GUI connects to it exactly as it would to VARA; nothing in the shared code changes.

mod feed;

use std::collections::VecDeque;
use std::net::{SocketAddr, TcpStream, UdpSocket};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, RichText};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedSender;

use crate::gui::state::GuiCommand;
use feed::Feed;

const LOG_LINES: usize = 5000;
const RECEIVED_LIMIT: usize = 500_000;
/// The waterfall's height in rows (about 13 s of HF at 11.7 rows a second)
const WF_ROWS: usize = 150;

pub const MODES: [(&str, &str); 5] = [
    ("hf500", "HF 500 Hz"),
    ("hf2300", "HF 2300 Hz"),
    ("hf2750", "HF 2750 Hz"),
    ("fm-narrow", "FM narrow"),
    ("fm-wide", "FM wide"),
];

#[cfg(target_os = "linux")]
pub const PTTS: &[(&str, &str)] = &[
    ("vox", "VOX"),
    ("rts", "Serial RTS"),
    ("dtr", "Serial DTR"),
    ("cm108", "CM108 / AIOC (/dev/hidraw)"),
    ("rigctld", "rigctld (hamlib CAT)"),
];
#[cfg(target_os = "macos")]
pub const PTTS: &[(&str, &str)] = &[
    ("vox", "VOX"),
    ("rts", "Serial RTS"),
    ("dtr", "Serial DTR"),
    ("cm108", "CM108 / AIOC (USB HID)"),
    ("rigctld", "rigctld (hamlib CAT)"),
];
/// CM108 GPIO PTT (HID) is not in XARA on Windows yet.
#[cfg(windows)]
pub const PTTS: &[(&str, &str)] = &[("vox", "VOX"), ("rts", "Serial RTS"), ("dtr", "Serial DTR"), ("rigctld", "rigctld (hamlib CAT)")];

#[cfg(target_os = "linux")]
const AUDIO: (&str, xara::audio::Backend) = ("pulse", xara::audio::Backend::Pulse);
#[cfg(target_os = "macos")]
const AUDIO: (&str, xara::audio::Backend) = ("coreaudio", xara::audio::Backend::CoreAudio);
#[cfg(windows)]
const AUDIO: (&str, xara::audio::Backend) = ("wasapi", xara::audio::Backend::Wasapi);

/// The sound cards (inputs, outputs), listed on a thread of their own. Listing them through
/// WASAPI puts the calling thread into multithreaded COM for good; done on the GUI's thread
/// before its window opens (the modem starting at launch), winit's OleInitialize then fails
/// and the Windows app panicked at start whenever the built-in modem was set up.
fn audio_devices() -> std::io::Result<(Vec<(String, String)>, Vec<(String, String)>)> {
    std::thread::spawn(|| xara::audio::list(AUDIO.1))
        .join()
        .unwrap_or_else(|_| Err(std::io::Error::other("listing the sound cards failed")))
}

#[cfg(target_os = "linux")]
const VIRTUAL_OUT: &str = "a PulseAudio / PipeWire null sink, e.g. pactl load-module module-null-sink sink_name=xara_out";
#[cfg(target_os = "macos")]
const VIRTUAL_OUT: &str = "such as BlackHole";
#[cfg(windows)]
const VIRTUAL_OUT: &str = "such as VB-Audio CABLE Input";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct XaraSettings {
    /// Use the built-in modem (started with the app); off = VARA or another modem the user runs.
    pub enabled: bool,
    pub mode: String,
    /// The audio devices: PulseAudio / PipeWire source and sink names (Linux), CoreAudio
    /// device UIDs (macOS), WASAPI endpoint names (Windows).
    pub capture: String,
    pub playback: String,
    /// Transmit through a real radio interface (else playback only into a virtual device).
    pub radio: bool,
    pub ptt: String,
    pub ptt_serial: String,
    pub ptt_cm108: String,
    pub ptt_rigctld: String,
    pub drive_db: f32,
    /// PTT on this long before the audio, and off this long after it
    pub lead_ms: u32,
    pub tail_ms: u32,
    /// Delay in the audio path that XARA does not add itself (a network audio link).
    pub audio_latency_ms: u32,
    pub ui_port: u16,
    /// Registered VARA speeds, for a user with a VARA licence.
    pub registered: bool,
    /// Decode VARA HF traffic between other stations (Received page), as VARA's monitor
    pub monitor: bool,
    /// Every packet, not just the text
    pub monitor_frames: bool,
    /// Calls to name answering stations by (else they show as #hash)
    pub monitor_calls: String,
    pub record_audio: bool,
    pub debug_log: bool,
    /// Accept VARA clients and web UI pages from the local network (else this computer only)
    pub lan: bool,
    /// The web UI can control the radio, with the token in its link
    pub ui_control: bool,
    /// Kept across launches, so a bookmarked controlling web UI keeps working.
    pub ui_token: String,
}

impl Default for XaraSettings {
    fn default() -> Self {
        Self {
            // XARA works out of the box; VARA is the user's choice on the Modem tab
            enabled: true,
            mode: "hf2300".into(),
            capture: String::new(),
            playback: String::new(),
            radio: false,
            ptt: "vox".into(),
            ptt_serial: String::new(),
            ptt_cm108: if cfg!(target_os = "linux") { "/dev/hidraw0".into() } else { "0d8c:0012".into() },
            ptt_rigctld: "127.0.0.1:4532".into(),
            drive_db: -5.0,
            // (XARA's own defaults)
            lead_ms: 0,
            tail_ms: 100,
            audio_latency_ms: 0,
            ui_port: 8080,
            registered: false,
            monitor: false,
            monitor_frames: false,
            monitor_calls: String::new(),
            record_audio: false,
            debug_log: false,
            lan: false,
            ui_control: false,
            ui_token: String::new(),
        }
    }
}

fn app_dirs() -> Option<directories::ProjectDirs> {
    crate::config::project_dirs()
}

/// The modem's state (its web UI settings and token). Private: XARA warns about a state folder
/// other users can write to, and a desktop's usual umask leaves new folders group-writable.
fn state_dir() -> Option<PathBuf> {
    let dir = app_dirs()?.data_dir().join("xara");
    std::fs::create_dir_all(&dir).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    Some(dir)
}

/// The debug log and recordings.
fn logs_dir() -> PathBuf {
    let dir = app_dirs().map_or_else(|| PathBuf::from("."), |d| d.data_dir().join("xara-logs"));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// XARA takes 8–128 characters of A-Z, a-z, 0-9, - and _.
fn valid_token(t: &str) -> bool {
    (8..=128).contains(&t.len()) && t.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn new_token() -> String {
    // Random enough for a local UI token: std's hasher keys come from the OS's random source.
    use std::hash::{BuildHasher, Hasher};
    let mut out = String::new();
    while out.len() < 32 {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_usize(out.len());
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out.truncate(32);
    out
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

    fn is_hf(&self) -> bool {
        self.mode.starts_with("hf")
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
        let bind = if self.lan { "0.0.0.0" } else { "127.0.0.1" };
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
        a.extend(["--audio", AUDIO.0, "--capture", &self.capture, "--playback", &self.playback].map(String::from));
        if self.radio {
            a.extend(["--radio".into(), "--ptt".into(), self.ptt_option()]);
            a.extend(["--ptt-lead-ms".into(), self.lead_ms.min(1000).to_string()]);
            a.extend(["--ptt-tail-ms".into(), self.tail_ms.min(1000).to_string()]);
        }
        a.extend(["--drive-db".into(), format!("{:.1}", self.drive_db)]);
        a.extend(["--addr".into(), bind.into(), "--port".into(), cmd_port.to_string()]);
        a.extend(["--kiss-port".into(), kiss_port.to_string()]);
        a.extend(["--ui-port".into(), self.ui_port.to_string(), "--ui-addr".into(), bind.into()]);
        if self.ui_control && valid_token(&self.ui_token) {
            a.extend(["--ui-control".into(), "--ui-token".into(), self.ui_token.clone()]);
        }
        if self.monitor && self.is_hf() {
            a.push("--monitor".into());
            if self.monitor_frames {
                a.push("--monitor-frames".into());
            }
            for c in self.monitor_calls.to_uppercase().split([' ', ',']).filter(|c| !c.is_empty()) {
                a.extend(["--monitor-call".into(), c.into()]);
            }
        }
        if self.registered {
            a.push("--registered".into());
        }
        if self.audio_latency_ms > 0 {
            a.extend(["--audio-latency-ms".into(), self.audio_latency_ms.to_string()]);
        }
        if self.debug_log {
            a.extend(["--debug-log".into(), logs_dir().join("debug.log").to_string_lossy().into_owned()]);
        }
        if self.record_audio {
            let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
            let f = logs_dir().join(format!("xara-{stamp}.flac"));
            a.extend(["--record".into(), f.to_string_lossy().into_owned()]);
        }
        if let Some(dir) = state_dir() {
            a.extend(["--state-dir".into(), dir.to_string_lossy().into_owned()]);
        }
        a
    }

    /// The equivalent `xara` command, for running it from a terminal instead.
    fn command_line(&self, call: &str, cmd_port: u16, kiss_port: u16) -> String {
        std::iter::once("xara".to_string())
            .chain(self.arguments(call, cmd_port, kiss_port))
            .map(|a| if a.is_empty() || a.contains(' ') { format!("\"{a}\"") } else { a })
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn web_url(&self) -> String {
        let token = if self.ui_control && valid_token(&self.ui_token) { format!("?token={}", self.ui_token) } else { String::new() };
        format!("http://127.0.0.1:{}/{token}", self.ui_port)
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

/// This computer's address on the local network, for clients elsewhere (no packet is sent).
fn local_address() -> Option<String> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.0.2.1:9").ok()?;
    Some(s.local_addr().ok()?.ip().to_string())
}

static RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The running modem. Global so the exit hook can stop it when the process ends without the
/// GUI returning to stop it itself (on macOS, Quit from the menu or the Dock ends the process
/// from inside AppKit).
static HANDLE: std::sync::Mutex<Option<xara_ffi::XaraModem>> = std::sync::Mutex::new(None);

fn handle() -> std::sync::MutexGuard<'static, Option<xara_ffi::XaraModem>> {
    HANDLE.lock().unwrap_or_else(|e| e.into_inner())
}

/// For the exit hook: stops a running modem, so it releases PTT and closes the audio
/// cleanly (cut off, macOS's audio server can stall the next start). At most `limit`, so a
/// stuck audio system can't hold up quitting.
#[allow(dead_code)] // not every platform's app has an exit hook
pub fn stop_at_exit(limit: Duration) {
    let Some(m) = handle().take() else { return };
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(m.stop());
    });
    let _ = rx.recv_timeout(limit);
    RUNNING.store(false, std::sync::atomic::Ordering::SeqCst);
}

fn xara_version() -> &'static str {
    // SAFETY: xara_version returns a static NUL-terminated string.
    unsafe { std::ffi::CStr::from_ptr(xara_ffi::xara_version()) }.to_str().unwrap_or("?")
}

/// The app's two tabs.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Openslot,
    Modem,
}

/// The Modem tab's pages, as the XARA app's tabs.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Modem,
    Received,
    Log,
    WebUi,
}

pub struct BuiltInModem {
    pub settings: XaraSettings,
    /// The settings as edited on the Modem tab, until Save
    draft: XaraSettings,
    call: String,
    cmd_port: u16,
    kiss_port: u16,
    stopping: Option<JoinHandle<u8>>,
    status: String,
    log: VecDeque<String>,
    /// What the modem decoded: the links' text, and other stations' with the monitor on
    received: String,
    tab: Tab,
    page: Page,
    inputs: Vec<(String, String)>,
    outputs: Vec<(String, String)>,
    serial_ports: Vec<crate::rig::PortInfo>,
    /// /dev/hidraw* (Linux's CM108 devices)
    hidraw: Vec<String>,
    devices_listed: bool,
    /// Start again once the current stop finishes (settings changed while running).
    restart_after_stop: bool,
    /// The GUI's drive level as last seen, to notice the Tune window changing it.
    gui_drive: Option<i32>,
    /// Started, not reporting its status yet: when, and the status to show once it does
    starting: Option<(Instant, String)>,
    feed: Option<Arc<Feed>>,
    waterfall: WaterfallView,
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
            received: String::new(),
            tab: Tab::Openslot,
            page: Page::Modem,
            inputs: Vec::new(),
            outputs: Vec::new(),
            serial_ports: Vec::new(),
            hidraw: Vec::new(),
            devices_listed: false,
            restart_after_stop: false,
            gui_drive: None,
            starting: None,
            feed: None,
            waterfall: WaterfallView::default(),
        }
    }

    /// Keep a drive level set in the Tune window (DRIVELEVEL, live in the running modem) for
    /// the next start too, which takes its level from xara.toml. Only a change counts: a level
    /// left over from before a restart mustn't undo one just saved on the Modem tab.
    pub fn follow_drive_level(&mut self, gui: Option<i32>) {
        if gui == std::mem::replace(&mut self.gui_drive, gui) {
            return;
        }
        let Some(db) = gui else { return };
        if !self.settings.enabled || !self.running() || db as f32 == self.settings.drive_db {
            return;
        }
        self.keep_drive(db as f32);
    }

    /// A drive level changed live (Tune window, the Modem tab's slider), kept for the next start.
    fn keep_drive(&mut self, db: f32) {
        self.settings.drive_db = db;
        self.draft.drive_db = db;
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
            self.tab = Tab::Modem;
            self.page = Page::Modem;
            return Err("choose the audio in and out on the Modem tab".into());
        }
        if let Err(e) = self.find_audio() {
            self.tab = Tab::Modem;
            self.page = Page::Modem;
            return Err(e);
        }
        let s = &self.settings;
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
                let running = format!("Running {} as {}", s.mode_label(), self.call.to_uppercase());
                self.status = "Starting…".into();
                self.starting = Some((Instant::now(), running));
                Ok(())
            }
            None => Err("a XARA modem is already running in this app".into()),
        }
    }

    /// Checks the radio's audio is plugged in. A USB sound card plugged into another port (or
    /// hub) comes back under a new name: the location in a CoreAudio UID, the "2-" in a Windows
    /// endpoint, the card number on Linux. Then the one device whose name differs only in its
    /// numbers is the same device, and is saved in place of the old name.
    fn find_audio(&mut self) -> Result<(), String> {
        let Ok((inputs, outputs)) = audio_devices() else {
            // (can't tell: let the modem try the names it has)
            return Ok(());
        };
        let capture = moved_device(&self.settings.capture, &inputs)
            .ok_or_else(|| format!("the radio's audio input ({}) isn't plugged in", self.settings.capture))?;
        let playback = moved_device(&self.settings.playback, &outputs)
            .ok_or_else(|| format!("the radio's audio output ({}) isn't plugged in", self.settings.playback))?;
        if capture != self.settings.capture || playback != self.settings.playback {
            self.settings.capture = capture.clone();
            self.settings.playback = playback.clone();
            self.draft.capture = capture;
            self.draft.playback = playback;
            let line = match self.settings.save() {
                Ok(()) => "xara: the radio's audio moved to another USB port: using it there".to_string(),
                Err(e) => format!("xara: the radio's audio moved to another USB port (couldn't save it: {e})"),
            };
            tracing::info!("{line}");
            self.log.push_back(line);
        }
        Ok(())
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
            self.starting = None;
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
            .unwrap_or("see the modem's Log")
            .to_string()
    }

    fn poll(&mut self, cmd_tx: &UnboundedSender<GuiCommand>) {
        while let Some(line) = xara_ffi::next_log() {
            if self.log.len() == LOG_LINES {
                self.log.pop_front();
            }
            self.log.push_back(line);
        }
        while let Some(piece) = xara_ffi::next_received() {
            self.received.push_str(&piece);
        }
        if self.received.len() > RECEIVED_LIMIT {
            let mut cut = self.received.len() - RECEIVED_LIMIT;
            while !self.received.is_char_boundary(cut) {
                cut += 1;
            }
            self.received.drain(..cut);
        }

        let ended = handle().as_ref().is_some_and(|m| !m.running());
        if ended {
            let m = handle().take();
            let code = m.map_or(0, |m| m.stop());
            RUNNING.store(false, std::sync::atomic::Ordering::SeqCst);
            self.starting = None;
            self.status = format!("Stopped (exit {code}): {}", self.last_error());
        }
        // Until its status feed answers (its audio is up then). Not by trying its command port:
        // it takes one client at a time, so once the GUI is connected that never answers, and a
        // try could take the place the GUI is about to connect to. A long wait on macOS is
        // usually it asking for the microphone (the audio can't start before that's answered),
        // with its dialog behind this window.
        if let Some((since, running)) = &mut self.starting {
            let up = self.feed.as_ref().is_some_and(|f| f.latest().status.is_some());
            if up || since.elapsed() >= Duration::from_secs(20) {
                self.status = std::mem::take(running);
                self.starting = None;
            } else if cfg!(target_os = "macos") && since.elapsed() >= Duration::from_secs(4) {
                self.status = "Waiting for the audio to start — if macOS asks to use the microphone, click Allow".into();
            }
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
        self.devices_listed = true;
        match audio_devices() {
            Ok((i, o)) => {
                // (a description reads better than a device name; keep the name when there's none)
                let named = |v: Vec<(String, String)>| -> Vec<(String, String)> {
                    v.into_iter().map(|(n, d)| if d.is_empty() { (n.clone(), n) } else { (n, d) }).collect()
                };
                self.inputs = named(i);
                self.outputs = named(o);
            }
            Err(e) => self.status = format!("Can't list audio devices: {e}"),
        }
        self.serial_ports = serial_ports();
        self.hidraw = hidraw_devices();
    }

    /// Saves the Modem tab's settings and applies them: a running modem restarts with them
    /// (a new drive level alone is set live), a stopped one starts.
    fn save(&mut self, cmd_tx: &UnboundedSender<GuiCommand>) {
        if self.draft.ui_control && !valid_token(&self.draft.ui_token) {
            self.draft.ui_token = new_token();
        }
        let drive_only = XaraSettings { drive_db: self.settings.drive_db, ..self.draft.clone() } == self.settings;
        let changed = self.draft != self.settings;
        self.settings = self.draft.clone();
        if let Some(f) = &self.feed {
            f.set_port(self.settings.ui_port);
        }
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
            if drive_only {
                xara_ffi::set_drive(f64::from(self.settings.drive_db));
                self.status = format!("TX drive {:.0} dB", self.settings.drive_db);
            } else {
                self.restart_after_stop = true;
                self.stop(cmd_tx);
                self.status = "Restarting with the new settings…".into();
            }
        } else if !self.running() && self.stopping.is_none() {
            self.start_and_connect(cmd_tx);
        }
    }

    /// The tab bar (above the GUI's own menu bar) and, on the Modem tab, the tab. Returns
    /// whether the Openslot tab is showing, i.e. whether the shared GUI should draw.
    pub fn ui(&mut self, ctx: &egui::Context, cmd_tx: &UnboundedSender<GuiCommand>) -> bool {
        self.poll(cmd_tx);
        let feed = self
            .feed
            .get_or_insert_with(|| {
                let c = ctx.clone();
                Feed::spawn(self.settings.ui_port, move || c.request_repaint())
            })
            .clone();
        feed.set_active(self.running());

        // Closing from the Modem tab: back to Openslot first, which asks during a QSO or CQ
        if self.tab == Tab::Modem && ctx.input(|i| i.viewport().close_requested()) {
            self.tab = Tab::Openslot;
        }

        egui::TopBottomPanel::top("xara_modem_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, Tab::Openslot, RichText::new("📻 Openslot").strong());
                if ui.selectable_value(&mut self.tab, Tab::Modem, RichText::new("📡 Modem").strong()).clicked() && !self.devices_listed {
                    self.refresh_devices();
                }
                ui.separator();
                let running = self.running();
                let color = if running { Color32::from_rgb(60, 180, 75) } else { Color32::GRAY };
                let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 5.0, color);
                let status = if self.settings.enabled || running {
                    format!("XARA: {}", self.shown_status(&feed))
                } else {
                    format!("XARA off: Openslot uses the modem on port {}", self.cmd_port)
                };
                ui.label(status);
                if self.tab == Tab::Openslot {
                    if let Some((_, st)) = feed.latest().status.as_ref().filter(|(t, _)| t.elapsed() < Duration::from_secs(5)) {
                        badge(ui, "PTT", st.ptt, Color32::from_rgb(220, 50, 50));
                        badge(ui, "BUSY", st.busy, Color32::from_rgb(230, 170, 30));
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if running {
                        if ui.button("Stop").clicked() {
                            self.stop(cmd_tx);
                        }
                    } else if self.settings.enabled && ui.add_enabled(self.stopping.is_none(), egui::Button::new("Start")).clicked() {
                        self.start_and_connect(cmd_tx);
                    }
                });
            });
        });

        let modem_tab = self.tab == Tab::Modem;
        feed.set_visible(modem_tab && self.page == Page::Modem);
        if modem_tab {
            egui::CentralPanel::default().show(ctx, |ui| self.tab_ui(ui, &feed, cmd_tx));
            // (the feed repaints on each status, ten a second while the modem runs)
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        !modem_tab
    }

    /// The status line: once running on VARA HF, with the bandwidth the modem uses now. It
    /// starts in its Mode's, and Openslot switches it (500 Hz on connecting, as VarAC; the
    /// 500 / 2300 / 2750 buttons).
    fn shown_status(&self, feed: &Feed) -> String {
        let bw = feed
            .latest()
            .status
            .as_ref()
            .filter(|(t, _)| t.elapsed() < Duration::from_secs(5))
            .and_then(|(_, st)| st.bw.parse::<u32>().ok());
        match bw {
            Some(bw) if self.running() && self.starting.is_none() && self.settings.is_hf() => {
                format!("Running VARA HF {bw} Hz as {}", self.call.to_uppercase())
            }
            _ => self.status.clone(),
        }
    }

    fn tab_ui(&mut self, ui: &mut egui::Ui, feed: &Feed, cmd_tx: &UnboundedSender<GuiCommand>) {
        ui.horizontal(|ui| {
            let received = if self.received.is_empty() { "💬 Received".to_string() } else { "💬 Received •".to_string() };
            for (page, label) in [(Page::Modem, "📡 Modem".to_string()), (Page::Received, received), (Page::Log, "☰ Log".into()), (Page::WebUi, "〰 Web UI".into())] {
                ui.selectable_value(&mut self.page, page, RichText::new(label).size(15.0));
            }
        });
        ui.separator();
        match self.page {
            Page::Modem => self.modem_page(ui, feed, cmd_tx),
            Page::Received => self.text_page(ui, true),
            Page::Log => self.text_page(ui, false),
            Page::WebUi => self.web_page(ui),
        }
    }

    fn modem_page(&mut self, ui: &mut egui::Ui, feed: &Feed, cmd_tx: &UnboundedSender<GuiCommand>) {
        let running = self.running();
        let unsaved = self.draft != self.settings;

        // Save stays in view while the settings scroll
        if unsaved {
            egui::Frame::none()
                .fill(Color32::from_rgba_unmultiplied(230, 170, 30, 40))
                .rounding(6.0)
                .inner_margin(egui::Margin::symmetric(10.0, 6.0))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Unsaved modem settings").strong());
                        let drive_only = XaraSettings { drive_db: self.settings.drive_db, ..self.draft.clone() } == self.settings;
                        let label = if !self.draft.enabled {
                            "Save (stops XARA)"
                        } else if running && !drive_only {
                            "Save and restart the modem"
                        } else if running {
                            "Save"
                        } else {
                            "Save and start the modem"
                        };
                        if ui.button(RichText::new(label).strong()).clicked() {
                            self.save(cmd_tx);
                        }
                        if ui.button("Undo").clicked() {
                            self.draft = self.settings.clone();
                        }
                    });
                });
            ui.add_space(4.0);
        }

        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            ui.group(|ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let color = if running { Color32::from_rgb(60, 180, 75) } else { Color32::GRAY };
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
                    ui.painter().circle_filled(rect.center(), 6.0, color);
                    let status = if self.settings.enabled || running {
                        self.shown_status(feed)
                    } else {
                        "Off: Openslot uses VARA or another modem you run".to_string()
                    };
                    ui.label(RichText::new(status).size(15.0));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let label = if running { "Stop" } else { "Start" };
                        let ok = running || (self.settings.enabled && self.stopping.is_none());
                        let b = egui::Button::new(RichText::new(label).size(16.0)).min_size(egui::vec2(90.0, 30.0));
                        let r = ui.add_enabled(ok, b);
                        let r = if self.settings.enabled { r } else { r.on_disabled_hover_text("Choose Built-in XARA below and Save") };
                        if r.clicked() {
                            if running { self.stop(cmd_tx) } else { self.start_and_connect(cmd_tx) }
                        }
                    });
                });
                if running {
                    self.status_panel(ui, feed);
                    let s = &self.settings;
                    note(
                        ui,
                        &format!(
                            "VARA command {}, data {}, KISS {}, web UI {}{}",
                            self.cmd_port,
                            self.cmd_port + 1,
                            self.kiss_port,
                            s.ui_port,
                            if s.lan { " (open to the local network)" } else { "" }
                        ),
                    );
                }
            });

            let d = &mut self.draft;
            let mut refresh = false;
            section(ui, "Modem", |ui| {
                ui.radio_value(&mut d.enabled, true, "Built-in XARA (starts with Openslot)");
                ui.radio_value(&mut d.enabled, false, "VARA, or another modem I run myself");
                note(
                    ui,
                    &if d.enabled {
                        format!("Openslot connects to XARA on this computer, port {} (Settings › Vara).", self.cmd_port)
                    } else {
                        format!(
                            "Start VARA HF (or VARA FM) yourself: Openslot connects to it on port {} (Settings › Vara). \
                             The XARA settings below aren't used.",
                            self.cmd_port
                        )
                    },
                );
            });

            section(ui, "Station", |ui| {
                egui::Grid::new("xara_station").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                    ui.label("Call sign");
                    ui.label(RichText::new(self.call.to_uppercase()).strong())
                        .on_hover_text("Set in Settings › My Information (used from the next launch)");
                    ui.end_row();
                    ui.label("Mode");
                    combo(ui, "xara_mode", &mut d.mode, &MODES);
                    ui.end_row();
                    ui.label("TX drive");
                    ui.add(egui::Slider::new(&mut d.drive_db, -30.0..=0.0).step_by(1.0).suffix(" dB"));
                    ui.end_row();
                });
                ui.checkbox(&mut d.registered, "Registered (I hold a VARA licence)");
                note(
                    ui,
                    "Only with a registered copy of VARA: higher speeds, and links at full speed with other registered stations.",
                );
            });

            section(ui, "Audio and radio", |ui| {
                egui::Grid::new("xara_audio").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                    ui.label("Audio in");
                    device_combo(ui, "xara_in", &mut d.capture, &self.inputs, "Choose…");
                    ui.end_row();
                    ui.label("Audio out");
                    device_combo(ui, "xara_out", &mut d.playback, &self.outputs, "Choose…");
                    ui.end_row();
                });
                ui.checkbox(&mut d.radio, "Radio mode: transmit through a real radio");
                if d.radio {
                    egui::Grid::new("xara_ptt").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                        ui.label("PTT");
                        combo(ui, "xara_ptt", &mut d.ptt, PTTS);
                        ui.end_row();
                        match d.ptt.as_str() {
                            "rts" | "dtr" => {
                                ui.label("Serial port");
                                let shown = match self.serial_ports.iter().find(|p| p.name == d.ptt_serial) {
                                    Some(p) => p.label(),
                                    None if d.ptt_serial.is_empty() => {
                                        if self.serial_ports.is_empty() { "No serial ports found" } else { "Choose…" }.to_string()
                                    }
                                    None => d.ptt_serial.clone(),
                                };
                                egui::ComboBox::from_id_salt("xara_serial").width(360.0).selected_text(shown).show_ui(ui, |ui| {
                                    for p in &self.serial_ports {
                                        ui.selectable_value(&mut d.ptt_serial, p.name.clone(), p.label());
                                    }
                                });
                                ui.end_row();
                            }
                            "cm108" if cfg!(target_os = "linux") => {
                                ui.label("HID device");
                                let devs: Vec<_> = self.hidraw.iter().map(|p| (p.clone(), p.clone())).collect();
                                let none = if devs.is_empty() { "No /dev/hidraw* found" } else { "Choose…" };
                                device_combo(ui, "xara_hidraw", &mut d.ptt_cm108, &devs, none);
                                ui.end_row();
                            }
                            "cm108" => {
                                ui.label("USB id (vendor:product)");
                                ui.text_edit_singleline(&mut d.ptt_cm108);
                                ui.end_row();
                            }
                            "rigctld" => {
                                ui.label("rigctld host:port");
                                ui.text_edit_singleline(&mut d.ptt_rigctld);
                                ui.end_row();
                            }
                            _ => {}
                        }
                        ui.label("PTT lead");
                        ui.add(egui::DragValue::new(&mut d.lead_ms).range(0..=1000).speed(10).suffix(" ms"));
                        ui.end_row();
                        ui.label("PTT tail");
                        ui.add(egui::DragValue::new(&mut d.tail_ms).range(0..=1000).speed(10).suffix(" ms"));
                        ui.end_row();
                    });
                    if matches!(d.ptt.as_str(), "rts" | "dtr") {
                        note(
                            ui,
                            "On a radio with two serial ports (Yaesu FTDX3000, FT-991…) key on the one marked (PTT/keying), \
                             the Standard port, and set the radio's PC keying menu to RTS or DTR to match. Or leave PTT on \
                             VOX here and use CAT PTT in Settings › RIG.",
                        );
                    }
                }
                ui.horizontal(|ui| {
                    ui.label("Audio delay");
                    ui.add(egui::DragValue::new(&mut d.audio_latency_ms).range(0..=1000).speed(50).suffix(" ms"));
                });
                note(
                    ui,
                    "Round-trip delay in the audio path that XARA does not add itself, such as network audio. \
                     Leave at 0 for a radio on a USB interface.",
                );
                if ui.button("Refresh devices").clicked() {
                    refresh = true;
                }
                note(
                    ui,
                    &if d.radio {
                        "XARA keys the radio and transmits into the chosen output. Pick your radio's USB audio interface."
                            .to_string()
                    } else {
                        format!(
                            "Without radio mode XARA only plays into a virtual device ({VIRTUAL_OUT}), so nothing reaches \
                             a transmitter."
                        )
                    },
                );
            });

            section(ui, "Receive", |ui| {
                let hf = d.is_hf();
                ui.add_enabled(hf, egui::Checkbox::new(&mut d.monitor, "Show other stations' traffic"));
                if d.monitor && hf {
                    ui.horizontal(|ui| {
                        ui.label("Show");
                        ui.selectable_value(&mut d.monitor_frames, false, "Text only");
                        ui.selectable_value(&mut d.monitor_frames, true, "Every packet");
                    });
                    ui.horizontal(|ui| {
                        ui.label("Calls to name (optional)");
                        ui.text_edit_singleline(&mut d.monitor_calls);
                    });
                }
                ui.checkbox(&mut d.record_audio, "Record received audio");
                ui.checkbox(&mut d.debug_log, "Keep a debug log");
                if (d.record_audio || d.debug_log) && ui.button("Open folder").clicked() {
                    let _ = open::that_detached(logs_dir());
                }
                note(
                    ui,
                    &format!(
                        "{} Recording saves what the modem hears and sends (about 3 MB a minute); the debug log adds a \
                         timestamped line for every frame, host command and, once a second, the audio levels. Both go to {}.",
                        if hf {
                            "The Received page shows what the modem decodes on your own links. With this on it also decodes \
                             VARA HF traffic between other stations, like VARA's monitor. Answering stations show as #hash \
                             unless their call is listed, separated by spaces."
                        } else {
                            "Showing other stations' traffic is for VARA HF; FM shows your own links only."
                        },
                        logs_dir().display()
                    ),
                );
            });

            section(ui, "Clients", |ui| {
                ui.checkbox(&mut d.lan, "Accept clients on the local network");
                ui.checkbox(&mut d.ui_control, "Web UI can control the radio");
                egui::Grid::new("xara_ports").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                    ui.label("VARA command port (data is +1)");
                    ui.label(self.cmd_port.to_string()).on_hover_text("Set in Settings › Vara");
                    ui.end_row();
                    ui.label("KISS port");
                    ui.label(self.kiss_port.to_string()).on_hover_text("Set in Settings › Vara");
                    ui.end_row();
                    ui.label("Web UI port");
                    ui.add(egui::DragValue::new(&mut d.ui_port).range(1024..=65535));
                    ui.end_row();
                });
                note(
                    ui,
                    &if d.lan {
                        format!(
                            "Clients elsewhere on the network connect to {}:{} (the modem takes one client at a time, and \
                             Openslot is one), and the web UI is at port {}. Anyone on this network can use the modem. A \
                             firewall may need the ports opened.",
                            local_address().unwrap_or_else(|| "this computer".into()),
                            self.cmd_port,
                            d.ui_port
                        )
                    } else {
                        format!(
                            "Only programs on this computer can connect (Openslot on 127.0.0.1:{}). The web UI {}.",
                            self.cmd_port,
                            if d.ui_control { "can tune and change the drive, from its link on the Web UI page" } else { "only shows" }
                        )
                    },
                );
            });

            section(ui, "App", |ui| {
                ui.horizontal(|ui| {
                    ui.label("Command line");
                    if ui.button("Copy").on_hover_text("The same modem, for running xara from a terminal").clicked() {
                        ui.ctx().copy_text(d.command_line(&self.call, self.cmd_port, self.kiss_port));
                    }
                });
                note(ui, &format!("Version: xara {}", xara_version()));
            });
            if refresh {
                self.refresh_devices();
            }
        });
    }

    /// The link as the web UI shows it: state, peer, speed level, bitrate, S/N, levels, and
    /// the waterfall.
    fn status_panel(&mut self, ui: &mut egui::Ui, feed: &Feed) {
        let (latest, rows, span, step) = {
            let mut l = feed.latest();
            let rows: Vec<_> = l.rows.drain(..).collect();
            (l.status.clone(), rows, l.span, l.step)
        };
        self.waterfall.take(&rows, step);
        let Some((_, st)) = latest.filter(|(t, _)| t.elapsed() < Duration::from_secs(5)) else {
            ui.label(RichText::new("Waiting for the modem's status…").weak());
            return;
        };
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let state = if st.state.is_empty() { "Idle" } else { st.state.as_str() };
            ui.label(RichText::new(state).size(18.0).strong());
            if !st.peer.is_empty() {
                ui.label(RichText::new(format!("with {}", st.peer)).size(16.0));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                badge(ui, "CLIP", st.clip, Color32::from_rgb(220, 50, 50));
                badge(ui, "BUSY", st.busy, Color32::from_rgb(230, 170, 30));
                badge(ui, "PTT", st.ptt, Color32::from_rgb(220, 50, 50));
            });
        });
        // (wide enough for each value on one line: "OFDM 49 carriers · QPSK", "1 / 15 retries")
        egui::Grid::new("xara_status").num_columns(4).spacing([24.0, 4.0]).min_col_width(130.0).show(ui, |ui| {
            let dash = || "–".to_string();
            stat(ui, "Speed level", st.level.map_or_else(dash, |l| l.to_string()));
            stat(
                ui,
                "Bitrate",
                st.bps.map_or_else(dash, |b| format!("{b} bps {}", if st.tx { "↑ TX" } else { "↓ RX" })),
            );
            stat(ui, "S/N", st.sn.map_or_else(dash, |v| format!("{v:+.1} dB")));
            stat(ui, "AFC", st.afc.map_or_else(dash, |v| format!("{v:+.1} Hz")));
            ui.end_row();
            stat(ui, "Bandwidth", if st.bw.is_empty() { dash() } else { format!("{} Hz", st.bw) });
            stat(ui, "Modulation", if st.modulation.is_empty() { dash() } else { st.modulation.clone() });
            stat(ui, "Attempts", format!("{} / {} retries", st.attempts, st.retries));
            stat(ui, "Buffer", format!("{} bytes", st.buffer));
            ui.end_row();
        });
        meter(ui, "RX", st.rx_rms);
        meter(ui, "TX", st.tx_rms);
        // Tune: a steady carrier to set the radio's ALC, with the TX drive live
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let (label, fill) = if st.tuning {
                (format!("STOP TUNE ({:.0} s)", st.tune_left.max(0.0)), Color32::from_rgb(200, 50, 50))
            } else {
                ("TUNE".to_string(), Color32::from_rgb(31, 111, 235))
            };
            let b = egui::Button::new(RichText::new(label).strong().color(Color32::WHITE)).fill(fill).min_size(egui::vec2(130.0, 26.0));
            if ui.add(b).on_hover_text("A steady carrier for at most 20 s, to set the radio's ALC and power").clicked() {
                xara_ffi::tune(!st.tuning);
            }
            ui.label("TX drive");
            let mut db = st.drive.map_or(self.settings.drive_db, |d| d as f32);
            if ui.add(egui::Slider::new(&mut db, -30.0..=0.0).step_by(1.0).suffix(" dB")).changed() {
                xara_ffi::set_drive(f64::from(db));
                // (kept for the next start too)
                self.keep_drive(db);
            }
        });
        note(
            ui,
            if self.settings.radio {
                "Tune keys the transmitter: use a clear frequency (not a calling frequency) or a dummy load. \
                 For digital modes turn the drive down until the ALC barely moves."
            } else {
                "Without radio mode the tune carrier only goes into the virtual device."
            },
        );
        ui.add_space(6.0);
        ui.label(RichText::new("Waterfall").strong());
        self.waterfall.show(ui, span, st.passband);
        ui.add_space(4.0);
    }

    fn web_page(&mut self, ui: &mut egui::Ui) {
        let running = self.running();
        let url = self.settings.web_url();
        ui.vertical_centered(|ui| {
            ui.add_space(60.0);
            if running {
                ui.heading("XARA's web UI");
                ui.label("The modem's own page: waterfall, levels, S/N history and the link's events, in your browser.");
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.add_space((ui.available_width() - 260.0).max(0.0) / 2.0);
                    if ui.button(RichText::new("Open in browser").size(16.0)).clicked() {
                        // (detached: a browser that isn't running yet mustn't hold up the window)
                        if let Err(e) = open::that_detached(&url) {
                            self.status = format!("Couldn't open the browser: {e}");
                        }
                    }
                    if ui.button(RichText::new("Copy link").size(16.0)).clicked() {
                        ui.ctx().copy_text(url.clone());
                    }
                });
                ui.add_space(6.0);
                ui.hyperlink_to(&url, &url);
                ui.add_space(10.0);
                note(
                    ui,
                    "If your browser is already open the page may open as a new tab behind this window (GNOME doesn't \
                     bring the browser forward). The Modem page has the same status and waterfall in Openslot.",
                );
                if self.settings.ui_control {
                    note(ui, "This link carries the token that lets the page control the radio: keep it to yourself.");
                }
            } else {
                ui.heading("Modem stopped");
                ui.label("Start the modem to see its web UI.");
            }
        });
    }

    fn text_page(&mut self, ui: &mut egui::Ui, received: bool) {
        ui.horizontal(|ui| {
            if received {
                let what = if self.received.is_empty() {
                    "What the modem decodes. Your QSOs also show in Openslot's chat.".to_string()
                } else {
                    format!("{} characters", self.received.chars().count())
                };
                ui.label(RichText::new(what).weak());
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Clear").clicked() {
                    if received { self.received.clear() } else { self.log.clear() }
                }
                if ui.button("Copy").clicked() {
                    let text = if received { self.received.clone() } else { self.log.iter().cloned().collect::<Vec<_>>().join("\n") };
                    ui.ctx().copy_text(text);
                }
            });
        });
        ui.separator();
        egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink(false).show(ui, |ui| {
            if received {
                if self.received.is_empty() {
                    ui.label(RichText::new("Nothing received yet.").weak());
                } else {
                    ui.add(egui::Label::new(RichText::new(&self.received).monospace()).selectable(true));
                }
            } else {
                for line in &self.log {
                    ui.add(egui::Label::new(RichText::new(line).monospace().size(12.0)).selectable(true));
                }
            }
        });
    }
}

/// The waterfall as the web UI draws it: each row scaled to the noise floor, newest on top.
struct WaterfallView {
    image: egui::ColorImage,
    texture: Option<egui::TextureHandle>,
    dirty: bool,
    /// The noise floor's running estimate, in row steps
    noise: Option<f32>,
    palette: [Color32; 256],
}

impl Default for WaterfallView {
    fn default() -> Self {
        // (the web UI's palette: black, blue, cyan, yellow, orange, white)
        const STOPS: [(f32, [f32; 3]); 7] = [
            (0.0, [0.0, 0.0, 0.0]),
            (0.22, [0.0, 0.0, 120.0]),
            (0.45, [0.0, 120.0, 220.0]),
            (0.62, [0.0, 220.0, 200.0]),
            (0.75, [240.0, 230.0, 0.0]),
            (0.88, [255.0, 70.0, 0.0]),
            (1.0, [255.0, 255.0, 255.0]),
        ];
        let palette = std::array::from_fn(|i| {
            let x = i as f32 / 255.0;
            let k = STOPS.iter().position(|s| s.0 >= x).unwrap_or(STOPS.len() - 1).max(1);
            let ((x0, a), (x1, b)) = (STOPS[k - 1], STOPS[k]);
            let f = (x - x0) / (x1 - x0);
            let c = |j: usize| (a[j] + (b[j] - a[j]) * f).round() as u8;
            Color32::from_rgb(c(0), c(1), c(2))
        });
        Self { image: egui::ColorImage::new([512, WF_ROWS], Color32::BLACK), texture: None, dirty: true, noise: None, palette }
    }
}

impl WaterfallView {
    fn take(&mut self, rows: &[(Vec<u8>, bool)], step: f64) {
        let step = step.max(0.01) as f32;
        for (v, ptt) in rows {
            let n = v.len();
            if n == 0 {
                continue;
            }
            if self.image.size[0] != n {
                self.image = egui::ColorImage::new([n, WF_ROWS], Color32::BLACK);
                self.noise = None;
            }
            let mut sorted = v.clone();
            sorted.sort_unstable();
            let p20 = f32::from(sorted[n / 5]);
            let noise = self.noise.map_or(p20, |m| m * 0.95 + p20 * 0.05);
            self.noise = Some(noise);
            let (lo, range) = (noise - 6.0 / step, 50.0 / step);
            let px = &mut self.image.pixels;
            px.copy_within(0..n * (WF_ROWS - 1), n);
            for (i, &b) in v.iter().enumerate() {
                let k = ((f32::from(b) - lo) / range * 255.0).round().clamp(0.0, 255.0) as usize;
                px[i] = self.palette[k];
            }
            if *ptt {
                // (a red edge on rows sent, as the web UI marks them)
                for p in &mut px[..4.min(n)] {
                    *p = Color32::from_rgb(255, 40, 40);
                }
            }
            self.dirty = true;
        }
    }

    fn show(&mut self, ui: &mut egui::Ui, span: f64, passband: Option<(f64, f64)>) {
        if self.dirty || self.texture.is_none() {
            self.dirty = false;
            match &mut self.texture {
                Some(t) => t.set(self.image.clone(), egui::TextureOptions::LINEAR),
                None => self.texture = Some(ui.ctx().load_texture("xara_waterfall", self.image.clone(), egui::TextureOptions::LINEAR)),
            }
        }
        let Some(tex) = &self.texture else { return };
        let w = ui.available_width();
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, WF_ROWS as f32), egui::Sense::hover());
        let p = ui.painter_at(rect.expand2(egui::vec2(0.0, 18.0)));
        p.image(tex.id(), rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
        let span = span.max(1.0) as f32;
        let x = |hz: f32| rect.left() + hz / span * rect.width();
        // The link's passband, as the web UI's orange marks
        if let Some((lo, hi)) = passband {
            let orange = egui::Stroke::new(1.5_f32, Color32::from_rgb(255, 150, 40));
            for hz in [lo as f32, hi as f32] {
                p.line_segment([egui::pos2(x(hz), rect.top()), egui::pos2(x(hz), rect.bottom())], orange);
            }
            p.line_segment([egui::pos2(x(lo as f32), rect.bottom()), egui::pos2(x(hi as f32), rect.bottom())], orange);
        }
        // An axis every 500 Hz (1 kHz on FM's wider span)
        let every = if span > 4000.0 { 1000.0 } else { 500.0 };
        let mut hz = 0.0;
        while hz <= span {
            let px = x(hz);
            p.line_segment([egui::pos2(px, rect.bottom()), egui::pos2(px, rect.bottom() + 4.0)], egui::Stroke::new(1.0_f32, Color32::GRAY));
            let align = if hz == 0.0 { egui::Align2::LEFT_TOP } else if hz + every > span { egui::Align2::RIGHT_TOP } else { egui::Align2::CENTER_TOP };
            p.text(egui::pos2(px, rect.bottom() + 4.0), align, format!("{hz:.0}"), egui::FontId::proportional(11.0), Color32::GRAY);
            hz += every;
        }
        ui.add_space(18.0);
        if let Some(pos) = resp.hover_pos() {
            let hz = (pos.x - rect.left()) / rect.width() * span;
            resp.on_hover_text(format!("{hz:.0} Hz"));
        }
    }
}

/// The saved audio device in `list` (pairs of name and description): itself, or the one device
/// whose name is the same apart from its digits (see find_audio). None when it isn't there, or
/// when more than one could be it (two radios with the same USB sound chip).
fn moved_device(saved: &str, list: &[(String, String)]) -> Option<String> {
    if list.iter().any(|(n, _)| n == saved) {
        return Some(saved.to_string());
    }
    let shape = |s: &str| s.chars().filter(|c| !c.is_ascii_digit()).collect::<String>();
    let want = shape(saved);
    let mut same = list.iter().filter(|(n, _)| shape(n) == want);
    match (same.next(), same.next()) {
        (Some((n, _)), None) => Some(n.clone()),
        _ => None,
    }
}

/// Serial ports for PTT. Linux: by their /dev/serial/by-id names where they have one, so the PTT
/// port survives the radio being power cycled. macOS: the call-out devices in /dev (no
/// Bluetooth), described as the system says.
fn serial_ports() -> Vec<crate::rig::PortInfo> {
    let mut known = crate::rig::list_serial_ports();
    if cfg!(target_os = "macos") {
        let mut names: Vec<String> = std::fs::read_dir("/dev")
            .map(|d| {
                d.filter_map(|e| e.ok())
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|n| n.starts_with("cu.") && !n.contains("Bluetooth"))
                    .map(|n| format!("/dev/{n}"))
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        return names
            .into_iter()
            .map(|name| {
                known.iter().find(|p| p.name == name).cloned().unwrap_or(crate::rig::PortInfo {
                    name,
                    description: String::new(),
                    cat_port: None,
                    usb: false,
                })
            })
            .collect();
    }
    known.sort_by(|a, b| a.name.cmp(&b.name));
    known
}

/// Linux's CM108 / CM119 devices, /dev/hidraw*, sorted.
fn hidraw_devices() -> Vec<String> {
    if !cfg!(target_os = "linux") {
        return Vec::new();
    }
    let mut v: Vec<String> = std::fs::read_dir("/dev")
        .map(|d| {
            d.flatten()
                .filter(|e| e.file_name().to_string_lossy().starts_with("hidraw"))
                .map(|e| e.path().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// A status pill: lit in `color` when on.
fn badge(ui: &mut egui::Ui, text: &str, on: bool, color: Color32) {
    let (fg, bg) = if on { (Color32::WHITE, color) } else { (Color32::GRAY, Color32::TRANSPARENT) };
    egui::Frame::none()
        .fill(bg)
        .stroke(egui::Stroke::new(1.0_f32, if on { color } else { Color32::GRAY }))
        .rounding(4.0)
        .inner_margin(egui::Margin::symmetric(6.0, 1.0))
        .show(ui, |ui| ui.label(RichText::new(text).color(fg).strong().size(12.0)));
}

/// A caption over a value, one cell of the status grid.
fn stat(ui: &mut egui::Ui, caption: &str, value: String) {
    ui.vertical(|ui| {
        ui.add(egui::Label::new(RichText::new(caption).weak().size(11.0)).extend());
        ui.add(egui::Label::new(RichText::new(value).size(15.0).monospace()).extend());
    });
}

/// An audio level bar, -60…0 dBFS.
fn meter(ui: &mut egui::Ui, label: &str, dbfs: f64) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).monospace());
        let frac = ((dbfs + 60.0) / 60.0).clamp(0.0, 1.0) as f32;
        let text = if dbfs <= -119.0 { "–".to_string() } else { format!("{dbfs:.1} dBFS") };
        ui.add(egui::ProgressBar::new(frac).desired_width(ui.available_width().min(420.0)).text(text));
    });
}

fn section(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(8.0);
    ui.label(RichText::new(title).strong().size(15.0));
    ui.group(|ui| {
        ui.set_width(ui.available_width());
        body(ui);
    });
}

fn note(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).weak().size(12.0));
}

fn combo(ui: &mut egui::Ui, id: &str, value: &mut String, choices: &[(&str, &str)]) {
    let shown = choices.iter().find(|(k, _)| k == value).map_or("", |(_, l)| l);
    egui::ComboBox::from_id_salt(id).selected_text(shown).show_ui(ui, |ui| {
        for (k, l) in choices {
            ui.selectable_value(value, k.to_string(), *l);
        }
    });
}

fn device_combo(ui: &mut egui::Ui, id: &str, value: &mut String, choices: &[(String, String)], none: &str) {
    let shown = choices
        .iter()
        .find(|(k, _)| k == value)
        .map_or_else(|| if value.is_empty() { none.to_string() } else { value.clone() }, |(_, l)| l.clone());
    egui::ComboBox::from_id_salt(id).width(360.0).selected_text(shown).show_ui(ui, |ui| {
        for (k, l) in choices {
            ui.selectable_value(value, k.clone(), l);
        }
    });
}

/// Remote mode's receive audio (crate::remote::audio): captured as the modem captures, from
/// its input unless the Remote control window names another.
pub fn register_remote_audio() {
    use crate::remote::audio::{set_capture, CaptureHooks, Reader, CAPTURE_RATE};
    // (the shared code takes 48 kHz mono, which is what the modem's audio delivers)
    if xara::audio::RATE != CAPTURE_RATE {
        return;
    }
    set_capture(CaptureHooks {
        open: Box::new(|name| {
            let mut a = xara::audio::open_capture(AUDIO.1, name)?;
            Ok(Box::new(move |buf: &mut [i16]| a.read(buf)) as Reader)
        }),
        inputs: Box::new(|| audio_devices().map(|(inputs, _)| inputs).unwrap_or_default()),
        default_input: Box::new(|| {
            let s = XaraSettings::load();
            if s.enabled { s.capture } else { String::new() }
        }),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_carry_the_settings() {
        let s = XaraSettings {
            radio: true,
            ptt: "rts".into(),
            ptt_serial: "/dev/ttyUSB1".into(),
            registered: true,
            monitor: true,
            monitor_calls: "w5abc, k1xyz".into(),
            lan: true,
            ui_control: true,
            ui_token: "0123456789abcdef".into(),
            lead_ms: 30,
            ..Default::default()
        };
        let a = s.arguments("nr5p", 8300, 8100);
        let has = |pair: [&str; 2]| a.windows(2).any(|w| w[0] == pair[0] && w[1] == pair[1]);
        assert!(has(["--call", "NR5P"]) && has(["--hf", "2300"]) && has(["--ptt", "rts:/dev/ttyUSB1"]));
        assert!(has(["--ptt-lead-ms", "30"]) && has(["--ptt-tail-ms", "100"]) && has(["--addr", "0.0.0.0"]));
        assert!(has(["--ui-token", "0123456789abcdef"]) && has(["--monitor-call", "W5ABC"]) && has(["--monitor-call", "K1XYZ"]));
        assert!(a.contains(&"--registered".into()) && a.contains(&"--monitor".into()) && a.contains(&"--ui-control".into()));
        // An FM mode: no monitor; no radio: no PTT; no licence: no registered speeds
        let fm = XaraSettings { mode: "fm-wide".into(), monitor: true, ..Default::default() }.arguments("nr5p", 8300, 8100);
        assert!(!fm.contains(&"--monitor".into()) && !fm.contains(&"--ptt".into()) && !fm.contains(&"--registered".into()));
        assert!(fm.windows(2).any(|w| w[0] == "--addr" && w[1] == "127.0.0.1"));
    }

    #[test]
    fn audio_found_on_another_usb_port() {
        let codec = |loc: &str| (format!("AppleUSBAudioEngine:Burr-Brown from TI:USB Audio CODEC:{loc}:2"), "USB Audio CODEC".to_string());
        let mic = ("BuiltInMicrophoneDevice".to_string(), "MacBook Air Microphone".to_string());
        let old = codec("142000").0;
        // still there, moved, gone, and two of them (can't tell which)
        assert_eq!(moved_device(&old, &[mic.clone(), codec("142000")]), Some(old.clone()));
        assert_eq!(moved_device(&old, &[mic.clone(), codec("1112000")]), Some(codec("1112000").0));
        assert_eq!(moved_device(&old, &[mic.clone()]), None);
        assert_eq!(moved_device(&old, &[codec("1112000"), codec("1113000")]), None);
        // Windows numbers the endpoint
        let win = |n: &str| (format!("Microphone ({n}- USB Audio CODEC )"), String::new());
        assert_eq!(moved_device(&win("2").0, &[win("3")]), Some(win("3").0));
    }

    #[test]
    fn an_older_xara_toml_still_loads() {
        let old = "enabled = true\nmode = \"hf500\"\nregistered = true\ndrive_db = -1.0\n";
        let s: XaraSettings = toml::from_str(old).unwrap();
        assert!(s.registered && s.mode == "hf500" && s.drive_db == -1.0);
        assert_eq!((s.lead_ms, s.tail_ms, s.lan, s.monitor), (0, 100, false, false));
    }

    #[test]
    fn tokens() {
        assert!(valid_token(&new_token()) && new_token().len() == 32);
        assert!(!valid_token("short") && !valid_token("has space 1234"));
    }

    /// Each page of the Modem tab, running with a link up, at the app's smallest window: no
    /// widget overlaps another or runs off the window's side.
    #[test]
    fn the_tab_fits_the_smallest_window() {
        let (w, h) = (1000.0, 600.0);
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut m = BuiltInModem::new("NR5P", 8300, 8100);
        m.settings = XaraSettings { radio: true, ptt: "rts".into(), monitor: true, debug_log: true, ..Default::default() };
        m.draft = XaraSettings { lan: true, ..m.settings.clone() };
        m.tab = Tab::Modem;
        m.received = "NR5P de W5ABC hello\n".into();
        m.log.push_back("xara: something".into());
        let feed = Feed::spawn(1, || {});
        let st = r#"{"t":"status","rx":{"rms":-30,"clip":0},"tx":{"rms":-120,"clip":0},"ptt":false,"busy":true,"state":"Receiving","level":7,"peer":"W5ABC","bw":"2300","passband":[350,2650],"attempts":2,"retries":1,"afc":-1.5,"sn":11.25,"bitrate":{"level":7,"bps":2412,"tx":false,"modulation":"OFDM 49 carriers · QPSK"},"buffer":120,"drive":-5}"#;
        feed.latest().status = Some((Instant::now(), feed::Status::parse(&serde_json::from_str(st).unwrap()).unwrap()));
        feed.latest().rows.push_back((vec![10u8; 512], false));
        m.feed = Some(feed.clone());
        let ctx = egui::Context::default();
        let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w, h))), ..Default::default() };
        for page in [Page::Modem, Page::Received, Page::Log, Page::WebUi] {
            m.page = page;
            for _ in 0..4 {
                let _ = ctx.run(input.clone(), |ctx| {
                    egui::TopBottomPanel::top("xara_modem_bar").show(ctx, |ui| ui.label("tabs"));
                    egui::CentralPanel::default().show(ctx, |ui| m.tab_ui(ui, &feed, &tx));
                });
            }
            let rects: Vec<egui::Rect> = ctx.viewport(|vp| {
                let widgets = &vp.prev_pass.widgets;
                let layers: Vec<_> = widgets.layer_ids().collect();
                layers
                    .into_iter()
                    .flat_map(|l| widgets.get_layer(l).cloned().collect::<Vec<_>>())
                    .filter(|r| r.sense.interactive() && r.interact_rect.is_positive())
                    .map(|r| r.interact_rect)
                    .collect()
            });
            assert!(rects.len() > 3);
            for (i, a) in rects.iter().enumerate() {
                assert!(a.max.x <= w + 0.5, "a widget runs off the window's side: {a:?}");
                for b in &rects[i + 1..] {
                    let o = a.intersect(*b);
                    // (one inside the other is a widget and its parts, or the scroll area and a row
                    // scrolled part way past its edge)
                    let nested = a.expand(4.0).contains_rect(*b) || b.expand(4.0).contains_rect(*a);
                    assert!(nested || !(o.is_positive() && o.area() > 4.0), "widgets overlap: {a:?} and {b:?}");
                }
            }
        }
    }

    #[test]
    fn waterfall_rows_scroll_down() {
        let mut w = WaterfallView::default();
        w.take(&[(vec![0u8; 512], false), (vec![200u8; 512], true)], 0.5);
        // newest on top, with the red transmit edge
        assert_eq!(w.image.pixels[0], Color32::from_rgb(255, 40, 40));
        assert_ne!(w.image.pixels[10], w.image.pixels[512 + 10]);
        assert_eq!(w.palette[0], Color32::BLACK);
        assert_eq!(w.palette[255], Color32::WHITE);
    }
}
