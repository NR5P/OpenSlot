use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// VarAC uses "//n" (two literal chars) as a line separator in messages,
/// not actual newline characters.
pub const LINE_SEPARATOR: &str = "//n";

// === Slot system ===

/// Audio slot offset from the calling frequency.
/// Encoded as SSID suffix on the CQ callsign.
///
/// | SSID | Direction | Offset     |
/// |------|-----------|------------|
/// | 1-5  | Below CF  | -750n Hz   |
/// | 9    | Beacon    | 0 Hz       |
/// | 11-15| Above CF  | +750n Hz   |
pub const SLOT_STEP_HZ: i32 = 750;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Slot(pub i8);

impl Slot {
    pub fn is_beacon(&self) -> bool {
        self.0 == 9
    }

    pub fn offset_hz(&self) -> i32 {
        match self.0 {
            1..=5 => -(self.0 as i32) * SLOT_STEP_HZ,
            11..=15 => (self.0 as i32 - 10) * SLOT_STEP_HZ,
            9 => 0,
            _ => 0,
        }
    }

    pub fn from_ssid(ssid: i8) -> Option<Self> {
        match ssid {
            1..=5 | 9 | 11..=15 => Some(Self(ssid)),
            _ => None,
        }
    }
}

// === CQ / Beacon ===

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CqFrameType {
    Cq = 1,
    Beacon = 2,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CqFrame {
    pub frame_type: CqFrameType,
    pub callsign: String,
    pub slot: Slot,
    pub bandwidth: u16,
    pub timestamp: DateTime<Utc>,
    pub snr: Option<f32>,
    pub locator: Option<String>,
    pub cq_type: Option<String>,
}

// === Services ===

/// Service capability flags, matching VarAC's service bit encoding.
/// In opening messages: `<S:VEB>` (letters concatenated).
/// In advanced beacons: status_char = '@' + sum_of_bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceFlags(pub u8);

impl ServiceFlags {
    pub const VMAIL: u8 = 1;   // V
    pub const EMAIL: u8 = 2;   // E
    pub const AI: u8 = 4;      // A
    pub const BBS: u8 = 8;     // B

    pub fn has_vmail(&self) -> bool { self.0 & Self::VMAIL != 0 }
    pub fn has_email(&self) -> bool { self.0 & Self::EMAIL != 0 }
    pub fn has_ai(&self) -> bool { self.0 & Self::AI != 0 }
    pub fn has_bbs(&self) -> bool { self.0 & Self::BBS != 0 }

    pub fn from_letters(s: &str) -> Self {
        let mut bits = 0u8;
        for c in s.chars() {
            match c {
                'V' => bits |= Self::VMAIL,
                'E' => bits |= Self::EMAIL,
                'A' => bits |= Self::AI,
                'B' => bits |= Self::BBS,
                _ => {}
            }
        }
        Self(bits)
    }

    pub fn to_letters(&self) -> String {
        let mut s = String::new();
        if self.has_vmail() { s.push('V'); }
        if self.has_email() { s.push('E'); }
        if self.has_ai() { s.push('A'); }
        if self.has_bbs() { s.push('B'); }
        s
    }

    pub fn to_beacon_char(&self) -> char {
        (b'@' + self.0) as char
    }

    pub fn from_beacon_char(c: char) -> Self {
        Self(c as u8 - b'@')
    }
}

// === Opening report ===

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpeningReport {
    pub callsign: String,
    pub signal_report_db: i8,
    pub name: Option<String>,
    pub qth: Option<String>,
    pub locator: Option<String>,
    pub full_callsign: Option<String>,
    pub services: ServiceFlags,
    /// The called station's welcome message, sent in front: "Hi! de CALL <R+18>".
    #[serde(default)]
    pub welcome: Option<String>,
    /// It carried `<R+NN>` (EmComm stations may leave it out)
    #[serde(default)]
    pub has_report: bool,
}

// === Chat ===

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub from: String,
    pub to: String,
    pub text: String,
    pub timestamp: DateTime<Utc>,
    pub disconnect_after: bool,
    pub reply_to: Option<String>,
}

// === Broadcast ===

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Broadcast {
    pub from: String,
    pub to: String,
    pub via: Option<String>,
    pub message: String,
    pub band: Option<String>,
    pub timestamp: DateTime<Utc>,
    pub snr: Option<f32>,
}

/// A VarAC broadcast as sent on the air: an AX.25 UI frame (PID 0xF0) from our
/// callsign to the addressee ("ALL" or a callsign) whose info field is the plain
/// message text. Captured from VarAC 16.1.8 via the modem's KISS port.
/// A broadcast's text goes out sanitized (`<>` as `«»`) like VarAC's broadcasts, UTF-8.
pub fn broadcast_frame(from: &str, to: &str, text: &str) -> crate::vara::kiss::UiFrame {
    ui_frame(from, to, &sanitize(text))
}

/// A UI frame with the text as is (VarAC's relay notification is a literal `<SMP>`).
pub fn ui_frame(from: &str, to: &str, text: &str) -> crate::vara::kiss::UiFrame {
    crate::vara::kiss::UiFrame {
        dest: to.to_uppercase(),
        source: from.to_uppercase(),
        digis: Vec::new(),
        pid: 0xF0,
        info: text.as_bytes().to_vec(),
    }
}

/// A VMail relay notification's text: `<SMP>` ("VMail is parked for you here"), `<SMU>` when
/// some of it is urgent. Sent as a UI frame RELAY -> RECIPIENT; it's never a broadcast.
pub fn is_relay_notification(text: &str) -> bool {
    matches!(text.trim(), "<SMP>" | "<SMU>")
}

/// A UI frame with digipeaters (as in VarAC: one or two, "via" given
/// space-separated; with three or more it goes out without any).
pub fn ui_frame_via(from: &str, to: &str, via: &[String], text: &str) -> crate::vara::kiss::UiFrame {
    let mut f = ui_frame(from, to, text);
    let digis: Vec<String> = via.iter().map(|d| d.trim().to_uppercase()).filter(|d| !d.is_empty()).collect();
    if digis.len() <= 2 {
        f.digis = digis;
    }
    f
}

/// VarAC's one-letter band codes, used by Path Finder.
const BAND_CODES: &[(char, u64, u64, &str)] = &[
    ('A', 1_000_000, 2_000_000, "160m"),
    ('B', 3_000_000, 4_000_000, "80m"),
    ('C', 5_000_000, 6_000_000, "60m"),
    ('D', 7_000_000, 8_000_000, "40m"),
    ('E', 10_000_000, 11_000_000, "30m"),
    ('F', 14_000_000, 15_000_000, "20m"),
    ('G', 18_000_000, 19_000_000, "17m"),
    ('H', 21_000_000, 22_000_000, "15m"),
    ('I', 24_000_000, 25_000_000, "12m"),
    ('J', 27_000_000, 28_000_000, "11m"),
    ('K', 28_000_000, 30_000_000, "10m"),
    ('T', 40_000_000, 50_000_000, "8m"),
    ('L', 50_000_000, 55_000_000, "6m"),
    ('M', 144_000_000, 148_000_000, "2m"),
    ('N', 420_000_000, 450_000_000, "70cm"),
    ('O', 1_000_000_000, 2_000_000_000, "23cm"),
    ('P', 2_000_000_000, 3_000_000_000, "13cm"),
    ('Q', 5_000_000_000, 6_000_000_000, "5cm"),
    ('R', 10_000_000_000, 11_000_000_000, "3cm"),
    ('S', 24_000_000_000, 25_000_000_000, "1.2cm"),
];

/// Frequency → band as in VarAC: whole-MHz windows (lower bound in, upper bound out; the
/// GHz bands include their upper bound), "NA" otherwise.
pub fn band_name(hz: u64) -> Option<&'static str> {
    BAND_CODES.iter()
        .find(|(_, lo, hi, _)| hz >= *lo && (hz < *hi || (*lo >= 1_000_000_000 && hz == *hi)))
        .map(|b| b.3)
}

/// VarAC's calling frequencies: what counts as "on a calling frequency" (rules for wide
/// bandwidth, QSY, files, HamPlay…) — the HF calling frequencies, 11m and 6m, no VHF/UHF.
pub const GATHERING_FREQUENCIES_HZ: &[u64] = &[
    1_995_000, 3_595_000, 5_355_000, 7_105_000, 10_133_000, 14_105_000, 18_107_000, 21_105_000,
    24_927_000, 27_195_000, 28_105_000, 50_330_000,
];

/// A link's width as VARA reports it on CONNECTED, for people: VARA HF gives the bandwidth
/// in Hz ("500" → "500 Hz"), VARA FM its mode ("NARROW" → "FM narrow").
pub fn link_width_text(bw: &str) -> String {
    let bw = bw.trim();
    if !bw.is_empty() && bw.bytes().all(|b| b.is_ascii_digit()) {
        format!("{bw} Hz")
    } else if bw.eq_ignore_ascii_case("narrow") || bw.eq_ignore_ascii_case("wide") {
        format!("FM {}", bw.to_ascii_lowercase())
    } else {
        bw.to_string()
    }
}

pub fn is_calling_frequency(hz: u64) -> bool {
    GATHERING_FREQUENCIES_HZ.contains(&hz)
}

pub fn band_code(hz: u64) -> char {
    BAND_CODES.iter()
        .find(|(_, lo, hi, _)| hz >= *lo && (hz < *hi || (*lo >= 1_000_000_000 && hz == *hi)))
        .map_or('Z', |b| b.0)
}

pub fn band_of_code(c: char) -> &'static str {
    BAND_CODES.iter().find(|b| b.0 == c).map_or("NA", |b| b.3)
}

/// VarAC's answer to `<LCR>` in EmComm mode: one line per QSO, "CALL (time) freq",
/// newest first (the frequency only when known).
pub fn last_connections_reply(rows: &[(String, chrono::DateTime<chrono::Utc>, i64)]) -> String {
    rows.iter()
        .map(|(call, t, f)| {
            let mut line = format!("{call} ({})", t.format("%Y-%m-%d %H:%M:%S"));
            if *f != 0 {
                line.push_str(&format!(" {f}"));
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\r\n")
}

/// Path Finder request: a broadcast to ALL whose text is `^CALL` (VarAC).
pub fn path_finder_request(target: &str) -> String {
    format!("^{}", target.trim().to_uppercase())
}

/// Path Finder answer from a station that heard the target: `~` + (SNR+50, two digits)
/// + band code + whole hours since it was heard. `~45F3` = -5 dB on 20m 3 hours ago.
pub fn path_finder_answer(snr: i32, freq_hz: u64, hours: i64) -> String {
    // (VarAC writes one digit below -40 dB, which its own decoder misreads: clamp)
    format!("~{}{}{}", snr.clamp(-40, 49) + 50, band_code(freq_hz), hours.max(0))
}

/// Decode `~45F3` → (SNR dB, band, hours).
pub fn parse_path_finder_answer(s: &str) -> Option<(i32, &'static str, i64)> {
    let s = s.trim().strip_prefix('~')?;
    let snr: i32 = s.get(0..2)?.parse().ok()?;
    let band = band_of_code(s.get(2..3)?.chars().next()?);
    let hours = s.get(3..)?.trim().parse().ok()?;
    Some((snr - 50, band, hours))
}

/// The frequency a QSY broadcast names: `<QF:14105000>` (Hz).
pub fn qsy_broadcast_freq(msg: &str) -> Option<u64> {
    let i = msg.find("<QF:")?;
    let rest = &msg[i + 4..];
    rest[..rest.find('>')?].trim().parse().ok()
}

/// How VarAC shows a broadcast's text, or None when it doesn't show it at all
/// (someone else's Path Finder answer). `pf_target` = the call our last request asked about.
pub fn broadcast_display(message: &str, to: &str, my_call: &str, pf_target: Option<&str>) -> Option<String> {
    let to_me = base_call_of(to) == base_call_of(my_call);
    if let Some(target) = message.strip_prefix('^').filter(|_| to.eq_ignore_ascii_case("ALL")) {
        return Some(format!("PATH FINDER REQUEST: {}", target.trim()));
    }
    if message.starts_with('~') {
        if !to_me {
            return None;
        }
        let (snr, band, hours) = parse_path_finder_answer(message)?;
        let who = pf_target.unwrap_or("station");
        return Some(format!("PATH FINDER REPLY: Heard {who} on {band} at {snr}dB about {hours} hours ago"));
    }
    let mut text = message.to_string();
    if to.eq_ignore_ascii_case("EMCOMM") && text.contains("<EC>") {
        text.push_str(" (EMCOMM CHECK-IN REQUEST)");
    }
    if text.contains("<INFOJ>") {
        text = text.replace("<INFOJ>", "<INFOJ> (INQUIRE REQUEST REJECTED)");
    }
    if let Some(f) = qsy_broadcast_freq(message) {
        text.push_str(&format!(" (QSY TO: {} MHz)", freq_dotted(f as i64)));
    }
    Some(text)
}

fn base_call_of(call: &str) -> String {
    call.split(['-', '/']).next().unwrap_or(call).trim().to_uppercase()
}

/// Maximum broadcast message length in bytes.
pub const BROADCAST_MAX_CHARS: usize = 81;

// === VMail ===

/// Wire format: `<SM><TME:YYYY-MM-DD HH:MM:SS><TO callsign><FRM:callsign><SBJ:subject><MSG:body>`
/// Receiver confirms with `<SMR>`.
/// Parking uses `<SMP>` instead of `<SM>`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VMail {
    pub guid: String,
    pub from: String,
    pub to: String,
    pub subject: String,
    pub body: String,
    pub compose_time: String,
    pub timestamp: DateTime<Utc>,
    pub urgent: bool,
    pub is_parked: bool,
    /// Arrived with a zip of attachments (VarAC `<SFM:>` transfer).
    #[serde(default)]
    pub has_attachment: bool,
    /// Email only: VarAC's REPLY TO address (`<RE:addr>` after `<EG>`); None = replies
    /// come back through the gateway to the sender's callsign.
    #[serde(default)]
    pub reply_to: Option<String>,
    /// Email only: send exactly as typed, without the gateway footer (`<RA>`, VarAC's
    /// "raw email" for services like Saildocs). Replies still route back by the
    /// "CALL VIA GATEWAY" sender name.
    #[serde(default)]
    pub raw: bool,
}

/// VarAC's frequency text: 14105000 → "14.105.000".
pub fn freq_dotted(hz: i64) -> String {
    let s = hz.abs().to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push('.');
        }
        out.push(c);
    }
    out
}

/// VarAC's SNR text: "+05", "-12", "+00".
/// SNR as VarAC formats it: "+00;-00;0" (0 → "+00"), its "none" values 9999 → "+00" and
/// −9999 → "".
pub fn snr_text(db: i32) -> String {
    match db {
        -9999 => String::new(),
        0 | 9999 => "+00".into(),
        _ => format!("{db:+03}"),
    }
}

/// One heard station for the last-heard answers.
#[derive(Debug, Clone)]
pub struct HeardEntry {
    pub callsign: String,
    /// "CQ", "BEACON", "QSO", "BROADCAST"
    pub source: String,
    pub time: chrono::DateTime<chrono::Utc>,
    pub frequency_hz: i64,
    pub band: String,
    pub snr: Option<i32>,
}

/// VarAC's answer to `<LHR>`: one line per station, newest first, joined by "\n",
/// `CALL> HH:MM:SS / CQ|BEACON / 20m / -05dB` (SNR part only when known). None = send
/// `<LHE>` (nothing heard).
pub fn last_heard_reply(entries: &[HeardEntry]) -> Option<String> {
    if entries.is_empty() {
        return None;
    }
    Some(
        entries
            .iter()
            .map(|e| {
                let mut line = format!("{}> {} / {} / {}", e.callsign, e.time.format("%H:%M:%S"), e.source, e.band);
                if let Some(s) = e.snr {
                    line.push_str(&format!(" / {}dB", snr_text(s)));
                }
                line
            })
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

/// VarAC's answer to `<LHC:CALL>`: `Oct 30, 2026 14:22:05 / CQ / 14.105.000 / -07dB`.
pub fn last_heard_callsign_reply(e: &HeardEntry) -> String {
    format!(
        "{} / {} / {} / {}dB",
        e.time.format("%b %d, %Y %H:%M:%S"),
        e.source,
        freq_dotted(e.frequency_hz),
        snr_text(e.snr.unwrap_or(0))
    )
}

/// VarAC's answer to a parked-VMail peek (`<VRP>`), plain text: one line per VMail,
/// "(TO) Parked N days ago." plus " Relayed M days ago." once delivered; "NO VMAILS"
/// when there are none. `entries` = (addressee, parked at, relayed at), newest first.
pub fn relay_peek_reply(
    entries: &[(String, chrono::DateTime<chrono::Utc>, Option<chrono::DateTime<chrono::Utc>>)],
    now: chrono::DateTime<chrono::Utc>,
) -> String {
    if entries.is_empty() {
        return "NO VMAILS".into();
    }
    entries
        .iter()
        .map(|(to, parked, relayed)| {
            let mut line = format!("({to}) Parked {} days ago.", (now - *parked).num_days());
            if let Some(r) = relayed {
                line.push_str(&format!(" Relayed {} days ago.", (now - *r).num_days()));
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\r\n")
}

/// VMail fields go over the air with `<`/`>`
/// replaced by `«`/`»`, so text like a quoted email reply can't close the `<MSG:…>` tag.
pub fn sanitize(s: &str) -> String {
    s.replace('>', "»").replace('<', "«")
}

/// Undo [`sanitize`] (and the older `//{` `//}` escapes).
pub fn desanitize(s: &str) -> String {
    s.replace("//}", ">").replace("//{", "<").replace('»', ">").replace('«', "<")
}

impl VMail {
    /// Addressed to an internet email address (sent through an email gateway station).
    pub fn is_email(&self) -> bool {
        let t = self.to.trim();
        t.contains('@') && t.rsplit('@').next().is_some_and(|d| d.contains('.'))
    }

    /// VarAC 16 wire format (verified): `<SM><TME:..><TO:CALL><FRM:CALL><SBJ:..><MSG:..>`,
    /// `<SMU>` when urgent. A VMail whose `<TO:>` isn't the station we're linked to is
    /// parked there (relay) and delivered when the addressee connects.
    pub fn to_wire(&self) -> String {
        // VarAC: always <SM>; urgency is a <U> after the
        // message (<SMU> is "urgent parked", which VarAC doesn't act on)
        let header = "<SM>";
        let urgent = if self.urgent { "<U>" } else { "" };
        // To an internet email address via a VarAC email gateway (captured from VarAC):
        // no <FRM:> (the gateway uses the link partner) and a trailing <EG>
        let (subject, body) = (sanitize(&self.subject), sanitize(&self.body));
        if self.is_email() {
            // Order as VarAC sends them: <EG>, <RA>, <RE:…>
            let re = self.reply_to.as_deref().map(|r| format!("<RE:{}>", sanitize(r))).unwrap_or_default();
            let raw = if self.raw { "<RA>" } else { "" };
            return format!(
                "{header}<TME:{}><TO:{}><SBJ:{subject}><MSG:{body}>{urgent}<EG>{raw}{re}",
                self.compose_time, sanitize(&self.to)
            );
        }
        format!(
            "{header}<TME:{time}><TO:{to}><FRM:{from}><SBJ:{subject}><MSG:{body}>{urgent}",
            time = self.compose_time,
            to = sanitize(&self.to),
            from = sanitize(&self.from),
        )
    }
}

pub const VMAIL_MAX_STORED: usize = 500;

// === BBS / File transfer ===

/// File offer from BBS: `<SF:filename.VarAC_gzip|compressed_size|MD5_UPPER>`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BbsFileOffer {
    pub name: String,
    pub compressed_size: usize,
    pub md5_hex: String,
}

impl BbsFileOffer {
    pub fn parse(value: &str) -> Option<Self> {
        let parts: Vec<&str> = value.split('|').collect();
        if parts.len() != 3 { return None; }
        Some(Self {
            name: parts[0].to_string(),
            compressed_size: parts[1].parse().ok()?,
            md5_hex: parts[2].to_string(),
        })
    }
}

/// VarAC has no size cap of its own besides VARA HF's per-bandwidth limits (100K at 500 Hz,
/// 1M at 2300 Hz, see `session::attachment_limit`); these only guard against absurd sizes
/// (and a decompression bomb from the air).
pub const BBS_MAX_COMPRESSED: usize = 64 * 1024 * 1024;
pub const BBS_MAX_EXPANDED: usize = 256 * 1024 * 1024;
pub const BBS_MAX_CATALOG_ENTRIES: usize = 500;
pub const BBS_MAX_LISTING_BYTES: usize = 65_536;  // 64 KiB
pub const BBS_RECEIVE_TIMEOUT_SECS: u64 = 1_800;  // 30 min
pub const BBS_OFFER_TIMEOUT_SECS: u64 = 60;

/// BBS directory listing entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BbsListingEntry {
    pub name: String,
    pub date: String,
    pub size: usize,
}

// === Incoming message variants ===

#[derive(Debug, Clone, PartialEq)]
pub enum HamPlayMsg {
    /// `<P:n>`: game 1 = tic-tac-toe 3×3, 2 = Connect Four
    Invite(u8),
    /// `<PM:RC>` (tic-tac-toe row/column) or `<PM:C>` (Connect Four column)
    Move(String),
    Accept,
    Reject,
    Abort,
    Rematch,
}

#[derive(Debug)]
pub enum IncomingMessage {
    Opening(OpeningReport),
    Chat(ChatMessage),
    Broadcast(Broadcast),
    VMail(VMail),
    VMailAck,
    /// `<EGA>`: the partner asks for our email gateway address.
    EmailGatewayAddressRequest,
    /// `<LHC:CALL>`: when did we last hear CALL?
    LastHeardCallsignRequest(String),
    /// `<LOCR>` / `<GPSR>`: the partner asks for our locator / GPS position
    LocatorRequest,
    GpsRequest,
    /// `<SMJ>`: the partner refused our VMail (its VMail feature is off)
    VMailRejected,
    /// `<SMF>`: our VMail failed on the partner's side (VarAC stops forwarding)
    VMailFailed,
    /// `<LCR>`: EmComm "last connections" peek
    LastConnectionsRequest,
    /// `<LCJ>`: the partner refused our last-connections peek
    LastConnectionsRejected,
    /// `<VSS>`: the partner stopped verbose SNR
    ExtendedSnrStop,
    /// `<VRPJ>`: the partner won't show its parked VMail
    RelayPeekRejected,
    /// `<EJ>`: the gateway refused our email (gateway off, or we're not on its allow list).
    EmailGatewayRejected,
    /// `<ER>`: the gateway mailed our email out.
    EmailSent,
    /// `<EF>`: the gateway couldn't mail it (it keeps it and retries once fixed).
    EmailFailed,
    /// We refused an email VMail as gateway (answered `<EJ>`): (sender, VMail, why).
    EmailRefused(String, VMail, String),
    Away(Option<String>),
    Disconnect,
    BbsListRequest,
    BbsListing(Vec<BbsListingEntry>),
    BbsFileRequest(String),
    BbsFileOffer(BbsFileOffer),
    BbsFileReady,
    BbsFilePayload(Vec<u8>),
    BbsFileOk,
    BbsFileAborted,
    /// `<SFFA>`: the partner got our file broken
    BbsFileFailed,
    /// Our VMail's attachment was declined or arrived broken: the VMail stays queued
    AttachmentRefused,
    BbsListRejected,
    BbsFileRejected,
    ServiceInfo(ServiceFlags),
    QsyRequest { offset_hz: i32 },
    QsyAbsolute { freq_hz: u64 },
    QsyAccepted,
    QsyRejected,
    QsyOutOfRange,
    TestLink,
    InfoRequest,
    /// KISS relay notification (`<SMP>` from a station to us): VMail is parked for us there.
    /// VMail parked for us at `relay` (`<SMP>`; `<SMU>` = some of it is urgent).
    RelayNotification { relay: String, urgent: bool },
    /// `<SFM:name.zip.VarAC_gzip|size|MD5>`: a VMail with attachment(s), sent as a zip
    /// over the file-transfer flow (`<SFRD>` / `<SFB>` / `<SFOK>`).
    VMailAttachmentOffer(BbsFileOffer),
    /// Reply to `<INFO>`: VarAC sends only the filled-in fields, e.g. `<NAME:Bob><QTH:Austin><LOC:EM10HW>`.
    StationInfo { name: Option<String>, qth: Option<String>, locator: Option<String>, email: Option<String> },
    /// Reply to `<SNRR>`: how the partner hears us, e.g. `<R+20>`.
    SnrReport(i32),
    VersionRequest,
    IsTyping,
    SnrRequest,
    ClusterStationsRequest,
    ClusterStationsReply(String),
    ClusterStationsEmpty,
    ClusterMessage(String),
    /// `<E>` / `<EA>` / `<A>`: VarAC's older gateway announcements (shown as VarAC words them)
    ServiceNotice(String),
    /// `<CIR>`: list the cluster's other active instances
    ClusterInstancesRequest,
    /// `FROM>text <CM>`: a line relayed by a VarAC cluster
    ClusterRelayed(String),
    /// HamPlay (`<P:n>`, `<PM:x>`, `<PR>`, `<PJ>`, `<PA>`, `<PE>`)
    HamPlay(HamPlayMsg),
    /// `<SP:n>`: change the PACTOR speed level (`<SPR>` yes / `<SPJ>` no)
    PactorLevelRequest(String),
    PactorLevelAnswer(bool),
    LastHeardRequest,
    LastHeardReply(String),
    LastHeardEmpty,
    LastHeardRejected,
    /// `AI:question` — a question for our AI gateway (VarAC: the message starts with "AI:")
    AiPrompt(String),
    /// `<ACIE>answer` (or `<SAFIE>answer`) from the partner's AI gateway
    AiResponse(String),
    AiError,
    AiRateLimited,
    AiRejected,
    AiDisable,
    RelayPeek,
    FreqScheduleRequest,
    FreqScheduleResponse(String),
    AwayQsy,
    AwayTimeExceeded,
    IdleTimeExceeded,
    VMailWaiting,
    /// The partner liked (or unliked) our line with this chat number (`&L1n`/`&L0n`).
    Reaction { like: bool, id: u32 },
    ParkingNotAllowed,
    ExtendedSnrRequest,
    ExtendedSnrAccepted,
    ExtendedSnrRejected,
    QsyReady,
    QsyAuto,
    QsyLock,
    InfoDenied,
    RawText(String),
}

// === Calling frequencies ===

pub const CALLING_FREQUENCIES_HZ: &[(u64, &str)] = &[
    (1_995_000, "160m"),
    (3_595_000, "80m"),
    (5_355_000, "60m"),
    (7_105_000, "40m"),
    (10_133_000, "30m"),
    (14_105_000, "20m"),
    (18_107_000, "17m"),
    (21_105_000, "15m"),
    (24_927_000, "12m"),
    (28_105_000, "10m"),
    (50_330_000, "6m SSB"),
    (51_550_000, "6m FM"),
    (144_170_000, "2m SSB"),
    (144_950_000, "2m FM"),
    (432_550_000, "70cm SSB"),
    (439_600_000, "70cm FM"),
];

/// The calling frequency of the band `hz` is in (6m, 2m, 70cm: the SSB one), if it has one.
pub fn band_calling_frequency(hz: u64) -> Option<u64> {
    let band = band_name(hz)?;
    CALLING_FREQUENCIES_HZ.iter()
        .find(|(_, b)| *b == band || b.strip_prefix(band).is_some_and(|r| r.starts_with(' ')))
        .map(|(f, _)| *f)
}

/// Validate a ham radio callsign (basic format check).
/// Accepts 1-2 letter/digit prefix, 1+ digits, 1-3 letters suffix, optional -N SSID.
pub fn is_valid_callsign(call: &str) -> bool {
    let base = call.split('-').next().unwrap_or(call);
    if base.len() < 3 || base.len() > 7 {
        return false;
    }
    let has_digit = base.chars().any(|c| c.is_ascii_digit());
    let has_letter = base.chars().any(|c| c.is_ascii_alphabetic());
    let all_valid = base.chars().all(|c| c.is_ascii_alphanumeric() || c == '/');
    has_digit && has_letter && all_valid
}

/// Grid locator check as in VarAC: exactly `^[A-R][A-R][0-9][0-9][A-X][A-X]$`.
fn grid6(loc: &str) -> Option<(f64, f64)> {
    let b = loc.trim().as_bytes();
    if b.len() != 6 || !(b'A'..=b'R').contains(&b[0]) || !(b'A'..=b'R').contains(&b[1]) || !b[2].is_ascii_digit()
        || !b[3].is_ascii_digit() || !(b'A'..=b'X').contains(&b[4]) || !(b'A'..=b'X').contains(&b[5])
    {
        return None;
    }
    // the centre of the subsquare (as in VarAC)
    let lon = ((b[0] - b'A') as f64 * 20.0 + (b[2] - b'0') as f64 * 2.0) + ((b[4] - b'A') as f64 + 0.5) / 12.0 - 180.0;
    let lat = ((b[1] - b'A') as f64 * 10.0 + (b[3] - b'0') as f64) + ((b[5] - b'A') as f64 + 0.5) / 24.0 - 90.0;
    Some((lat, lon))
}

/// Round half to even (banker's rounding), as VarAC rounds.
fn to_int32(x: f64) -> i64 {
    let r = x.round();
    if (x - x.trunc()).abs() == 0.5 && (r as i64) % 2 != 0 { (r - x.signum()) as i64 } else { r as i64 }
}

/// VarAC's distance and bearing between two 6-character locators: (miles, km, bearing°).
/// A frequency typed by the operator, in Hz: MHz below 1000 (`14.1055`), kHz below
/// 1,000,000 (`14105.5`), else Hz; `14.105.500` (the readout's own form) is Hz. A comma
/// counts as a decimal point. None outside 100 kHz – 1.3 GHz.
pub fn parse_frequency_input(text: &str) -> Option<u64> {
    let t: String = text.trim().trim_end_matches(|c: char| c.is_ascii_alphabetic() || c == ' ')
        .chars().filter(|c| !c.is_whitespace()).map(|c| if c == ',' { '.' } else { c }).collect();
    if t.is_empty() {
        return None;
    }
    let hz = if t.matches('.').count() >= 2 {
        t.replace('.', "").parse::<u64>().ok()?
    } else {
        let v: f64 = t.parse().ok().filter(|v: &f64| v.is_finite() && *v > 0.0)?;
        (if v < 1000.0 { v * 1e6 } else if v < 1e6 { v * 1e3 } else { v }).round() as u64
    };
    (100_000..=1_300_000_000).contains(&hz).then_some(hz)
}

/// Our locator as stored and sent: trimmed and upper case (Openslot's distances and map
/// read upper case only; upper case is the form every receiver accepts), None when empty.
/// Nothing else changes.
pub fn normalize_locator(l: &str) -> Option<String> {
    Some(l.trim().to_uppercase()).filter(|l| !l.is_empty())
}

/// What's wrong with our own locator as Settings has it, for a hint under the field.
#[derive(Debug, Clone, PartialEq)]
pub enum LocatorAdvice {
    /// 4 characters (EM12): no distance or bearing (they need 6), and a square is ~110 × 185 km
    Coarse,
    /// Longer than 6 (EM12KP34): advanced beacons and typed CQs are skipped; the first 6
    TooLong(String),
    /// Not a Maidenhead locator at all
    Invalid,
}

/// Our locator as it works best: 6 characters (`EM12KP`), checked without regard to case
/// (Settings stores it upper case). None when it's fine, or empty (leaving it out is allowed:
/// VarAC answers "NO LOCATOR").
pub fn locator_advice(l: &str) -> Option<LocatorAdvice> {
    let u = l.trim().to_uppercase();
    let b = u.as_bytes();
    let field = |i: usize| b.len() > i + 1 && b[i..i + 2].iter().all(|c| (b'A'..=b'R').contains(c));
    let square = |i: usize| b.len() > i + 1 && b[i..i + 2].iter().all(u8::is_ascii_digit);
    match b.len() {
        0 => None,
        6 if is_grid_locator(&u) => None,
        4 if field(0) && square(2) => Some(LocatorAdvice::Coarse),
        n if n > 6 && u.get(..6).is_some_and(is_grid_locator) => Some(LocatorAdvice::TooLong(u[..6].to_string())),
        _ => Some(LocatorAdvice::Invalid),
    }
}

/// Grid locator check as in VarAC: `^[A-R][A-R][0-9][0-9][A-X][A-X]$` (6 characters only).
pub fn is_grid_locator(l: &str) -> bool {
    let b = l.as_bytes();
    b.len() == 6
        && b[..2].iter().all(|c| (b'A'..=b'R').contains(c))
        && b[2..4].iter().all(u8::is_ascii_digit)
        && b[4..].iter().all(|c| (b'A'..=b'X').contains(c))
}

pub fn loc_distance(a: &str, b: &str) -> Option<(i64, i64, i64)> {
    let ((la1, lo1), (la2, lo2)) = (grid6(a)?, grid6(b)?);
    let (p1, p2, dl) = (la1.to_radians(), la2.to_radians(), (lo2 - lo1).to_radians());
    let deg = (p1.sin() * p2.sin() + p1.cos() * p2.cos() * dl.cos()).clamp(-1.0, 1.0).acos().to_degrees();
    let mi = to_int32(deg * 60.0 * 1.1515);
    let km = to_int32(mi as f64 * 1.60934);
    let y = dl.sin() * p2.cos();
    let x = p1.cos() * p2.sin() - p1.sin() * p2.cos() * dl.cos();
    let bearing = to_int32((y.atan2(x) * 180.0 / std::f64::consts::PI + 360.0) % 360.0);
    Some((mi, km, bearing))
}

/// As VarAC shows it: "1234 mi (45º)" or "1986 Km (45º)".
pub fn loc_distance_text(a: &str, b: &str, km: bool) -> Option<String> {
    let (mi, k, brg) = loc_distance(a, b)?;
    Some(if km { format!("{k} Km ({brg}º)") } else { format!("{mi} mi ({brg}º)") })
}

/// VarAC's callsign check for what the operator types: the regex
/// `[a-z][0-9]?[0-9]?[0-9][a-z]` (case ignored, anywhere in the text) — a letter, one to
/// three digits, a letter.
pub fn varac_callsign_ok(s: &str) -> bool {
    let b: Vec<char> = s.chars().map(|c| c.to_ascii_lowercase()).collect();
    (0..b.len()).any(|i| {
        if !b[i].is_ascii_lowercase() {
            return false;
        }
        (1..=3).any(|n| {
            i + n + 1 < b.len() && b[i + 1..=i + n].iter().all(|c| c.is_ascii_digit()) && b[i + n + 1].is_ascii_lowercase()
        })
    })
}

#[cfg(test)]
mod callsign_tests {
    use super::*;

    #[test]
    fn valid_calls() {
        assert!(is_valid_callsign("W1ABC"));
        assert!(is_valid_callsign("NR5P"));
        assert!(is_valid_callsign("4Z1AC"));
        assert!(is_valid_callsign("VK2ABC"));
        assert!(is_valid_callsign("NR5P-1"));
        assert!(is_valid_callsign("W1ABC-9"));
        assert!(is_valid_callsign("JA1XYZ"));
    }

    /// Settings warns about a locator that can't be used fully: 4 characters, over 6
    /// (beacons and typed CQs skipped), not a locator. Case doesn't matter.
    #[test]
    fn locator_advice_for_our_own() {
        use LocatorAdvice::*;
        assert_eq!(locator_advice("EM12KP"), None);
        assert_eq!(locator_advice(" em12kp "), None, "lower case is stored upper case");
        assert_eq!(locator_advice(""), None, "no locator is allowed: no warning");
        assert_eq!(locator_advice("EM12"), Some(Coarse));
        assert_eq!(locator_advice("em12kp34"), Some(TooLong("EM12KP".into())));
        assert_eq!(locator_advice("EM12KP3"), Some(TooLong("EM12KP".into())));
        for bad in ["EM1", "EM12K", "ZZ12KP", "EM12ZZ", "12EMKP", "30.26,-97.74", "EMÉ2KP34"] {
            assert_eq!(locator_advice(bad), Some(Invalid), "{bad}");
        }
    }

    /// The stored locator: trimmed, upper case, nothing else changed; empty is none.
    #[test]
    fn normalizes_our_locator() {
        assert_eq!(normalize_locator(" em12kp\n").as_deref(), Some("EM12KP"));
        assert_eq!(normalize_locator("Em12"), Some("EM12".into()), "4 characters are kept");
        assert_eq!(normalize_locator("em12kp34"), Some("EM12KP34".into()), "not cut: Settings warns instead");
        assert_eq!(normalize_locator("   "), None);
        assert_eq!(normalize_locator(""), None);
        assert!(is_grid_locator(&normalize_locator("fn42ab").unwrap()));
    }

    #[test]
    fn locator_distance() {
        // FN42AA to FN42XX: corner to corner of one square (2° × 1° at 42°N), ~118 mi
        let (mi, km, brg) = loc_distance("FN42AA", "FN42XX").unwrap();
        assert!((110..=125).contains(&mi), "{mi}");
        assert_eq!(km, to_int32(mi as f64 * 1.60934));
        assert!((40..=60).contains(&brg), "north-east: {brg}");
        assert!(loc_distance("FN42", "JO01").is_none(), "4-character locators: nothing");
        assert!(loc_distance("fn42aa", "FN42XX").is_none(), "VarAC's check is case-sensitive");
        let t = loc_distance_text("FN42AA", "EM10HX", false).unwrap();
        assert!(t.ends_with("º)") && t.contains(" mi ("), "{t}");
        assert_eq!(to_int32(2.5), 2);
        assert_eq!(to_int32(3.5), 4);
    }

    #[test]
    fn varac_rule() {
        for ok in ["W1ABC", "VK100ANZAC", "DL/W1ABC/P", "W1AW_X", "nr5p", "4Z1AC", "JA1XYZ"] {
            assert!(varac_callsign_ok(ok), "{ok}");
        }
        for bad in ["ABCDE1", "4X1234", "", "12345", "W1", "1A"] {
            assert!(!varac_callsign_ok(bad), "{bad}");
        }
    }

    #[test]
    fn invalid_calls() {
        assert!(!is_valid_callsign(""));
        assert!(!is_valid_callsign("AB"));
        assert!(!is_valid_callsign("12345"));
        assert!(!is_valid_callsign("ABCDEF"));
        assert!(!is_valid_callsign("A"));
    }
}

#[cfg(test)]
mod link_width_tests {
    #[test]
    fn hf_in_hz_and_fm_by_mode() {
        assert_eq!(super::link_width_text("500"), "500 Hz");
        assert_eq!(super::link_width_text(" 2300 "), "2300 Hz");
        assert_eq!(super::link_width_text("NARROW"), "FM narrow");
        assert_eq!(super::link_width_text("WIDE"), "FM wide");
        assert_eq!(super::link_width_text("?"), "?");
    }
}

#[cfg(test)]
mod email_wire_tests {
    #[test]
    fn angle_brackets_survive_the_air() {
        let vm = super::VMail {
            guid: String::new(), from: "bob@example.com".into(), to: "W5TST".into(),
            subject: "Re: <test>".into(), body: "thanks!\n> quoted line\n> <RELAY>".into(),
            compose_time: "2026-10-30 10:00:00".into(), timestamp: chrono::Utc::now(),
            urgent: false, is_parked: false, has_attachment: false, reply_to: None, raw: false,
        };
        let wire = vm.to_wire();
        assert!(wire.contains("<MSG:thanks!\n» quoted line\n» «RELAY»>"), "{wire}");
        match crate::varac::session::parse_application_message(&wire) {
            super::IncomingMessage::VMail(got) => {
                assert_eq!(got.body, vm.body);
                assert_eq!(got.subject, vm.subject);
                assert_eq!(got.from, "bob@example.com");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn last_heard_replies_like_varac() {
        use chrono::TimeZone;
        let t = chrono::Utc.with_ymd_and_hms(2026, 10, 30, 14, 22, 5).unwrap();
        let e = |c: &str, src: &str, snr| super::HeardEntry {
            callsign: c.into(), source: src.into(), time: t, frequency_hz: 14_105_000, band: "20m".into(), snr,
        };
        assert_eq!(super::last_heard_reply(&[]), None);
        assert_eq!(
            super::last_heard_reply(&[e("4X1ABC", "CQ", Some(-5)), e("W1XYZ", "BEACON", None)]).unwrap(),
            "4X1ABC> 14:22:05 / CQ / 20m / -05dB\nW1XYZ> 14:22:05 / BEACON / 20m"
        );
        assert_eq!(super::last_heard_callsign_reply(&e("W1XYZ", "BROADCAST", Some(-7))), "Oct 30, 2026 14:22:05 / BROADCAST / 14.105.000 / -07dB");
        assert_eq!(super::freq_dotted(7_105_000), "7.105.000");
        assert_eq!(super::snr_text(3), "+03");
    }

    #[test]
    fn relay_peek_reply_like_varac() {
        let now = chrono::Utc::now();
        let d = |n| now - chrono::Duration::days(n);
        assert_eq!(super::relay_peek_reply(&[], now), "NO VMAILS");
        assert_eq!(
            super::relay_peek_reply(&[("4X1ABC".into(), d(3), Some(d(1))), ("W1XYZ".into(), d(5), None)], now),
            "(4X1ABC) Parked 3 days ago. Relayed 1 days ago.\r\n(W1XYZ) Parked 5 days ago."
        );
    }

    #[test]
    fn broadcasts_are_sanitized_but_relay_notification_is_not() {
        assert_eq!(super::broadcast_frame("W5TST", "ALL", "a <b> c").info, "a «b» c".as_bytes());
        assert_eq!(super::ui_frame("NR5P", "K5XYZ", "<SMP>").info, b"<SMP>".to_vec());
    }

    #[test]
    fn raw_flag_wire_order() {
        // VarAC order: <EG>, <RA>, <RE:…>
        let wire = "<SM><TME:2026-10-30 10:00:00><TO:query@saildocs.com><SBJ:wx><MSG:send gfs><EG><RA><RE:me@example.net>";
        match crate::varac::session::parse_application_message(wire) {
            super::IncomingMessage::VMail(vm) => {
                assert!(vm.raw);
                assert_eq!(vm.reply_to.as_deref(), Some("me@example.net"));
                assert_eq!(vm.to_wire(), wire);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn reply_to_round_trips() {
        // Captured from original VarAC with REPLY TO set to an address
        let wire = "<SM><TME:2026-10-29 11:51:28><TO:A@EXAMPLE.COM><SBJ:test><MSG:testing message><EG><RE:test@test.com>";
        match crate::varac::session::parse_application_message(wire) {
            super::IncomingMessage::VMail(vm) => {
                assert_eq!(vm.reply_to.as_deref(), Some("test@test.com"));
                assert_eq!(vm.to_wire(), wire);
            }
            other => panic!("{other:?}"),
        }
    }

    use super::*;

    /// Captured from VarAC 16.1.8 sending through an email gateway.
    #[test]
    fn email_vmail_matches_varac() {
        let vm = VMail {
            guid: String::new(), from: "W5TST".into(), to: "TEST@EXAMPLE.COM".into(),
            subject: "test".into(), body: "test message".into(),
            compose_time: "2026-10-29 11:44:22".into(), timestamp: chrono::Utc::now(),
            urgent: false, is_parked: false, has_attachment: false, reply_to: None, raw: false,
        };
        assert_eq!(vm.to_wire(), "<SM><TME:2026-10-29 11:44:22><TO:TEST@EXAMPLE.COM><SBJ:test><MSG:test message><EG>");
        let call = VMail { to: "NR5P".into(), ..vm };
        assert!(!call.is_email());
        assert!(call.to_wire().contains("<FRM:W5TST>"));
    }
}

#[cfg(test)]
mod broadcast_tests {
    use super::*;

    /// VarAC's Path Finder: `^CALL` to ALL; answers `~` + (SNR+50) + band code + hours.
    #[test]
    fn path_finder_wire_format() {
        assert_eq!(path_finder_request(" k1abc "), "^K1ABC");
        assert_eq!(path_finder_answer(-5, 14_105_000, 3), "~45F3");
        assert_eq!(path_finder_answer(12, 7_105_000, 0), "~62D0");
        assert_eq!(path_finder_answer(-60, 3_590_000, 23), "~10B23", "two digits even below -40 dB");
        assert_eq!(parse_path_finder_answer("~45F3"), Some((-5, "20m", 3)));
        assert_eq!(parse_path_finder_answer("~62D12"), Some((12, "40m", 12)));
        assert_eq!(band_code(146_520_000), 'M');
        assert_eq!(band_code(9_000_000), 'Z');
    }

    /// EmComm <LCR> answer (VarAC): "CALL (time) freq" per QSO, CRLF between.
    #[test]
    fn last_connections_lines() {
        use chrono::TimeZone;
        let t = chrono::Utc.with_ymd_and_hms(2026, 10, 30, 19, 46, 36).unwrap();
        assert_eq!(
            last_connections_reply(&[("NR5P".into(), t, 14_102_000), ("K1ABC".into(), t, 0)]),
            "NR5P (2026-10-30 19:46:36) 14102000\r\nK1ABC (2026-10-30 19:46:36)"
        );
        assert_eq!(
            broadcast_display("<EC>", "EMCOMM", "W5TST", None).as_deref(),
            Some("<EC> (EMCOMM CHECK-IN REQUEST)")
        );
    }

    #[test]
    fn broadcasts_shown_like_varac() {
        assert_eq!(broadcast_display("^K1ABC", "ALL", "W5TST", None).as_deref(), Some("PATH FINDER REQUEST: K1ABC"));
        assert_eq!(
            broadcast_display("~45F3", "W5TST", "W5TST", Some("K1ABC")).as_deref(),
            Some("PATH FINDER REPLY: Heard K1ABC on 20m at -5dB about 3 hours ago")
        );
        assert_eq!(broadcast_display("~45F3", "NR5P", "W5TST", None), None, "someone else's answer");
        assert_eq!(
            broadcast_display("Net here <QF:14105000>", "ALL", "W5TST", None).as_deref(),
            Some("Net here <QF:14105000> (QSY TO: 14.105.000 MHz)")
        );
        assert_eq!(qsy_broadcast_freq("go <QF:7105000> now"), Some(7_105_000));
        assert_eq!(broadcast_display("<INFOJ>", "W5TST", "W5TST", None).as_deref(), Some("<INFOJ> (INQUIRE REQUEST REJECTED)"));
    }
}

// === Profile, INFO reply and VarAC's message tags ===

/// Our station profile as VarAC keeps it ([MY_INFO]): what INFO answers and tags expand to.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Profile {
    pub name: String,
    pub qth: String,
    pub locator: String,
    pub email: String,
    pub rig: String,
    /// Watts, without the unit (VarAC adds "W")
    pub power: String,
    pub antenna: String,
    pub age: String,
    pub profession: String,
    pub hobbies: String,
}

const NL: &str = "\r\n";

fn info_tag(tag: &str, value: &str) -> String {
    if value.trim().is_empty() { String::new() } else { format!("<{tag}:{value}>") }
}

/// VarAC's answer to `<INFO>`: `<NAME:..><QTH:..><LOC:..><EM:..>`, then lines `RIG: …`,
/// `PWR: …W`, `ANT: …`, `AGE: …`, `PROFESSION: …`, `HOBBIES: …` for what's filled in,
/// or "NO INFO". (VarAC sends the email only when a QTH is set — its own check — and
/// we do the same so both answer alike.)
pub fn info_reply(p: &Profile) -> String {
    let mut s = String::new();
    if !p.name.is_empty() { s.push_str(&info_tag("NAME", &p.name)); }
    if !p.qth.is_empty() { s.push_str(&info_tag("QTH", &p.qth)); }
    if !p.locator.is_empty() { s.push_str(&info_tag("LOC", &p.locator)); }
    if !p.qth.is_empty() { s.push_str(&info_tag("EM", &p.email)); }
    for (label, value, unit) in [
        ("RIG", p.rig.replace("//n", NL), ""),
        ("PWR", p.power.clone(), "W"),
        ("ANT", p.antenna.clone(), ""),
        ("AGE", p.age.clone(), ""),
        ("PROFESSION", p.profession.clone(), ""),
        ("HOBBIES", p.hobbies.replace("//n", NL), ""),
    ] {
        if !value.is_empty() {
            s.push_str(&format!("{NL}{label}: {value}{unit}"));
        }
    }
    if s.is_empty() { "NO INFO".into() } else { s }
}

/// What VarAC's message tags expand to: our profile and the
/// station we're connected to.
pub struct TagContext<'a> {
    pub my_call: &'a str,
    pub me: &'a Profile,
    pub his_call: &'a str,
    pub his_name: &'a str,
    pub his_qth: &'a str,
    pub his_loc: &'a str,
}

/// Expand VarAC's tags in an outgoing message (typed chat and canned messages alike):
/// `<CALL> <NAME> <QTH> <EMAIL> <LOC> <RIG> <PWR> <ANT> <ICE> <MYDT> <UTCDT> <UTCD> <UTCT>
/// <HCALL> <HNAME> <HQTH> <HLOC>`. NAME/QTH/EMAIL/LOC become their protocol tags (so the
/// partner's VarAC files them in its log); `{MYCALL}`-style text goes as typed, as in VarAC.
pub fn expand_tags(text: &str, c: &TagContext) -> String {
    let me = c.me;
    let now = chrono::Utc::now();
    let local = chrono::Local::now();
    let ice = format!(
        "Age: {}{NL}Profession: {}{NL}Hobbies: {}",
        me.age, me.profession, me.hobbies.replace("//n", NL)
    );
    // (VarAC trims only to test for empty: the value goes as typed)
    let pairs: [(&str, String); 18] = [
        ("<CALL>", c.my_call.to_string()),
        ("<NAME>", info_tag("NAME", &me.name)),
        ("<QTH>", info_tag("QTH", &me.qth)),
        ("<EMAIL>", info_tag("EM", &me.email)),
        ("<LOC>", info_tag("LOC", &me.locator)),
        ("<GPSLOC>", crate::config::gps_answer()),
        ("<RIG>", me.rig.replace("//n", NL)),
        ("<PWR>", format!("{}W", me.power)),
        ("<MYDT>", local.format("%b %d, %Y %H:%M Local").to_string()),
        ("<UTCDT>", now.format("%b %d, %Y %H:%M UTC").to_string()),
        ("<UTCD>", now.format("%b %d, %Y").to_string()),
        ("<UTCT>", now.format("%H:%M UTC").to_string()),
        ("<ANT>", me.antenna.clone()),
        ("<ICE>", ice),
        ("<HCALL>", c.his_call.to_string()),
        ("<HNAME>", c.his_name.to_string()),
        ("<HQTH>", c.his_qth.to_string()),
        ("<HLOC>", c.his_loc.to_string()),
    ];
    let mut out = text.to_string();
    for (tag, value) in pairs {
        out = out.replace(tag, &value);
    }
    out
}

#[cfg(test)]
mod profile_tests {
    use super::*;

    #[test]
    fn typed_frequencies() {
        assert_eq!(parse_frequency_input("14.1055"), Some(14_105_500));
        assert_eq!(parse_frequency_input("14105.5"), Some(14_105_500));
        assert_eq!(parse_frequency_input("14105500"), Some(14_105_500));
        assert_eq!(parse_frequency_input("14.105.500"), Some(14_105_500));
        assert_eq!(parse_frequency_input(" 7,105 MHz"), Some(7_105_000));
        assert_eq!(parse_frequency_input("146.52"), Some(146_520_000));
        assert_eq!(parse_frequency_input("abc"), None);
        assert_eq!(parse_frequency_input("0"), None);
        assert_eq!(parse_frequency_input(""), None);
    }

    #[test]
    fn via_digipeaters_like_varac() {
        let v = |l: &[&str]| l.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(ui_frame_via("w5tst", "cq", &v(&["k5dig-1"]), "x").digis, ["K5DIG-1"]);
        assert_eq!(ui_frame_via("w5tst", "cq", &v(&["a", "b"]), "x").digis, ["A", "B"]);
        assert!(ui_frame_via("w5tst", "cq", &v(&["a", "b", "c"]), "x").digis.is_empty());
    }

    #[test]
    fn info_reply_like_varac() {
        let mut p = Profile { name: "Joe".into(), qth: "Paris".into(), locator: "JN18EU".into(), email: "a@b.c".into(), ..Default::default() };
        p.rig = "IC-7300".into();
        p.power = "100".into();
        p.antenna = "Dipole".into();
        assert_eq!(info_reply(&p), "<NAME:Joe><QTH:Paris><LOC:JN18EU><EM:a@b.c>\r\nRIG: IC-7300\r\nPWR: 100W\r\nANT: Dipole");
        // VarAC's own check: the email goes only with a QTH
        let p2 = Profile { email: "a@b.c".into(), ..Default::default() };
        assert_eq!(info_reply(&p2), "NO INFO");
        let p3 = Profile { rig: "FTDX3000".into(), ..Default::default() };
        assert_eq!(info_reply(&p3), "\r\nRIG: FTDX3000");
    }

    #[test]
    fn tags_expand_like_varac() {
        let me = Profile { name: "Joe".into(), locator: "EM10".into(), rig: "FTDX3000".into(), power: "50".into(), ..Default::default() };
        let c = TagContext { my_call: "W5TST/P", me: &me, his_call: "NR5P", his_name: "Nate", his_qth: "", his_loc: "EM12" };
        assert_eq!(expand_tags("de <CALL> <NAME><QTH> <RIG> <PWR>", &c), "de W5TST/P <NAME:Joe> FTDX3000 50W");
        assert_eq!(expand_tags("hi <HNAME> (<HCALL>, <HLOC>) {THEIRCALL}", &c), "hi Nate (NR5P, EM12) {THEIRCALL}", "VarAC sends braces as typed");
        assert!(expand_tags("<UTCT>", &c).ends_with(" UTC"));
        assert_eq!(expand_tags("<LOC>", &c), "<LOC:EM10>");
    }

    #[test]
    fn band_calling_frequency_follows_the_dial() {
        assert_eq!(band_calling_frequency(7_107_250), Some(7_105_000));
        assert_eq!(band_calling_frequency(7_074_000), Some(7_105_000));
        assert_eq!(band_calling_frequency(14_105_000), Some(14_105_000));
        assert_eq!(band_calling_frequency(3_573_000), Some(3_595_000));
        assert_eq!(band_calling_frequency(50_313_000), Some(50_330_000));
        assert_eq!(band_calling_frequency(9_000_000), None);
    }

}
