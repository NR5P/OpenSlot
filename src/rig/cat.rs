use std::io::{Read, Write};
use std::time::Duration;

use serialport::SerialPort;

use super::{CatModel, RadioMode, RigControl, RigError, RigResult};

pub struct CatSerial {
    port_name: String,
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
            baud,
            model,
            port: None,
            freq_digits: match model {
                CatModel::Ftdx3000 => 8,
                CatModel::YaesuGeneric => 9,
                CatModel::KenwoodGeneric => 11,
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
}

impl RigControl for CatSerial {
    fn connect(&mut self) -> RigResult<()> {
        // Yaesu CAT is spec'd 8N2; Kenwood/QMX 8N1. (Receivers only check the first stop bit,
        // so 2 is safe either way on the RX side.)
        let stop_bits = match self.model {
            CatModel::KenwoodGeneric => serialport::StopBits::One,
            _ => serialport::StopBits::Two,
        };
        let mut port = serialport::new(&self.port_name, self.baud)
            .timeout(Duration::from_millis(300))
            .stop_bits(stop_bits)
            .data_bits(serialport::DataBits::Eight)
            .parity(serialport::Parity::None)
            .flow_control(serialport::FlowControl::None)
            .open()
            .map_err(|e| RigError::Io(std::io::Error::new(std::io::ErrorKind::Other, format!("{}: {e}", self.port_name))))?;

        // Yaesu radios with "CAT RTS" enabled (the default) ignore commands unless RTS is
        // asserted. On the Enhanced (CAT) port RTS is not a keying line, so this is safe.
        // DTR stays low so a port wired for DTR keying can't transmit on open.
        let _ = port.write_data_terminal_ready(false);
        let _ = port.write_request_to_send(!matches!(self.model, CatModel::KenwoodGeneric));

        self.port = Some(port);

        // Verify by reading frequency; this also learns the FA field width.
        if let Err(e) = self.get_freq() {
            self.port = None;
            return Err(match e {
                RigError::Timeout => RigError::Protocol(format!(
                    "no CAT reply on {} at {} baud — check port, baud rate and the radio's CAT RATE menu",
                    self.port_name, self.baud
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
        }
    }

    fn name(&self) -> &str {
        match self.model {
            CatModel::Ftdx3000 => "FTDX3000 CAT",
            CatModel::YaesuGeneric => "Yaesu CAT",
            CatModel::KenwoodGeneric => "Kenwood/QMX CAT",
        }
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
    fn ftdx3000_defaults_to_8_digits() {
        let c = CatSerial::new("/dev/null", 38400, CatModel::Ftdx3000);
        assert_eq!(format!("FA{:0w$};", 14_105_000u64, w = c.freq_digits), "FA14105000;");
    }
}
