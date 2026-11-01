use std::fmt;

/// VarAC's VaraModemType: which VARA program we talk to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ModemType {
    #[default]
    Hf,
    Fm,
    Sat,
}

impl ModemType {
    pub fn from_key(s: &str) -> Self {
        match s.trim().to_ascii_uppercase().as_str() {
            "VARAFM" | "FM" => ModemType::Fm,
            "VARASAT" | "SAT" => ModemType::Sat,
            _ => ModemType::Hf,
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            ModemType::Hf => "VaraHF",
            ModemType::Fm => "VaraFM",
            ModemType::Sat => "VaraSAT",
        }
    }
    /// The word VARA's VERSION reply must contain
    pub fn version_word(self) -> &'static str {
        match self {
            ModemType::Hf => "HF",
            ModemType::Fm => "FM",
            ModemType::Sat => "SAT",
        }
    }
    /// VarAC's ADIF SUBMODE
    pub fn submode(self) -> &'static str {
        match self {
            ModemType::Hf => "VARA HF",
            ModemType::Fm => "VARA FM 1200",
            ModemType::Sat => "VARA SATELLITE",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bandwidth {
    Bw500,
    Bw2300,
    Bw2750,
}

impl fmt::Display for Bandwidth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bw500 => write!(f, "BW500"),
            Self::Bw2300 => write!(f, "BW2300"),
            Self::Bw2750 => write!(f, "BW2750"),
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionMode {
    Off,
    Text,
    Files,
    On,
}

impl fmt::Display for CompressionMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Off => write!(f, "OFF"),
            Self::Text => write!(f, "TEXT"),
            Self::Files => write!(f, "FILES"),
            Self::On => write!(f, "ON"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListenMode {
    On,
    Cq, // enables CQ frame notifications
    Off,
}

impl fmt::Display for ListenMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::On => write!(f, "ON"),
            Self::Cq => write!(f, "CQ"),
            Self::Off => write!(f, "OFF"),
        }
    }
}

/// Commands sent to the VARA modem over the command TCP port (default 8300).
/// Each command is CR-terminated. Modem replies OK or WRONG synchronously.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum VaraCommand {
    /// MYCALL <call1> [call2..call5]
    MyCall(Vec<String>),
    /// CONNECT <source> <destination>
    Connect { source: String, destination: String },
    /// Graceful disconnect (flushes TX buffer first).
    Disconnect,
    /// Immediate dirty disconnect.
    Abort,
    /// LISTEN ON|CQ|OFF — CQ mode enables CQFRAME notifications.
    Listen(ListenMode),
    /// BW500 / BW2300 / BW2750
    SetBandwidth(Bandwidth),
    /// COMPRESSION OFF|TEXT|FILES|ON
    Compression(CompressionMode),
    /// CHAT ON|OFF — keeps idle link open, enables CQ reports.
    Chat(bool),
    /// CQFRAME <source_with_ssid> <bandwidth> — send a CQ frame.
    CqFrame { source: String, bandwidth: u16 },
    /// VARA FM / SAT: `CQFRAME source[ digi digi]` (no bandwidth; digipeaters on FM)
    CqFrameNoBw { source: String, via: Option<String> },
    /// CLEANTXBUFFER — clear queued data.
    CleanTxBuffer,
    /// VERSION — query modem version.
    Version,
    /// TUNE ON|OFF — transmit a steady carrier for antenna tuning.
    Tune(bool),
    /// TUNE ? — ask the modem for its TX drive level (it answers `TUNE -5`)
    TuneQuery,
    /// TUNE n — the carrier at drive level n dB, as VarAC starts it on VARA
    TuneAt(i32),
    /// DRIVELEVEL n — the modem's TX drive in dB, -30…0 (VarAC's tune slider)
    DriveLevel(i32),
    /// IGNOREKISSDCD ON|OFF — send KISS frames without waiting for a clear channel.
    IgnoreKissDcd(bool),
}

impl VaraCommand {
    pub fn serialize(&self) -> String {
        let cmd = match self {
            Self::MyCall(calls) => format!("MYCALL {}", calls.join(" ")),
            Self::Connect { source, destination } => {
                format!("CONNECT {source} {destination}")
            }
            Self::Disconnect => "DISCONNECT".into(),
            Self::Abort => "ABORT".into(),
            Self::Listen(mode) => format!("LISTEN {mode}"),
            Self::SetBandwidth(bw) => bw.to_string(),
            Self::Compression(mode) => format!("COMPRESSION {mode}"),
            Self::Chat(on) => format!("CHAT {}", if *on { "ON" } else { "OFF" }),
            Self::CqFrame { source, bandwidth } => {
                format!("CQFRAME {source} {bandwidth}")
            }
            Self::CqFrameNoBw { source, via } => match via.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
                Some(v) => format!("CQFRAME {source} {v}"),
                None => format!("CQFRAME {source}"),
            },
            Self::CleanTxBuffer => "CLEANTXBUFFER".into(),
            Self::Version => "VERSION".into(),
            Self::Tune(on) => format!("TUNE {}", if *on { "ON" } else { "OFF" }),
            Self::TuneQuery => "TUNE ?".into(),
            Self::TuneAt(db) => format!("TUNE {}", (*db).clamp(-30, 0)),
            Self::DriveLevel(db) => format!("DRIVELEVEL {}", (*db).clamp(-30, 0)),
            Self::IgnoreKissDcd(on) => format!("IGNOREKISSDCD {}", if *on { "ON" } else { "OFF" }),
        };
        format!("{cmd}\r")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_mycall() {
        let cmd = VaraCommand::MyCall(vec!["W1ABC".into(), "W1ABC-5".into()]);
        assert_eq!(cmd.serialize(), "MYCALL W1ABC W1ABC-5\r");
    }

    #[test]
    fn serialize_cqframe() {
        let cmd = VaraCommand::CqFrame {
            source: "W1ABC-3".into(),
            bandwidth: 500,
        };
        assert_eq!(cmd.serialize(), "CQFRAME W1ABC-3 500\r");
    }

    #[test]
    fn serialize_listen_cq() {
        let cmd = VaraCommand::Listen(ListenMode::Cq);
        assert_eq!(cmd.serialize(), "LISTEN CQ\r");
    }
}
