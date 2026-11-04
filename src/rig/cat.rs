use std::io::{Read, Write};
use std::time::Duration;

use serialport::SerialPort;

use super::{CatModel, RadioMode, RigControl, RigError, RigResult};

pub struct CatSerial {
    /// The port from the settings
    port_name: String,
    /// What the connect log shows: the radio, and the port when it isn't the saved one
    name: String,
    baud: u32,
    model: CatModel,
    port: Option<Box<dyn SerialPort>>,
    /// Digit count of the `FA` frequency field, learned from the radio's own reply
    /// (FTDX3000/FT-950/FTDX1200: 8, FT-991/FT-891/FTDX10/101/FT-710: 9, Kenwood: 11).
    freq_digits: usize,
}

impl CatSerial {
    pub fn new(port_name: &str, baud: u32, model: CatModel) -> Self {
        Self {
            port_name: port_name.to_string(),
            name: model_name(model).to_string(),
            baud,
            model,
            port: None,
            freq_digits: match model {
                CatModel::Ftdx3000 => 8,
                CatModel::YaesuGeneric => 9,
                CatModel::KenwoodGeneric => 11,
                CatModel::Icom(_) => 10,
            },
        }
    }

    fn send_cat_query(&mut self, cmd: &str) -> RigResult<String> {
        let port = self.port.as_mut().ok_or(RigError::NotConnected)?;

        // Drop stale bytes (late replies, "?;" from a rejected set) before querying
        let _ = port.clear(serialport::ClearBuffer::Input);

        port.write_all(cmd.as_bytes())?;
        port.flush()?;

        std::thread::sleep(Duration::from_millis(50));

        let mut buf = [0u8; 256];
        let mut response = String::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);

        loop {
            match port.read(&mut buf) {
                Ok(n) if n > 0 => {
                    response.push_str(&String::from_utf8_lossy(&buf[..n]));
                    if response.ends_with(';') {
                        break;
                    }
                }
                Ok(_) => {}
                Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => {}
                Err(e) => return Err(RigError::Io(e)),
            }
            if std::time::Instant::now() > deadline {
                if response.is_empty() {
                    return Err(RigError::Timeout);
                }
                break;
            }
        }
        if response.trim() == "?;" {
            return Err(RigError::Protocol(format!("radio rejected {cmd}")));
        }
        // Keep only the reply to our command if other output got interleaved
        let prefix = &cmd[..2.min(cmd.len())];
        if let Some(i) = response.rfind(prefix) {
            response = response[i..].to_string();
        }
        Ok(response)
    }

    fn send_cat_set(&mut self, cmd: &str) -> RigResult<()> {
        let port = self.port.as_mut().ok_or(RigError::NotConnected)?;
        port.write_all(cmd.as_bytes())?;
        port.flush()?;
        std::thread::sleep(Duration::from_millis(50));
        Ok(())
    }

    /// One CI-V command to the radio and its reply (the command byte(s) and data, or FB for
    /// "OK"). Our own frames echoed back on the bus, and the radio's transceive broadcasts, are
    /// skipped.
    fn civ(&mut self, cmd: &[u8]) -> RigResult<Vec<u8>> {
        let CatModel::Icom(addr) = self.model else { return Err(RigError::Protocol("not a CI-V radio".into())) };
        let port = self.port.as_mut().ok_or(RigError::NotConnected)?;
        let _ = port.clear(serialport::ClearBuffer::Input);
        port.write_all(&civ_frame(addr, cmd))?;
        port.flush()?;
        let mut got = Vec::new();
        let mut buf = [0u8; 256];
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while std::time::Instant::now() < deadline {
            match port.read(&mut buf) {
                Ok(n) => got.extend_from_slice(&buf[..n]),
                Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => {}
                Err(e) => return Err(RigError::Io(e)),
            }
            if let Some(reply) = civ_frames(&got).into_iter().find(|f| f.0 == CIV_ME && f.1 == addr).map(|f| f.2) {
                if reply == [CIV_NG] {
                    return Err(RigError::Protocol(format!("radio rejected CI-V {cmd:02X?}")));
                }
                return Ok(reply);
            }
        }
        Err(RigError::Timeout)
    }

    /// The radio's CI-V address, asked for at the broadcast address and the usual ones.
    fn find_civ_address(&mut self) -> RigResult<u8> {
        let port = self.port.as_mut().ok_or(RigError::NotConnected)?;
        let _ = port.clear(serialport::ClearBuffer::Input);
        let mut out = civ_frame(0x00, &[0x03]);
        for a in super::CIV_ADDRESSES {
            out.extend(civ_frame(a, &[0x03]));
        }
        port.write_all(&out)?;
        port.flush()?;
        let mut got = Vec::new();
        let mut buf = [0u8; 256];
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while std::time::Instant::now() < deadline {
            match port.read(&mut buf) {
                Ok(n) => got.extend_from_slice(&buf[..n]),
                Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => {}
                Err(e) => return Err(RigError::Io(e)),
            }
            if let Some(a) = civ_answer(&got) {
                return Ok(a);
            }
        }
        Err(RigError::Timeout)
    }

    fn civ_mode(&mut self) -> RigResult<RadioMode> {
        let reply = self.civ(&[0x04])?;
        let mode = *reply.get(1).ok_or_else(|| RigError::Protocol(format!("unexpected: {reply:02X?}")))?;
        // DATA on (IC-7300 and later; older radios reject it)
        let data = self.civ(&[0x1A, 0x06]).ok().and_then(|r| r.get(2).copied()).unwrap_or(0) != 0;
        Ok(match (mode, data) {
            (0x00, false) => RadioMode::Lsb,
            (0x00, true) => RadioMode::LsbData,
            (0x01, false) => RadioMode::Usb,
            (0x01, true) => RadioMode::UsbData,
            (0x02, _) => RadioMode::Am,
            (0x03 | 0x07, _) => RadioMode::Cw,
            (0x05, false) => RadioMode::Fm,
            (0x05, true) => RadioMode::FmData,
            _ => RadioMode::Usb,
        })
    }

    fn set_civ_mode(&mut self, mode: RadioMode) -> RigResult<()> {
        let (m, data) = match mode {
            RadioMode::Lsb => (0x00, false),
            RadioMode::LsbData => (0x00, true),
            RadioMode::Usb => (0x01, false),
            RadioMode::UsbData => (0x01, true),
            RadioMode::Am => (0x02, false),
            RadioMode::Cw => (0x03, false),
            RadioMode::Fm => (0x05, false),
            RadioMode::FmData => (0x05, true),
        };
        self.civ(&[0x06, m])?;
        // (filter 1 with DATA on, as the radio's own D button)
        let set_data = self.civ(&[0x1A, 0x06, u8::from(data), u8::from(data)]);
        if data { set_data.map(|_| ()) } else { Ok(()) }
    }
}

/// Our CI-V address (the usual controller address)
const CIV_ME: u8 = 0xE0;
/// CI-V "NG": the radio rejected the command
const CIV_NG: u8 = 0xFA;

/// A CI-V frame from us to `to`: FE FE to E0 command… FD
pub(super) fn civ_frame(to: u8, cmd: &[u8]) -> Vec<u8> {
    let mut f = vec![0xFE, 0xFE, to, CIV_ME];
    f.extend_from_slice(cmd);
    f.push(0xFD);
    f
}

/// The complete frames in `bytes`: (to, from, command and data).
fn civ_frames(bytes: &[u8]) -> Vec<(u8, u8, Vec<u8>)> {
    let mut frames = Vec::new();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] != 0xFE || bytes[i + 1] != 0xFE {
            i += 1;
            continue;
        }
        let mut start = i + 2;
        // (a run of preamble bytes)
        while bytes.get(start) == Some(&0xFE) {
            start += 1;
        }
        let Some(len) = bytes[start..].iter().position(|&b| b == 0xFD) else { break };
        let body = &bytes[start..start + len];
        if body.len() >= 3 {
            frames.push((body[0], body[1], body[2..].to_vec()));
        }
        i = start + len + 1;
    }
    frames
}

/// The address of a radio that answered our frequency query, in what came back.
pub(super) fn civ_answer(bytes: &[u8]) -> Option<u8> {
    civ_frames(bytes).into_iter().find(|f| f.0 == CIV_ME && f.1 != CIV_ME && f.2.first() == Some(&0x03) && f.2.len() >= 5).map(|f| f.1)
}

/// CI-V frequency: 5 BCD bytes, least significant first (some radios send 4 below 1 GHz).
fn civ_freq(data: &[u8]) -> RigResult<u64> {
    if data.len() < 4 {
        return Err(RigError::Protocol(format!("bad CI-V frequency {data:02X?}")));
    }
    let mut hz = 0u64;
    for &b in data.iter().take(5).rev() {
        let (hi, lo) = (b >> 4, b & 0x0F);
        if hi > 9 || lo > 9 {
            return Err(RigError::Protocol(format!("bad CI-V frequency {data:02X?}")));
        }
        hz = hz * 100 + u64::from(hi) * 10 + u64::from(lo);
    }
    Ok(hz)
}

fn civ_freq_bytes(hz: u64) -> [u8; 5] {
    let mut out = [0u8; 5];
    let mut f = hz;
    for b in &mut out {
        let lo = (f % 10) as u8;
        f /= 10;
        let hi = (f % 10) as u8;
        f /= 10;
        *b = (hi << 4) | lo;
    }
    out
}

impl RigControl for CatSerial {
    fn connect(&mut self) -> RigResult<()> {
        // Yaesu CAT is spec'd 8N2; Kenwood/QMX 8N1. (Receivers only check the first stop bit,
        // so 2 is safe either way on the RX side.)
        let stop_bits = match self.model {
            CatModel::KenwoodGeneric | CatModel::Icom(_) => serialport::StopBits::One,
            _ => serialport::StopBits::Two,
        };
        // A saved Linux /dev/ttyUSBn that is gone (the radio came back under another number): use
        // the radio's CAT port where it is now, if that's unambiguous.
        let port_name = match super::replacement_cat_port(&self.port_name) {
            Some(found) => {
                let now = std::fs::canonicalize(&found).map_or_else(|_| found.clone(), |p| p.to_string_lossy().into_owned());
                self.name = format!("{} on {now} ({} is gone)", model_name(self.model), self.port_name);
                found
            }
            None => {
                self.name = model_name(self.model).to_string();
                self.port_name.clone()
            }
        };
        let mut port = serialport::new(&port_name, self.baud)
            .timeout(Duration::from_millis(300))
            .stop_bits(stop_bits)
            .data_bits(serialport::DataBits::Eight)
            .parity(serialport::Parity::None)
            .flow_control(serialport::FlowControl::None)
            .open()
            .map_err(|e| RigError::Io(std::io::Error::new(std::io::ErrorKind::Other, format!("{port_name}: {e}"))))?;

        // Yaesu radios with "CAT RTS" enabled (the default) ignore commands unless RTS is
        // asserted. On the Enhanced (CAT) port RTS is not a keying line, so this is safe.
        // DTR stays low so a port wired for DTR keying can't transmit on open.
        let _ = port.write_data_terminal_ready(false);
        // (An Icom can key on RTS or DTR, by its USB SEND / USB keying settings: both stay low.)
        let _ = port.write_request_to_send(!matches!(self.model, CatModel::KenwoodGeneric | CatModel::Icom(_)));

        self.port = Some(port);

        if self.model == CatModel::Icom(0) {
            match self.find_civ_address() {
                Ok(a) => {
                    self.model = CatModel::Icom(a);
                    self.name = format!("{} ({a:02X}h)", self.name);
                }
                Err(_) => {
                    self.port = None;
                    return Err(RigError::Protocol(format!(
                        "no CI-V reply on {port_name} at {} baud — check port, baud rate and the radio's CI-V settings",
                        self.baud
                    )));
                }
            }
        }

        // Verify by reading frequency; this also learns the FA field width.
        if let Err(e) = self.get_freq() {
            self.port = None;
            return Err(match e {
                RigError::Timeout => RigError::Protocol(format!(
                    "no CAT reply on {} at {} baud — check port, baud rate and the radio's CAT RATE menu",
                    port_name, self.baud
                )),
                e => e,
            });
        }
        Ok(())
    }

    fn disconnect(&mut self) {
        self.port = None;
    }

    fn is_connected(&self) -> bool {
        self.port.is_some()
    }

    fn get_freq(&mut self) -> RigResult<u64> {
        match self.model {
            CatModel::Ftdx3000 | CatModel::YaesuGeneric => {
                let reply = self.send_cat_query("FA;")?;
                let digits = freq_field_width(&reply);
                let hz = parse_yaesu_freq(&reply)?;
                if let Some(d) = digits {
                    self.freq_digits = d;
                }
                Ok(hz)
            }
            CatModel::KenwoodGeneric => {
                let reply = self.send_cat_query("FA;")?;
                parse_kenwood_freq(&reply)
            }
            CatModel::Icom(_) => {
                let reply = self.civ(&[0x03])?;
                civ_freq(&reply[1..])
            }
        }
    }

    fn set_freq(&mut self, hz: u64) -> RigResult<()> {
        match self.model {
            CatModel::Ftdx3000 | CatModel::YaesuGeneric => {
                let cmd = format!("FA{:0width$};", hz, width = self.freq_digits);
                self.send_cat_set(&cmd)
            }
            CatModel::KenwoodGeneric => {
                let cmd = format!("FA{:011};", hz);
                self.send_cat_set(&cmd)
            }
            CatModel::Icom(_) => {
                let mut cmd = vec![0x05];
                cmd.extend(civ_freq_bytes(hz));
                self.civ(&cmd).map(|_| ())
            }
        }
    }

    fn get_mode(&mut self) -> RigResult<RadioMode> {
        match self.model {
            CatModel::Ftdx3000 | CatModel::YaesuGeneric => {
                let reply = self.send_cat_query("MD0;")?;
                parse_yaesu_mode(&reply)
            }
            CatModel::KenwoodGeneric => {
                let reply = self.send_cat_query("MD;")?;
                parse_kenwood_mode(&reply)
            }
            CatModel::Icom(_) => self.civ_mode(),
        }
    }

    fn set_mode(&mut self, mode: RadioMode) -> RigResult<()> {
        match self.model {
            CatModel::Ftdx3000 | CatModel::YaesuGeneric => {
                let digit = yaesu_mode_digit(mode);
                let cmd = format!("MD0{digit};");
                self.send_cat_set(&cmd)
            }
            CatModel::KenwoodGeneric => {
                let digit = kenwood_mode_digit(mode);
                let cmd = format!("MD{digit};");
                self.send_cat_set(&cmd)
            }
            CatModel::Icom(_) => self.set_civ_mode(mode),
        }
    }

    fn set_ptt(&mut self, on: bool) -> RigResult<()> {
        match self.model {
            CatModel::Ftdx3000 | CatModel::YaesuGeneric => {
                let cmd = if on { "TX1;" } else { "TX0;" };
                self.send_cat_set(cmd)
            }
            CatModel::KenwoodGeneric => {
                let cmd = if on { "TX;" } else { "RX;" };
                self.send_cat_set(cmd)
            }
            CatModel::Icom(_) => self.civ(&[0x1C, 0x00, u8::from(on)]).map(|_| ()),
        }
    }

    fn get_alc(&mut self) -> RigResult<f32> {
        match self.model {
            // RM4 = the ALC meter, 000…255 (FTDX101/FT-710 add a second field)
            CatModel::Ftdx3000 | CatModel::YaesuGeneric => {
                let reply = self.send_cat_query("RM4;")?;
                parse_yaesu_alc(&reply)
            }
            // RM reads the meter the radio shows: only an ALC one (P1 = 3, 0…30) counts
            CatModel::KenwoodGeneric => {
                let reply = self.send_cat_query("RM;")?;
                parse_kenwood_alc(&reply)
            }
            // 15 13 = the ALC meter, BCD 0000…0120 across the ALC zone (Hamlib's IC-7300 scale)
            CatModel::Icom(_) => {
                let reply = self.civ(&[0x15, 0x13])?;
                civ_alc(&reply)
            }
        }
    }

    fn name(&self) -> &str {
        &self.name
    }
}

fn parse_yaesu_alc(reply: &str) -> RigResult<f32> {
    let digits = reply.trim().strip_prefix("RM4").and_then(|d| d.get(..3)).filter(|d| d.bytes().all(|b| b.is_ascii_digit()));
    let raw: u32 = digits.and_then(|d| d.parse().ok()).ok_or_else(|| RigError::Protocol(format!("unexpected ALC reply: {reply}")))?;
    Ok((raw as f32 / 255.0).min(1.0))
}

fn civ_alc(reply: &[u8]) -> RigResult<f32> {
    let bcd = match reply {
        [0x15, 0x13, hi, lo, ..] => [*hi, *lo],
        _ => return Err(RigError::Protocol(format!("unexpected ALC reply: {reply:02X?}"))),
    };
    let raw = bcd.iter().fold(0u32, |n, b| n * 100 + u32::from(b >> 4) * 10 + u32::from(b & 0x0F));
    Ok((raw as f32 / 120.0).min(1.0))
}

fn parse_kenwood_alc(reply: &str) -> RigResult<f32> {
    let s = reply.trim().trim_end_matches(';');
    let raw = s.strip_prefix("RM3").filter(|d| d.len() == 4).and_then(|d| d.parse::<u32>().ok())
        .ok_or_else(|| RigError::Protocol(format!("the radio's meter isn't on ALC ({reply})")))?;
    Ok((raw as f32 / 30.0).min(1.0))
}

fn model_name(model: CatModel) -> &'static str {
    match model {
        CatModel::Ftdx3000 => "FTDX3000 CAT",
        CatModel::YaesuGeneric => "Yaesu CAT",
        CatModel::KenwoodGeneric => "Kenwood/QMX CAT",
        CatModel::Icom(_) => "Icom CI-V",
    }
}

fn parse_yaesu_freq(reply: &str) -> RigResult<u64> {
    // FA14074000; or FA014074000;
    let s = reply.trim().trim_end_matches(';');
    if s.len() >= 3 && s.starts_with("FA") {
        s[2..]
            .parse::<u64>()
            .map_err(|e| RigError::Protocol(format!("bad freq: {e}")))
    } else {
        Err(RigError::Protocol(format!("unexpected: {reply}")))
    }
}

/// Number of digits in an `FA...;` reply, if it looks like one.
fn freq_field_width(reply: &str) -> Option<usize> {
    let s = reply.trim().trim_end_matches(';');
    let digits = s.strip_prefix("FA")?;
    (digits.len() >= 8 && digits.bytes().all(|b| b.is_ascii_digit())).then_some(digits.len())
}

fn parse_kenwood_freq(reply: &str) -> RigResult<u64> {
    // FA00014074000;
    let s = reply.trim().trim_end_matches(';');
    if s.len() >= 3 && s.starts_with("FA") {
        s[2..]
            .trim_start_matches('0')
            .parse::<u64>()
            .or_else(|_| Ok(0))
            .map_err(|e: std::num::ParseIntError| RigError::Protocol(format!("bad freq: {e}")))
    } else {
        Err(RigError::Protocol(format!("unexpected: {reply}")))
    }
}

fn parse_yaesu_mode(reply: &str) -> RigResult<RadioMode> {
    // MD0X; where X: 1=LSB 2=USB 3=CW 4=FM 5=AM 6=RTTY-LSB 7=CW-R 8=DATA-LSB 9=RTTY-USB A=DATA-FM C=DATA-USB E=DATA-FM-N
    let s = reply.trim().trim_end_matches(';');
    let digit = s.chars().last().unwrap_or('0');
    match digit {
        '1' => Ok(RadioMode::Lsb),
        '2' => Ok(RadioMode::Usb),
        '3' | '7' => Ok(RadioMode::Cw),
        '4' => Ok(RadioMode::Fm),
        '5' => Ok(RadioMode::Am),
        '8' => Ok(RadioMode::LsbData),
        'C' | 'c' => Ok(RadioMode::UsbData),
        'A' | 'a' | 'E' | 'e' => Ok(RadioMode::FmData),
        _ => Ok(RadioMode::Usb),
    }
}

fn parse_kenwood_mode(reply: &str) -> RigResult<RadioMode> {
    // MDn; where n: 1=LSB 2=USB 3=CW 4=FM 5=AM 6=DATA 7=CW-R 9=DATA-R
    let s = reply.trim().trim_end_matches(';');
    let digit = s.chars().last().unwrap_or('0');
    match digit {
        '1' => Ok(RadioMode::Lsb),
        '2' => Ok(RadioMode::Usb),
        '3' | '7' => Ok(RadioMode::Cw),
        '4' => Ok(RadioMode::Fm),
        '5' => Ok(RadioMode::Am),
        '6' => Ok(RadioMode::UsbData),
        '9' => Ok(RadioMode::LsbData),
        _ => Ok(RadioMode::Usb),
    }
}

fn yaesu_mode_digit(mode: RadioMode) -> char {
    match mode {
        RadioMode::Lsb => '1',
        RadioMode::Usb => '2',
        RadioMode::Cw => '3',
        RadioMode::Fm => '4',
        RadioMode::Am => '5',
        RadioMode::LsbData => '8',
        RadioMode::UsbData => 'C',
        // (DATA-FM)
        RadioMode::FmData => 'A',
    }
}

fn kenwood_mode_digit(mode: RadioMode) -> char {
    match mode {
        RadioMode::Lsb => '1',
        RadioMode::Usb => '2',
        RadioMode::Cw => '3',
        RadioMode::Fm => '4',
        RadioMode::Am => '5',
        RadioMode::UsbData => '6',
        RadioMode::LsbData => '9',
        RadioMode::FmData => '4',
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn learns_freq_width() {
        assert_eq!(freq_field_width("FA14074000;"), Some(8));
        assert_eq!(freq_field_width("FA014074000;"), Some(9));
        assert_eq!(freq_field_width("FA00014074000;"), Some(11));
        assert_eq!(freq_field_width("?;"), None);
        assert_eq!(parse_yaesu_freq("FA14074000;").unwrap(), 14_074_000);
        assert_eq!(parse_kenwood_freq("FA00014074000;").unwrap(), 14_074_000);
    }

    #[test]
    fn civ_frames_and_frequency() {
        assert_eq!(civ_frame(0x94, &[0x03]), [0xFE, 0xFE, 0x94, 0xE0, 0x03, 0xFD]);
        // 14.074 MHz is 00 40 07 14 00
        assert_eq!(civ_freq_bytes(14_074_000), [0x00, 0x40, 0x07, 0x14, 0x00]);
        assert_eq!(civ_freq(&[0x00, 0x40, 0x07, 0x14, 0x00]).unwrap(), 14_074_000);
        // Our echoed query, a transceive broadcast and then the IC-7300's answer
        let mut bus = civ_frame(0x94, &[0x03]);
        bus.extend([0xFE, 0xFE, 0x00, 0x94, 0x00, 0x00, 0x40, 0x07, 0x14, 0x00, 0xFD]);
        assert_eq!(civ_answer(&bus), None);
        bus.extend([0xFE, 0xFE, 0xE0, 0x94, 0x03, 0x00, 0x40, 0x07, 0x14, 0x00, 0xFD]);
        assert_eq!(civ_answer(&bus), Some(0x94));
        assert_eq!(civ_answer(b"ID0462;"), None);
    }

    #[test]
    fn reads_alc_meters() {
        assert_eq!(parse_yaesu_alc("RM4000;").unwrap(), 0.0);
        assert_eq!(parse_yaesu_alc("RM4255;").unwrap(), 1.0);
        assert!((parse_yaesu_alc("RM4051000;").unwrap() - 0.2).abs() < 1e-6);
        assert!(parse_yaesu_alc("?;").is_err());
        assert_eq!(parse_kenwood_alc("RM30015;").unwrap(), 0.5);
        assert!(parse_kenwood_alc("RM10005;").is_err());
        assert_eq!(civ_alc(&[0x15, 0x13, 0x00, 0x60]).unwrap(), 0.5);
        assert_eq!(civ_alc(&[0x15, 0x13, 0x02, 0x55]).unwrap(), 1.0);
        assert!(civ_alc(&[0xFB]).is_err());
        // (a reply with no digits, a short one, or over full scale)
        assert!(parse_yaesu_alc("RM4;").is_err());
        assert!(parse_yaesu_alc("RM40;").is_err());
        assert!(parse_yaesu_alc("RM1100;").is_err());
        assert_eq!(parse_yaesu_alc(" RM4128; ").unwrap(), 128.0 / 255.0);
        assert!(parse_kenwood_alc("RM3;").is_err());
        assert!(parse_kenwood_alc("RM30x15;").is_err());
        assert_eq!(parse_kenwood_alc("RM30040;").unwrap(), 1.0, "capped at full scale");
        assert_eq!(civ_alc(&[0x15, 0x13, 0x01, 0x20]).unwrap(), 1.0);
        assert_eq!(civ_alc(&[0x15, 0x13, 0x00, 0x00]).unwrap(), 0.0);
        assert!(civ_alc(&[0x15, 0x13, 0x00]).is_err());
        assert!(civ_alc(&[0x15, 0x02, 0x00, 0x60]).is_err(), "another meter's reply");
    }

    #[test]
    fn ftdx3000_defaults_to_8_digits() {
        let c = CatSerial::new("/dev/null", 38400, CatModel::Ftdx3000);
        assert_eq!(format!("FA{:0w$};", 14_105_000u64, w = c.freq_digits), "FA14105000;");
    }
}
