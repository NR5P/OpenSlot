//! AI gateway, as VarAC runs it:
//! a connected station sends `AI:question`; we ask Google Gemini with our own API key and
//! answer `<ACIE>answer` (`<AIE>` on failure, `<AIJ>` when the gateway is off, `<AIL>`
//! over the per-QSO limit). The prompt asks the model to keep answers short (radio) and to
//! write `<DISAI>` when the operator wants to stop.

use std::io::{Read, Write};
use std::time::Duration;

pub const AGENT_NAME: &str = "ACie";
pub const DEFAULT_MODEL: &str = "gemini-flash-latest";
const HOST: &str = "generativelanguage.googleapis.com";

/// Gateway settings (VarAC [AI]).
#[derive(Debug, Clone, Default)]
pub struct AiSettings {
    pub enabled: bool,
    pub api_key: String,
    pub model: String,
    pub timeout_secs: u64,
    /// LimitResponseCharacters (200)
    pub limit_chars: u32,
    /// EnableChatContext: send the QSO's earlier questions and answers too
    pub chat_context: bool,
    /// LimitInteractionsPerQSO (0 = no limit)
    pub limit_per_qso: u32,
    /// Announce the gateway in beacons and the opening's `<S:..A..>`
    pub announce: bool,
    pub customization: String,
}

/// Who is talking to whom, for the prompt.
#[derive(Debug, Clone, Default)]
pub struct PromptPeople {
    pub my_call: String,
    pub my_name: String,
    pub my_qth: String,
    pub my_locator: String,
    pub my_rig: String,
    pub my_antenna: String,
    pub my_power: String,
    pub their_call: String,
    pub their_name: String,
    pub their_qth: String,
    pub their_locator: String,
}

/// The gateway's system prompt: who the assistant is, how to answer over a slow radio link,
/// and what we know about both stations, then the operator's own extra instructions.
pub fn system_prompt(s: &AiSettings, p: &PromptPeople, now: chrono::DateTime<chrono::Utc>) -> String {
    let known = |v: &str| if v.trim().is_empty() { "unknown".to_string() } else { v.trim().to_string() };
    let (me, them) = (p.my_call.as_str(), p.their_call.as_str());
    let limit = s.limit_chars.max(1);
    let rig = known(&p.my_rig.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(", "));
    let power = if p.my_power.trim().is_empty() { known("") } else { format!("{} W", p.my_power.trim()) };
    let mut prompt = format!(
        "You are {AGENT_NAME}, the AI assistant of amateur radio station {me}, running in Openslot. \
         You are answering questions from {them}, who is connected to {me} by radio.\n\
         \n\
         Rules:\n\
         - Answer questions on any topic. You cannot do anything in the program; you only answer.\n\
         - The link is slow: keep every reply to {limit} characters or fewer. If an answer needs more, \
         send the first part, ask whether to continue, and send the next part when they say yes.\n\
         - If you need more details to answer well, ask a short question first.\n\
         - Use a bit of ham radio language now and then, and sometimes call the operator by name.\n\
         - Only say who you are if you are asked.\n\
         - When the operator wants to stop, say 73 and put <DISAI> at the end of your reply.\n\
         - It is now {}.\n\
         \n\
         Station {me} (the station you work for): name {}, QTH {}, locator {}, rig {rig}, antenna {}, power {power}.\n\
         Station {them} (the operator asking): name {}, QTH {}, locator {}.",
        now.format("%b %d, %Y %H:%M UTC"),
        known(&p.my_name), known(&p.my_qth), known(&p.my_locator), known(&p.my_antenna),
        known(&p.their_name), known(&p.their_qth), known(&p.their_locator),
    );
    if !s.customization.trim().is_empty() {
        prompt.push_str(&format!("\n\nMore instructions from {me}:\n{}", s.customization.trim()));
    }
    prompt
}

/// One turn of the conversation: (from the operator?, text).
pub type Turn = (bool, String);

/// The generateContent body: the system prompt as the first user part (as VarAC does),
/// then the earlier turns (with chat context), then the question.
pub fn request_body(prompt: &str, history: &[Turn], question: &str) -> String {
    let mut contents = vec![serde_json::json!({"role": "user", "parts": [{"text": prompt}]})];
    for (user, text) in history {
        contents.push(serde_json::json!({"role": if *user { "user" } else { "model" }, "parts": [{"text": text}]}));
    }
    contents.push(serde_json::json!({"role": "user", "parts": [{"text": question}]}));
    serde_json::json!({ "contents": contents }).to_string()
}

/// The answer text of a generateContent response.
pub fn answer_text(body: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let parts = v.get("candidates")?.get(0)?.get("content")?.get("parts")?.as_array()?;
    let text: String = parts.iter().filter_map(|p| p.get("text").and_then(|t| t.as_str())).collect();
    let text = text.trim_end_matches(['\r', '\n']).to_string();
    (!text.trim().is_empty()).then_some(text)
}

/// Ask Gemini. Blocking; run it on its own thread.
pub fn ask(s: &AiSettings, prompt: &str, history: &[Turn], question: &str) -> Result<String, String> {
    if s.api_key.trim().is_empty() {
        return Err("no API key".into());
    }
    let model = if s.model.trim().is_empty() { DEFAULT_MODEL } else { s.model.trim() };
    let body = request_body(prompt, history, question);
    let path = format!("/v1beta/models/{model}:generateContent");
    let (status, resp) = https_post(HOST, &path, &[("x-goog-api-key", s.api_key.trim())], &body, Duration::from_secs(s.timeout_secs.clamp(5, 300)))?;
    if status != 200 {
        let msg = serde_json::from_str::<serde_json::Value>(&resp).ok()
            .and_then(|v| v.get("error")?.get("message")?.as_str().map(str::to_string))
            .unwrap_or_default();
        return Err(format!("HTTP {status} {msg}"));
    }
    answer_text(&resp).ok_or_else(|| "empty answer".into())
}

/// A small HTTPS POST (HTTP/1.1, Connection: close) over rustls. Returns (status, body).
pub fn https_post(host: &str, path: &str, headers: &[(&str, &str)], body: &str, timeout: Duration) -> Result<(u16, String), String> {
    let mut tcp = crate::vara::connect_any(host, 443, timeout).map_err(|e| e.to_string())?;
    tcp.set_read_timeout(Some(timeout)).ok();
    tcp.set_write_timeout(Some(timeout)).ok();
    let roots = rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
    let config = rustls::ClientConfig::builder_with_provider(std::sync::Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_root_certificates(roots)
        .with_no_client_auth();
    let name = rustls::pki_types::ServerName::try_from(host.to_string()).map_err(|e| e.to_string())?;
    let conn = rustls::ClientConnection::new(std::sync::Arc::new(config), name).map_err(|e| e.to_string())?;
    let mut tls = rustls::StreamOwned::new(conn, &mut tcp);
    let mut req = format!("POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n", body.len());
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    tls.write_all(req.as_bytes()).map_err(|e| e.to_string())?;
    tls.write_all(body.as_bytes()).map_err(|e| e.to_string())?;
    let mut raw = Vec::new();
    // (servers may close without close_notify: keep what arrived)
    if let Err(e) = tls.read_to_end(&mut raw) {
        if raw.is_empty() {
            return Err(e.to_string());
        }
    }
    parse_http_response(&raw)
}

fn parse_http_response(raw: &[u8]) -> Result<(u16, String), String> {
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").ok_or("bad HTTP response")?;
    let head = String::from_utf8_lossy(&raw[..split]).to_string();
    let mut body = raw[split + 4..].to_vec();
    let status = head.split_whitespace().nth(1).and_then(|s| s.parse().ok()).ok_or("bad status line")?;
    if head.lines().any(|l| l.to_ascii_lowercase().starts_with("transfer-encoding:") && l.to_ascii_lowercase().contains("chunked")) {
        body = dechunk(&body);
    }
    Ok((status, String::from_utf8_lossy(&body).to_string()))
}

fn dechunk(mut data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    while let Some(eol) = data.windows(2).position(|w| w == b"\r\n") {
        let size_str = String::from_utf8_lossy(&data[..eol]);
        let Ok(size) = usize::from_str_radix(size_str.split(';').next().unwrap_or("").trim(), 16) else { break };
        data = &data[eol + 2..];
        if size == 0 || data.len() < size {
            out.extend_from_slice(&data[..size.min(data.len())]);
            break;
        }
        out.extend_from_slice(&data[..size]);
        data = data.get(size + 2..).unwrap_or(&[]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_is_filled() {
        let s = AiSettings { limit_chars: 200, customization: "Be brief.".into(), ..Default::default() };
        let p = PromptPeople { my_call: "W5TST".into(), my_power: "100".into(), their_call: "NR5P".into(), my_rig: "FTDX3000\nIC-7300".into(), ..Default::default() };
        let now = chrono::DateTime::parse_from_rfc3339("2026-10-30T18:05:00Z").unwrap().with_timezone(&chrono::Utc);
        let t = system_prompt(&s, &p, now);
        assert!(t.starts_with("You are ACie, the AI assistant of amateur radio station W5TST"));
        assert!(t.contains("questions from NR5P, who is connected to W5TST"));
        assert!(t.contains("200 characters or fewer"));
        assert!(t.contains("It is now Oct 30, 2026 18:05 UTC."));
        assert!(t.contains("put <DISAI> at the end"));
        assert!(t.contains("rig FTDX3000, IC-7300, antenna unknown, power 100 W."));
        assert!(t.contains("Station NR5P (the operator asking): name unknown"));
        assert!(t.ends_with("More instructions from W5TST:\nBe brief."));
        let plain = system_prompt(&AiSettings::default(), &p, now);
        assert!(plain.contains("1 characters or fewer") && !plain.contains("More instructions"));
    }

    #[test]
    fn request_and_answer() {
        let b = request_body("SYS", &[(true, "hi".into()), (false, "hello".into())], "wx?");
        let v: serde_json::Value = serde_json::from_str(&b).unwrap();
        let c = v["contents"].as_array().unwrap();
        assert_eq!(c.len(), 4);
        assert_eq!(c[0]["parts"][0]["text"], "SYS");
        assert_eq!(c[2]["role"], "model");
        assert_eq!(c[3]["parts"][0]["text"], "wx?");
        let resp = r#"{"candidates":[{"content":{"parts":[{"text":"Sunny, "},{"text":"73\n"}],"role":"model"}}]}"#;
        assert_eq!(answer_text(resp).as_deref(), Some("Sunny, 73"));
        assert_eq!(answer_text(r#"{"candidates":[]}"#), None);
    }

    #[test]
    fn http_parsing() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
        assert_eq!(parse_http_response(raw).unwrap(), (200, "hello world".into()));
        let raw = b"HTTP/1.1 403 Forbidden\r\nContent-Length: 2\r\n\r\n{}";
        assert_eq!(parse_http_response(raw).unwrap(), (403, "{}".into()));
    }
}
