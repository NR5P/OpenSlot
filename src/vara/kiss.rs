//! KISS interface of the VARA modem (default TCP 8100) and AX.25 UI framing.
//!
//! VarAC sends broadcasts (and some beacon/CQ data) as AX.25 UI frames over the
//! modem's KISS port; the modem transmits them unconnected ("unproto").

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

const FEND: u8 = 0xC0;
const FESC: u8 = 0xDB;
const TFEND: u8 = 0xDC;
const TFESC: u8 = 0xDD;

/// KISS command byte VarAC uses (captured: `C0 01 …`). 0x01 is VARA's "long AX.25"
/// variant: address fields hold 7 callsign characters (8 bytes per address)
/// instead of the standard 6 (7 bytes). 0x00 is plain AX.25.
pub const VARAC_KISS_CMD: u8 = 0x01;
#[allow(dead_code)]
pub const KISS_CMD_AX25: u8 = 0x00;

/// KISS command of VarAC's ADVANCED beacon (`C0 02 … C0` on the KISS port):
/// unformatted ASCII, no AX.25 header.
pub const VARAC_BEACON_CMD: u8 = 0x02;

/// Wrap one payload in a KISS frame with VarAC's command byte.
pub fn kiss_encode(payload: &[u8]) -> Vec<u8> {
    kiss_encode_cmd(VARAC_KISS_CMD, payload)
}

/// Wrap one payload in a KISS frame with the given command byte.
pub fn kiss_encode_cmd(cmd: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 4);
    out.push(FEND);
    out.push(cmd);
    for &b in payload {
        match b {
            FEND => out.extend_from_slice(&[FESC, TFEND]),
            FESC => out.extend_from_slice(&[FESC, TFESC]),
            b => out.push(b),
        }
    }
    out.push(FEND);
    out
}

/// Incremental KISS decoder: feed bytes, get complete data-frame payloads.
#[derive(Default)]
pub struct KissDecoder {
    buf: Vec<u8>,
    in_frame: bool,
    escape: bool,
}

impl KissDecoder {
    /// Returns `(kiss_command, payload)` for each complete AX.25 frame.
    pub fn feed(&mut self, data: &[u8]) -> Vec<(u8, Vec<u8>)> {
        let mut frames = Vec::new();
        for &b in data {
            match b {
                FEND => {
                    if self.in_frame && !self.buf.is_empty() {
                        // First byte is the KISS command: 0x00 AX.25 data, 0x01 VARA long
                        // AX.25 (VarAC broadcasts), 0x02 VarAC advanced beacon
                        let cmd = self.buf[0];
                        if cmd <= VARAC_BEACON_CMD && self.buf.len() > 1 {
                            frames.push((cmd, self.buf[1..].to_vec()));
                        }
                    }
                    self.buf.clear();
                    self.in_frame = true;
                    self.escape = false;
                }
                FESC if self.in_frame => self.escape = true,
                b if self.in_frame => {
                    let b = if self.escape {
                        self.escape = false;
                        match b {
                            TFEND => FEND,
                            TFESC => FESC,
                            other => other,
                        }
                    } else {
                        b
                    };
                    self.buf.push(b);
                }
                _ => {}
            }
        }
        frames
    }
}

/// An AX.25 UI (unnumbered information) frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiFrame {
    pub dest: String,
    pub source: String,
    pub digis: Vec<String>,
    pub pid: u8,
    pub info: Vec<u8>,
}

fn encode_addr(call: &str, last: bool, width: usize, out: &mut Vec<u8>) {
    let (base, ssid) = match call.split_once('-') {
        Some((b, s)) => (b, s.parse::<u8>().unwrap_or(0) & 0x0F),
        None => (call, 0),
    };
    let mut chars = base.to_uppercase().into_bytes();
    chars.resize(width, b' ');
    for c in chars.iter().take(width) {
        out.push(c << 1);
    }
    out.push(0x60 | (ssid << 1) | u8::from(last));
}

fn decode_addr(bytes: &[u8], width: usize) -> String {
    let base: String = bytes[..width].iter().map(|b| (b >> 1) as char).collect::<String>().trim_end().to_string();
    let ssid = (bytes[width] >> 1) & 0x0F;
    if ssid == 0 { base } else { format!("{base}-{ssid}") }
}

/// Callsign characters per address field for a KISS command byte.
fn addr_width(kiss_cmd: u8) -> usize {
    if kiss_cmd == VARAC_KISS_CMD { 7 } else { 6 }
}

impl UiFrame {
    /// Encode for the given KISS command (VARAC_KISS_CMD = 7-char address fields).
    pub fn encode(&self, kiss_cmd: u8) -> Vec<u8> {
        let w = addr_width(kiss_cmd);
        let mut out = Vec::with_capacity(2 * (w + 1) + (w + 1) * self.digis.len() + 2 + self.info.len());
        encode_addr(&self.dest, false, w, &mut out);
        encode_addr(&self.source, self.digis.is_empty(), w, &mut out);
        for (i, d) in self.digis.iter().enumerate() {
            let last = i + 1 == self.digis.len();
            encode_addr(d, last, w, &mut out);
            // As in VarAC: a digipeater without "-N" gets SSID byte 0x00
            // (0x01 when last), not 0x60
            if !d.contains('-') && kiss_cmd == VARAC_KISS_CMD {
                *out.last_mut().unwrap() = u8::from(last);
            }
        }
        out.push(0x03); // UI
        out.push(self.pid);
        out.extend_from_slice(&self.info);
        out
    }

    pub fn decode(kiss_cmd: u8, frame: &[u8]) -> Option<Self> {
        let w = addr_width(kiss_cmd);
        let a = w + 1;
        if frame.len() < 2 * a + 2 {
            return None;
        }
        let dest = decode_addr(&frame[0..a], w);
        let source = decode_addr(&frame[a..2 * a], w);
        let mut pos = 2 * a;
        let mut digis = Vec::new();
        let mut last = frame[2 * a - 1] & 1 == 1;
        while !last {
            if frame.len() < pos + a + 2 {
                return None;
            }
            digis.push(decode_addr(&frame[pos..pos + a], w));
            last = frame[pos + w] & 1 == 1;
            pos += a;
        }
        if frame[pos] & 0xEF != 0x03 {
            return None; // not a UI frame
        }
        Some(UiFrame { dest, source, digis, pid: frame[pos + 1], info: frame[pos + 2..].to_vec() })
    }
}

/// Connection to the modem's KISS TCP port.
pub struct KissConnection {
    writer: tokio::io::WriteHalf<TcpStream>,
    /// Received AX.25 frames `(kiss_command, frame)`, without KISS framing.
    pub frame_rx: mpsc::Receiver<(u8, Vec<u8>)>,
    /// Holds the read half: stopped on drop, so the socket closes with the connection.
    reader: tokio::task::JoinHandle<()>,
}

impl Drop for KissConnection {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

impl KissConnection {
    pub async fn connect(host: &str, port: u16) -> Result<Self, std::io::Error> {
        let stream = TcpStream::connect((host, port)).await?;
        let (mut reader, writer) = tokio::io::split(stream);
        let (tx, frame_rx) = mpsc::channel(64);
        let reader = tokio::spawn(async move {
            let mut dec = KissDecoder::default();
            let mut buf = vec![0u8; 4096];
            loop {
                match reader.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        for f in dec.feed(&buf[..n]) {
                            if tx.send(f).await.is_err() {
                                return;
                            }
                        }
                    }
                }
            }
        });
        Ok(Self { writer, frame_rx, reader })
    }

    /// Send VarAC's ADVANCED beacon (KISS 0x02).
    pub async fn send_beacon(&mut self, beacon: &AdvancedBeacon) -> Result<(), std::io::Error> {
        self.writer.write_all(&kiss_encode_cmd(VARAC_BEACON_CMD, &beacon.payload())).await
    }

    /// VarAC's typed CQ, on the same KISS command as the advanced beacon.
    pub async fn send_typed_cq(&mut self, cq: &TypedCq) -> Result<(), std::io::Error> {
        self.writer.write_all(&kiss_encode_cmd(VARAC_BEACON_CMD, &cq.payload())).await
    }

    /// Send a UI frame the way VarAC does (KISS command 0x01, long addresses).
    pub async fn send_ui(&mut self, frame: &UiFrame) -> Result<(), std::io::Error> {
        self.writer.write_all(&kiss_encode(&frame.encode(VARAC_KISS_CMD))).await
    }
}

/// VarAC's ADVANCED beacon, 20 ASCII chars:
/// `"2"` + tag(2) + locator(6, space-padded) + callsign(7) + data(4, always blank).
/// Tag char 1: diploma/scanner (`'0'` = neither); char 2: `'A'` + services mask
/// (EmComm 1, email gateway 2, AI gateway 4, BBS 8), see [`AdvancedBeacon::new`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AdvancedBeacon {
    pub callsign: String,
    pub locator: String,
    pub tag: String,
}

/// VarAC's typed CQ (any type but STANDARD): KISS command 0x02, payload
/// `"1"` + slot (2 digits) + locator (6) + callsign (7) + type (4, e.g. "CHAT", "SOS").
#[derive(Debug, Clone, PartialEq)]
pub struct TypedCq {
    pub slot: u8,
    pub locator: String,
    pub callsign: String,
    pub cq_type: String,
}

impl TypedCq {
    pub fn payload(&self) -> Vec<u8> {
        format!("1{:0>2}{:<6}{:<7}{:<4}", self.slot, self.locator, self.callsign, self.cq_type).into_bytes()
    }

    pub fn parse(payload: &[u8]) -> Option<Self> {
        let s = std::str::from_utf8(payload).ok()?;
        if !s.starts_with('1') || s.len() < 16 {
            return None;
        }
        let field = |a: usize, b: usize| s.get(a..b.min(s.len())).unwrap_or("").trim().to_string();
        let callsign = field(9, 16);
        (!callsign.is_empty()).then(|| Self {
            slot: field(1, 3).parse().unwrap_or(0),
            locator: field(3, 9),
            callsign,
            cq_type: field(16, 20),
        })
    }
}

impl AdvancedBeacon {
    pub const EMCOMM: u8 = 1;
    pub const EMAIL_GATEWAY: u8 = 2;
    pub const AI_GATEWAY: u8 = 4;
    pub const BBS: u8 = 8;
    /// Not a service: the frequency scanner runs with "announce" on (tag char 1 'A')
    pub const SCANNING: u8 = 0x10;

    pub fn new(callsign: &str, locator: &str, services: u8) -> Self {
        let first = if services & Self::SCANNING != 0 { 'A' } else { '0' };
        Self {
            callsign: callsign.split('-').next().unwrap_or(callsign).to_ascii_uppercase(),
            locator: locator.to_string(),
            tag: format!("{first}{}", (b'A' + (services & 0x0f)) as char),
        }
    }

    /// The station scans several frequencies (VarAC shows ≫: be patient when connecting).
    pub fn is_scanning(&self) -> bool {
        matches!(self.tag.chars().next(), Some('A'..='D'))
    }

    pub fn payload(&self) -> Vec<u8> {
        format!("2{:0>2}{:<6}{:<7}{:<4}", self.tag, self.locator, self.callsign, "").into_bytes()
    }

    pub fn parse(payload: &[u8]) -> Option<Self> {
        let s = std::str::from_utf8(payload).ok()?;
        if !s.starts_with('2') || s.len() < 16 {
            return None;
        }
        let field = |a: usize, b: usize| s.get(a..b.min(s.len())).unwrap_or("").trim().to_string();
        let callsign = field(9, 16);
        (!callsign.is_empty()).then(|| Self { tag: field(1, 3), locator: field(3, 9), callsign })
    }

    /// The services mask from the tag (0 when unknown).
    pub fn services(&self) -> u8 {
        match self.tag.chars().nth(1) {
            Some(c @ 'A'..='P') => c as u8 - b'A',
            _ => 0,
        }
    }

    pub fn email_gateway(&self) -> bool {
        self.services() & Self::EMAIL_GATEWAY != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// As in VarAC: digipeaters without an SSID get SSID byte 0x00
    /// (0x01 on the last), not 0x60; with "-N" the usual 0x60 | N<<1.
    #[test]
    fn digipeater_ssid_bytes_match_varac() {
        let f = UiFrame { dest: "K1ABC".into(), source: "W5TST".into(), digis: vec!["RELAY".into(), "WIDE2-2".into()], pid: 0xF0, info: b"hi".to_vec() };
        let raw = f.encode(VARAC_KISS_CMD);
        assert_eq!(raw[15], 0x60, "source, not last");
        assert_eq!(raw[23], 0x00, "RELAY, no SSID, not last");
        assert_eq!(raw[31], 0x60 | (2 << 1) | 1, "WIDE2-2, last");
        let f2 = UiFrame { digis: vec!["RELAY".into()], ..f };
        assert_eq!(f2.encode(VARAC_KISS_CMD)[23], 0x01);
        let back = UiFrame::decode(VARAC_KISS_CMD, &f2.encode(VARAC_KISS_CMD)).unwrap();
        assert_eq!(back.digis, vec!["RELAY".to_string()]);
    }

    #[test]
    fn typed_cq_layout() {
        let cq = TypedCq { slot: 12, locator: "EM10".into(), callsign: "W5TST".into(), cq_type: "CHAT".into() };
        assert_eq!(cq.payload(), b"112EM10  W5TST  CHAT".to_vec());
        assert_eq!(TypedCq::parse(&cq.payload()), Some(cq));
        assert_eq!(TypedCq::parse(b"105FN42AAK1ABC  SOS ").unwrap().cq_type, "SOS");
        assert!(AdvancedBeacon::parse(b"112EM10  W5TST  CHAT").is_none(), "a CQ isn't a beacon");
    }

    #[test]
    fn advanced_beacon_matches_varac_layout() {
        let b = AdvancedBeacon::new("W5TST", "EM10", AdvancedBeacon::EMAIL_GATEWAY);
        assert_eq!(b.payload(), b"20CEM10  W5TST      ".to_vec());
        let frame = kiss_encode_cmd(VARAC_BEACON_CMD, &b.payload());
        let (cmd, raw) = KissDecoder::default().feed(&frame).remove(0);
        assert_eq!(cmd, VARAC_BEACON_CMD);
        let got = AdvancedBeacon::parse(&raw).unwrap();
        assert_eq!(got, AdvancedBeacon { callsign: "W5TST".into(), locator: "EM10".into(), tag: "0C".into() });
        assert!(got.email_gateway());
        assert!(!AdvancedBeacon::new("NR5P", "EM10HW", AdvancedBeacon::BBS).email_gateway());
    }

    #[test]
    fn kiss_roundtrip_with_escapes() {
        let payload = [1u8, FEND, 2, FESC, 3];
        let enc = kiss_encode(&payload);
        assert_eq!(enc, vec![FEND, VARAC_KISS_CMD, 1, FESC, TFEND, 2, FESC, TFESC, 3, FEND]);
        let mut d = KissDecoder::default();
        // split delivery
        let mut got = d.feed(&enc[..4]);
        got.extend(d.feed(&enc[4..]));
        assert_eq!(got, vec![(VARAC_KISS_CMD, payload.to_vec())]);
    }

    #[test]
    fn ax25_ui_roundtrip() {
        let f = UiFrame { dest: "ALL".into(), source: "NR5P-9".into(), digis: vec![], pid: 0xF0, info: b"hello".to_vec() };
        for cmd in [KISS_CMD_AX25, VARAC_KISS_CMD] {
            let bytes = f.encode(cmd);
            assert_eq!(bytes[2 * (addr_width(cmd) + 1) - 1] & 1, 1, "last address bit");
            assert_eq!(UiFrame::decode(cmd, &bytes), Some(f.clone()));
            let g = UiFrame { dest: "CQ".into(), source: "W5TST".into(), digis: vec!["K5DIG-1".into()], pid: 0xF0, info: vec![] };
            assert_eq!(UiFrame::decode(cmd, &g.encode(cmd)), Some(g));
        }
    }
}

#[cfg(test)]
mod capture_tests {
    use super::*;

    /// VarAC's relay notification as received (NR5P -> K5XYZ, text "<SMP>").
    #[test]
    fn relay_notification_frame() {
        let f = crate::varac::messages::ui_frame("NR5P", "K5XYZ", "<SMP>");
        let d = UiFrame::decode(VARAC_KISS_CMD, &f.encode(VARAC_KISS_CMD)).unwrap();
        assert_eq!((d.source.as_str(), d.dest.as_str(), &d.info[..]), ("NR5P", "K5XYZ", &b"<SMP>"[..]));
    }

    /// Exact bytes real VarAC 16.1.8 put on the KISS port for a broadcast "test 123".
    #[test]
    fn matches_varac_broadcast_capture() {
        // c0 01 | 82 98 98 40 40 40 40 60 | 9c a4 6a a0 40 40 40 61 | 03 f0 | "test 123" | c0
        let captured = b"\xc0\x01\x82\x98\x98\x40\x40\x40\x40\x60\x9c\xa4\x6a\xa0\x40\x40\x40\x61\x03\xf0test 123\xc0";
        let ours = kiss_encode(&crate::varac::messages::broadcast_frame("NR5P", "ALL", "test 123").encode(VARAC_KISS_CMD));
        assert_eq!(ours, captured.to_vec());
        let mut d = KissDecoder::default();
        let (cmd, raw) = d.feed(captured).remove(0);
        let f = UiFrame::decode(cmd, &raw).unwrap();
        assert_eq!((f.source.as_str(), f.dest.as_str(), &f.info[..]), ("NR5P", "ALL", &b"test 123"[..]));
    }
}
