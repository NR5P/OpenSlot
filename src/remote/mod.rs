//! Remote mode: control the Openslot that's connected to the radio from another computer
//! on the LAN (any of the desktop apps, whatever its OS) or from a phone's web browser.
//!
//! The station (the Openslot at the radio) stays in charge: its modem, session, logs and
//! VMail are the ones used. It serves one TCP port (default 8473):
//!
//! - `GET /` the phone page (web.html): the same controls in a browser, no app needed
//! - `GET /ws` a WebSocket every client uses
//!
//! On the WebSocket (JSON text messages, each with a "type"):
//!
//! ```text
//! S: {"type":"hello","protocol":1,"name":"NR5P","nonce":"…16 hex…","version":"0.1.12"}
//! C: {"type":"auth","proof":"…hex HMAC-SHA256(password, \"openslot:\" + nonce)…","client":"…"}
//! S: {"type":"welcome","audio":true} | {"type":"denied","reason":"…"} (and close)
//! S: {"type":"status",…} {"type":"chat","lines":[…]} {"type":"stations",…} {"type":"rig",…}
//!    whenever they change; {"type":"notice","text":…} for a refused command
//! C: {"type":"cmd","cmd":"chat","text":"…"} (see [`Command`]), {"type":"ping"} every few s
//! ```
//!
//! Receive audio, when the station has an audio input for it, goes in binary messages:
//! byte 0 = 1, bytes 1-4 the frame number (u32 LE), then 20 ms of 16-bit mono samples at
//! [`AUDIO_RATE`]. It's optional: a station without one says `"audio":false` and everything
//! else works the same.
//!
//! The password is never sent, only proof of it, and five wrong ones lock an address out for
//! a minute. There is no encryption: it's for the home LAN (or a VPN into it), not the
//! open internet.
//!
//! Stations are found on the LAN by a UDP probe on [`DISCOVERY_PORT`] (see discovery()).

pub mod audio;
pub mod client;
pub mod server;
pub mod ws;

use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::rig::{RadioMode, RigAction, RigExtras};

pub const DEFAULT_PORT: u16 = 8473;
pub const DISCOVERY_PORT: u16 = 8474;
pub const PROTOCOL: u32 = 1;
/// Receive audio's sample rate: an SSB channel needs no more, and a phone on Wi-Fi copes.
pub const AUDIO_RATE: u32 = 12_000;
/// Samples in one 20 ms audio frame.
pub const AUDIO_FRAME: usize = (AUDIO_RATE / 50) as usize;
/// Binary message type: receive audio.
pub const AUDIO_RX: u8 = 1;
const PROBE: &[u8] = b"OPENSLOT-DISCOVER";
const BEACON: &str = "OPENSLOT-REMOTE";

/// The station's state as a remote sees it (the main screen's essentials).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Status {
    pub callsign: String,
    pub version: String,
    pub modem: bool,
    /// "idle", "connecting", "connected", "disconnecting"
    pub link: String,
    pub partner: Option<String>,
    pub partner_name: Option<String>,
    pub partner_qth: Option<String>,
    pub partner_locator: Option<String>,
    /// The modem is transmitting
    pub tx: bool,
    pub busy: bool,
    pub snr: Option<f32>,
    pub bandwidth: String,
    /// The dial, and the calling frequency the station works from
    pub frequency: u64,
    pub calling_frequency: u64,
    pub band: String,
    pub on_calling_frequency: bool,
    pub cq_active: bool,
    pub beacon_auto: bool,
    pub buffer: u32,
    pub ptt_disabled: bool,
    pub unread_vmail: i64,
    pub status: String,
    /// Receive audio can be listened to, and why not when not
    pub audio: bool,
    pub audio_note: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChatItem {
    pub time: String,
    pub call: String,
    pub text: String,
    #[serde(default)]
    pub out: bool,
    #[serde(default)]
    pub system: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StationItem {
    pub time: String,
    pub call: String,
    pub snr: Option<String>,
    pub locator: Option<String>,
    pub bandwidth: u16,
    pub beacon: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Stations {
    pub cq: Vec<StationItem>,
    pub beacons: Vec<StationItem>,
}

/// The radio, for the remote's Rig controls panel.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RigView {
    /// "None" when the station has no rig control
    pub backend: String,
    pub connected: bool,
    pub frequency: Option<u64>,
    pub mode: Option<RadioMode>,
    pub ptt: bool,
    pub extras: RigExtras,
    pub may_transmit: bool,
}

/// What a remote may ask of the station: the same as its own buttons do.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    /// A chat line to the partner (VarAC's tags are expanded as typed at the station)
    Chat { text: String },
    Connect { call: String },
    Disconnect,
    Abort,
    /// CALL CQ (on the station's chosen slot), and END CQ
    Cq,
    EndCq,
    Beacon,
    AutoBeacon { on: bool },
    /// Tune to this frequency and work from it (as the frequency picker does)
    Frequency { hz: u64 },
    Rig { action: RigAction },
    /// Send receive audio to this client (or stop)
    Audio { on: bool },
}

/// Client → station messages.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMsg {
    Auth { proof: String, #[serde(default)] client: String },
    Cmd(Command),
    Ping,
}

/// Station → client messages.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMsg {
    Hello { protocol: u32, name: String, nonce: String, version: String },
    Welcome { audio: bool },
    Denied { reason: String },
    Status(Status),
    Chat { lines: Vec<ChatItem> },
    Stations(Stations),
    Rig(RigView),
    Notice { text: String },
    Pong,
}

/// Proof of the password for this challenge (hex HMAC-SHA256).
pub fn proof(password: &str, nonce: &str) -> String {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(password.as_bytes()).expect("HMAC takes any key");
    mac.update(format!("openslot:{nonce}").as_bytes());
    mac.finalize().into_bytes().iter().map(|b| format!("{b:02x}")).collect()
}

/// Compares in constant time, so the proof can't be found byte by byte.
pub fn proof_ok(password: &str, nonce: &str, given: &str) -> bool {
    let want = proof(password, nonce);
    want.len() == given.len() && want.bytes().zip(given.bytes()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
}

/// A station found on the LAN.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub audio: bool,
}

fn beacon_text(id: &str, name: &str, port: u16, audio: bool) -> String {
    format!("{BEACON} {PROTOCOL};id={id};name={name};port={port};audio={}", audio as u8)
}

fn parse_beacon(text: &str) -> Option<(String, Found)> {
    let rest = text.strip_prefix(BEACON)?.trim_start();
    let mut parts = rest.split(';');
    let _version = parts.next()?;
    let (mut id, mut found) = (String::new(), Found { name: String::new(), host: String::new(), port: DEFAULT_PORT, audio: false });
    for kv in parts {
        match kv.split_once('=') {
            Some(("id", v)) => id = v.to_string(),
            Some(("name", v)) => found.name = v.to_string(),
            Some(("port", v)) => found.port = v.parse().ok()?,
            Some(("audio", v)) => found.audio = v == "1",
            _ => {}
        }
    }
    Some((id, found))
}

/// This computer's address on the LAN (the one its default route uses), for the phone URL.
pub fn lan_address() -> Option<std::net::IpAddr> {
    // (connecting a UDP socket sends nothing; it only picks the route)
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.0.2.1:9").ok()?;
    s.local_addr().ok().map(|a| a.ip()).filter(|ip| !ip.is_unspecified())
}

/// Looks for stations on the LAN for `wait`: a broadcast, and a probe to every address of
/// this computer's /24 (Wi-Fi access points often drop broadcasts). Blocking.
pub fn discover(wait: std::time::Duration) -> Vec<Found> {
    use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
    let Ok(sock) = UdpSocket::bind("0.0.0.0:0") else { return Vec::new() };
    let _ = sock.set_broadcast(true);
    let _ = sock.send_to(PROBE, (Ipv4Addr::BROADCAST, DISCOVERY_PORT));
    let _ = sock.send_to(PROBE, (Ipv4Addr::LOCALHOST, DISCOVERY_PORT));
    if let Some(std::net::IpAddr::V4(me)) = lan_address() {
        let o = me.octets();
        for last in 1..=254u8 {
            let _ = sock.send_to(PROBE, (Ipv4Addr::new(o[0], o[1], o[2], last), DISCOVERY_PORT));
        }
    }
    let deadline = std::time::Instant::now() + wait;
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    let mut buf = [0u8; 512];
    while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
        let _ = sock.set_read_timeout(Some(left.max(std::time::Duration::from_millis(1))));
        let Ok((n, from)) = sock.recv_from(&mut buf) else { break };
        let Some((id, mut found)) = parse_beacon(&String::from_utf8_lossy(&buf[..n])) else { continue };
        if !seen.insert(id) {
            continue;
        }
        found.host = match from {
            SocketAddr::V4(a) if a.ip().is_loopback() => lan_address().map(|ip| ip.to_string()).unwrap_or_else(|| "127.0.0.1".into()),
            a => a.ip().to_string(),
        };
        out.push(found);
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proof_checks_the_password() {
        let p = proof("secret", "0011223344556677");
        assert_eq!(p.len(), 64);
        assert!(proof_ok("secret", "0011223344556677", &p));
        assert!(!proof_ok("Secret", "0011223344556677", &p));
        assert!(!proof_ok("secret", "0011223344556678", &p));
        assert!(!proof_ok("secret", "0011223344556677", ""));
        // RFC 4231 test case 2 (the browser page computes the same)
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(b"Jefe").unwrap();
        mac.update(b"what do ya want for nothing?");
        let hex: String = mac.finalize().into_bytes().iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
    }

    #[test]
    fn messages_in_json() {
        let c: ClientMsg = serde_json::from_str(r#"{"type":"cmd","cmd":"chat","text":"hi"}"#).unwrap();
        assert_eq!(c, ClientMsg::Cmd(Command::Chat { text: "hi".into() }));
        let c: ClientMsg = serde_json::from_str(r#"{"type":"cmd","cmd":"rig","action":{"op":"level","level":"rf_power","value":0.5}}"#).unwrap();
        assert!(matches!(c, ClientMsg::Cmd(Command::Rig { action: RigAction::Level { .. } })));
        let c: ClientMsg = serde_json::from_str(r#"{"type":"cmd","cmd":"rig","action":{"op":"mode","mode":"UsbData"}}"#).unwrap();
        assert!(matches!(c, ClientMsg::Cmd(Command::Rig { action: RigAction::Mode { mode: RadioMode::UsbData } })));
        let s = serde_json::to_string(&ServerMsg::Status(Status { callsign: "NR5P".into(), ..Default::default() })).unwrap();
        assert!(s.starts_with(r#"{"type":"status","callsign":"NR5P""#), "{s}");
        assert_eq!(serde_json::to_string(&ClientMsg::Ping).unwrap(), r#"{"type":"ping"}"#);
    }

    #[test]
    fn beacon_round_trip() {
        let t = beacon_text("ab12", "NR5P", 8473, true);
        let (id, f) = parse_beacon(&t).unwrap();
        assert_eq!(id, "ab12");
        assert_eq!((f.name.as_str(), f.port, f.audio), ("NR5P", 8473, true));
        assert!(parse_beacon("something else").is_none());
    }
}
