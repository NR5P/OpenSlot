//! The station side of remote mode: serves the phone page and remote sessions, publishes
//! the station's state to them, and hands their commands to the GUI (which carries them
//! out as its own buttons would). See mod.rs for the protocol.

use std::collections::HashMap;
use std::io;
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::BufReader;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, watch};

use super::audio::AudioHub;
use super::screen::ScreenInput;
use super::ws::{self, Message};
use super::{ChatItem, ClientMsg, Command, RigView, ServerMsg, Stations, Status, PROTOCOL};

/// The phone page (and any browser's).
const WEB_PAGE: &str = include_str!("web.html");
/// At most this many remotes at once.
const MAX_SESSIONS: usize = 8;
/// Wrong passwords from one address before it's locked out, and for how long.
const MAX_FAILURES: u32 = 5;
const LOCKOUT: Duration = Duration::from_secs(60);
/// A remote that says nothing (it pings every 5 s) for this long is gone.
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);
/// A remote that takes nothing in for this long (a phone asleep, gone from the Wi-Fi) is
/// gone too: a write waiting on it would hold the session, and its slot, forever.
const WRITE_TIMEOUT: Duration = if cfg!(test) { Duration::from_secs(2) } else { Duration::from_secs(10) };

/// One message to a remote, given up on when it isn't taken in time.
async fn put<W: tokio::io::AsyncWrite + Unpin>(w: &mut W, m: &Message) -> io::Result<()> {
    tokio::time::timeout(WRITE_TIMEOUT, ws::write_message(w, m, false))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "the remote isn't reading"))?
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerConfig {
    pub port: u16,
    pub password: String,
    /// Shown to remotes (the call sign)
    pub name: String,
    /// The radio's audio input ("" = the built-in modem's)
    pub audio_device: String,
}

struct Shared {
    config: ServerConfig,
    version: String,
    id: String,
    status: watch::Sender<Option<Arc<str>>>,
    chat: watch::Sender<Option<Arc<str>>>,
    stations: watch::Sender<Option<Arc<str>>>,
    rig: watch::Sender<Option<Arc<str>>>,
    notices: broadcast::Sender<Arc<str>>,
    commands: std::sync::mpsc::Sender<(String, Command)>,
    sessions: AtomicUsize,
    audio: AudioHub,
    failures: Mutex<HashMap<IpAddr, (u32, Instant)>>,
    stop: AtomicBool,
    /// Remotes watching the station's screen: their input, and where their frames go
    screens: Mutex<HashMap<u64, ScreenSlot>>,
    next_session: AtomicU64,
}

struct ScreenSlot {
    /// The session it's for (a remote that starts its screen again gets a new slot)
    owner: u64,
    input: Vec<ScreenInput>,
    frames: tokio::sync::mpsc::Sender<Vec<u8>>,
}

/// Input a remote may queue between two of the station's passes (a stalled station
/// can't be made to hold an endless backlog).
const MAX_QUEUED_INPUT: usize = 64;

/// A running station server; stops when dropped.
pub struct RemoteServer {
    shared: Arc<Shared>,
    commands: std::sync::mpsc::Receiver<(String, Command)>,
    shutdown: watch::Sender<bool>,
    /// The listener and discovery have let go of their ports (so a restart can take them)
    done: std::sync::mpsc::Receiver<()>,
    discovery: Option<std::thread::JoinHandle<()>>,
    pub port: u16,
}

impl RemoteServer {
    pub fn start(rt: &tokio::runtime::Handle, config: ServerConfig) -> io::Result<Self> {
        if config.password.trim().is_empty() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "set a password first: a remote can transmit"));
        }
        let std_listener = bind_port(config.port)?;
        std_listener.set_nonblocking(true)?;
        let port = std_listener.local_addr()?.port();
        let (tx, commands) = std::sync::mpsc::channel();
        let (shutdown, shutdown_rx) = watch::channel(false);
        let shared = Arc::new(Shared {
            audio: AudioHub::new(&config.audio_device),
            config: ServerConfig { port, ..config },
            version: env!("CARGO_PKG_VERSION").to_string(),
            id: uuid::Uuid::new_v4().simple().to_string()[..16].to_string(),
            status: watch::channel(None).0,
            chat: watch::channel(None).0,
            stations: watch::channel(None).0,
            rig: watch::channel(None).0,
            notices: broadcast::channel(32).0,
            commands: tx,
            sessions: AtomicUsize::new(0),
            failures: Mutex::new(HashMap::new()),
            stop: AtomicBool::new(false),
            screens: Mutex::new(HashMap::new()),
            next_session: AtomicU64::new(1),
        });
        let listener = {
            let _guard = rt.enter();
            TcpListener::from_std(std_listener)?
        };
        let sh = shared.clone();
        let (done_tx, done) = std::sync::mpsc::channel();
        rt.spawn(async move {
            let mut shutdown_rx = shutdown_rx;
            loop {
                tokio::select! {
                    r = listener.accept() => match r {
                        Ok((stream, addr)) => {
                            let _ = stream.set_nodelay(true);
                            let (sh, stop) = (sh.clone(), shutdown_rx.clone());
                            tokio::spawn(async move {
                                if let Err(e) = serve(stream, addr.ip(), sh, stop).await {
                                    tracing::debug!("remote {addr}: {e}");
                                }
                            });
                        }
                        Err(e) => {
                            tracing::warn!("remote server: {e}");
                            tokio::time::sleep(Duration::from_millis(500)).await;
                        }
                    },
                    _ = shutdown_rx.changed() => break,
                }
            }
            drop(listener);
            let _ = done_tx.send(());
        });
        let discovery = spawn_discovery(shared.clone());
        tracing::info!("remote server listening on port {port}");
        Ok(Self { shared, commands, shutdown, done, discovery, port })
    }

    /// Remotes connected now.
    pub fn sessions(&self) -> usize {
        self.shared.sessions.load(Ordering::Relaxed)
    }

    pub fn audio_listeners(&self) -> usize {
        self.shared.audio.listeners()
    }

    pub fn audio_available(&self) -> bool {
        self.shared.audio.available()
    }

    pub fn audio_note(&self) -> String {
        self.shared.audio.note()
    }

    /// Commands remotes sent since the last call, with who sent them (their address).
    pub fn commands(&self) -> Vec<(String, Command)> {
        self.commands.try_iter().collect()
    }

    pub fn publish_status(&self, s: &Status) {
        publish(&self.shared.status, &ServerMsg::Status(s.clone()));
    }

    pub fn publish_chat(&self, lines: Vec<ChatItem>) {
        publish(&self.shared.chat, &ServerMsg::Chat { lines });
    }

    pub fn publish_stations(&self, s: Stations) {
        publish(&self.shared.stations, &ServerMsg::Stations(s));
    }

    pub fn publish_rig(&self, r: &RigView) {
        publish(&self.shared.rig, &ServerMsg::Rig(r.clone()));
    }

    /// The remotes watching the station's screen, with the input each sent since the last call.
    pub fn screens(&self) -> Vec<(u64, Vec<ScreenInput>)> {
        let Ok(mut s) = self.shared.screens.lock() else { return Vec::new() };
        s.iter_mut().map(|(id, slot)| (*id, std::mem::take(&mut slot.input))).collect()
    }

    /// A frame of the screen for one remote. False when it hasn't taken the last ones yet
    /// (or is gone): the frame isn't sent.
    pub fn send_screen(&self, id: u64, frame: Vec<u8>) -> bool {
        let Ok(s) = self.shared.screens.lock() else { return false };
        s.get(&id).is_some_and(|slot| slot.frames.try_send(frame).is_ok())
    }

    /// A message for every remote ("Modem not connected", a refused command…).
    pub fn notice(&self, text: impl Into<String>) {
        let _ = self.shared.notices.send(Arc::from(json(&ServerMsg::Notice { text: text.into() })));
    }
}

impl Drop for RemoteServer {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        let _ = self.shutdown.send(true);
        let _ = self.done.recv_timeout(Duration::from_secs(1));
        if let Some(t) = self.discovery.take() {
            let _ = t.join();
        }
    }
}

fn json(msg: &ServerMsg) -> String {
    serde_json::to_string(msg).unwrap_or_default()
}

/// Sends only what changed (the GUI publishes every tick).
fn publish(tx: &watch::Sender<Option<Arc<str>>>, msg: &ServerMsg) {
    let text = json(msg);
    tx.send_if_modified(|cur| {
        if cur.as_deref() == Some(text.as_str()) {
            return false;
        }
        *cur = Some(Arc::from(text));
        true
    });
}

/// One TCP connection: the phone page, or a WebSocket session.
async fn serve(stream: TcpStream, ip: IpAddr, sh: Arc<Shared>, mut stop: watch::Receiver<bool>) -> io::Result<()> {
    let (r, mut w) = stream.into_split();
    let mut r = BufReader::new(r);
    let req = tokio::time::timeout(Duration::from_secs(10), ws::read_head(&mut r))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "no request"))??;
    if !req.is_upgrade() {
        return match (req.method.as_str(), req.path.split('?').next().unwrap_or("")) {
            ("GET", "/" | "/index.html") => ws::respond(&mut w, "200 OK", "text/html; charset=utf-8", WEB_PAGE.as_bytes()).await,
            ("GET", "/manifest.json") => {
                let m = format!(r##"{{"name":"Openslot {}","short_name":"Openslot","start_url":"/","display":"standalone","background_color":"#0d1017","theme_color":"#0d1017"}}"##, sh.config.name);
                ws::respond(&mut w, "200 OK", "application/manifest+json", m.as_bytes()).await
            }
            _ => ws::respond(&mut w, "404 Not Found", "text/plain", b"Not found").await,
        };
    }
    ws::accept(&mut w, &req).await?;
    let send = |msg: ServerMsg| Message::Text(json(&msg));

    let full = || ServerMsg::Denied { reason: "too many remotes connected".into(), retry: true };
    let locked_out = || ServerMsg::Denied { reason: "too many wrong passwords: try again in a minute".into(), retry: true };
    if sh.sessions.load(Ordering::SeqCst) >= MAX_SESSIONS {
        put(&mut w, &send(full())).await?;
        return Ok(());
    }
    if locked(&sh, ip) {
        put(&mut w, &send(locked_out())).await?;
        return Ok(());
    }

    // Challenge: the password's proof for a fresh nonce
    let nonce = uuid::Uuid::new_v4().simple().to_string()[..16].to_string();
    let hello = ServerMsg::Hello { protocol: PROTOCOL, name: sh.config.name.clone(), nonce: nonce.clone(), version: sh.version.clone() };
    put(&mut w, &send(hello)).await?;
    let first = tokio::time::timeout(Duration::from_secs(20), ws::read_message(&mut r))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "no password"))??;
    let (proof, client) = match first {
        Message::Text(t) => match serde_json::from_str::<ClientMsg>(&t) {
            Ok(ClientMsg::Auth { proof, client }) => (proof, client),
            // (anything else first isn't a guess at the password: no strike for it)
            _ => {
                put(&mut w, &send(ServerMsg::Denied { reason: "log in first".into(), retry: true })).await?;
                return Ok(());
            }
        },
        _ => return Ok(()),
    };
    match check_proof(&sh, ip, &nonce, &proof) {
        Proof::Right => {}
        Proof::Locked => {
            put(&mut w, &send(locked_out())).await?;
            return Ok(());
        }
        Proof::Wrong => {
            tokio::time::sleep(Duration::from_millis(500)).await;
            put(&mut w, &send(ServerMsg::Denied { reason: "wrong password".into(), retry: false })).await?;
            return Ok(());
        }
    }
    let who = if client.is_empty() { ip.to_string() } else { format!("{client} at {ip}") };
    tracing::info!("remote connected: {who}");

    // (a slot taken only now, and only while one is free: many logging in at once can't
    // all slip past the check before the counter moves)
    if !take_slot(&sh.sessions) {
        put(&mut w, &send(full())).await?;
        return Ok(());
    }
    let session = sh.next_session.fetch_add(1, Ordering::SeqCst);
    struct Leave<'a>(&'a Shared, bool, u64);
    impl Drop for Leave<'_> {
        fn drop(&mut self) {
            self.0.sessions.fetch_sub(1, Ordering::SeqCst);
            if self.1 {
                self.0.audio.unsubscribe();
            }
            if let Ok(mut s) = self.0.screens.lock() {
                s.retain(|_, slot| slot.owner != self.2);
            }
        }
    }
    let mut leave = Leave(&sh, false, session);
    // Frames of the screen, once this remote asks for it
    let mut screen: Option<tokio::sync::mpsc::Receiver<Vec<u8>>> = None;
    let mut screen_key: Option<u64> = None;

    put(&mut w, &send(ServerMsg::Welcome { audio: sh.audio.available(), screen: true })).await?;
    let mut topics = [sh.status.subscribe(), sh.chat.subscribe(), sh.stations.subscribe(), sh.rig.subscribe()];
    for t in topics.iter_mut() {
        let cur = t.borrow_and_update().clone();
        if let Some(text) = cur {
            put(&mut w, &Message::Text(text.to_string())).await?;
        }
    }
    let mut notices = sh.notices.subscribe();
    let mut audio: Option<broadcast::Receiver<Arc<Vec<u8>>>> = None;

    // Frames are read on a task of their own (a read cut short by select! would lose data)
    let (in_tx, mut incoming) = tokio::sync::mpsc::channel::<io::Result<Message>>(32);
    let reader = tokio::spawn(async move {
        loop {
            let m = ws::read_message(&mut r).await;
            let end = m.is_err() || matches!(m, Ok(Message::Close));
            if in_tx.send(m).await.is_err() || end {
                break;
            }
        }
    });
    struct Abort(tokio::task::JoinHandle<()>);
    impl Drop for Abort {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    let _reader = Abort(reader);

    let [status, chat, stations, rig] = &mut topics;
    let mut last_heard = Instant::now();
    let mut check = tokio::time::interval(Duration::from_secs(5));
    loop {
        let out: Option<Message> = tokio::select! {
            m = incoming.recv() => {
                last_heard = Instant::now();
                match m {
                    Some(Ok(Message::Text(t))) => match serde_json::from_str::<ClientMsg>(&t) {
                        Ok(ClientMsg::Ping) => Some(send(ServerMsg::Pong)),
                        Ok(ClientMsg::Cmd(Command::Audio { on })) => {
                            if on && audio.is_none() {
                                if sh.audio.available() {
                                    audio = Some(sh.audio.subscribe());
                                    leave.1 = true;
                                    None
                                } else {
                                    Some(send(ServerMsg::Notice { text: format!("No audio: {}", sh.audio.note()) }))
                                }
                            } else {
                                if !on && audio.take().is_some() {
                                    sh.audio.unsubscribe();
                                    leave.1 = false;
                                }
                                None
                            }
                        }
                        Ok(ClientMsg::Cmd(cmd)) => {
                            let _ = sh.commands.send((who.clone(), cmd));
                            None
                        }
                        Ok(ClientMsg::Screen(input)) => {
                            if let Ok(mut s) = sh.screens.lock() {
                                match screen_key.and_then(|k| s.get_mut(&k)) {
                                    Some(slot) if slot.input.len() < MAX_QUEUED_INPUT => slot.input.push(input),
                                    Some(_) => {}
                                    None => {
                                        let (tx, rx) = tokio::sync::mpsc::channel(2);
                                        let key = sh.next_session.fetch_add(1, Ordering::SeqCst);
                                        s.insert(key, ScreenSlot { owner: session, input: vec![input], frames: tx });
                                        (screen, screen_key) = (Some(rx), Some(key));
                                    }
                                }
                            }
                            None
                        }
                        Ok(ClientMsg::ScreenOff) => {
                            if let (Ok(mut s), Some(k)) = (sh.screens.lock(), screen_key.take()) {
                                s.remove(&k);
                            }
                            screen = None;
                            None
                        }
                        Ok(ClientMsg::Auth { .. }) => None,
                        Err(e) => Some(send(ServerMsg::Notice { text: format!("not understood: {e}") })),
                    },
                    Some(Ok(Message::Ping(p))) => Some(Message::Pong(p)),
                    Some(Ok(Message::Pong(_) | Message::Binary(_))) => None,
                    Some(Ok(Message::Close)) | Some(Err(_)) | None => break,
                }
            }
            r = status.changed() => { r.map_err(|_| closed())?; status.borrow_and_update().clone().map(|t| Message::Text(t.to_string())) }
            r = chat.changed() => { r.map_err(|_| closed())?; chat.borrow_and_update().clone().map(|t| Message::Text(t.to_string())) }
            r = stations.changed() => { r.map_err(|_| closed())?; stations.borrow_and_update().clone().map(|t| Message::Text(t.to_string())) }
            r = rig.changed() => { r.map_err(|_| closed())?; rig.borrow_and_update().clone().map(|t| Message::Text(t.to_string())) }
            n = notices.recv() => match n {
                Ok(t) => Some(Message::Text(t.to_string())),
                Err(broadcast::error::RecvError::Lagged(_)) => None,
                Err(_) => break,
            },
            f = async { match screen.as_mut() { Some(s) => s.recv().await, None => std::future::pending().await } } => match f {
                Some(frame) => Some(Message::Binary(frame)),
                None => {
                    screen = None;
                    None
                }
            },
            f = async { match audio.as_mut() { Some(a) => a.recv().await, None => std::future::pending().await } } => match f {
                Ok(frame) => Some(Message::Binary(frame.to_vec())),
                // (a slow link drops audio, never the session)
                Err(broadcast::error::RecvError::Lagged(_)) => None,
                Err(_) => {
                    audio = None;
                    None
                }
            },
            _ = check.tick() => {
                if last_heard.elapsed() > IDLE_TIMEOUT {
                    break;
                }
                None
            }
            _ = stop.changed() => {
                // (only for now: Apply in the Remote control window restarts it at once)
                let _ = put(&mut w, &send(ServerMsg::Denied { reason: "the station stopped remote control".into(), retry: true })).await;
                break;
            }
        };
        if let Some(m) = out {
            put(&mut w, &m).await?;
        }
    }
    tracing::info!("remote disconnected: {who}");
    let _ = put(&mut w, &Message::Close).await;
    Ok(())
}

/// Binds the port. Just after a stop (Apply restarts the server) the old listener may still
/// be letting go of it, so a busy port is tried again for a moment before giving up.
fn bind_port(port: u16) -> io::Result<std::net::TcpListener> {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match std::net::TcpListener::bind(("0.0.0.0", port)) {
            Err(e) if e.kind() == io::ErrorKind::AddrInUse && port != 0 && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
            r => return r,
        }
    }
}

/// One more remote, if there's room for it.
fn take_slot(sessions: &AtomicUsize) -> bool {
    let mut n = sessions.load(Ordering::SeqCst);
    while n < MAX_SESSIONS {
        match sessions.compare_exchange(n, n + 1, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => return true,
            Err(now) => n = now,
        }
    }
    false
}

/// Locked out: too many wrong passwords from this address lately.
fn locked(sh: &Shared, ip: IpAddr) -> bool {
    sh.failures.lock().ok().and_then(|f| f.get(&ip).copied()).is_some_and(|(n, at)| n >= MAX_FAILURES && at.elapsed() < LOCKOUT)
}

enum Proof {
    Right,
    Wrong,
    Locked,
}

/// Checks a password proof. The lockout is looked at, the proof checked and a wrong one
/// counted under one lock, so guesses sent on many connections at once are held to
/// MAX_FAILURES a minute like guesses one after another.
fn check_proof(sh: &Shared, ip: IpAddr, nonce: &str, proof: &str) -> Proof {
    let Ok(mut f) = sh.failures.lock() else { return Proof::Wrong };
    // (old entries go, so the map doesn't keep every address that ever mistyped)
    f.retain(|_, (_, at)| at.elapsed() < LOCKOUT);
    if f.get(&ip).is_some_and(|&(n, _)| n >= MAX_FAILURES) {
        return Proof::Locked;
    }
    if super::proof_ok(&sh.config.password, nonce, proof) {
        f.remove(&ip);
        return Proof::Right;
    }
    let e = f.entry(ip).or_insert((0, Instant::now()));
    e.0 += 1;
    e.1 = Instant::now();
    Proof::Wrong
}

fn closed() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "server stopped")
}

/// Answers discovery probes on the LAN (see super::discover).
fn spawn_discovery(sh: Arc<Shared>) -> Option<std::thread::JoinHandle<()>> {
    let sock = match std::net::UdpSocket::bind(("0.0.0.0", super::DISCOVERY_PORT)) {
        Ok(s) => s,
        Err(e) => {
            // (another Openslot on this computer has it: this one can still be reached by address)
            tracing::info!("remote discovery not available: {e}");
            return None;
        }
    };
    let _ = sock.set_read_timeout(Some(Duration::from_millis(300)));
    std::thread::Builder::new().name("remote-discovery".into()).spawn(move || {
        let mut buf = [0u8; 64];
        while !sh.stop.load(Ordering::SeqCst) {
            let Ok((n, from)) = sock.recv_from(&mut buf) else { continue };
            if &buf[..n] == super::PROBE {
                let text = super::beacon_text(&sh.id, &sh.config.name, sh.config.port, sh.audio.available());
                let _ = sock.send_to(text.as_bytes(), from);
            }
        }
    }).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::client::{ClientConfig, Phase, RemoteClient};

    fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !f() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// A station and a remote over the loopback: the password, state one way, commands
    /// the other, and the phone page.
    #[test]
    fn station_and_remote() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let cfg = ServerConfig { port: 0, password: "pa55".into(), name: "NR5P".into(), audio_device: String::new() };
        let server = RemoteServer::start(rt.handle(), cfg).unwrap();
        let port = server.port;
        let none: Arc<dyn Fn() + Send + Sync> = Arc::new(|| {});

        let wrong = RemoteClient::connect(rt.handle(), ClientConfig { host: "127.0.0.1".into(), port, password: "nope".into() }, none.clone());
        wait_for("the wrong password refused", || matches!(wrong.view().phase, Phase::Denied(_)));
        assert_eq!(wrong.view().phase, Phase::Denied("wrong password".into()));

        server.publish_status(&Status { callsign: "NR5P".into(), frequency: 14_105_000, ..Default::default() });
        let remote = RemoteClient::connect(rt.handle(), ClientConfig { host: "127.0.0.1".into(), port, password: "pa55".into() }, none);
        wait_for("connected", || remote.view().phase == Phase::Connected);
        wait_for("the status", || remote.view().status.is_some_and(|s| s.frequency == 14_105_000));
        assert_eq!(remote.view().name, "NR5P");
        assert_eq!(server.sessions(), 1);

        // a change reaches it, an unchanged one isn't sent again
        server.publish_chat(vec![ChatItem { call: "W5TST".into(), text: "hello".into(), ..Default::default() }]);
        wait_for("the chat", || remote.view().chat.len() == 1);

        remote.send(Command::Chat { text: "hi there".into() });
        remote.send(Command::Frequency { hz: 7_105_000 });
        let mut got = Vec::new();
        wait_for("the commands", || {
            got.extend(server.commands().into_iter().map(|(_, c)| c));
            got.len() >= 2
        });
        assert_eq!(got, vec![Command::Chat { text: "hi there".into() }, Command::Frequency { hz: 7_105_000 }]);

        // no audio input here: listening is refused politely
        remote.send(Command::Audio { on: true });
        wait_for("the notice", || !remote.view().notices.is_empty());

        // the phone page
        let page = rt.block_on(async {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
            s.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n").await.unwrap();
            let mut out = String::new();
            s.read_to_string(&mut out).await.unwrap();
            out
        });
        assert!(page.starts_with("HTTP/1.1 200 OK") && page.contains("<title>Openslot"), "{page}");

        drop(remote);
        wait_for("the session to end", || server.sessions() == 0);
    }

    /// A raw session, as the phone page talks: the password, then the welcome (or not).
    async fn raw_login(port: u16, password: &str) -> (ServerMsg, BufReader<tokio::net::tcp::OwnedReadHalf>, tokio::net::tcp::OwnedWriteHalf) {
        let s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let (r, mut w) = s.into_split();
        let mut r = BufReader::new(r);
        ws::connect(&mut r, &mut w, "localhost", "/ws").await.unwrap();
        let next = |m: Message| match m {
            Message::Text(t) => serde_json::from_str::<ServerMsg>(&t).unwrap(),
            other => panic!("expected text, got {other:?}"),
        };
        let hello = next(ws::read_message(&mut r).await.unwrap());
        let ServerMsg::Hello { nonce, .. } = hello else { return (hello, r, w) };
        let auth = ClientMsg::Auth { proof: crate::remote::proof(password, &nonce), client: "test".into() };
        ws::write_message(&mut w, &Message::Text(serde_json::to_string(&auth).unwrap()), true).await.unwrap();
        let answer = next(ws::read_message(&mut r).await.unwrap());
        (answer, r, w)
    }

    /// Five wrong passwords lock the address out for a minute, even with the right one.
    #[test]
    fn wrong_passwords_lock_out() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let cfg = ServerConfig { port: 0, password: "right".into(), name: "NR5P".into(), audio_device: String::new() };
        let srv = RemoteServer::start(rt.handle(), cfg).unwrap();
        rt.block_on(async {
            assert!(matches!(raw_login(srv.port, "right").await.0, ServerMsg::Welcome { .. }));
            for _ in 0..MAX_FAILURES {
                assert_eq!(raw_login(srv.port, "wrong").await.0, ServerMsg::Denied { reason: "wrong password".into(), retry: false });
            }
            assert!(matches!(raw_login(srv.port, "right").await.0, ServerMsg::Denied { reason, retry: true } if reason.contains("too many")));
        });
    }

    /// Guesses sent on many connections at once are held to MAX_FAILURES like guesses one
    /// after another: the rest are refused unchecked, the right password included.
    #[test]
    fn parallel_guesses_are_locked_out_too() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let cfg = ServerConfig { port: 0, password: "right".into(), name: "NR5P".into(), audio_device: String::new() };
        let srv = RemoteServer::start(rt.handle(), cfg).unwrap();
        let port = srv.port;
        rt.block_on(async {
            let mut tries = Vec::new();
            for i in 0..30 {
                tries.push(tokio::spawn(async move { raw_login(port, if i == 29 { "right" } else { "wrong" }).await.0 }));
            }
            let mut checked = 0;
            for t in tries {
                match t.await.unwrap() {
                    ServerMsg::Denied { retry: false, .. } => checked += 1,
                    ServerMsg::Denied { retry: true, .. } => {}
                    ServerMsg::Welcome { .. } => {} // (the right one may come before the fifth wrong one)
                    other => panic!("{other:?}"),
                }
            }
            assert_eq!(checked, MAX_FAILURES as usize, "wrong passwords checked");
            assert!(matches!(raw_login(port, "right").await.0, ServerMsg::Denied { retry: true, .. }));
        });
    }

    /// Many remotes logging in at once still get no more than MAX_SESSIONS slots.
    #[test]
    fn parallel_logins_keep_to_the_limit() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let cfg = ServerConfig { port: 0, password: "pw".into(), name: "NR5P".into(), audio_device: String::new() };
        let srv = RemoteServer::start(rt.handle(), cfg).unwrap();
        let port = srv.port;
        let held = rt.block_on(async {
            let tries: Vec<_> = (0..MAX_SESSIONS + 4).map(|_| tokio::spawn(async move { raw_login(port, "pw").await })).collect();
            let mut held = Vec::new();
            let mut full = 0;
            for t in tries {
                let (m, r, w) = t.await.unwrap();
                match m {
                    ServerMsg::Welcome { .. } => held.push((r, w)),
                    ServerMsg::Denied { retry: true, reason } if reason.contains("too many remotes") => full += 1,
                    other => panic!("{other:?}"),
                }
            }
            assert_eq!((held.len(), full), (MAX_SESSIONS, 4));
            held
        });
        assert_eq!(srv.sessions(), MAX_SESSIONS);
        drop(held);
    }

    /// A remote that stops taking anything in (a phone asleep) is let go, rather than
    /// holding its session and slot for good.
    #[test]
    fn a_remote_that_stops_reading_is_let_go() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let cfg = ServerConfig { port: 0, password: "pw".into(), name: "NR5P".into(), audio_device: String::new() };
        let srv = RemoteServer::start(rt.handle(), cfg).unwrap();
        let (m, r, w) = rt.block_on(raw_login(srv.port, "pw"));
        assert!(matches!(m, ServerMsg::Welcome { .. }));
        wait_for("the session", || srv.sessions() == 1);
        // (it keeps pinging, so only the writes can tell it's gone)
        let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel::<()>();
        let pinger = rt.spawn(async move {
            let mut w = w;
            let ping = Message::Text(serde_json::to_string(&ClientMsg::Ping).unwrap());
            loop {
                tokio::select! {
                    _ = &mut stop_rx => return,
                    _ = tokio::time::sleep(Duration::from_millis(500)) => {
                        if ws::write_message(&mut w, &ping, true).await.is_err() {
                            return;
                        }
                    }
                }
            }
        });
        let big = "x".repeat(200_000);
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut i = 0u64;
        while srv.sessions() > 0 {
            assert!(Instant::now() < deadline, "the stalled remote is still held");
            i += 1;
            srv.publish_status(&Status { status: format!("{i}{big}"), ..Default::default() });
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = stop_tx.send(());
        let _ = rt.block_on(pinger);
        drop(r);
    }

    /// A remote that loses the station (restarted, Wi-Fi gone) keeps trying and gets back in.
    #[test]
    fn remote_reconnects() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        // (a port below the system's temporary ones: while the station is down, the remote's
        // own reconnects get those, and one could take a temporary port the station had)
        let (srv, cfg) = (23_473..23_573)
            .find_map(|port| {
                let cfg = ServerConfig { port, password: "pw".into(), name: "NR5P".into(), audio_device: String::new() };
                RemoteServer::start(rt.handle(), cfg.clone()).ok().map(|s| (s, cfg))
            })
            .expect("a free port");
        let port = srv.port;
        let none: Arc<dyn Fn() + Send + Sync> = Arc::new(|| {});
        let remote = RemoteClient::connect(rt.handle(), ClientConfig { host: "127.0.0.1".into(), port, password: "pw".into() }, none);
        wait_for("connected", || remote.view().phase == Phase::Connected);
        drop(srv);
        wait_for("noticing it's gone", || matches!(remote.view().phase, Phase::Retrying(_)));
        let srv = RemoteServer::start(rt.handle(), cfg).unwrap();
        srv.publish_status(&Status { callsign: "NR5P".into(), frequency: 7_105_000, ..Default::default() });
        wait_for("back", || remote.view().phase == Phase::Connected);
        wait_for("fresh state", || remote.view().status.is_some_and(|s| s.frequency == 7_105_000));
    }

    /// Receive audio end to end: captured at 48 kHz (a 1 kHz tone here), sent at 12 kHz in
    /// 20 ms frames, only while someone listens.
    #[test]
    fn audio_reaches_a_listener() {
        use crate::remote::audio::{set_capture, CaptureHooks, Reader};
        set_capture(CaptureHooks {
            open: Box::new(|name| {
                if name != "tone" {
                    return Err(io::Error::new(io::ErrorKind::NotFound, "no such input"));
                }
                let mut phase = 0f32;
                Ok(Box::new(move |buf: &mut [i16]| {
                    for s in buf.iter_mut() {
                        *s = (8000.0 * phase.sin()) as i16;
                        phase += 2.0 * std::f32::consts::PI * 1000.0 / 48_000.0;
                    }
                    // (as fast as a sound card: real time)
                    std::thread::sleep(Duration::from_secs_f32(buf.len() as f32 / 48_000.0));
                    Ok(())
                }) as Reader)
            }),
            inputs: Box::new(|| vec![("tone".into(), "Test tone".into())]),
            // (none by default, so the other tests' stations offer no audio)
            default_input: Box::new(String::new),
        });
        let rt = tokio::runtime::Runtime::new().unwrap();
        let cfg = ServerConfig { port: 0, password: "pw".into(), name: "NR5P".into(), audio_device: "tone".into() };
        let srv = RemoteServer::start(rt.handle(), cfg).unwrap();
        assert!(srv.audio_available());
        rt.block_on(async {
            let (welcome, mut r, mut w) = raw_login(srv.port, "pw").await;
            assert_eq!(welcome, ServerMsg::Welcome { audio: true, screen: true });
            let on = ClientMsg::Cmd(Command::Audio { on: true });
            ws::write_message(&mut w, &Message::Text(serde_json::to_string(&on).unwrap()), true).await.unwrap();
            let mut frames = Vec::new();
            let start = Instant::now();
            while frames.len() < 25 {
                assert!(start.elapsed() < Duration::from_secs(10), "audio didn't come");
                if let Message::Binary(b) = ws::read_message(&mut r).await.unwrap() {
                    frames.push(crate::remote::audio::decode_frame(&b).expect("an audio frame"));
                }
            }
            // in order, 20 ms each, about real time
            assert!(frames.windows(2).all(|p| p[1].0 == p[0].0.wrapping_add(1)));
            assert!(frames.iter().all(|(_, s)| s.len() == crate::remote::AUDIO_FRAME));
            let secs = start.elapsed().as_secs_f32();
            assert!(secs > 0.3, "faster than real time: {secs} s for 0.5 s of audio");
            // the tone survives: RMS of 8000 / sqrt 2
            let samples: Vec<f32> = frames[5..].iter().flat_map(|(_, s)| s.iter().map(|&v| v as f32)).collect();
            let rms = (samples.iter().map(|v| v * v).sum::<f32>() / samples.len() as f32).sqrt();
            assert!((rms - 5657.0).abs() < 300.0, "rms {rms}");
            assert_eq!(srv.audio_listeners(), 1);
            let off = ClientMsg::Cmd(Command::Audio { on: false });
            ws::write_message(&mut w, &Message::Text(serde_json::to_string(&off).unwrap()), true).await.unwrap();
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while srv.audio_listeners() != 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(srv.audio_listeners(), 0, "stopped listening");
    }

    /// Stopping lets go of the port at once, so Apply can restart on it.
    #[test]
    fn restarts_on_the_same_port() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        // (a port below the system's temporary ones, as in remote_reconnects: other tests'
        // connections could take a temporary port in the moment it's free)
        let (first, cfg) = (23_600..23_700)
            .find_map(|port| {
                let cfg = ServerConfig { port, password: "x".into(), name: "NR5P".into(), audio_device: String::new() };
                RemoteServer::start(rt.handle(), cfg.clone()).ok().map(|s| (s, cfg))
            })
            .expect("a free port");
        let port = first.port;
        drop(first);
        let again = RemoteServer::start(rt.handle(), ServerConfig { port, ..cfg }).unwrap();
        assert_eq!(again.port, port);
    }

    #[test]
    fn refuses_to_start_without_a_password() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let cfg = ServerConfig { port: 0, password: " ".into(), name: "NR5P".into(), audio_device: String::new() };
        assert!(RemoteServer::start(rt.handle(), cfg).is_err());
    }
}
