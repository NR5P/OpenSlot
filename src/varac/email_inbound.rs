//! Email gateway, inbound: poll the gateway mailbox over IMAP and park each email for
//! the station it's addressed to, compatible with VarAC email gateways:
//!
//! - every unseen message in INBOX is fetched; it's marked seen once handled (also when
//!   skipped), except when saving it failed, so the next poll retries it;
//! - destination callsign = the `<RELAY_TO_CALLSIGN:CALL>` tag in the body (a reply quotes
//!   our forwarding footer), else a To/Cc display name `CALL VIA MYCALL` (what a mail client
//!   puts in To when replying to "CALL VIA MYCALL" <gateway@…>); otherwise it's skipped;
//! - the routing tag and the `<VMAIL_METADATA:…:VMAIL_METADATA_END>` footer are removed;
//! - "URGENT" in the subject makes the VMail urgent;
//! - attachments travel as a VMail package; above the radio limit the sender gets a bounce;
//! - it's filed in Parking (Inbox when it's for us) with the sender's email address as From,
//!   and collected with the normal relay mechanism (`<VW>`, relay notification on beacon).

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::varac::email_gateway::SmtpSecurity;

/// Largest VMail package the gateway will relay over the air.
pub const ATTACHMENT_LIMIT_BYTES: usize = 1_000_000;

#[derive(Debug, Clone, Default)]
pub struct ImapConfig {
    pub server: String,
    pub port: u16,
    pub security: SmtpSecurity,
    pub username: String,
    pub password: String,
}

impl ImapConfig {
    pub fn is_usable(&self) -> bool {
        !self.server.trim().is_empty() && !self.username.trim().is_empty()
    }
}

/// An email bound for a station.
#[derive(Debug, Clone)]
pub struct InboundEmail {
    /// Sender's email address (becomes the VMail's From).
    pub from: String,
    pub to_call: String,
    pub subject: String,
    pub body: String,
    pub urgent: bool,
    pub date: DateTime<Utc>,
    pub attachments: Vec<(String, Vec<u8>)>,
}

// === Routing ===

/// `<RELAY_TO_CALLSIGN:CALL>` anywhere in the text (value runs up to the next `>`).
pub fn relay_tag(body: &str) -> Option<String> {
    const TAG: &str = "<RELAY_TO_CALLSIGN:";
    let start = body.find(TAG)? + TAG.len();
    let end = body[start..].find('>')? + start;
    Some(body[start..end].trim().to_string()).filter(|c| !c.is_empty())
}

/// A display name `CALL VIA MYCALL` (VarAC: `^(\S+) VIA MYCALL([-/]\S*)?$`, any case).
pub fn via_name(name: &str, my_call: &str) -> Option<String> {
    let name = name.trim();
    let upper = name.to_ascii_uppercase();
    let at = upper.find(" VIA ")?;
    let (call, rest) = (&name[..at], &upper[at + 5..]);
    let my = my_call.trim().to_ascii_uppercase();
    let tail = rest.strip_prefix(my.as_str())?;
    let tail_ok = tail.is_empty() || (tail.starts_with(['-', '/']) && !tail.contains(char::is_whitespace));
    if call.is_empty() || call.contains(char::is_whitespace) || !tail_ok {
        return None;
    }
    let call = call.to_ascii_uppercase();
    crate::varac::messages::is_valid_callsign(&call).then_some(call)
}

/// The station an incoming email is for, or None (VarAC skips it).
pub fn destination(body: &str, recipient_names: &[String], my_call: &str) -> Option<String> {
    relay_tag(body)
        .map(|c| c.to_ascii_uppercase())
        .or_else(|| recipient_names.iter().find_map(|n| via_name(n, my_call)))
}

/// Remove our routing tag and metadata footer (also when quoted in a reply).
pub fn clean_body(body: &str) -> String {
    let mut s = body.replace("\r\n", "\n");
    while let Some(start) = s.find("<RELAY_TO_CALLSIGN:") {
        let Some(len) = s[start..].find('>') else { break };
        s.replace_range(start..start + len + 1, "");
    }
    const OPEN: &str = "<VMAIL_METADATA:";
    const CLOSE: &str = ":VMAIL_METADATA_END>";
    while let Some(start) = s.find(OPEN) {
        let Some(len) = s[start..].find(CLOSE) else { break };
        s.replace_range(start..start + len + CLOSE.len(), "");
    }
    // What's left of a quoted footer: trailing lines of only ">"
    let mut lines: Vec<&str> = s.lines().collect();
    while lines.last().is_some_and(|l| l.trim_start_matches(['>', ' ']).trim().is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

/// Parse a fetched message. None = no destination callsign (skip it).
pub fn parse(raw: &[u8], my_call: &str) -> Option<InboundEmail> {
    let msg = mail_parser::MessageParser::default().parse(raw)?;
    let mut names = Vec::new();
    for list in [msg.to(), msg.cc()].into_iter().flatten() {
        names.extend(list.mailboxes().filter_map(|m| m.name()).map(str::to_string));
    }
    let text = msg.body_text(0).map(|t| t.into_owned()).unwrap_or_default();
    let to_call = destination(&text, &names, my_call)?;
    let subject = msg.subject().unwrap_or("").to_string();
    let attachments = msg
        .attachments()
        .enumerate()
        .map(|(i, p)| {
            let name = p.attachment_name().map(str::to_string).unwrap_or_else(|| format!("attachment_{}", i + 1));
            (name, p.decoded().into_owned())
        })
        .collect();
    Some(InboundEmail {
        from: msg.from().and_then(|l| l.first()).and_then(|m| m.address()).unwrap_or("").to_string(),
        to_call,
        urgent: subject.contains("URGENT"),
        subject,
        body: clean_body(&text),
        date: msg.date().and_then(|d| DateTime::from_timestamp(d.to_timestamp(), 0)).unwrap_or_else(Utc::now),
        attachments,
    })
}

// === Minimal IMAP4rev1 client (LOGIN, SELECT, UID SEARCH/FETCH/STORE) ===

enum Conn {
    Plain(TcpStream),
    Tls(Box<rustls::StreamOwned<rustls::ClientConnection, TcpStream>>),
    /// Placeholder while upgrading to TLS (STARTTLS)
    Closed,
}

fn closed() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::NotConnected, "IMAP connection closed")
}

impl Read for Conn {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Conn::Plain(s) => s.read(buf),
            Conn::Tls(s) => s.read(buf),
            Conn::Closed => Err(closed()),
        }
    }
}

impl Write for Conn {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Conn::Plain(s) => s.write(buf),
            Conn::Tls(s) => s.write(buf),
            Conn::Closed => Err(closed()),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Conn::Plain(s) => s.flush(),
            Conn::Tls(s) => s.flush(),
            Conn::Closed => Err(closed()),
        }
    }
}

fn tls_wrap(server: &str, tcp: TcpStream) -> Result<Conn, String> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_root_certificates(roots)
        .with_no_client_auth();
    let name = rustls::pki_types::ServerName::try_from(server.to_string()).map_err(|e| e.to_string())?;
    let conn = rustls::ClientConnection::new(Arc::new(config), name).map_err(|e| e.to_string())?;
    Ok(Conn::Tls(Box::new(rustls::StreamOwned::new(conn, tcp))))
}

/// One server reply: the text lines (literals cut out) and the literals in order.
#[derive(Default)]
struct Reply {
    text: String,
    literals: Vec<Vec<u8>>,
}

struct Imap {
    conn: Conn,
    buf: Vec<u8>,
    tag: u32,
}

impl Imap {
    fn connect(cfg: &ImapConfig) -> Result<Self, String> {
        let server = cfg.server.trim();
        let tcp = TcpStream::connect((server, cfg.port)).map_err(|e| format!("connect {server}:{}: {e}", cfg.port))?;
        // (VarAC IMAPTimeoutSeconds: 15)
        let timeout = Some(Duration::from_secs(15));
        let _ = tcp.set_read_timeout(timeout);
        let _ = tcp.set_write_timeout(timeout);
        let conn = match cfg.security {
            SmtpSecurity::Tls => tls_wrap(server, tcp)?,
            _ => Conn::Plain(tcp),
        };
        let mut s = Imap { conn, buf: Vec::new(), tag: 0 };
        let greeting = s.line()?;
        if !greeting.starts_with("* OK") && !greeting.starts_with("* PREAUTH") {
            return Err(format!("unexpected greeting: {greeting}"));
        }
        if cfg.security == SmtpSecurity::StartTls {
            s.command("STARTTLS")?;
            let Conn::Plain(tcp) = std::mem::replace(&mut s.conn, Conn::Closed) else {
                return Err("STARTTLS on a non-plain connection".into());
            };
            s.buf.clear();
            s.conn = tls_wrap(server, tcp)?;
        }
        s.command(&format!("LOGIN {} {}", quote(&cfg.username), quote(&cfg.password)))
            .map_err(|e| format!("login failed: {e}"))?;
        Ok(s)
    }

    fn fill(&mut self) -> Result<(), String> {
        let mut chunk = [0u8; 8192];
        let n = self.conn.read(&mut chunk).map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("connection closed by server".into());
        }
        self.buf.extend_from_slice(&chunk[..n]);
        Ok(())
    }

    fn line(&mut self) -> Result<String, String> {
        loop {
            if let Some(i) = self.buf.windows(2).position(|w| w == b"\r\n") {
                let l: Vec<u8> = self.buf.drain(..i + 2).collect();
                return Ok(String::from_utf8_lossy(&l[..i]).into_owned());
            }
            self.fill()?;
        }
    }

    fn bytes(&mut self, n: usize) -> Result<Vec<u8>, String> {
        while self.buf.len() < n {
            self.fill()?;
        }
        Ok(self.buf.drain(..n).collect())
    }

    /// Send a command and collect everything up to its tagged completion.
    fn command(&mut self, cmd: &str) -> Result<Reply, String> {
        self.tag += 1;
        let tag = format!("A{:03}", self.tag);
        self.conn.write_all(format!("{tag} {cmd}\r\n").as_bytes()).map_err(|e| e.to_string())?;
        self.conn.flush().map_err(|e| e.to_string())?;
        let mut reply = Reply::default();
        loop {
            let mut l = self.line()?;
            // A line ending in {n} is followed by n raw bytes, then the rest of the line
            while let Some(n) = l.strip_suffix('}').and_then(|h| h.rsplit_once('{')).and_then(|(_, n)| n.parse::<usize>().ok()) {
                reply.text.push_str(&l);
                reply.text.push('\n');
                let lit = self.bytes(n)?;
                reply.literals.push(lit);
                l = self.line()?;
            }
            if let Some(status) = l.strip_prefix(&format!("{tag} ")) {
                return if status.starts_with("OK") { Ok(reply) } else { Err(status.to_string()) };
            }
            reply.text.push_str(&l);
            reply.text.push('\n');
        }
    }

    fn unseen(&mut self) -> Result<Vec<u32>, String> {
        let r = self.command("UID SEARCH UNSEEN")?;
        Ok(r.text
            .lines()
            .filter_map(|l| l.strip_prefix("* SEARCH"))
            .flat_map(|ids| ids.split_whitespace().filter_map(|n| n.parse().ok()).collect::<Vec<u32>>())
            .collect())
    }

    fn fetch(&mut self, uid: u32) -> Result<Vec<u8>, String> {
        let r = self.command(&format!("UID FETCH {uid} BODY.PEEK[]"))?;
        r.literals.into_iter().max_by_key(|l| l.len()).ok_or_else(|| format!("no body for UID {uid}"))
    }

    fn mark_seen(&mut self, uid: u32) -> Result<(), String> {
        self.command(&format!("UID STORE {uid} +FLAGS.SILENT (\\Seen)")).map(|_| ())
    }
}

/// IMAP quoted string.
fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// "IMAP TEST": log in and open INBOX. Returns e.g. "INBOX: 3 unseen".
pub fn test_connection(cfg: &ImapConfig) -> Result<String, String> {
    let mut s = Imap::connect(cfg)?;
    s.command("SELECT INBOX")?;
    let n = s.unseen()?.len();
    let _ = s.command("LOGOUT");
    Ok(format!("INBOX: {n} unseen"))
}

/// What the caller did with one message.
pub enum Handled {
    /// Done with it: mark it seen.
    Done,
    /// Couldn't save it: leave it unseen for the next poll.
    Retry,
}

/// One poll: hand every unseen INBOX message to `handle` (None = no destination
/// callsign found), then mark it seen unless the handler asks to retry.
pub fn poll(
    cfg: &ImapConfig,
    my_call: &str,
    mut handle: impl FnMut(Option<InboundEmail>) -> Handled,
) -> Result<usize, String> {
    let mut s = Imap::connect(cfg)?;
    s.command("SELECT INBOX")?;
    let uids = s.unseen()?;
    for &uid in &uids {
        let raw = s.fetch(uid)?;
        if let Handled::Done = handle(parse(&raw, my_call)) {
            s.mark_seen(uid)?;
        }
    }
    let _ = s.command("LOGOUT");
    Ok(uids.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_like_varac() {
        assert_eq!(relay_tag("hi\n> <RELAY_TO_CALLSIGN:W5TST>\n"), Some("W5TST".into()));
        assert_eq!(via_name("W5TST VIA NR5P", "NR5P"), Some("W5TST".into()));
        assert_eq!(via_name("w5tst via nr5p-1", "NR5P"), Some("W5TST".into()));
        assert_eq!(via_name("W5TST VIA K1ABC", "NR5P"), None);
        assert_eq!(via_name("Bob Smith VIA NR5P", "NR5P"), None);
        assert_eq!(via_name("W5TST VIA NR5PX", "NR5P"), None);
        // body tag wins over the To name
        assert_eq!(destination("<RELAY_TO_CALLSIGN:K5XYZ>", &["W5TST VIA NR5P".into()], "NR5P"), Some("K5XYZ".into()));
        assert_eq!(destination("no tag", &["W5TST VIA NR5P".into()], "NR5P"), Some("W5TST".into()));
        assert_eq!(destination("no tag", &["Gateway".into()], "NR5P"), None);
    }

    #[test]
    fn parses_a_reply() {
        let raw = b"From: Bob <bob@example.com>\r\n\
To: \"W5TST VIA NR5P\" <gateway@example.com>\r\n\
Subject: Re: URGENT test\r\n\
Date: Fri, 30 Oct 2026 10:00:00 +0000\r\n\
Content-Type: text/plain\r\n\r\n\
Got it, thanks.\r\n\r\n\
> test message\r\n>\r\n> <RELAY_TO_CALLSIGN:W5TST>\r\n>\r\n\
> <VMAIL_METADATA:\r\n> ----\r\n> VMAIL FORWARDED VIA EMAIL GATEWAY OF NR5P\r\n> :VMAIL_METADATA_END>\r\n";
        let e = parse(raw, "NR5P").unwrap();
        assert_eq!(e.to_call, "W5TST");
        assert_eq!(e.from, "bob@example.com");
        assert!(e.urgent);
        assert!(!e.body.contains("RELAY_TO_CALLSIGN"));
        assert!(!e.body.contains("METADATA"));
        assert!(e.body.starts_with("Got it, thanks."));
        assert!(e.body.ends_with("> test message"), "{:?}", e.body);
        assert!(e.attachments.is_empty());
    }

    #[test]
    fn unrouted_mail_is_skipped() {
        let raw = b"From: spam@example.com\r\nTo: gateway@example.com\r\nSubject: hi\r\n\r\nbuy now\r\n";
        assert!(parse(raw, "NR5P").is_none());
    }
}
