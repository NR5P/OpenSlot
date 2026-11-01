/// VarAC control tags embedded in length-prefixed messages.
///
/// Two forms exist:
/// - Value tags: `<TAG:value>` (colon-separated) or `<TAG value>` (space-separated for TO)
/// - Bare tags: `<TAG>` (no value)
///
/// Values must not contain `<`, `>`, CR, or LF — no escaping mechanism exists.
///
/// Sources: hfmodem (AGPL-3.0, on-air verified), VarAC manual v10.1.0

/// A parsed VarAC control tag.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq)]
pub enum Tag {
    // === Opening / identity ===
    /// Signal report: <R+05> or <R-12> (dB, clamped ±99)
    SignalReport(i8),
    /// Operator name: <NAME:value>
    Name(String),
    /// Location: <QTH:value>
    Qth(String),
    /// Grid locator: <LOC:value>
    Locator(String),
    /// Email address: <EM:value>
    Email(String),
    /// Full/complex callsign: <FC:value>
    FullCallsign(String),
    /// Services offered: <S:VEB> (V=VMail, E=Email, A=AI, B=BBS)
    Services(String),

    // === Session control ===
    /// Disconnect after delivery
    Disc,
    /// Auto-send trigger
    Snd,
    /// Station is away: <AWAY>
    Away,
    /// Away with auto-QSY/VMail capability
    Awq,
    /// Away time exceeded
    Ae,
    /// Idle time exceeded
    Ie,

    // === Information requests ===
    /// Request profile info
    Info,
    /// Deny profile info
    InfoJ,
    /// Request SNR report
    Snr,
    /// Request VarAC/VARA version
    Ver,
    /// Extended SNR request
    Vsi,
    /// Extended SNR accepted
    VsiR,
    /// Extended SNR rejected
    VsiJ,

    // === Last-heard peeking ===
    /// Last-heard request to partner
    Lhp,
    /// Last-heard response (list)
    Lhr,
    /// Last-heard empty
    Lhe,
    /// Last-heard rejected
    Lhj,
    /// SNR report request
    Snrr,

    // === QSY (frequency change) ===
    /// Relative QSY request: <Q:75> (in tens of Hz)
    QsyRelative(i32),
    /// Absolute QSY request: <QF:14110000> (Hz)
    QsyAbsolute(u64),
    /// Ready to QSY / sent on reconnect
    QsyReady,
    /// QSY up 750 Hz
    QsyUp,
    /// QSY down 750 Hz
    QsyDown,
    /// QSY accepted
    QsyAccepted,
    /// QSY rejected
    QsyRejected,
    /// QSY out of allowed range
    QsyOutOfRange,
    /// Arrived on new frequency (test link)
    TestLink,
    /// Ready on new frequency
    QsyR,
    /// QSY auto
    QsyAuto,
    /// QSY lock
    QsyLock,
    /// VMail waiting, QSY needed
    Vw,

    // === VMail ===
    /// VMail header marker: <SM> (followed by TO/FRM/TME/SBJ/MSG fields)
    Sm,
    /// VMail acknowledgement
    Smr,
    /// VMail urgent: <SMU>
    Smu,
    /// VMail parking (store at intermediary)
    Smp,
    /// Parking not allowed
    Smfp,
    /// VMail subject: <SBJ:text>
    Subject(String),
    /// VMail body: <MSG:text>
    Message(String),
    /// VMail compose time: <TME:YYYY-MM-DD HH:MM:SS>
    ComposeTime(String),
    /// VMail recipient: <TO callsign> (space-separated, no colon)
    VmailTo(String),
    /// VMail sender: <FRM:callsign>
    VmailFrom(String),

    // === BBS / File transfer ===
    /// BBS directory listing request
    BbsListRequest,
    /// BBS directory listing: <BL:name|date|size\r\n...>
    BbsListing(String),
    /// BBS listing rejected
    BbsListRejected,
    /// BBS file request: <BG:filename>
    BbsFileRequest(String),
    /// BBS file request rejected
    BbsFileRejected,
    /// File offer: <SF:name|compressed_size|MD5>
    FileOffer(String),
    VmailFileOffer(String),
    /// Ready to receive file
    FileReady,
    /// File payload marker: <SFB> (base64 data follows as plain text in same frame)
    Sfb,
    /// File payload with inline data: <SFB:base64data>
    FilePayload(String),
    /// File receipt confirmed
    FileOk,
    /// File transfer aborted
    FileAborted,
    /// File transfer failed
    FileFailed,

    // === Relay/peeking ===
    /// Parked VMail peeking
    RelayPeek,
    /// Frequency schedule peeking
    FreqSchedule,
    /// Frequency schedule response
    FreqScheduleResponse,

    // === Chat enhancements ===
    /// Typing indicator
    IsTyping,
    /// Reply to specific chat message: <RPL:chat_id>
    ReplyChat(String),
    /// Emoji reaction: <RCT:reaction_id>
    Reaction(String),

    // === VMail metadata envelope ===
    /// VMail metadata start: <VMAIL_METADATA:...>
    VmailMetadata(String),

    // === AI gateway ===
    /// AI prompt prefix
    AiPromptPrefix,
    /// AI response prefix
    AiResponsePrefix,
    /// AI prompt error
    AiPromptError,
    /// AI rate limit exceeded
    AiLimitExceeded,
    /// AI prompt rejected
    AiPromptRejected,
    /// Exit AI mode
    AiDisable,

    // === Cluster ===
    /// Request connected stations list
    ClusterStationsRequest,
    /// Connected stations reply
    ClusterStationsReply(String),
    /// No stations connected
    ClusterStationsEmpty,
    /// Cluster message
    ClusterMessage(String),

    /// Unrecognized tag
    EmailReplyTo(String),
    EmailGateway,
    EmailGatewayRejected,
    EmailSent,
    EmailFailed,
    EmailRaw,
    EmailGatewayAddressRequest,
    /// `<U>`: urgent VMail
    Urgent,
    LastHeardCallsign(String),
    LocatorRequest,
    GpsRequest,
    VmailRejected,
    VmailFailed,
    LastConnectionsRequest,
    LastConnectionsRejected,
    /// `<VSS>`: stop verbose SNR
    VsiStop,
    /// `<VRPJ>`: parked-VMail peek refused
    RelayPeekRejected,
    Unknown(String),
}

/// Parse a tag string (the content between `<` and `>`).
pub fn parse_tag(inner: &str) -> Tag {
    // Signal report: R+05, R-12
    if inner.len() >= 3 && inner.starts_with('R') {
        let sign = inner.as_bytes()[1];
        if (sign == b'+' || sign == b'-') && inner[2..].chars().all(|c| c.is_ascii_digit()) {
            if let Ok(n) = inner[1..].parse::<i8>() {
                return Tag::SignalReport(n);
            }
        }
    }

    // Value tags with colon: <TAG:value>
    if let Some(colon_pos) = inner.find(':') {
        let key = &inner[..colon_pos];
        let value = &inner[colon_pos + 1..];
        return match key {
            "NAME" => Tag::Name(value.to_string()),
            "QTH" => Tag::Qth(value.to_string()),
            "LOC" => Tag::Locator(value.to_string()),
            "EM" => Tag::Email(value.to_string()),
            "FC" => Tag::FullCallsign(value.to_string()),
            "S" => Tag::Services(value.to_string()),
            "Q" => Tag::QsyRelative(value.parse().unwrap_or(0)),
            "QF" => Tag::QsyAbsolute(value.parse().unwrap_or(0)),
            "SBJ" => Tag::Subject(value.to_string()),
            "MSG" => Tag::Message(value.to_string()),
            "TME" => Tag::ComposeTime(value.to_string()),
            "FRM" => Tag::VmailFrom(value.to_string()),
            // VarAC 16 writes <TO:CALL> (older notes had <TO CALL>, still accepted)
            "TO" => Tag::VmailTo(value.to_string()),
            "BL" => Tag::BbsListing(value.to_string()),
            "BG" => Tag::BbsFileRequest(value.to_string()),
            "SF" => Tag::FileOffer(value.to_string()),
            // VMail with attachment: same offer format, payload is a zip (VarAC 16)
            "SFM" => Tag::VmailFileOffer(value.to_string()),
            "SFB" => Tag::FilePayload(value.to_string()),
            // Reply to a chat line: <C:id> (as in VarAC)
            "C" => Tag::ReplyChat(value.to_string()),
            // Last-heard request for one callsign: when did you last hear CALL?
            "LHC" => Tag::LastHeardCallsign(value.trim().to_uppercase()),
            // Email VMail: reply-to address (captured from VarAC 16)
            "RE" => Tag::EmailReplyTo(value.to_string()),
            "VMAIL_METADATA" => Tag::VmailMetadata(value.to_string()),
            "CC" => Tag::ClusterStationsReply(value.to_string()),
            _ => Tag::Unknown(inner.to_string()),
        };
    }

    // Space-separated: <TO callsign>
    if let Some(rest) = inner.strip_prefix("TO ") {
        return Tag::VmailTo(rest.to_string());
    }

    // Bare tags
    match inner {
        "DISC" => Tag::Disc,
        "AWAY" => Tag::Away,
        "AWQ" => Tag::Awq,
        "AE" => Tag::Ae,
        "IE" => Tag::Ie,
        "INFO" => Tag::Info,
        "INFOJ" => Tag::InfoJ,
        // SNR request: <SR> (current) and <SNRR> (the older form), both
        // answered with <R+NN>
        "SR" | "SNRR" => Tag::Snrr,
        "VER" => Tag::Ver,
        "VSI" => Tag::Vsi,
        "VSIR" => Tag::VsiR,
        "VSIJ" => Tag::VsiJ,
        "VSS" => Tag::VsiStop,
        // Last-heard peeking: <LHR> is the *request* in VarAC
        "LHR" => Tag::Lhp,
        "LHE" | "LHCE" => Tag::Lhe,
        "LHJ" => Tag::Lhj,
        // Urgent VMail marker: <SM>…<MSG:…><U> (as in VarAC)
        "U" => Tag::Urgent,
        "Q" => Tag::QsyReady,
        "QSYU" => Tag::QsyUp,
        "QSYD" => Tag::QsyDown,
        "QSYR" => Tag::QsyAccepted,
        "QSYJ" => Tag::QsyRejected,
        "QJO" => Tag::QsyOutOfRange,
        "TL" => Tag::TestLink,
        "VW" => Tag::Vw,
        "SM" => Tag::Sm,
        "SMU" => Tag::Smu,
        "SMR" => Tag::Smr,
        // VMail refused (VMail feature off) / failed on the receiving side (VarAC)
        "SMJ" => Tag::VmailRejected,
        "SMF" => Tag::VmailFailed,
        // EmComm last-connections peek / refused
        "LCR" => Tag::LastConnectionsRequest,
        // Locator / GPS inquiries (as in VarAC)
        "LOCR" => Tag::LocatorRequest,
        "GPSR" => Tag::GpsRequest,
        "LCJ" => Tag::LastConnectionsRejected,
        // Email gateway (VarAC 16): VMail is for email / gateway refused it / email went
        // out / email failed / send without the gateway footer
        "EG" => Tag::EmailGateway,
        "EJ" => Tag::EmailGatewayRejected,
        "ER" => Tag::EmailSent,
        "EF" => Tag::EmailFailed,
        "RA" => Tag::EmailRaw,
        "EGA" => Tag::EmailGatewayAddressRequest,
        "SMP" => Tag::Smp,
        "SMFP" => Tag::Smfp,
        "BLR" => Tag::BbsListRequest,
        "BLJ" => Tag::BbsListRejected,
        "BGJ" => Tag::BbsFileRejected,
        "SFB" => Tag::Sfb,
        "SFRD" => Tag::FileReady,
        "SFOK" => Tag::FileOk,
        "SFAB" => Tag::FileAborted,
        "SFFA" => Tag::FileFailed,
        // Parked-VMail peeking (as in VarAC)
        "VRP" => Tag::RelayPeek,
        "VRPJ" => Tag::RelayPeekRejected,
        // Frequency-scheduler peeking: <FSR> is the *request*, <FSO> = scheduler off
        "FSR" => Tag::FreqSchedule,
        "FSO" => Tag::FreqScheduleResponse,
        "SAFIE" => Tag::AiResponsePrefix,
        "AIE" => Tag::AiPromptError,
        "AIL" => Tag::AiLimitExceeded,
        "AIJ" => Tag::AiPromptRejected,
        "DISAI" => Tag::AiDisable,
        // Cluster (as in VarAC)
        "CCR" => Tag::ClusterStationsRequest,
        "CCE" => Tag::ClusterStationsEmpty,
        _ => Tag::Unknown(inner.to_string()),
    }
}

/// Extract tags from a message string, returning (tags, remaining_text).
pub fn extract_tags(message: &str) -> (Vec<Tag>, String) {
    let mut tags = Vec::new();
    let mut text = String::new();
    let mut chars = message.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '<' {
            let mut inner = String::new();
            let mut closed = false;
            for ic in chars.by_ref() {
                if ic == '>' {
                    closed = true;
                    break;
                }
                inner.push(ic);
            }
            if closed && !inner.is_empty() {
                tags.push(parse_tag(&inner));
            } else {
                text.push('<');
                text.push_str(&inner);
            }
        } else {
            text.push(c);
        }
    }

    (tags, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_signal_report() {
        assert_eq!(parse_tag("R+05"), Tag::SignalReport(5));
        assert_eq!(parse_tag("R-12"), Tag::SignalReport(-12));
    }

    #[test]
    fn parse_value_tags() {
        assert_eq!(parse_tag("NAME:John"), Tag::Name("John".into()));
        assert_eq!(parse_tag("S:VEB"), Tag::Services("VEB".into()));
        assert_eq!(parse_tag("Q:75"), Tag::QsyRelative(75));
    }

    #[test]
    fn parse_vmail_to() {
        assert_eq!(parse_tag("TO 4Z1AC"), Tag::VmailTo("4Z1AC".into()));
        assert_eq!(parse_tag("TO:K5XYZ"), Tag::VmailTo("K5XYZ".into()));
    }

    #[test]
    fn parse_bare_tags() {
        assert_eq!(parse_tag("DISC"), Tag::Disc);
        assert_eq!(parse_tag("SM"), Tag::Sm);
        assert_eq!(parse_tag("AWAY"), Tag::Away);
    }

    #[test]
    fn extract_opening_report() {
        let msg = "de W1ABC <R+05>";
        let (tags, text) = extract_tags(msg);
        assert_eq!(tags, vec![Tag::SignalReport(5)]);
        assert_eq!(text, "de W1ABC ");
    }

    #[test]
    fn extract_vmail() {
        let msg = "<SM><TO 4Z1AC><FRM:W1ABC><SBJ:Hello><MSG:Test message>";
        let (tags, _text) = extract_tags(msg);
        assert_eq!(tags.len(), 5);
        assert_eq!(tags[0], Tag::Sm);
        assert_eq!(tags[1], Tag::VmailTo("4Z1AC".into()));
        assert_eq!(tags[2], Tag::VmailFrom("W1ABC".into()));
        assert_eq!(tags[3], Tag::Subject("Hello".into()));
        assert_eq!(tags[4], Tag::Message("Test message".into()));
    }
}
