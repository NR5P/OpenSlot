//! The remote side: a desktop Openslot controlling a station elsewhere on the LAN. Keeps
//! a [`View`] of the station up to date for the GUI to draw, sends its commands, and plays
//! the station's audio when asked. Reconnects by itself, except after a wrong password.

use std::collections::VecDeque;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::BufReader;
use tokio::sync::{mpsc, watch};

use super::audio::{decode_frame, Player};
use super::ws::{self, Message};
use super::{ChatItem, ClientMsg, Command, RigView, ServerMsg, Stations, Status};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientConfig {
    pub host: String,
    pub port: u16,
    pub password: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub enum Phase {
    #[default]
    Connecting,
    Connected,
    /// Lost or not reached: trying again (why)
    Retrying(String),
    /// Refused (wrong password, too many remotes…): not tried again
    Denied(String),
}

/// The station as last heard.
#[derive(Debug, Clone, Default)]
pub struct View {
    pub phase: Phase,
    pub name: String,
    pub version: String,
    pub status: Option<Status>,
    pub chat: Vec<ChatItem>,
    pub stations: Stations,
    pub rig: Option<RigView>,
    /// Recent notices from the station (newest last)
    pub notices: VecDeque<(Instant, String)>,
    /// The station offers audio, we're listening, and how that's going
    pub audio_offered: bool,
    pub listening: bool,
    pub audio_error: Option<String>,
}

pub struct RemoteClient {
    pub config: ClientConfig,
    view: Arc<Mutex<View>>,
    tx: mpsc::UnboundedSender<Command>,
    stop: watch::Sender<bool>,
    player: Arc<Mutex<Option<Player>>>,
    rt: tokio::runtime::Handle,
    repaint: Arc<dyn Fn() + Send + Sync>,
}

impl RemoteClient {
    pub fn connect(rt: &tokio::runtime::Handle, config: ClientConfig, repaint: Arc<dyn Fn() + Send + Sync>) -> Self {
        let view = Arc::new(Mutex::new(View::default()));
        let (tx, rx) = mpsc::unbounded_channel();
        let (stop, stop_rx) = watch::channel(false);
        let player: Arc<Mutex<Option<Player>>> = Arc::new(Mutex::new(None));
        let (v, p, r, cfg) = (view.clone(), player.clone(), repaint.clone(), config.clone());
        rt.spawn(run(cfg, v, p, rx, stop_rx, r));
        Self { config, view, tx, stop, player, rt: rt.clone(), repaint }
    }

    pub fn view(&self) -> View {
        self.view.lock().map(|v| v.clone()).unwrap_or_default()
    }

    pub fn send(&self, cmd: Command) {
        let _ = self.tx.send(cmd);
    }

    /// Listen to the station's receive audio here (or stop).
    pub fn set_listening(&self, on: bool) {
        if let Ok(mut v) = self.view.lock() {
            v.listening = on;
            v.audio_error = None;
        }
        if !on {
            if let Ok(mut p) = self.player.lock() {
                *p = None;
            }
            self.send(Command::Audio { on: false });
            return;
        }
        let (player, view, tx, repaint) = (self.player.clone(), self.view.clone(), self.tx.clone(), self.repaint.clone());
        // (opening the output can take a moment)
        self.rt.spawn_blocking(move || {
            match Player::start() {
                Ok(p) => {
                    if let Ok(mut slot) = player.lock() {
                        *slot = Some(p);
                    }
                    let _ = tx.send(Command::Audio { on: true });
                }
                Err(e) => {
                    if let Ok(mut v) = view.lock() {
                        v.listening = false;
                        v.audio_error = Some(e);
                    }
                }
            }
            repaint();
        });
    }

    pub fn set_volume(&self, volume: f32) {
        if let Ok(p) = self.player.lock() {
            if let Some(p) = p.as_ref() {
                if let Ok(mut v) = p.volume.lock() {
                    *v = volume.clamp(0.0, 1.0);
                }
            }
        }
    }
}

impl Drop for RemoteClient {
    fn drop(&mut self) {
        let _ = self.stop.send(true);
    }
}

async fn run(
    cfg: ClientConfig,
    view: Arc<Mutex<View>>,
    player: Arc<Mutex<Option<Player>>>,
    mut rx: mpsc::UnboundedReceiver<Command>,
    mut stop: watch::Receiver<bool>,
    repaint: Arc<dyn Fn() + Send + Sync>,
) {
    let set = |f: &dyn Fn(&mut View)| {
        if let Ok(mut v) = view.lock() {
            f(&mut v);
        }
        repaint();
    };
    loop {
        set(&|v| {
            if !matches!(v.phase, Phase::Retrying(_)) {
                v.phase = Phase::Connecting;
            }
        });
        let r = tokio::select! {
            r = session(&cfg, &view, &player, &mut rx, &repaint) => r,
            _ = stop.changed() => return,
        };
        match r {
            Err(Ended::Denied(why)) => {
                set(&|v| v.phase = Phase::Denied(why.clone()));
                return;
            }
            Err(Ended::Lost(e)) => {
                let why = e.to_string();
                set(&|v| v.phase = Phase::Retrying(why.clone()));
            }
            Ok(()) => set(&|v| v.phase = Phase::Retrying("the station closed the connection".into())),
        }
        // (commands typed while away are dropped, never sent late)
        while rx.try_recv().is_ok() {}
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(3)) => {}
            _ = stop.changed() => return,
        }
    }
}

enum Ended {
    Denied(String),
    Lost(io::Error),
}

/// A refusal for now (too many remotes, a lockout, the station restarting) is tried again;
/// a wrong password isn't.
fn refused(reason: String, retry: bool) -> Ended {
    if retry { Ended::Lost(io::Error::other(reason)) } else { Ended::Denied(reason) }
}

impl From<io::Error> for Ended {
    fn from(e: io::Error) -> Self {
        Ended::Lost(e)
    }
}

async fn session(
    cfg: &ClientConfig,
    view: &Arc<Mutex<View>>,
    player: &Arc<Mutex<Option<Player>>>,
    rx: &mut mpsc::UnboundedReceiver<Command>,
    repaint: &Arc<dyn Fn() + Send + Sync>,
) -> Result<(), Ended> {
    let addr = format!("{}:{}", cfg.host.trim(), cfg.port);
    let stream = tokio::time::timeout(Duration::from_secs(5), tokio::net::TcpStream::connect(&addr))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, format!("no answer from {addr}")))??;
    let _ = stream.set_nodelay(true);
    let (r, mut w) = stream.into_split();
    let mut r = BufReader::new(r);
    tokio::time::timeout(Duration::from_secs(10), ws::connect(&mut r, &mut w, &addr, "/ws"))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, format!("{addr} didn't answer as an Openslot station")))??;
    let text = |m: io::Result<Message>| -> Result<ServerMsg, Ended> {
        match m? {
            Message::Text(t) => serde_json::from_str(&t).map_err(|e| Ended::Lost(io::Error::new(io::ErrorKind::InvalidData, e))),
            _ => Err(Ended::Lost(io::Error::new(io::ErrorKind::InvalidData, "unexpected message"))),
        }
    };
    let timeout = |d| move |_| Ended::Lost(io::Error::new(io::ErrorKind::TimedOut, format!("the station didn't answer in {d} s")));
    let hello = text(tokio::time::timeout(Duration::from_secs(10), ws::read_message(&mut r)).await.map_err(timeout(10))?)?;
    let (name, nonce, version) = match hello {
        ServerMsg::Hello { name, nonce, version, .. } => (name, nonce, version),
        ServerMsg::Denied { reason, retry } => return Err(refused(reason, retry)),
        _ => return Err(Ended::Lost(io::Error::new(io::ErrorKind::InvalidData, "not an Openslot station"))),
    };
    let auth = ClientMsg::Auth {
        proof: super::proof(&cfg.password, &nonce),
        client: format!("Openslot {} ({})", env!("CARGO_PKG_VERSION"), std::env::consts::OS),
    };
    ws::write_message(&mut w, &Message::Text(serde_json::to_string(&auth).unwrap_or_default()), true).await?;
    let audio = match text(tokio::time::timeout(Duration::from_secs(10), ws::read_message(&mut r)).await.map_err(timeout(10))?)? {
        ServerMsg::Welcome { audio } => audio,
        ServerMsg::Denied { reason, retry } => return Err(refused(reason, retry)),
        _ => return Err(Ended::Lost(io::Error::new(io::ErrorKind::InvalidData, "unexpected answer to the password"))),
    };
    // (the station may have restarted, numbering its audio afresh)
    if let Ok(mut p) = player.lock() {
        if let Some(p) = p.as_mut() {
            p.new_session();
        }
    }
    let listening = {
        let mut v = view.lock().map_err(|_| io::Error::other("view"))?;
        v.phase = Phase::Connected;
        v.name = name;
        v.version = version;
        v.audio_offered = audio;
        v.listening
    };
    repaint();
    if listening {
        ws::write_message(&mut w, &Message::Text(serde_json::to_string(&ClientMsg::Cmd(Command::Audio { on: true })).unwrap_or_default()), true).await?;
    }

    let (in_tx, mut incoming) = mpsc::channel::<io::Result<Message>>(64);
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

    let mut ping = tokio::time::interval(Duration::from_secs(5));
    let mut last_heard = Instant::now();
    loop {
        tokio::select! {
            m = incoming.recv() => {
                last_heard = Instant::now();
                match m {
                    Some(Ok(Message::Text(t))) => {
                        let Ok(msg) = serde_json::from_str::<ServerMsg>(&t) else { continue };
                        if let ServerMsg::Denied { reason, .. } = msg {
                            return Err(Ended::Lost(io::Error::other(reason)));
                        }
                        if let Ok(mut v) = view.lock() {
                            match msg {
                                ServerMsg::Status(s) => v.status = Some(s),
                                ServerMsg::Chat { lines } => v.chat = lines,
                                ServerMsg::Stations(s) => v.stations = s,
                                ServerMsg::Rig(r) => v.rig = Some(r),
                                ServerMsg::Notice { text } => {
                                    v.notices.push_back((Instant::now(), text));
                                    while v.notices.len() > 5 {
                                        v.notices.pop_front();
                                    }
                                }
                                _ => {}
                            }
                        }
                        repaint();
                    }
                    Some(Ok(Message::Binary(b))) => {
                        if let Some((seq, samples)) = decode_frame(&b) {
                            if let Ok(mut p) = player.lock() {
                                if let Some(p) = p.as_mut() {
                                    p.push(seq, &samples);
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Ping(p))) => ws::write_message(&mut w, &Message::Pong(p), true).await?,
                    Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(Message::Close)) | None => return Ok(()),
                    Some(Err(e)) => return Err(e.into()),
                }
            }
            c = rx.recv() => {
                let Some(c) = c else { return Ok(()) };
                ws::write_message(&mut w, &Message::Text(serde_json::to_string(&ClientMsg::Cmd(c)).unwrap_or_default()), true).await?;
            }
            _ = ping.tick() => {
                if last_heard.elapsed() > Duration::from_secs(20) {
                    return Err(Ended::Lost(io::Error::new(io::ErrorKind::TimedOut, "the station stopped answering")));
                }
                ws::write_message(&mut w, &Message::Text(serde_json::to_string(&ClientMsg::Ping).unwrap_or_default()), true).await?;
            }
        }
    }
}
