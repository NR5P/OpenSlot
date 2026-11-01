//! Email gateway: forward VMails addressed to internet email addresses over SMTP,
//! the way a VarAC 16 gateway does (captured from original VarAC):
//!
//! ```text
//! From: W5TST VIA NR5P <gateway@example.com>
//! To: TEST@EXAMPLE.COM
//! Subject: test
//!
//! test message
//!
//! <RELAY_TO_CALLSIGN:W5TST>
//!
//! <VMAIL_METADATA:
//! ----
//! VMAIL FORWARDED VIA EMAIL GATEWAY OF NR5P
//! ...
//! :VMAIL_METADATA_END>
//! ```
//!
//! The `<RELAY_TO_CALLSIGN:…>` tag lets a gateway route a reply back to the station.

use lettre::message::{Attachment, Mailbox, MultiPart, SinglePart, header::ContentType};
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::{Message, SmtpTransport, Transport};

use crate::varac::messages::VMail;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SmtpSecurity {
    /// Plain SMTP, then upgrade with STARTTLS (port 587; the usual choice).
    #[default]
    StartTls,
    /// TLS from the first byte (port 465).
    Tls,
    /// No encryption (local test servers only).
    None,
}

impl SmtpSecurity {
    pub fn from_key(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "tls" | "ssl" => Self::Tls,
            "none" | "off" | "plain" => Self::None,
            _ => Self::StartTls,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct GatewayConfig {
    pub server: String,
    pub port: u16,
    pub security: SmtpSecurity,
    pub username: String,
    pub password: String,
    /// Envelope / From address of the gateway mailbox.
    pub from_address: String,
}

impl GatewayConfig {
    pub fn is_usable(&self) -> bool {
        !self.server.trim().is_empty() && self.from_address.contains('@')
    }
}

/// Who may send email through us, shared live between the settings and the protocol
/// task (VarAC: `EnableEmailGateway`, `LimitAccessToCallsigns(+List)`, and the gateway
/// shutting itself down after too many failed sends).
#[derive(Debug, Clone, Default)]
pub struct EmailGate(std::sync::Arc<std::sync::RwLock<GateState>>);

#[derive(Debug, Clone, Default)]
pub struct GateState {
    pub enabled: bool,
    /// Some = only these full callsigns (VarAC splits the list on ',' and matches exactly).
    pub allow: Option<Vec<String>>,
    /// Shut down after the retry limit; back on when the operator resumes it.
    pub suspended: bool,
    /// Gateway mailbox, the answer to `<EGA>` (VarAC's "EMAIL GATEWAY ADDRESS" inquiry).
    pub address: String,
}

impl EmailGate {
    pub fn new(enabled: bool, allow: Option<Vec<String>>) -> Self {
        Self(std::sync::Arc::new(std::sync::RwLock::new(GateState { enabled, allow, ..Default::default() })))
    }

    /// For the BBS: VarAC matches the station's base call or its full callsign.
    pub fn allows(&self, full_call: &str) -> bool {
        let g = self.state();
        let base = full_call.split(['-', '/']).next().unwrap_or(full_call).trim();
        g.enabled
            && g.allow.as_ref().is_none_or(|list| {
                list.iter().any(|c| c.eq_ignore_ascii_case(full_call.trim()) || c.eq_ignore_ascii_case(base))
            })
    }

    /// `<EGA>` answer: our gateway address, or "N/A" when we're not a working gateway.
    pub fn address_reply(&self) -> String {
        let g = self.state();
        if g.enabled && !g.suspended && !g.address.trim().is_empty() { g.address.trim().to_string() } else { "N/A".into() }
    }

    pub fn state(&self) -> GateState {
        self.0.read().map(|g| g.clone()).unwrap_or_default()
    }

    pub fn update(&self, f: impl FnOnce(&mut GateState)) {
        if let Ok(mut g) = self.0.write() {
            f(&mut g);
        }
    }

    /// Ok if `full_call` may send email through us now, else the reason we refuse.
    pub fn check(&self, full_call: &str) -> Result<(), &'static str> {
        let g = self.state();
        if !g.enabled || g.suspended {
            return Err("EMAIL GATEWAY IS DISABLED");
        }
        match &g.allow {
            Some(list) if !list.iter().any(|c| c.eq_ignore_ascii_case(full_call.trim())) => {
                Err("EMAIL SENDER IS NOT ON THE EMAIL GATEWAY ALLOW LIST")
            }
            _ => Ok(()),
        }
    }
}

/// VarAC's allow-list text ("W5TST,K5XYZ-1") as callsigns.
pub fn parse_allow_list(s: &str) -> Vec<String> {
    s.split(',').map(|c| c.trim().to_ascii_uppercase()).filter(|c| !c.is_empty()).collect()
}

/// Where and how the VMail reached us (for the metadata footer).
#[derive(Debug, Clone, Default)]
pub struct Delivery {
    pub sender_call: String,
    pub gateway_call: String,
    pub band: Option<String>,
    pub frequency_hz: Option<u64>,
    pub snr: Option<f32>,
}

const RULE: &str = "-------------------------------------------------------------------------------------------------------";

/// VarAC writes frequencies as `14.102.750`.
fn freq(hz: u64) -> String {
    format!("{}.{:03}.{:03}", hz / 1_000_000, (hz / 1000) % 1000, hz % 1000)
}

/// VarAC always prints both lines (empty when unknown), SNR as "+05".
fn band_lines(d: &Delivery, label: &str) -> String {
    let band = d.band.clone().unwrap_or_default();
    let f = d.frequency_hz.map(freq).unwrap_or_default();
    let snr = d.snr.map(|s| crate::varac::messages::snr_text(s.round() as i32)).unwrap_or_default();
    format!("{label}: {band} ({f} MHz)\nSNR: {snr}\n")
}

/// Mail body in VarAC's layout (as VarAC's gateway mails it): message, routing tag,
/// metadata block. The routing tag is what brings a reply back to the station.
pub fn body(vm: &VMail, d: &Delivery) -> String {
    let mut meta = format!("{RULE}\nVMAIL FORWARDED VIA EMAIL GATEWAY OF {}\n", d.gateway_call);
    meta.push_str(&format!("COMPOSE TIME: {}\n", vm.compose_time));
    meta.push_str(&band_lines(d, "RECEIVED ON"));
    meta.push('\n');
    if vm.reply_to.is_some() {
        meta.push_str("REPLYING TO THIS EMAIL WILL SEND YOUR MESSAGE DIRECTLY\n");
        meta.push_str("TO THE REPLY-TO EMAIL ADDRESS PROVIDED BY THE SENDER.\n");
    } else {
        meta.push_str("REPLYING TO THIS EMAIL WILL SEND YOUR MESSAGE TO THE VARAC\n");
        meta.push_str("EMAIL GATEWAY IT ORIGINATED FROM.\n\n");
        meta.push_str(&format!("IT WILL BE FORWARDED TO {} ONCE THEY\nCONNECT TO RETRIEVE IT.\n\n", d.sender_call));
        meta.push_str("YOU WILL RECEIVE AN EMAIL NOTIFICATION ONCE YOUR MESSAGE IS DELIVERED.\n");
    }
    meta.push_str(RULE);
    meta.push('\n');
    // VarAC writes "//n" for line breaks in VMail text
    let text = vm.body.replace("//n", "\n");
    format!(
        "{text}\n\n<RELAY_TO_CALLSIGN:{}>\n\n<VMAIL_METADATA:\n{meta}:VMAIL_METADATA_END>",
        d.sender_call
    )
}

/// Build the email (without sending). Errors on bad addresses. A raw email (`<RA>`) is
/// the text exactly as typed, without routing tag and footer; it still comes from
/// "CALL VIA GATEWAY", which is how replies find their way back.
#[cfg_attr(not(test), allow(dead_code))]
pub fn build(cfg: &GatewayConfig, vm: &VMail, d: &Delivery) -> Result<Message, String> {
    build_with(cfg, vm, d, &[])
}

pub fn build_with(cfg: &GatewayConfig, vm: &VMail, d: &Delivery, attachments: &[(String, Vec<u8>)]) -> Result<Message, String> {
    let from_mailbox = Mailbox::new(
        Some(format!("{} VIA {}", d.sender_call, d.gateway_call)),
        cfg.from_address.trim().parse().map_err(|e| format!("gateway address: {e}"))?,
    );
    // (raw email, VarAC <RA>: the subject as it is and no Reply-To, so automated services
    // answer "CALL VIA GATEWAY" and the reply comes back over the air)
    let subject = if vm.urgent && !vm.raw { format!(">> URGENT: {}", vm.subject) } else { vm.subject.clone() };
    let mut b = Message::builder().from(from_mailbox).subject(subject);
    // Sender chose an email address for replies (VarAC REPLY TO); a bad one is ignored
    // rather than losing the mail
    if let Some(Ok(re)) = vm.reply_to.as_deref().filter(|_| !vm.raw).map(|r| r.trim().parse::<Mailbox>()) {
        b = b.reply_to(re);
    }
    // "Use comma for multiple recipients" (VarAC compose)
    let mut any = false;
    for addr in vm.to.split([',', ';']).map(str::trim).filter(|a| !a.is_empty()) {
        b = b.to(addr.parse().map_err(|e| format!("recipient {addr}: {e}"))?);
        any = true;
    }
    if !any {
        return Err("no recipient".into());
    }
    let text = if vm.raw { vm.body.replace("//n", "\n") } else { body(vm, d) };
    if attachments.is_empty() {
        return b.header(ContentType::TEXT_PLAIN).body(text).map_err(|e| e.to_string());
    }
    let octets = ContentType::parse("application/octet-stream").map_err(|e| e.to_string())?;
    let mut parts = MultiPart::mixed().singlepart(SinglePart::plain(text));
    for (name, data) in attachments {
        parts = parts.singlepart(Attachment::new(name.clone()).body(data.clone(), octets.clone()));
    }
    b.multipart(parts).map_err(|e| e.to_string())
}

fn transport(cfg: &GatewayConfig) -> Result<SmtpTransport, String> {
    if !cfg.is_usable() {
        return Err("email gateway not configured (SMTP server and gateway address)".into());
    }
    let server = cfg.server.trim();
    let mut t = match cfg.security {
        SmtpSecurity::None => SmtpTransport::builder_dangerous(server),
        SmtpSecurity::StartTls => SmtpTransport::builder_dangerous(server).tls(Tls::Required(
            TlsParameters::new(server.into()).map_err(|e| e.to_string())?,
        )),
        SmtpSecurity::Tls => SmtpTransport::builder_dangerous(server).tls(Tls::Wrapper(
            TlsParameters::new(server.into()).map_err(|e| e.to_string())?,
        )),
    }
    .port(cfg.port)
    // (VarAC SMTPTimeoutSeconds: 15)
    .timeout(Some(std::time::Duration::from_secs(15)));
    if !cfg.username.is_empty() {
        t = t.credentials(Credentials::new(cfg.username.clone(), cfg.password.clone()));
    }
    Ok(t.build())
}

/// Send over SMTP (blocking: call from `spawn_blocking`).
pub fn send(cfg: &GatewayConfig, vm: &VMail, d: &Delivery) -> Result<(), String> {
    send_with(cfg, vm, d, &[])
}

pub fn send_with(cfg: &GatewayConfig, vm: &VMail, d: &Delivery, attachments: &[(String, Vec<u8>)]) -> Result<(), String> {
    let t = transport(cfg)?;
    let email = build_with(cfg, vm, d, attachments)?;
    t.send(&email).map(|_| ()).map_err(|e| e.to_string())
}

/// A gateway notice to an email user, From "CALL Email gateway (DO NOT REPLY)" (VarAC).
fn notice(cfg: &GatewayConfig, gateway_call: &str, to: &str, subject: String, text: String) -> Result<Message, String> {
    Message::builder()
        .from(Mailbox::new(
            Some(format!("{gateway_call} Email gateway (DO NOT REPLY)")),
            cfg.from_address.trim().parse().map_err(|e| format!("gateway address: {e}"))?,
        ))
        .to(to.trim().parse().map_err(|e| format!("recipient {to}: {e}"))?)
        .subject(subject)
        .header(ContentType::TEXT_PLAIN)
        .body(text)
        .map_err(|e| e.to_string())
}

/// VarAC's "Your Email to CALL has been delivered" notice, sent to the email user once
/// the station collected their message over the air.
pub fn delivery_notice(cfg: &GatewayConfig, vm: &VMail, d: &Delivery) -> Result<Message, String> {
    let to_call = &vm.to; // (VarAC: the VMail's To, which is the station that collected it)
    let mut text = format!(
        "The following email you sent to {to_call} has been successfully delivered to them via {}\n\nSUBJECT: {}\n\nMESSAGE: {}\n\n",
        d.gateway_call, vm.subject, vm.body.replace("//n", "\n")
    );
    text.push_str(&format!("{RULE}\nVMAIL DELIVERED VIA EMAIL GATEWAY OF {}\n", d.gateway_call));
    text.push_str(&band_lines(d, "DELIVERED ON"));
    text.push_str(&format!("\n{RULE}\n"));
    notice(cfg, &d.gateway_call, &vm.from, format!("Your Email to {to_call} has been delivered"), text)
}

pub fn send_delivery_notice(cfg: &GatewayConfig, vm: &VMail, d: &Delivery) -> Result<(), String> {
    let t = transport(cfg)?;
    t.send(&delivery_notice(cfg, vm, d)?).map(|_| ()).map_err(|e| e.to_string())
}

/// VarAC's bounce when an incoming email's attachments are too big for the radio link.
pub fn send_attachment_bounce(
    cfg: &GatewayConfig,
    gateway_call: &str,
    sender: &str,
    to_call: &str,
    subject: &str,
    limit_kb: usize,
) -> Result<(), String> {
    let t = transport(cfg)?;
    let text = format!(
        "The attachments of your email (subject: {subject}) exceed the {limit_kb} KB limit of the radio link.\nPlease resend with smaller attachments.\n"
    );
    let m = notice(cfg, gateway_call, sender, format!("Your email to {to_call} was NOT delivered"), text)?;
    t.send(&m).map(|_| ()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vm(to: &str) -> VMail {
        VMail {
            guid: String::new(),
            from: "W5TST".into(),
            to: to.into(),
            subject: "test".into(),
            body: "line one//nline two".into(),
            compose_time: "2026-10-29 11:44:22".into(),
            timestamp: chrono::Utc::now(),
            urgent: false,
            is_parked: false,
            has_attachment: false,
            reply_to: None,
            raw: false,
        }
    }

    #[test]
    fn email_looks_like_varacs() {
        let cfg = GatewayConfig { server: "127.0.0.1".into(), port: 2525, from_address: "gw@example.com".into(), ..Default::default() };
        let d = Delivery { sender_call: "W5TST".into(), gateway_call: "NR5P".into(), band: Some("20m".into()), frequency_hz: Some(14_102_750), snr: Some(32.0) };
        let raw = String::from_utf8(build(&cfg, &vm("TEST@EXAMPLE.COM, b@example.org"), &d).unwrap().formatted()).unwrap();
        assert!(raw.contains("From: \"W5TST VIA NR5P\" <gw@example.com>"), "{raw}");
        assert!(raw.contains("To: TEST@EXAMPLE.COM, b@example.org"), "{raw}");
        assert!(raw.contains("Subject: test"));
        assert!(!raw.contains("Reply-To"));
        let mut with_re = vm("x@example.com");
        with_re.reply_to = Some("me@example.net".into());
        let raw = String::from_utf8(build(&cfg, &with_re, &d).unwrap().formatted()).unwrap();
        assert!(raw.contains("Reply-To: me@example.net"), "{raw}");
        let b = body(&vm("x@example.com"), &d);
        assert!(b.starts_with("line one\nline two\n\n<RELAY_TO_CALLSIGN:W5TST>"));
        assert!(b.contains("VMAIL FORWARDED VIA EMAIL GATEWAY OF NR5P"));
        assert!(b.contains("RECEIVED ON: 20m (14.102.750 MHz)"));
        assert!(b.contains("IT WILL BE FORWARDED TO W5TST ONCE THEY\nCONNECT TO RETRIEVE IT."));
        assert!(b.contains("YOU WILL RECEIVE AN EMAIL NOTIFICATION ONCE YOUR MESSAGE IS DELIVERED."));
        assert!(b.ends_with(":VMAIL_METADATA_END>"));
    }

    #[test]
    fn urgent_and_delivery_notice() {
        let cfg = GatewayConfig { server: "127.0.0.1".into(), port: 2525, from_address: "gw@example.com".into(), ..Default::default() };
        let d = Delivery { sender_call: "W5TST".into(), gateway_call: "NR5P".into(), ..Default::default() };
        let mut v = vm("x@example.com");
        v.urgent = true;
        let raw = String::from_utf8(build(&cfg, &v, &d).unwrap().formatted()).unwrap();
        assert!(raw.contains("Subject: >> URGENT: test"), "{raw}");
        let mut from_email = vm("W5TST");
        from_email.from = "bob@example.com".into();
        let raw = String::from_utf8(delivery_notice(&cfg, &from_email, &d).unwrap().formatted()).unwrap();
        assert!(raw.contains("To: bob@example.com"), "{raw}");
        assert!(raw.contains("Subject: Your Email to W5TST has been delivered"), "{raw}");
        // "NR5P Email gateway (DO NOT REPLY)": the parentheses make lettre encode the name
        assert!(raw.contains("=?utf-8?b?TlI1UCBFbWFpbCBnYXRld2F5IChETyBOT1QgUkVQTFkp?="), "{raw}");
    }

    #[test]
    fn raw_email_and_attachments() {
        let cfg = GatewayConfig { server: "127.0.0.1".into(), port: 2525, from_address: "gw@example.com".into(), ..Default::default() };
        let d = Delivery { sender_call: "W5TST".into(), gateway_call: "NR5P".into(), ..Default::default() };
        let mut v = vm("query@saildocs.com");
        v.raw = true;
        v.body = "send gfs:40N,60N,50W,10W".into();
        let raw = String::from_utf8(build(&cfg, &v, &d).unwrap().formatted()).unwrap();
        assert!(raw.contains("From: \"W5TST VIA NR5P\" <gw@example.com>"), "{raw}");
        assert!(raw.trim_end().ends_with("send gfs:40N,60N,50W,10W"), "no footer in a raw email: {raw}");
        assert!(!raw.contains("RELAY_TO_CALLSIGN"));
        let with = String::from_utf8(
            build_with(&cfg, &vm("x@example.com"), &d, &[("plan.txt".into(), b"hello".to_vec())]).unwrap().formatted(),
        ).unwrap();
        assert!(with.contains("multipart/mixed") && with.contains("filename=\"plan.txt\""), "{with}");
    }

    #[test]
    fn gate_follows_varac_rules() {
        let open = EmailGate::new(true, None);
        assert!(open.check("W5TST").is_ok());
        let limited = EmailGate::new(true, Some(parse_allow_list(" w5tst , K5XYZ-1")));
        assert!(limited.check("W5TST").is_ok());
        assert!(limited.check("K5XYZ-1").is_ok());
        // full callsign, exactly (as in VarAC)
        assert!(limited.check("K5XYZ").is_err());
        assert!(limited.check("NR5P").is_err());
        assert!(EmailGate::new(false, None).check("W5TST").is_err());
        open.update(|g| g.suspended = true);
        assert!(open.check("W5TST").is_err(), "a shut-down gateway refuses too");
    }

    #[test]
    fn bbs_list_matches_base_or_full_call_and_ega_reply() {
        let bbs = EmailGate::new(true, Some(parse_allow_list("W5TST,K5XYZ-1")));
        assert!(bbs.allows("W5TST-2"), "base call listed");
        assert!(bbs.allows("K5XYZ-1"));
        assert!(!bbs.allows("NR5P"));
        assert!(EmailGate::new(true, None).allows("NR5P"));
        let gw = EmailGate::new(true, None);
        assert_eq!(gw.address_reply(), "N/A");
        gw.update(|g| g.address = "gw@example.com".into());
        assert_eq!(gw.address_reply(), "gw@example.com");
        gw.update(|g| g.enabled = false);
        assert_eq!(gw.address_reply(), "N/A");
    }

    #[test]
    fn bad_recipient_is_rejected() {
        let cfg = GatewayConfig { server: "x".into(), port: 25, from_address: "gw@example.com".into(), ..Default::default() };
        assert!(build(&cfg, &vm("not an address"), &Delivery::default()).is_err());
    }
}
