//! PSKReporter spots, as VarAC sends them (through Philip Gladstone's PSKReporter.dll):
//! stations heard (CQ frames, beacons, broadcasts, the QSO partner) with frequency, SNR,
//! mode "VARAC" and locator, plus a self report. Packets follow PSKReporter's IPFIX
//! format (UDP to report.pskreporter.info:4739): a receiver record (us) and sender
//! records (who we heard), templates in the first packets and then hourly.

use std::time::{Duration, Instant};

pub const SERVER: &str = "report.pskreporter.info:4739";
/// VarAC's mode name on PSKReporter
pub const MODE: &str = "VARAC";
/// VarAC pushes queued spots at most every 45 s, and reports itself every 4 hours
const FLUSH_EVERY: Duration = Duration::from_secs(45);
const SELF_REPORT_EVERY: Duration = Duration::from_secs(4 * 3600);
const TEMPLATES_EVERY: Duration = Duration::from_secs(3600);

const RECEIVER_TEMPLATE: u16 = 0x9992;
const SENDER_TEMPLATE: u16 = 0x9993;
/// Spots without an SNR (the self report: VarAC passes an empty SNR)
const SENDER_TEMPLATE_NO_SNR: u16 = 0x9994;
/// Enterprise number of PSKReporter's information elements
const PEN: [u8; 4] = [0x00, 0x00, 0x76, 0x8F];

/// One station heard.
#[derive(Debug, Clone, PartialEq)]
pub struct Spot {
    pub call: String,
    pub freq_hz: u64,
    /// dB; None for a self report
    pub snr: Option<i32>,
    pub grid: String,
    /// UNIX seconds
    pub time: u32,
}

/// Us: callsign, locator and software (the receiver record).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Station {
    pub call: String,
    pub grid: String,
    pub software: String,
    /// VarAC's MY_ANTENNA
    pub antenna: String,
}

fn var(out: &mut Vec<u8>, s: &str) {
    let b = s.as_bytes();
    let n = b.len().min(254);
    out.push(n as u8);
    out.extend_from_slice(&b[..n]);
}

fn pad4(set: &mut Vec<u8>) {
    while set.len() % 4 != 0 {
        set.push(0);
    }
}

fn field(out: &mut Vec<u8>, id: u16, len: u16) {
    out.extend_from_slice(&(id | 0x8000).to_be_bytes());
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(&PEN);
}

/// Options template for the receiver: receiverCallsign, receiverLocator, decodingSoftware,
/// antennaInformation.
fn receiver_template() -> Vec<u8> {
    let mut t = vec![0, 3, 0, 0];
    t.extend_from_slice(&RECEIVER_TEMPLATE.to_be_bytes());
    t.extend_from_slice(&[0, 4, 0, 0]); // 4 fields, scope count 0
    field(&mut t, 2, 0xFFFF);
    field(&mut t, 4, 0xFFFF);
    field(&mut t, 8, 0xFFFF);
    field(&mut t, 9, 0xFFFF);
    pad4(&mut t);
    let len = t.len() as u16;
    t[2..4].copy_from_slice(&len.to_be_bytes());
    t
}

/// Template for spots: senderCallsign, frequency (4), sNR (1), mode, senderLocator,
/// informationSource (1), flowStartSeconds (IANA 150, 4).
fn sender_template(with_snr: bool) -> Vec<u8> {
    let mut t = vec![0, 2, 0, 0];
    t.extend_from_slice(&if with_snr { SENDER_TEMPLATE } else { SENDER_TEMPLATE_NO_SNR }.to_be_bytes());
    t.extend_from_slice(&[0, if with_snr { 7 } else { 6 }]);
    field(&mut t, 1, 0xFFFF);
    field(&mut t, 5, 4);
    if with_snr {
        field(&mut t, 6, 1);
    }
    field(&mut t, 10, 0xFFFF);
    field(&mut t, 3, 0xFFFF);
    field(&mut t, 11, 1);
    t.extend_from_slice(&[0x00, 0x96, 0x00, 0x04]);
    let len = t.len() as u16;
    t[2..4].copy_from_slice(&len.to_be_bytes());
    t
}

/// One IPFIX message: header, templates (when asked), our receiver record, the spots.
pub fn packet(station: &Station, spots: &[Spot], with_templates: bool, seq: u32, domain: u32, now: u32) -> Vec<u8> {
    let mut p = vec![0x00, 0x0A, 0, 0];
    p.extend_from_slice(&now.to_be_bytes());
    p.extend_from_slice(&seq.to_be_bytes());
    p.extend_from_slice(&domain.to_be_bytes());
    if with_templates {
        p.extend(receiver_template());
        p.extend(sender_template(true));
        p.extend(sender_template(false));
    }
    let mut rx = RECEIVER_TEMPLATE.to_be_bytes().to_vec();
    rx.extend_from_slice(&[0, 0]);
    var(&mut rx, &station.call);
    var(&mut rx, &station.grid);
    var(&mut rx, &station.software);
    var(&mut rx, &station.antenna);
    pad4(&mut rx);
    let len = rx.len() as u16;
    rx[2..4].copy_from_slice(&len.to_be_bytes());
    p.extend(rx);
    for with_snr in [true, false] {
        let set: Vec<&Spot> = spots.iter().filter(|s| s.snr.is_some() == with_snr).collect();
        if set.is_empty() {
            continue;
        }
        let mut tx = if with_snr { SENDER_TEMPLATE } else { SENDER_TEMPLATE_NO_SNR }.to_be_bytes().to_vec();
        tx.extend_from_slice(&[0, 0]);
        for s in set {
            var(&mut tx, &s.call);
            tx.extend_from_slice(&(s.freq_hz as u32).to_be_bytes());
            if let Some(snr) = s.snr {
                tx.push(snr.clamp(-128, 127) as i8 as u8);
            }
            var(&mut tx, MODE);
            var(&mut tx, &s.grid);
            tx.push(1); // informationSource: automatically extracted
            tx.extend_from_slice(&s.time.to_be_bytes());
        }
        pad4(&mut tx);
        let len = tx.len() as u16;
        tx[2..4].copy_from_slice(&len.to_be_bytes());
        p.extend(tx);
    }
    let len = p.len() as u16;
    p[2..4].copy_from_slice(&len.to_be_bytes());
    p
}

/// What the app tells the reporter thread.
pub enum Command {
    /// Our call / locator / software, and whether reporting (and self reports) are on
    Configure { station: Station, enabled: bool, self_report: bool },
    Heard(Spot),
    /// We changed frequency: VarAC reports itself again 60 s later
    Frequency(u64),
}

/// Start the reporter thread. Spots are queued and sent in batches (VarAC: when 45 s or
/// more have passed since the last send); our own station is reported every 4 hours
/// and a minute after a frequency change. Log lines go to `log`.
pub fn spawn(log: impl Fn(String) + Send + 'static) -> std::sync::mpsc::Sender<Command> {
    let (tx, rx) = std::sync::mpsc::channel::<Command>();
    std::thread::Builder::new()
        .name("pskreporter".into())
        .spawn(move || {
            let mut station = Station::default();
            let (mut enabled, mut self_report) = (false, false);
            let mut queue: Vec<Spot> = Vec::new();
            let mut last_send = Instant::now() - FLUSH_EVERY;
            let mut last_templates: Option<Instant> = None;
            let mut packets_with_templates = 0;
            let mut seq = 0u32;
            let domain = std::process::id().wrapping_mul(2_654_435_761) ^ 0x4F53_4C54;
            let mut freq = 0u64;
            let mut next_self: Option<Instant> = None;
            let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok();
            loop {
                match rx.recv_timeout(Duration::from_secs(5)) {
                    Ok(Command::Configure { station: s, enabled: e, self_report: sr }) => {
                        station = s;
                        (enabled, self_report) = (e, sr);
                        if e && sr && next_self.is_none() {
                            next_self = Some(Instant::now() + Duration::from_secs(60));
                        }
                    }
                    Ok(Command::Heard(spot)) => {
                        if enabled && !spot.call.eq_ignore_ascii_case(&station.call) {
                            queue.push(spot);
                        }
                    }
                    Ok(Command::Frequency(f)) => {
                        if f != freq {
                            freq = f;
                            if enabled && self_report {
                                next_self = Some(Instant::now() + Duration::from_secs(60));
                            }
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Err(_) => break,
                }
                let now_unix = || std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as u32).unwrap_or(0);
                if enabled && self_report && freq > 0 && next_self.is_some_and(|t| Instant::now() >= t) {
                    next_self = Some(Instant::now() + SELF_REPORT_EVERY);
                    queue.push(Spot { call: station.call.clone(), freq_hz: freq, snr: None, grid: station.grid.clone(), time: now_unix() });
                    log("PSKReporter: self report queued".into());
                }
                if !enabled || queue.is_empty() || last_send.elapsed() < FLUSH_EVERY || station.call.is_empty() {
                    continue;
                }
                let Some(sock) = sock.as_ref() else { continue };
                let with_templates = packets_with_templates < 3 || last_templates.is_none_or(|t| t.elapsed() >= TEMPLATES_EVERY);
                // (keep each datagram well under a typical MTU)
                let batch: Vec<Spot> = queue.drain(..queue.len().min(20)).collect();
                let pkt = packet(&station, &batch, with_templates, seq, domain, now_unix());
                log("PSKReporter: Sending accumulated records".into());
                match sock.send_to(&pkt, SERVER) {
                    Ok(_) => {
                        seq = seq.wrapping_add(batch.len() as u32);
                        if with_templates {
                            packets_with_templates += 1;
                            last_templates = Some(Instant::now());
                        }
                        for s in &batch {
                            log(format!("PSKReporter: Logged successfully ({})", s.call));
                        }
                    }
                    Err(e) => log(format!("PSKReporter: send failed: {e}")),
                }
                last_send = Instant::now();
            }
        })
        .ok();
    tx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_and_records_are_well_formed() {
        // PSKReporter's published layouts: receiver options template (with antenna) 0x2C long
        let r = receiver_template();
        assert_eq!(r.len(), 0x2C);
        assert_eq!(&r[..10], &[0x00, 0x03, 0x00, 0x2C, 0x99, 0x92, 0x00, 0x04, 0x00, 0x00]);
        assert_eq!(&r[10..18], &[0x80, 0x02, 0xFF, 0xFF, 0x00, 0x00, 0x76, 0x8F]);
        let s = sender_template(true);
        assert_eq!(s.len() % 4, 0);
        assert_eq!(&s[..8], &[0x00, 0x02, 0x00, s.len() as u8, 0x99, 0x93, 0x00, 0x07]);
        let s = sender_template(false);
        assert_eq!(s.len() % 4, 0);
        assert_eq!(&s[..8], &[0x00, 0x02, 0x00, s.len() as u8, 0x99, 0x94, 0x00, 0x06]);
        let station = Station { call: "W5TST".into(), grid: "EM10".into(), software: "Openslot".into(), antenna: "Dipole".into() };
        let spot = Spot { call: "NR5P".into(), freq_hz: 14_105_000, snr: Some(-7), grid: "EM12".into(), time: 1_800_000_000 };
        // (a self report: no SNR, in its own set)
        let me = Spot { call: "W5TST".into(), freq_hz: 14_105_000, snr: None, grid: "EM10".into(), time: 1_800_000_000 };
        let p = packet(&station, &[spot, me], true, 0, 1, 1_800_000_000);
        let at = p.windows(6).position(|w| w == b"\x05W5TST").map(|i| i + 6).unwrap();
        let at = p[at..].windows(6).position(|w| w == b"\x05W5TST").map(|i| at + i).unwrap();
        assert_eq!(&p[at + 10..at + 16], b"\x05VARAC");
        assert_eq!(&p[..2], &[0x00, 0x0A]);
        assert_eq!(u16::from_be_bytes([p[2], p[3]]) as usize, p.len());
        // every set is a multiple of 4 and the sets add up to the message
        let mut i = 16;
        while i < p.len() {
            let len = u16::from_be_bytes([p[i + 2], p[i + 3]]) as usize;
            assert!(len >= 4 && len % 4 == 0, "set at {i}: {len}");
            i += len;
        }
        assert_eq!(i, p.len());
        // the spot: callsign, frequency, SNR, mode
        let at = p.windows(5).position(|w| w == b"\x04NR5P").unwrap();
        assert_eq!(&p[at + 5..at + 9], &14_105_000u32.to_be_bytes());
        assert_eq!(p[at + 9], (-7i8) as u8);
        assert_eq!(&p[at + 10..at + 16], b"\x05VARAC");
    }
}
