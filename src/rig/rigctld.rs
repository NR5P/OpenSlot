//! Hamlib `rigctld` network client. Also works with RigHelm's built-in
//! rigctld-compatible server (default `127.0.0.1:4532`), which covers ~30
//! Yaesu/Icom/Kenwood/Elecraft radios while sharing the CAT port with other apps.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::Duration;

use super::{Agc, Level, Meter, RadioMode, RigControl, RigError, RigResult, Switch};

pub struct RigctldClient {
    host: String,
    port: u16,
    /// Writer half plus a persistent reader. The reader must outlive each call:
    /// a fresh `BufReader` per command would swallow buffered extra lines
    /// (e.g. the passband line after `m`) and desync every later reply.
    conn: Option<(TcpStream, BufReader<TcpStream>)>,
    /// The preamp / attenuator step (dB) the radio accepted or reported, to switch back on
    steps: std::collections::HashMap<&'static str, u32>,
}

impl RigctldClient {
    pub fn new(host: &str, port: u16) -> Self {
        Self {
            host: host.to_string(),
            port,
            conn: None,
            steps: Default::default(),
        }
    }

    /// A one-line reply (`l`, `u`): its value, or the error code as an error.
    fn query_value(&mut self, cmd: &str) -> RigResult<String> {
        let reply = self.send_cmd(cmd)?;
        if reply.starts_with("RPRT") {
            Self::check_rprt(&reply)?;
            return Err(RigError::Protocol(reply));
        }
        Ok(reply.trim().to_string())
    }

    fn query_f32(&mut self, cmd: &str) -> RigResult<f32> {
        let v = self.query_value(cmd)?;
        v.parse::<f32>().map_err(|_| RigError::Protocol(format!("bad value {v:?} for {cmd}")))
    }

    fn set(&mut self, cmd: &str) -> RigResult<()> {
        let reply = self.send_cmd(cmd)?;
        Self::check_rprt(&reply)
    }

    fn read_line(&mut self) -> RigResult<String> {
        let (_, reader) = self.conn.as_mut().ok_or(RigError::NotConnected)?;
        let mut line = String::new();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            self.conn = None;
            return Err(RigError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "rigctld closed the connection",
            )));
        }
        Ok(line.trim_end().to_string())
    }

    fn send_cmd(&mut self, cmd: &str) -> RigResult<String> {
        let (writer, _) = self.conn.as_mut().ok_or(RigError::NotConnected)?;
        writer.write_all(format!("{cmd}\n").as_bytes())?;
        writer.flush()?;
        self.read_line()
    }

    fn check_rprt(reply: &str) -> RigResult<()> {
        match reply.strip_prefix("RPRT ") {
            Some(code) => match code.trim().parse::<i32>().unwrap_or(-1) {
                0 => Ok(()),
                code => Err(RigError::Protocol(format!("rigctld error code {code}"))),
            },
            None => Ok(()),
        }
    }
}

impl RigControl for RigctldClient {
    fn connect(&mut self) -> RigResult<()> {
        let stream = crate::vara::connect_any(&self.host, self.port, Duration::from_secs(3))?;
        stream.set_read_timeout(Some(Duration::from_secs(3)))?;
        stream.set_write_timeout(Some(Duration::from_secs(3)))?;
        stream.set_nodelay(true)?;
        let reader = BufReader::new(stream.try_clone()?);
        self.conn = Some((stream, reader));

        // Verify the server actually answers before reporting success.
        if let Err(e) = self.get_freq() {
            self.disconnect();
            return Err(e);
        }
        Ok(())
    }

    fn disconnect(&mut self) {
        if let Some((s, _)) = self.conn.take() {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
    }

    fn is_connected(&self) -> bool {
        self.conn.is_some()
    }

    fn get_freq(&mut self) -> RigResult<u64> {
        let reply = self.send_cmd("f")?;
        if reply.starts_with("RPRT") {
            return Err(RigError::Protocol(reply));
        }
        // Some servers report "14074000.000000"
        reply
            .trim()
            .parse::<f64>()
            .map(|f| f as u64)
            .map_err(|e| RigError::Protocol(format!("bad frequency {reply:?}: {e}")))
    }

    fn set_freq(&mut self, hz: u64) -> RigResult<()> {
        let reply = self.send_cmd(&format!("F {hz}"))?;
        Self::check_rprt(&reply)
    }

    fn get_mode(&mut self) -> RigResult<RadioMode> {
        let reply = self.send_cmd("m")?;
        if reply.starts_with("RPRT") {
            return Err(RigError::Protocol(reply));
        }
        // Second line is the passband width; consume it to keep the stream in sync.
        let _passband = self.read_line()?;
        parse_rigctld_mode(&reply)
    }

    fn set_mode(&mut self, mode: RadioMode) -> RigResult<()> {
        // Passband -1 = leave the radio's filter alone. 0 would mean "the mode's default",
        // and Hamlib then narrows the receive filter (2400 → 2000 Hz on an FTDX3000).
        let reply = self.send_cmd(&format!("M {} -1", rigctld_mode_str(mode)))?;
        Self::check_rprt(&reply)
    }

    fn set_ptt(&mut self, on: bool) -> RigResult<()> {
        let reply = self.send_cmd(if on { "T 1" } else { "T 0" })?;
        Self::check_rprt(&reply)
    }

    /// VarAC's Hamlib tune: `G TUNE`
    fn tune_atu(&mut self) -> RigResult<()> {
        let reply = self.send_cmd("G TUNE")?;
        Self::check_rprt(&reply)
    }

    /// Hamlib's ALC level (0…1)
    fn get_alc(&mut self) -> RigResult<f32> {
        let reply = self.send_cmd("l ALC")?;
        if reply.starts_with("RPRT") {
            return Err(RigError::Protocol(reply));
        }
        reply.trim().parse::<f32>().map_err(|e| RigError::Protocol(format!("bad ALC level {reply:?}: {e}")))
    }

    fn get_level(&mut self, level: Level) -> RigResult<f32> {
        self.query_f32(&format!("l {}", level_name(level)))
    }

    fn set_level(&mut self, level: Level, value: f32) -> RigResult<()> {
        self.set(&format!("L {} {value:.3}", level_name(level)))
    }

    fn get_switch(&mut self, switch: Switch) -> RigResult<bool> {
        match switch {
            Switch::NoiseBlanker => Ok(self.query_value("u NB")? == "1"),
            Switch::NoiseReduction => Ok(self.query_value("u NR")? == "1"),
            // Hamlib has these as levels in dB (0 = off)
            Switch::Preamp | Switch::Attenuator => {
                let name = step_name(switch);
                let db = self.query_f32(&format!("l {name}"))? as u32;
                if db > 0 {
                    self.steps.insert(name, db);
                }
                Ok(db > 0)
            }
        }
    }

    fn set_switch(&mut self, switch: Switch, on: bool) -> RigResult<()> {
        match switch {
            Switch::NoiseBlanker => self.set(&format!("U NB {}", on as u8)),
            Switch::NoiseReduction => self.set(&format!("U NR {}", on as u8)),
            Switch::Preamp | Switch::Attenuator => {
                let name = step_name(switch);
                if !on {
                    return self.set(&format!("L {name} 0"));
                }
                // Hamlib takes only the radio's own steps: the one seen before, else the usual ones
                let tries: Vec<u32> = match self.steps.get(name) {
                    Some(&db) => vec![db],
                    None if switch == Switch::Preamp => vec![10, 20, 12, 6, 1],
                    None => vec![6, 12, 10, 20, 18, 3, 1],
                };
                let mut last = Err(unsupported_step());
                for db in tries {
                    last = self.set(&format!("L {name} {db}"));
                    if last.is_ok() {
                        self.steps.insert(name, db);
                        break;
                    }
                    if last.as_ref().is_err_and(|e| e.is_link_failure()) {
                        break;
                    }
                }
                last
            }
        }
    }

    fn get_agc(&mut self) -> RigResult<Agc> {
        // Hamlib's AGC levels: 0 off, 1 superfast, 2 fast, 3 slow, 4 user, 5 medium, 6 auto
        Ok(match self.query_f32("l AGC")? as i32 {
            0 => Agc::Off,
            1 | 2 => Agc::Fast,
            3 => Agc::Slow,
            5 => Agc::Medium,
            _ => Agc::Auto,
        })
    }

    fn set_agc(&mut self, agc: Agc) -> RigResult<()> {
        let n = match agc {
            Agc::Off => 0,
            Agc::Fast => 2,
            Agc::Medium => 5,
            Agc::Slow => 3,
            Agc::Auto => 6,
        };
        self.set(&format!("L AGC {n}"))
    }

    fn get_meter(&mut self, meter: Meter) -> RigResult<f32> {
        match meter {
            // dB relative to S9, as Hamlib reports it
            Meter::Signal => self.query_f32("l STRENGTH"),
            Meter::Power => self.query_f32("l RFPOWER_METER"),
            Meter::Swr => self.query_f32("l SWR"),
            Meter::Alc => self.query_f32("l ALC"),
        }
    }

    fn name(&self) -> &str {
        "rigctld"
    }
}

fn level_name(level: Level) -> &'static str {
    match level {
        Level::RfPower => "RFPOWER",
        Level::AfGain => "AF",
        Level::RfGain => "RF",
        Level::Squelch => "SQL",
        Level::MicGain => "MICGAIN",
    }
}

fn step_name(switch: Switch) -> &'static str {
    if switch == Switch::Preamp { "PREAMP" } else { "ATT" }
}

fn unsupported_step() -> RigError {
    RigError::Protocol("the radio took none of the usual steps".into())
}

fn parse_rigctld_mode(s: &str) -> RigResult<RadioMode> {
    let upper = s.trim().to_uppercase();
    let mode_part = upper.split_whitespace().next().unwrap_or("");
    match mode_part {
        "USB" => Ok(RadioMode::Usb),
        "LSB" => Ok(RadioMode::Lsb),
        "PKTUSB" | "USB-D" => Ok(RadioMode::UsbData),
        "PKTLSB" | "LSB-D" => Ok(RadioMode::LsbData),
        "CW" | "CWR" => Ok(RadioMode::Cw),
        "AM" | "PKTAM" => Ok(RadioMode::Am),
        "PKTFM" | "PKTFMN" => Ok(RadioMode::FmData),
        "FM" | "WFM" | "FMN" => Ok(RadioMode::Fm),
        // RTTY isn't modelled; report the sideband it lives on
        "RTTY" => Ok(RadioMode::Lsb),
        "RTTYR" => Ok(RadioMode::Usb),
        other => Err(RigError::Protocol(format!("unknown mode: {other}"))),
    }
}

fn rigctld_mode_str(mode: RadioMode) -> &'static str {
    match mode {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::net::TcpListener;

    /// Minimal fake rigctld that answers like the real thing (two-line `m` reply).
    fn fake_server() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = [0u8; 256];
            let mut pending = String::new();
            let mut freq = 14_105_000u64;
            loop {
                let n = match s.read(&mut buf) {
                    Ok(0) | Err(_) => return,
                    Ok(n) => n,
                };
                pending.push_str(&String::from_utf8_lossy(&buf[..n]));
                while let Some(i) = pending.find('\n') {
                    let line: String = pending.drain(..=i).collect();
                    let line = line.trim();
                    let reply = if line == "f" {
                        format!("{freq}\n")
                    } else if let Some(f) = line.strip_prefix("F ") {
                        freq = f.parse().unwrap();
                        "RPRT 0\n".into()
                    } else if line == "m" {
                        "PKTUSB\n3000\n".into()
                    } else if let Some(m) = line.strip_prefix("M ") {
                        // Only "keep the passband" (-1) is accepted: any width changes the filter
                        if m.ends_with(" -1") { "RPRT 0\n".into() } else { "RPRT -1\n".into() }
                    } else if line.starts_with('T') {
                        "RPRT 0\n".into()
                    } else if line == "l ALC" {
                        "0.250000\n".into()
                    } else if line == "l RFPOWER" {
                        "0.500000\n".into()
                    } else if line == "l STRENGTH" {
                        "-12\n".into()
                    } else if line == "u NB" {
                        "1\n".into()
                    } else if line == "l PREAMP" {
                        "0\n".into()
                    } else if line == "L PREAMP 20" || line.starts_with("L RFPOWER ") {
                        "RPRT 0\n".into()
                    } else {
                        "RPRT -11\n".into()
                    };
                    s.write_all(reply.as_bytes()).unwrap();
                }
            }
        });
        port
    }

    #[test]
    fn stays_in_sync_across_multiline_replies() {
        let port = fake_server();
        let mut c = RigctldClient::new("localhost", port);
        c.connect().unwrap();
        assert_eq!(c.get_mode().unwrap(), RadioMode::UsbData);
        assert_eq!(c.get_freq().unwrap(), 14_105_000);
        c.set_freq(7_105_000).unwrap();
        assert_eq!(c.get_mode().unwrap(), RadioMode::UsbData);
        assert_eq!(c.get_freq().unwrap(), 7_105_000);
        c.set_ptt(true).unwrap();
        assert_eq!(c.get_alc().unwrap(), 0.25);
        c.set_ptt(false).unwrap();
    }

    /// Setting the mode never touches the radio's receive filter width.
    #[test]
    fn set_mode_keeps_the_passband() {
        let port = fake_server();
        let mut c = RigctldClient::new("localhost", port);
        c.connect().unwrap();
        c.set_mode(RadioMode::UsbData).unwrap();
        c.set_mode(RadioMode::Usb).unwrap();
    }

    /// The extra controls: values come back in Hamlib's units, and a refused one is an error.
    #[test]
    fn extra_controls() {
        let port = fake_server();
        let mut c = RigctldClient::new("localhost", port);
        c.connect().unwrap();
        assert_eq!(c.get_level(Level::RfPower).unwrap(), 0.5);
        c.set_level(Level::RfPower, 0.25).unwrap();
        assert!(c.get_level(Level::Squelch).is_err());
        assert_eq!(c.get_meter(Meter::Signal).unwrap(), -12.0);
        assert!(c.get_switch(Switch::NoiseBlanker).unwrap());
        assert!(!c.get_switch(Switch::Preamp).unwrap());
        // (10 dB refused, then 20 taken)
        c.set_switch(Switch::Preamp, true).unwrap();
        assert_eq!(c.steps.get("PREAMP"), Some(&20));
        // still in step with the stream
        assert_eq!(c.get_freq().unwrap(), 14_105_000);
    }
}
