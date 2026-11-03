//! The built-in modem's live feed for the Modem tab: the `status` messages and waterfall rows
//! XARA's web UI gets on its websocket (`/ws`), read here by a small client so the tab shows
//! the link, levels and waterfall itself (as the NR5P XARA app's Modem tab does), with no
//! browser.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

/// Waterfall rows kept for the view to take (it takes them every frame it draws).
const PENDING_ROWS: usize = 256;

/// One `{"t":"status",…}` message, the parts the tab shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    pub state: String,
    pub peer: String,
    pub bw: String,
    pub level: Option<u64>,
    pub bps: Option<u64>,
    pub tx: bool,
    pub modulation: String,
    pub sn: Option<f64>,
    pub afc: Option<f64>,
    pub attempts: u64,
    pub retries: u64,
    pub buffer: u64,
    pub ptt: bool,
    pub busy: bool,
    pub rx_rms: f64,
    pub tx_rms: f64,
    pub clip: bool,
    /// A tune carrier is on, for `tune_left` more seconds
    pub tuning: bool,
    pub tune_left: f64,
    /// The TX drive the modem uses now (dB)
    pub drive: Option<f64>,
    /// The link's passband on the waterfall (Hz)
    pub passband: Option<(f64, f64)>,
    /// The calls the modem answers to
    pub calls: Vec<String>,
}

impl Status {
    /// A status message's JSON; None for any other message.
    pub fn parse(v: &serde_json::Value) -> Option<Self> {
        if v.get("t")?.as_str()? != "status" {
            return None;
        }
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or_default().to_string();
        let n = |k: &str| v.get(k).and_then(serde_json::Value::as_f64);
        let u = |k: &str| v.get(k).and_then(serde_json::Value::as_u64).unwrap_or(0);
        let b = |k: &str| v.get(k).and_then(serde_json::Value::as_bool).unwrap_or(false);
        let rate = v.get("bitrate").filter(|r| !r.is_null());
        let meter = |k: &str, f: &str| v.get(k).and_then(|m| m.get(f)).and_then(serde_json::Value::as_f64);
        let passband = v.get("passband").and_then(|p| p.as_array()).and_then(|p| match p.as_slice() {
            [lo, hi] => Some((lo.as_f64()?, hi.as_f64()?)),
            _ => None,
        });
        Some(Self {
            state: s("state"),
            peer: s("peer"),
            bw: s("bw"),
            level: v.get("level").and_then(serde_json::Value::as_u64).filter(|_| rate.is_some()),
            bps: rate.and_then(|r| r.get("bps")).and_then(serde_json::Value::as_u64),
            tx: rate.and_then(|r| r.get("tx")).and_then(serde_json::Value::as_bool).unwrap_or(false),
            modulation: rate.and_then(|r| r.get("modulation")).and_then(|m| m.as_str()).unwrap_or_default().to_string(),
            sn: n("sn"),
            afc: n("afc"),
            attempts: u("attempts"),
            retries: u("retries"),
            buffer: u("buffer"),
            ptt: b("ptt"),
            busy: b("busy"),
            rx_rms: meter("rx", "rms").unwrap_or(-120.0),
            tx_rms: meter("tx", "rms").unwrap_or(-120.0),
            clip: meter("rx", "clip").is_some_and(|c| c > 0.0),
            tuning: b("tuning"),
            tune_left: n("tune_left").unwrap_or(0.0),
            drive: n("drive"),
            passband,
            calls: v
                .get("calls")
                .and_then(|c| c.as_array())
                .map(|c| c.iter().filter_map(|x| x.as_str().map(String::from)).collect())
                .unwrap_or_default(),
        })
    }
}

/// What the feed has read, for the tab.
#[derive(Default)]
pub struct Latest {
    /// The latest status and when it came; None while the modem's web UI isn't reachable.
    pub status: Option<(Instant, Status)>,
    /// The waterfall's width in Hz and its dB per step (from the modem's `waterfall` message)
    pub span: f64,
    pub step: f64,
    /// Rows not drawn yet, oldest first, each with whether the modem was transmitting
    pub rows: VecDeque<(Vec<u8>, bool)>,
    /// What the modem calls its mode ("hf 2300", "fm narrow"…) and its version
    pub mode: String,
    pub version: String,
}

pub struct Feed {
    latest: Mutex<Latest>,
    /// The web UI port to connect to
    port: AtomicU16,
    /// Connect only while the built-in modem runs (another program may have the port)
    active: AtomicBool,
    /// The Modem tab is on screen: repaint on each message, and send the waterfall
    visible: AtomicBool,
    /// The open connection, for sending WATERFALL ON / OFF; and what was sent last
    writer: Mutex<Option<(TcpStream, bool)>>,
}

impl Feed {
    /// Starts reading in the background (connecting whenever the modem runs).
    pub fn spawn(port: u16, repaint: impl Fn() + Send + 'static) -> Arc<Self> {
        let feed = Arc::new(Self {
            latest: Mutex::new(Latest { span: 3000.0, step: 0.5, ..Default::default() }),
            port: AtomicU16::new(port),
            active: AtomicBool::new(false),
            visible: AtomicBool::new(false),
            writer: Mutex::new(None),
        });
        let f = feed.clone();
        std::thread::Builder::new()
            .name("xara-feed".into())
            .spawn(move || loop {
                if f.active.load(Ordering::Relaxed) {
                    if let Err(e) = f.follow(&repaint) {
                        tracing::debug!("modem feed: {e}");
                    }
                    *f.writer.lock().unwrap_or_else(PoisonError::into_inner) = None;
                    f.latest().status = None;
                    repaint();
                }
                std::thread::sleep(Duration::from_secs(1));
            })
            .ok();
        feed
    }

    pub fn latest(&self) -> MutexGuard<'_, Latest> {
        self.latest.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn set_port(&self, port: u16) {
        self.port.store(port, Ordering::Relaxed);
    }

    pub fn set_active(&self, on: bool) {
        self.active.store(on, Ordering::Relaxed);
    }

    /// The tab is (or isn't) showing: the waterfall streams only while it is.
    pub fn set_visible(&self, on: bool) {
        self.visible.store(on, Ordering::Relaxed);
        let mut w = self.writer.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((s, sent)) = w.as_mut() {
            if *sent != on {
                *sent = on;
                if send_text(s, if on { "WATERFALL ON" } else { "WATERFALL OFF" }).is_err() {
                    *w = None;
                }
            }
        }
    }

    /// One websocket session: the upgrade, then frames until it closes.
    fn follow(&self, repaint: &impl Fn()) -> std::io::Result<()> {
        let port = self.port.load(Ordering::Relaxed);
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        let mut s = TcpStream::connect_timeout(&addr, Duration::from_millis(500))?;
        s.set_read_timeout(Some(Duration::from_secs(10)))?;
        write!(
            s,
            "GET /ws HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
             Sec-WebSocket-Key: b3BlbnNsb3QtbW9kZW0tdGFiPQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
        )?;
        let mut r = BufReader::new(s.try_clone()?);
        let mut line = String::new();
        r.read_line(&mut line)?;
        if !line.contains(" 101 ") {
            return Err(std::io::Error::other(format!("no websocket: {}", line.trim())));
        }
        loop {
            line.clear();
            if r.read_line(&mut line)? == 0 || line == "\r\n" {
                break;
            }
        }
        {
            let wf = self.visible.load(Ordering::Relaxed);
            let mut w = s.try_clone()?;
            if wf {
                send_text(&mut w, "WATERFALL ON")?;
            }
            *self.writer.lock().unwrap_or_else(PoisonError::into_inner) = Some((w, wf));
        }
        let mut message = Vec::new();
        loop {
            let (fin, op, payload) = frame(&mut r)?;
            match op {
                // text (and its continuations)
                0 | 1 => {
                    message.extend_from_slice(&payload);
                    if fin {
                        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&message) {
                            if self.take(&v) && self.visible.load(Ordering::Relaxed) {
                                repaint();
                            }
                        }
                        message.clear();
                    }
                }
                // a waterfall row: type 1, a byte, the length (LE), the bins
                2 => {
                    if payload.len() >= 4 && payload[0] == 1 {
                        let n = usize::from(u16::from_le_bytes([payload[2], payload[3]]));
                        if let Some(bins) = payload.get(4..4 + n) {
                            let mut l = self.latest();
                            let ptt = l.status.as_ref().is_some_and(|(_, s)| s.ptt);
                            if l.rows.len() == PENDING_ROWS {
                                l.rows.pop_front();
                            }
                            l.rows.push_back((bins.to_vec(), ptt));
                        }
                    }
                }
                8 => return Ok(()),
                // ping: a client's frames are masked (a zero mask leaves the bytes as they are)
                9 => {
                    let mut pong = vec![0x8A, 0x80 | payload.len().min(125) as u8, 0, 0, 0, 0];
                    pong.extend_from_slice(&payload[..payload.len().min(125)]);
                    s.write_all(&pong)?;
                }
                _ => {}
            }
        }
    }

    /// Keeps what the tab shows from one message; true for one worth a repaint.
    fn take(&self, v: &serde_json::Value) -> bool {
        let mut l = self.latest();
        match v.get("t").and_then(|t| t.as_str()) {
            Some("status") => match Status::parse(v) {
                Some(st) => {
                    l.status = Some((Instant::now(), st));
                    true
                }
                None => false,
            },
            Some("hello") => {
                let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or_default().to_string();
                l.mode = s("mode");
                l.version = s("version");
                if let Some(span) = v.get("span").and_then(serde_json::Value::as_f64) {
                    l.span = span;
                }
                true
            }
            Some("waterfall") => {
                if let Some(span) = v.get("span").and_then(serde_json::Value::as_f64) {
                    l.span = span;
                }
                if let Some(step) = v.get("step").and_then(serde_json::Value::as_f64) {
                    l.step = step;
                }
                true
            }
            _ => false,
        }
    }
}

/// A text frame from the client (masked, with a zero mask).
fn send_text(s: &mut TcpStream, text: &str) -> std::io::Result<()> {
    let mut f = vec![0x81, 0x80 | text.len() as u8, 0, 0, 0, 0];
    f.extend_from_slice(text.as_bytes());
    s.write_all(&f)
}

/// One frame from the server (unmasked): FIN, opcode, payload.
fn frame(r: &mut impl Read) -> std::io::Result<(bool, u8, Vec<u8>)> {
    let mut h = [0u8; 2];
    r.read_exact(&mut h)?;
    let (fin, op, masked) = (h[0] & 0x80 != 0, h[0] & 0x0F, h[1] & 0x80 != 0);
    let len = match h[1] & 0x7F {
        126 => {
            let mut b = [0u8; 2];
            r.read_exact(&mut b)?;
            u16::from_be_bytes(b) as u64
        }
        127 => {
            let mut b = [0u8; 8];
            r.read_exact(&mut b)?;
            u64::from_be_bytes(b)
        }
        n => n as u64,
    };
    if len > 16 << 20 {
        return Err(std::io::Error::other("websocket frame too long"));
    }
    let mut mask = [0u8; 4];
    if masked {
        r.read_exact(&mut mask)?;
    }
    let mut payload = vec![0u8; len as usize];
    r.read_exact(&mut payload)?;
    if masked {
        for (i, b) in payload.iter_mut().enumerate() {
            *b ^= mask[i % 4];
        }
    }
    Ok((fin, op, payload))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_modems_status() {
        // (the message xara-ui's own test checks)
        let m = r#"{"t":"status","ts":1.5,"rx":{"rms":-6.02,"peak":-6.02,"clip":0},"tx":{"rms":-120,"peak":-120,"clip":0},"ptt":true,"busy":false,"tuning":false,"tune_left":0,"drive":0,"state":"Receiving","level":7,"peer":"N1CALL","bw":"2300","passband":[350,2650],"attempts":0,"retries":0,"afc":null,"sn":11.25,"bitrate":{"level":7,"bps":2412,"tx":false,"modulation":"OFDM 49 carriers · QPSK"},"buffer":0,"calls":["N0CALL"],"dropped":0}"#;
        let s = Status::parse(&serde_json::from_str(m).unwrap()).unwrap();
        assert_eq!((s.state.as_str(), s.peer.as_str(), s.bw.as_str()), ("Receiving", "N1CALL", "2300"));
        assert_eq!((s.level, s.bps, s.sn, s.afc), (Some(7), Some(2412), Some(11.25), None));
        assert_eq!(s.modulation, "OFDM 49 carriers · QPSK");
        assert_eq!((s.passband, s.calls.as_slice()), (Some((350.0, 2650.0)), &["N0CALL".to_string()][..]));
        assert!(s.ptt && !s.busy && !s.clip && !s.tuning);
        assert_eq!(s.drive, Some(0.0));
        let idle = r#"{"t":"status","level":0,"bitrate":null,"passband":[null,null]}"#;
        let s = Status::parse(&serde_json::from_str(idle).unwrap()).unwrap();
        assert_eq!((s.level, s.bps, s.passband), (None, None, None));
        assert!(Status::parse(&serde_json::from_str(r#"{"t":"notice","ts":0,"kind":"timed_out"}"#).unwrap()).is_none());
    }

    #[test]
    fn frames() {
        // short text, then a 16-bit length
        let mut b = vec![0x81, 3];
        b.extend_from_slice(b"abc");
        let long = vec![b'x'; 300];
        b.extend_from_slice(&[0x81, 126, 1, 44]);
        b.extend_from_slice(&long);
        let mut r = &b[..];
        assert_eq!(frame(&mut r).unwrap(), (true, 1, b"abc".to_vec()));
        assert_eq!(frame(&mut r).unwrap(), (true, 1, long));
    }
}
