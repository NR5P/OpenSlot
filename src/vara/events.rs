/// Events received from the VARA modem on the command TCP port (default 8300).
/// Each event is a CR-terminated line.
#[derive(Debug, Clone, PartialEq)]
pub enum VaraEvent {
    /// Synchronous command acknowledgement.
    Ok,
    /// Synchronous command rejection.
    Wrong,
    /// ARQ link established: CONNECTED <src> <dst> [bw]
    Connected {
        source: String,
        destination: String,
        bandwidth: Option<String>,
    },
    /// ARQ link closed.
    Disconnected,
    /// PTT keyed on/off.
    Ptt(bool),
    /// Periodic heartbeat (~60s while connected).
    IAmAlive,
    /// Modem is registered (licensed).
    Registered,
    /// Outgoing connection attempt is pending.
    Pending,
    /// Pending connection attempt was cancelled.
    CancelPending,
    /// Channel-busy detector edges.
    Busy(bool),
    /// TX buffer byte count: BUFFER <n>
    Buffer(u32),
    /// Modem version: VERSION <string>
    Version(String),
    /// The modem's TX drive level in dB, answering `TUNE ?`: TUNE <level>
    TuneLevel(i32),
    /// Signal report: SN <value_db>
    SignalReport(f32),
    /// CQ frame received: CQFRAME <call-ssid> <bw>
    CqFrame { source: String, bandwidth: u16 },
    /// VARA FM: `CQFRAME source digi digi` (the digipeater path, no bandwidth)
    CqFrameVia { source: String, via: String },
    /// Bitrate notification: BITRATE (N)  x bps TX|RX
    Bitrate(String),
    /// Clean TX buffer result.
    CleanTxBuffer(String),
    /// TX buffer has been fully drained.
    BufferEmpty,
    /// Disconnect is in progress (buffer draining).
    DisconnectPending,
    /// Unrecognized line from the modem.
    Unknown(String),
}

pub fn parse_event(line: &str) -> VaraEvent {
    let trimmed = line.trim();

    if trimmed == "OK" {
        return VaraEvent::Ok;
    }
    if trimmed == "WRONG" {
        return VaraEvent::Wrong;
    }

    if let Some(rest) = trimmed.strip_prefix("CONNECTED ") {
        let parts: Vec<&str> = rest.split_whitespace().collect();
        if parts.len() >= 2 {
            return VaraEvent::Connected {
                source: parts[0].to_string(),
                destination: parts[1].to_string(),
                bandwidth: parts.get(2).map(|s| s.to_string()),
            };
        }
    }

    if trimmed == "DISCONNECTED" {
        return VaraEvent::Disconnected;
    }
    if trimmed == "PTT ON" {
        return VaraEvent::Ptt(true);
    }
    if trimmed == "PTT OFF" {
        return VaraEvent::Ptt(false);
    }
    if trimmed == "IAMALIVE" {
        return VaraEvent::IAmAlive;
    }
    if trimmed == "REGISTERED" {
        return VaraEvent::Registered;
    }
    if trimmed == "PENDING" {
        return VaraEvent::Pending;
    }
    if trimmed == "CANCELPENDING" {
        return VaraEvent::CancelPending;
    }
    if trimmed == "BUSY ON" {
        return VaraEvent::Busy(true);
    }
    if trimmed == "BUSY OFF" {
        return VaraEvent::Busy(false);
    }
    if trimmed == "BUFFEREMPTY" {
        return VaraEvent::BufferEmpty;
    }
    if trimmed == "DISCONNECT-PENDING" {
        return VaraEvent::DisconnectPending;
    }

    if let Some(rest) = trimmed.strip_prefix("BUFFER ") {
        if let Ok(n) = rest.trim().parse::<u32>() {
            return VaraEvent::Buffer(n);
        }
    }

    if let Some(level) = trimmed.strip_prefix("TUNE ").and_then(|r| r.trim().parse::<f64>().ok()) {
        return VaraEvent::TuneLevel(level.round() as i32);
    }
    if let Some(rest) = trimmed.strip_prefix("VERSION ") {
        return VaraEvent::Version(rest.to_string());
    }

    if let Some(rest) = trimmed.strip_prefix("SN ") {
        if let Ok(v) = rest.trim().parse::<f32>() {
            return VaraEvent::SignalReport(v);
        }
    }

    if let Some(rest) = trimmed.strip_prefix("CQFRAME ") {
        let parts: Vec<&str> = rest.split_whitespace().collect();
        if parts.len() >= 2 {
            if let Ok(bw) = parts[1].parse::<u16>() {
                return VaraEvent::CqFrame {
                    source: parts[0].to_string(),
                    bandwidth: bw,
                };
            }
            // VARA FM: the fields after the call are the digipeater path
            return VaraEvent::CqFrameVia { source: parts[0].to_string(), via: parts[1..].join(" ") };
        }
        // VARA FM / SAT without digipeaters: just the call
        if parts.len() == 1 {
            return VaraEvent::CqFrame { source: parts[0].to_string(), bandwidth: 0 };
        }
    }

    if trimmed.starts_with("BITRATE") {
        return VaraEvent::Bitrate(trimmed.to_string());
    }

    if let Some(rest) = trimmed.strip_prefix("CLEANTXBUFFER ") {
        return VaraEvent::CleanTxBuffer(rest.to_string());
    }

    VaraEvent::Unknown(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_connected_with_bw() {
        assert_eq!(
            parse_event("CONNECTED W1ABC W2DEF 2300\r"),
            VaraEvent::Connected {
                source: "W1ABC".into(),
                destination: "W2DEF".into(),
                bandwidth: Some("2300".into()),
            }
        );
    }

    #[test]
    fn parse_connected_without_bw() {
        assert_eq!(
            parse_event("CONNECTED W1ABC W2DEF\r"),
            VaraEvent::Connected {
                source: "W1ABC".into(),
                destination: "W2DEF".into(),
                bandwidth: None,
            }
        );
    }

    #[test]
    fn parse_tune_level() {
        assert_eq!(parse_event("TUNE -5"), VaraEvent::TuneLevel(-5));
        assert_eq!(parse_event("TUNE -12.0"), VaraEvent::TuneLevel(-12));
        assert_eq!(crate::vara::commands::VaraCommand::DriveLevel(-7).serialize().trim(), "DRIVELEVEL -7");
        assert_eq!(crate::vara::commands::VaraCommand::DriveLevel(5).serialize().trim(), "DRIVELEVEL 0");
        assert_eq!(crate::vara::commands::VaraCommand::TuneQuery.serialize().trim(), "TUNE ?");
        assert_eq!(crate::vara::commands::VaraCommand::TuneAt(-6).serialize().trim(), "TUNE -6");
    }

    #[test]
    fn parse_buffer() {
        assert_eq!(parse_event("BUFFER 1024\r"), VaraEvent::Buffer(1024));
    }

    #[test]
    fn parse_disconnected() {
        assert_eq!(parse_event("DISCONNECTED\r"), VaraEvent::Disconnected);
    }

    #[test]
    fn parse_signal_report() {
        assert_eq!(parse_event("SN -12.5\r"), VaraEvent::SignalReport(-12.5));
    }

    #[test]
    fn parse_cqframe() {
        assert_eq!(
            parse_event("CQFRAME W1ABC-3 500\r"),
            VaraEvent::CqFrame {
                source: "W1ABC-3".into(),
                bandwidth: 500,
            }
        );
    }

    #[test]
    fn parse_ok_wrong() {
        assert_eq!(parse_event("OK\r"), VaraEvent::Ok);
        assert_eq!(parse_event("WRONG\r"), VaraEvent::Wrong);
    }

    #[test]
    fn vara_fm_cqframes() {
        assert_eq!(parse_event("CQFRAME W1ABC-9 W1DIG W2DIG\r"), VaraEvent::CqFrameVia { source: "W1ABC-9".into(), via: "W1DIG W2DIG".into() });
        assert_eq!(parse_event("CQFRAME W1ABC-9\r"), VaraEvent::CqFrame { source: "W1ABC-9".into(), bandwidth: 0 });
        use crate::vara::commands::VaraCommand;
        assert_eq!(VaraCommand::CqFrameNoBw { source: "W5TST-9".into(), via: Some("DIGI1 DIGI2".into()) }.serialize(), "CQFRAME W5TST-9 DIGI1 DIGI2\r");
        assert_eq!(VaraCommand::CqFrameNoBw { source: "W5TST-9".into(), via: None }.serialize(), "CQFRAME W5TST-9\r");
    }
}
