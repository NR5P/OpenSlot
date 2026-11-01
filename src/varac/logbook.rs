//! QSO logging the way VarAC does it: one ADIF record per QSO appended to a
//! live `.adi` file, and the same record handed to a logging program.

use std::io::Write;

/// One QSO as VarAC writes it. Times are UTC.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LogQso {
    pub call: String,
    /// "+05" style (as in VarAC)
    pub rst_sent: String,
    pub rst_rcvd: String,
    pub start: chrono::NaiveDateTime,
    pub end: chrono::NaiveDateTime,
    pub band: String,
    /// Dial frequency when the link came up, Hz (0 = unknown, FREQ left out)
    pub freq_hz: u64,
    pub name: String,
    pub qth: String,
    pub grid: String,
    pub station_callsign: String,
    /// Watts as typed (may be empty: VarAC still writes `<TX_PWR:0> `)
    pub tx_pwr: String,
    pub comment: String,
    /// "VARA HF" (VarAC AdifSubMode; "NONE" leaves SUBMODE out)
    pub submode: String,
}

/// One ADIF element as VarAC writes it: `<TAG:len>value ` (always a trailing space).
fn element(tag: &str, value: &str) -> String {
    format!("<{tag}:{}>{value} ", value.encode_utf16().count())
}

/// VarAC's "+05" SNR text (0 → "+00").
pub fn rst(snr: i32) -> String {
    if snr == 0 { "+00".into() } else { format!("{snr:+03}") }
}

/// The ADIF record (no line ending), in VarAC's field order: CALL, MODE (DYNAMIC),
/// SUBMODE, RST_SENT, RST_RCVD, QSO_DATE, TIME_ON, QSO_DATE_OFF, TIME_OFF, BAND, FREQ
/// (MHz, 6 decimals), NAME, QTH, GRIDSQUARE (when known), STATION_CALLSIGN, TX_PWR
/// (always), COMMENT, <EOR>.
pub fn adif_record(q: &LogQso) -> String {
    let mut s = String::new();
    s += &element("CALL", &q.call);
    s += &element("MODE", "DYNAMIC");
    if !q.submode.is_empty() && !q.submode.eq_ignore_ascii_case("NONE") {
        s += &element("SUBMODE", &q.submode);
    }
    s += &element("RST_SENT", &q.rst_sent);
    s += &element("RST_RCVD", &q.rst_rcvd);
    s += &element("QSO_DATE", &q.start.format("%Y%m%d").to_string());
    s += &element("TIME_ON", &q.start.format("%H%M%S").to_string());
    s += &element("QSO_DATE_OFF", &q.end.format("%Y%m%d").to_string());
    s += &element("TIME_OFF", &q.end.format("%H%M%S").to_string());
    s += &element("BAND", &q.band);
    if q.freq_hz > 0 {
        s += &element("FREQ", &format!("{}.{:06}", q.freq_hz / 1_000_000, q.freq_hz % 1_000_000));
    }
    for (tag, v) in [("NAME", &q.name), ("QTH", &q.qth), ("GRIDSQUARE", &q.grid)] {
        if !v.is_empty() {
            s += &element(tag, v);
        }
    }
    s += &element("STATION_CALLSIGN", &q.station_callsign);
    s += &element("TX_PWR", &q.tx_pwr);
    if !q.comment.is_empty() {
        s += &element("COMMENT", &q.comment);
    }
    s += "<EOR>";
    s
}

/// What VarAC refuses to log without (its "Missing QSO fields" box lists them).
pub fn missing_fields(q: &LogQso) -> Vec<&'static str> {
    let mut m = Vec::new();
    for (empty, name) in [
        (q.call.trim().is_empty(), "CALLSIGN"),
        (q.rst_sent.trim().is_empty(), "REPORT SENT"),
        (q.rst_rcvd.trim().is_empty(), "REPORT RECEIVED"),
        (q.band.trim().is_empty(), "BAND"),
        (q.start == chrono::NaiveDateTime::default(), "START TIME"),
        (q.station_callsign.trim().is_empty(), "MY CALLSIGN"),
    ] {
        if empty {
            m.push(name);
        }
    }
    m
}

/// Append the record to the live ADIF file, writing VarAC's one-line header first when
/// the file is new: "{program} ADIF QSO Log<EOH>".
pub fn append_to_file(path: &std::path::Path, record: &str, program: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let new = !path.exists();
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    if new {
        write!(f, "{program} ADIF QSO Log<EOH>\r\n")?;
    }
    write!(f, "{record}\r\n")
}

/// Logging programs VarAC can hand a QSO to (Settings → Logging), with their default
/// ports.
pub const LOGGERS: &[(&str, u16)] = &[
    ("NONE", 0),
    ("Log4OM(UDP)", 1200),
    ("N1MM(UDP)", 2237),
    ("HRD_Logbook(UDP)", 2333),
    ("UcxLog(UDP)", 2237),
    ("Winlog32(UDP)", 12060),
    ("DXKeeper(TCP)", 52001),
    ("Logger32(TCP)", 52001),
    ("Swisslog(TCP)", 52001),
    ("N3FJP(TCP)", 1100),
];

/// The bytes VarAC sends a logger: UDP datagrams, or for TCP loggers the messages sent
/// over one connection each.
#[derive(Debug, PartialEq)]
pub enum LoggerMessage {
    Udp(Vec<u8>),
    Tcp(Vec<Vec<u8>>),
}

/// Build the message for `logger` (one of LOGGERS) from a QSO and its ADIF record.
pub fn logger_message(logger: &str, record: &str, q: &LogQso, program: &str) -> Option<LoggerMessage> {
    let rec = record;
    // (VarAC: Encoding.ASCII.GetBytes, every character outside ASCII becomes '?')
    let ascii = |t: String| -> Vec<u8> { t.encode_utf16().map(|u| if u < 128 { u as u8 } else { b'?' }).collect() };
    Some(match logger {
        "Log4OM(UDP)" | "Winlog32(UDP)" => LoggerMessage::Udp(ascii(rec.to_string())),
        "N1MM(UDP)" => LoggerMessage::Udp(ascii(format!("<command:3>Log<parameters:{}>{rec}", rec.encode_utf16().count()))),
        "Swisslog(TCP)" => LoggerMessage::Tcp(vec![ascii(format!("<command:3>Log<parameters:{}>{rec}", rec.encode_utf16().count()))]),
        "HRD_Logbook(UDP)" => LoggerMessage::Udp(ascii(rec.replace('<', " <"))),
        "UcxLog(UDP)" => {
            // WSJT-X "Logged ADIF" (type 12): magic, schema 2, type, id, then the ADIF text
            let id = format!("{program:<6}");
            let text = format!("<adif_ver:5>3.0.7<programid:{}>{program}<EOH>{rec}", program.encode_utf16().count());
            let mut b = vec![0xAD, 0xBC, 0xCB, 0xDA, 0, 0, 0, 2, 0, 0, 0, 12];
            let (id, text) = (ascii(id), ascii(text));
            b.extend_from_slice(&(id.len() as u32).to_be_bytes());
            b.extend_from_slice(&id);
            b.extend_from_slice(&(text.len() as u32 + 1).to_be_bytes());
            b.push(b'\n');
            b.extend_from_slice(&text);
            LoggerMessage::Udp(b)
        }
        "DXKeeper(TCP)" | "Logger32(TCP)" => {
            let p = format!("<ExternalLogADIF:{}>{rec}<DeduceMissing:1>Y<CheckOverrides:1>Y", rec.encode_utf16().count());
            let mut msgs = vec![ascii(format!("<command:11>externallog<parameters:{}>{p}", p.encode_utf16().count()))];
            if logger == "DXKeeper(TCP)" {
                let call = q.call.to_uppercase();
                msgs.push(ascii(format!("<command:5>check<parameters:{}>{call}", call.encode_utf16().count())));
            }
            LoggerMessage::Tcp(msgs)
        }
        "N3FJP(TCP)" => {
            let freq = if q.freq_hz > 0 { format!("{}.{:06}", q.freq_hz / 1_000_000, q.freq_hz % 1_000_000) } else { String::new() };
            let mut s = String::new();
            for (control, value) in [
                ("TXTENTRYCALL", q.call.clone()),
                ("TXTENTRYMODE", q.submode.clone()),
                ("TXTENTRYRSTR", q.rst_rcvd.clone()),
                ("TXTENTRYRSTS", q.rst_sent.clone()),
                ("TXTENTRYBAND", q.band.clone()),
                ("TXTENTRYDATE", q.start.format("%Y/%m/%d").to_string()),
                ("TXTENTRYTIMEON", q.start.format("%H:%M:%S").to_string()),
                ("TXTENTRYTIMEOFF", q.end.format("%H:%M:%S").to_string()),
                ("TXTENTRYNAMER", q.name.clone()),
                ("TXTENTRYQTHGROUP", q.qth.clone()),
                ("TXTENTRYPOWER", q.tx_pwr.clone()),
                ("TXTENTRYGRID", q.grid.clone()),
                ("TXTENTRYCOMMENTS", q.comment.clone()),
                ("TXTENTRYFREQUENCY", freq),
            ] {
                s += &format!("<CMD><UPDATE><CONTROL>{control}</CONTROL><VALUE>{value}</VALUE></CMD>");
            }
            s += "<CMD><ACTION><VALUE>ENTER</VALUE></CMD>";
            LoggerMessage::Tcp(vec![ascii(s)])
        }
        _ => return None,
    })
}

/// A UDP socket for sending to host:port, of the same family as the address (on Windows
/// "localhost" resolves to ::1 first, which an IPv4 socket cannot reach): IPv4 preferred.
fn udp_to(host: &str, port: u16) -> std::io::Result<(std::net::UdpSocket, std::net::SocketAddr)> {
    let addrs: Vec<_> = std::net::ToSocketAddrs::to_socket_addrs(&(host, port))?.collect();
    let addr = addrs.iter().find(|a| a.is_ipv4()).or(addrs.first()).copied()
        .ok_or_else(|| std::io::Error::other("no address"))?;
    let sock = std::net::UdpSocket::bind(if addr.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" })?;
    Ok((sock, addr))
}

/// Send to a logger at host:port (TCP with VarAC's 1 s connect timeout).
pub fn send_to_logger(logger: &str, host: &str, port: u16, record: &str, q: &LogQso, program: &str) -> std::io::Result<()> {
    let Some(msg) = logger_message(logger, record, q, program) else { return Ok(()) };
    match msg {
        LoggerMessage::Udp(b) => {
            let (sock, addr) = udp_to(host, port)?;
            sock.send_to(&b, addr)?;
        }
        LoggerMessage::Tcp(msgs) => {
            for m in msgs {
                let mut s = crate::vara::connect_any(host, port, std::time::Duration::from_secs(1))?;
                s.write_all(&m)?;
            }
        }
    }
    Ok(())
}

/// Log4OM's callsign lookup (VarAC LoggerAppCallsignLookup): show the partner in Log4OM
/// when a link comes up (`SetCallsign`), clear it after (`ClearUI`). UDP to the API port.
pub fn log4om_lookup(host: &str, api_port: u16, call: Option<&str>) -> std::io::Result<()> {
    let id = uuid::Uuid::new_v4();
    let msg = match call {
        Some(c) => format!("<RemoteControlRequest><MessageId>{id}</MessageId><RemoteControlMessage>SetCallsign</RemoteControlMessage><Callsign>{c}</Callsign></RemoteControlRequest>"),
        None => format!("<RemoteControlRequest><MessageId>{id}</MessageId><RemoteControlMessage>ClearUI</RemoteControlMessage></RemoteControlRequest>"),
    };
    let (sock, addr) = udp_to(host, api_port)?;
    sock.send_to(msg.as_bytes(), addr)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> LogQso {
        let t = |s: &str| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").unwrap();
        LogQso {
            call: "W5TST".into(), rst_sent: "+20".into(), rst_rcvd: "+00".into(),
            start: t("2026-10-28 20:02:39"), end: t("2026-10-28 20:02:51"), band: "20m".into(),
            freq_hz: 14_105_000, name: "Tester".into(), qth: String::new(), grid: String::new(),
            station_callsign: "NR5P".into(), tx_pwr: String::new(), comment: "Made with VarAC".into(),
            submode: "VARA HF".into(),
        }
    }

    /// Byte for byte the line original VarAC wrote to its VarAC_qso_log.adi.
    #[test]
    fn adif_record_matches_varac_file() {
        assert_eq!(
            adif_record(&sample()),
            "<CALL:5>W5TST <MODE:7>DYNAMIC <SUBMODE:7>VARA HF <RST_SENT:3>+20 <RST_RCVD:3>+00 <QSO_DATE:8>20261028 <TIME_ON:6>200239 <QSO_DATE_OFF:8>20261028 <TIME_OFF:6>200251 <BAND:3>20m <FREQ:9>14.105000 <NAME:6>Tester <STATION_CALLSIGN:4>NR5P <TX_PWR:0> <COMMENT:15>Made with VarAC <EOR>"
        );
        assert_eq!(rst(-7), "-07");
        assert_eq!(rst(0), "+00");
        assert!(missing_fields(&sample()).is_empty());
        assert_eq!(missing_fields(&LogQso { rst_rcvd: String::new(), ..sample() }), vec!["REPORT RECEIVED"]);
    }

    #[test]
    fn logger_messages_like_varac() {
        let q = sample();
        let rec = adif_record(&q);
        let Some(LoggerMessage::Udp(n1mm)) = logger_message("N1MM(UDP)", &rec, &q, "VarAC") else { panic!() };
        assert!(String::from_utf8(n1mm).unwrap().starts_with(&format!("<command:3>Log<parameters:{}><CALL:5>W5TST", rec.len())));
        let Some(LoggerMessage::Udp(ucx)) = logger_message("UcxLog(UDP)", &rec, &q, "VarAC") else { panic!() };
        assert_eq!(&ucx[..22], b"\xAD\xBC\xCB\xDA\x00\x00\x00\x02\x00\x00\x00\x0C\x00\x00\x00\x06VarAC ");
        let Some(LoggerMessage::Tcp(dx)) = logger_message("DXKeeper(TCP)", &rec, &q, "VarAC") else { panic!() };
        assert_eq!(dx.len(), 2);
        assert_eq!(dx[1], b"<command:5>check<parameters:5>W5TST".to_vec());
        assert!(logger_message("NONE", &rec, &q, "VarAC").is_none());
    }

    #[test]
    fn adif_file_gets_varac_header_once() {
        let dir = std::env::temp_dir().join(format!("openslot_adif_{}", std::process::id()));
        let path = dir.join("log.adi");
        let _ = std::fs::remove_file(&path);
        append_to_file(&path, "<CALL:2>AB <EOR>", "Openslot").unwrap();
        append_to_file(&path, "<CALL:2>CD <EOR>", "Openslot").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text, "Openslot ADIF QSO Log<EOH>\r\n<CALL:2>AB <EOR>\r\n<CALL:2>CD <EOR>\r\n");
        let _ = std::fs::remove_dir_all(dir);
    }
}
