//! Hamlib `rigctld` network client. Also works with RigHelm's built-in
//! rigctld-compatible server (default `127.0.0.1:4532`), which covers ~30
//! Yaesu/Icom/Kenwood/Elecraft radios while sharing the CAT port with other apps.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::Duration;

use super::{RadioMode, RigControl, RigError, RigResult};

pub struct RigctldClient {
    host: String,
    port: u16,
    /// Writer half plus a persistent reader. The reader must outlive each call:
    /// a fresh `BufReader` per command would swallow buffered extra lines
    /// (e.g. the passband line after `m`) and desync every later reply.
    conn: Option<(TcpStream, BufReader<TcpStream>)>,
}

impl RigctldClient {
    pub fn new(host: &str, port: u16) -> Self {
        Self {
            host: host.to_string(),
            port,
            conn: None,
        }
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
        // Passband 0 = radio's default for the mode.
        let reply = self.send_cmd(&format!("M {} 0", rigctld_mode_str(mode)))?;
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

    fn name(&self) -> &str {
        "rigctld"
    }
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
                    } else if line.starts_with('M') || line.starts_with('T') {
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
        c.set_ptt(false).unwrap();
    }
}
