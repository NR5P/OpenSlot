use crate::vara::commands::VaraCommand;
use crate::vara::events::{VaraEvent, parse_event};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

/// Manages the two TCP connections to the VARA modem:
/// - Command port (default 8300): sends commands, receives events
/// - Data port (default 8301): bidirectional data stream
pub struct VaraConnection {
    cmd_writer: tokio::io::WriteHalf<TcpStream>,
    data_writer: tokio::io::WriteHalf<TcpStream>,
    pub event_rx: mpsc::Receiver<VaraEvent>,
    pub data_rx: mpsc::Receiver<Vec<u8>>,
    /// The tasks holding the read halves: stopped on drop, so both sockets close with the
    /// connection (else the modem keeps a stale client until it next writes to it)
    readers: [tokio::task::JoinHandle<()>; 2],
}

impl Drop for VaraConnection {
    fn drop(&mut self) {
        for r in &self.readers {
            r.abort();
        }
    }
}

impl VaraConnection {
    pub async fn connect(
        host: &str,
        cmd_port: u16,
        data_port: u16,
    ) -> Result<Self, std::io::Error> {
        let cmd_stream = TcpStream::connect((host, cmd_port)).await?;
        let data_stream = TcpStream::connect((host, data_port)).await?;

        let (cmd_reader, cmd_writer) = tokio::io::split(cmd_stream);
        let (data_reader, data_writer) = tokio::io::split(data_stream);

        let (event_tx, event_rx) = mpsc::channel(256);
        let (data_tx, data_rx) = mpsc::channel(256);

        // Read command port lines. VARA terminates them with CR only (no LF), so
        // line readers that wait for '\n' never see an event; split on either.
        let cmd_task = tokio::spawn(async move {
            let mut reader = BufReader::new(cmd_reader);
            let mut buf = Vec::new();
            loop {
                buf.clear();
                match reader.read_until(b'\r', &mut buf).await {
                    Ok(0) => break,
                    Ok(_) => {
                        let text = String::from_utf8_lossy(&buf);
                        for line in text.split(['\r', '\n']).filter(|l| !l.trim().is_empty()) {
                            if event_tx.send(parse_event(line)).await.is_err() {
                                return;
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        // Read data port as raw bytes
        let data_task = tokio::spawn(async move {
            let mut reader = BufReader::new(data_reader);
            let mut buf = vec![0u8; 4096];
            loop {
                match tokio::io::AsyncReadExt::read(&mut reader, &mut buf).await {
                    Ok(0) => break,
                    Ok(n) => {
                        if data_tx.send(buf[..n].to_vec()).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(Self {
            cmd_writer,
            data_writer,
            event_rx,
            data_rx,
            readers: [cmd_task, data_task],
        })
    }

    /// Send a command to the VARA modem command port.
    pub async fn send_command(&mut self, cmd: &VaraCommand) -> Result<(), std::io::Error> {
        let serialized = cmd.serialize();
        tracing::debug!("VARA CMD TX: {}", serialized.trim());
        self.cmd_writer.write_all(serialized.as_bytes()).await
    }

    /// Bring the modem up exactly the way VarAC 16 does (captured from a real
    /// VarAC talking to a VARA-compatible modem):
    /// `BW… · CHAT ON · LISTEN ON · IGNOREKISSDCD ON · LISTEN CQ · VERSION · MYCALL CALL CALL-T`
    /// The `-T` alias is the address VarAC uses for link tests/pings.
    pub async fn initialize(
        &mut self,
        callsign: &str,
        bandwidth: crate::vara::commands::Bandwidth,
    ) -> Result<(), std::io::Error> {
        self.initialize_for(callsign, bandwidth, crate::vara::commands::ModemType::Hf, true).await
    }

    /// As VarAC sets up the modem: only VARA HF gets a bandwidth command.
    pub async fn initialize_for(
        &mut self,
        callsign: &str,
        bandwidth: crate::vara::commands::Bandwidth,
        modem: crate::vara::commands::ModemType,
        ping_alias: bool,
    ) -> Result<(), std::io::Error> {
        use crate::vara::commands::ListenMode;
        let base = callsign.split('-').next().unwrap_or(callsign).to_string();
        if modem == crate::vara::commands::ModemType::Hf {
            self.send_command(&VaraCommand::SetBandwidth(bandwidth)).await?;
        }
        for cmd in [
            VaraCommand::Chat(true),
            VaraCommand::Listen(ListenMode::On),
            VaraCommand::IgnoreKissDcd(true),
            VaraCommand::Listen(ListenMode::Cq),
            VaraCommand::Version,
            // (VarAC: one MYCALL, with the -T ping alias only when pings are allowed)
            VaraCommand::MyCall(if ping_alias { vec![callsign.to_string(), format!("{base}-T")] } else { vec![callsign.to_string()] }),
            // (VarAC reads the modem's drive level for its tune slider; without it the
            // slider shows a guess and the first TUNE goes out at whatever the modem has)
            VaraCommand::TuneQuery,
        ] {
            self.send_command(&cmd).await?;
        }
        Ok(())
    }

    /// Send raw data over the VARA data port.
    pub async fn send_data(&mut self, data: &[u8]) -> Result<(), std::io::Error> {
        self.data_writer.write_all(data).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    /// VARA ends command-port lines with a bare CR; every event must still arrive.
    #[tokio::test]
    async fn cr_terminated_events_are_delivered() {
        let cmd = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let data = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (cp, dp) = (cmd.local_addr().unwrap().port(), data.local_addr().unwrap().port());
        tokio::spawn(async move {
            let (mut c, _) = cmd.accept().await.unwrap();
            let (_d, _) = data.accept().await.unwrap();
            c.write_all(b"OK\rPTT ON\rCONNECTED W5TST NR5P 500\rPTT OFF\r").await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        });
        let mut v = VaraConnection::connect("127.0.0.1", cp, dp).await.unwrap();
        let mut got = Vec::new();
        for _ in 0..4 {
            got.push(tokio::time::timeout(std::time::Duration::from_secs(2), v.event_rx.recv()).await.unwrap().unwrap());
        }
        assert_eq!(got[0], VaraEvent::Ok);
        assert_eq!(got[1], VaraEvent::Ptt(true));
        assert!(matches!(got[2], VaraEvent::Connected { .. }));
        assert_eq!(got[3], VaraEvent::Ptt(false));
    }

    /// Dropping the connection (disconnecting from the modem) closes both sockets, so the
    /// modem sees its client go at once.
    #[tokio::test]
    async fn drop_closes_both_sockets() {
        use tokio::io::AsyncReadExt;
        let cmd = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let data = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (cp, dp) = (cmd.local_addr().unwrap().port(), data.local_addr().unwrap().port());
        let server = tokio::spawn(async move {
            let (mut c, _) = cmd.accept().await.unwrap();
            let (mut d, _) = data.accept().await.unwrap();
            let mut b = [0u8; 16];
            let wait = std::time::Duration::from_secs(2);
            let c_eof = tokio::time::timeout(wait, c.read(&mut b)).await;
            let d_eof = tokio::time::timeout(wait, d.read(&mut b)).await;
            (matches!(c_eof, Ok(Ok(0))), matches!(d_eof, Ok(Ok(0))))
        });
        let v = VaraConnection::connect("127.0.0.1", cp, dp).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        drop(v);
        assert_eq!(server.await.unwrap(), (true, true));
    }
}
