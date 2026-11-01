//! Headless Openslot station for interop testing against real VarAC through
//! a VARA-compatible modem (VARA, XARA). Prints every session event.
//!
//!   cargo run --example interop_peer -- --call W5TST --port 8400 [--bw 500|2300|2750] [--connect NR5P] [--chat "hi"] [--secs 90]

use std::time::{Duration, Instant};

use openslot::vara::commands::{Bandwidth, VaraCommand};
use openslot::vara::connection::VaraConnection;
use openslot::varac::session::{SessionEvent, StationConfig, VaracSession};

/// Wait up to 500 ms for an input, then process it fully (cancel-safe).
async fn poll_briefly(s: &mut VaracSession) -> Result<Option<SessionEvent>, ()> {
    match tokio::time::timeout(Duration::from_millis(500), s.next_input()).await {
        Ok(Some(i)) => Ok(s.handle_input(i).await),
        Ok(None) => Ok(None),
        Err(_) => Err(()),
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let opt = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let call = opt("--call").unwrap_or_else(|| "W5TST".into());
    let port: u16 = opt("--port").and_then(|p| p.parse().ok()).unwrap_or(8300);
    let secs: u64 = opt("--secs").and_then(|p| p.parse().ok()).unwrap_or(90);
    let connect = opt("--connect");
    let chats: Vec<String> = args.windows(2).filter(|w| w[0] == "--chat").map(|w| w[1].clone()).collect();

    let mut vara = VaraConnection::connect("127.0.0.1", port, port + 1).await?;
    let bw = match opt("--bw").as_deref() {
        Some("500") => Bandwidth::Bw500,
        Some("2750") => Bandwidth::Bw2750,
        _ => Bandwidth::Bw2300,
    };
    vara.initialize(&call, bw).await?;
    let cfg = StationConfig {
        callsign: call.clone(),
        name: Some("Tester".into()),
        qth: Some("Testville".into()),
        locator: Some(opt("--locator").unwrap_or_else(|| "EM12".into())),
        services: openslot::varac::messages::ServiceFlags(openslot::varac::messages::ServiceFlags::VMAIL),
            bbs_dir: None,
            email_gate: None,
            bbs_gate: None,
            policy: Default::default(),
    };
    let mut s = VaracSession::new(vara, cfg);
    // Optional KISS port (broadcasts): --kiss-port N [--broadcast TEXT]
    if let Some(kp) = opt("--kiss-port").and_then(|p| p.parse::<u16>().ok()) {
        s.attach_kiss("127.0.0.1", kp).await?;
    }
    s.qsy_offset_tens_hz = opt("--qsy").and_then(|v| v.parse().ok()).unwrap_or(0);
    // --ask: exercise the GUI path (QsyInvitationRequested -> send_qsy_invitation)
    s.ask_qsy_invite = args.iter().any(|a| a == "--ask");
    s.services_reply = opt("--sq-reply");
    let t0 = Instant::now();
    let ts = |t0: Instant| format!("{:6.1}", t0.elapsed().as_secs_f32());

    if let Some(ref dest) = connect {
        println!("{} >> CONNECT {call} {dest}", ts(t0));
        s.vara.send_command(&VaraCommand::Connect { source: call.clone(), destination: dest.clone() }).await?;
    }

    if let Some(target) = opt("--notify") {
        tokio::time::sleep(Duration::from_secs(8)).await;
        println!("{} >> relay notification to {target}", ts(t0));
        s.send_relay_notification(&target).await?;
    }
    if args.iter().any(|a| a == "--beacon") {
        tokio::time::sleep(Duration::from_secs(8)).await;
        println!("{} >> beacon CQFRAME {call}-9", ts(t0));
        s.vara.send_command(&VaraCommand::CqFrame { source: format!("{call}-9"), bandwidth: 500 }).await?;
    }
    // --advanced-beacon MASK: VarAC's ADVANCED (KISS 0x02) beacon, e.g. 2 = email gateway
    if let Some(mask) = opt("--advanced-beacon").and_then(|m| m.parse::<u8>().ok()) {
        tokio::time::sleep(Duration::from_secs(8)).await;
        println!("{} >> advanced beacon services={mask}", ts(t0));
        s.send_advanced_beacon(mask).await?;
    }
    if let Some(text) = opt("--broadcast") {
        // The modem accepts traffic only after its startup registration (~5 s)
        tokio::time::sleep(Duration::from_secs(8)).await;
        println!("{} >> broadcast {text:?}", ts(t0));
        s.send_broadcast(&text).await?;
    }
    let mut file_offer: Option<openslot::varac::messages::BbsFileOffer> = None;
    let mut pending_chats = chats.into_iter();
    let mut next_chat_at: Option<Instant> = None;
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        if let Some(t) = next_chat_at {
            if Instant::now() >= t {
                match pending_chats.next() {
                    Some(c) => {
                        println!("{} >> chat {c:?}", ts(t0));
                        s.send_chat(&c).await?;
                        next_chat_at = Some(Instant::now() + Duration::from_secs(opt("--gap").and_then(|g| g.parse().ok()).unwrap_or(20)));
                    }
                    None => next_chat_at = None,
                }
            }
        }
        match poll_briefly(&mut s).await {
            Ok(Some(ev)) => {
                match &ev {
                    SessionEvent::Ptt(_) | SessionEvent::BufferUpdate(_) => continue,
                    SessionEvent::QsyInvitationRequested => {
                        println!("{} << {ev:?} -> inviting {} Hz", ts(t0), s.qsy_offset_tens_hz * 10);
                        let tens = s.qsy_offset_tens_hz;
                        s.send_qsy_invitation(tens).await?;
                        continue;
                    }
                    // VarAC's "QSY NOW" countdown: accept at once
                    SessionEvent::QsyInvitation { target, .. } => {
                        println!("{} << {ev:?} -> accept", ts(t0));
                        let hz = match target {
                            openslot::varac::session::QsyTarget::Offset(hz) => *hz,
                            openslot::varac::session::QsyTarget::Up => 750,
                            openslot::varac::session::QsyTarget::Down => -750,
                            openslot::varac::session::QsyTarget::Absolute(_) => 0,
                        };
                        s.answer_qsy(true, false, hz).await?;
                        continue;
                    }
                    SessionEvent::HandshakeComplete => {
                        println!("{} << {ev:?}", ts(t0));
                        // --mid-qsy TENS: a QSY invitation once the QSO is open (VarAC's QSY window)
                        if let Some(tens) = opt("--mid-qsy").and_then(|v| v.parse::<i32>().ok()) {
                            tokio::time::sleep(Duration::from_secs(3)).await;
                            println!("{} >> mid-QSO QSY invitation <Q:{tens}>", ts(t0));
                            s.send_qsy_request(tens).await?;
                        }
                        // --vmail-attach may be repeated: each becomes its own VMail (queued transfers)
                        let attach: Vec<String> = args.windows(2).filter(|w| w[0] == "--vmail-attach").map(|w| w[1].clone()).collect();
                        for (i, path) in attach.iter().enumerate() {
                            let Some(to) = connect.clone() else { break };
                            let data = std::fs::read(path)?;
                            let name = std::path::Path::new(path).file_name().unwrap().to_string_lossy().into_owned();
                            let now = chrono::Utc::now();
                            let vm = openslot::varac::messages::VMail {
                                guid: String::new(), from: call.clone(), to,
                                subject: format!("Rust attachment {}", i + 1),
                                body: format!("VMail {} with {name} from Openslot", i + 1),
                                compose_time: now.format("%Y-%m-%d %H:%M:%S").to_string(),
                                timestamp: now, urgent: false, is_parked: false, has_attachment: true, reply_to: None, raw: false,
                            };
                            println!("{} >> vmail with attachment {name}", ts(t0));
                            s.send_vmail_with_attachments(&vm, &[(name, data)]).await?;
                        }
                        if let Some(path) = opt("--send-file") {
                            let data = std::fs::read(&path)?;
                            let name = std::path::Path::new(&path).file_name().unwrap().to_string_lossy().into_owned();
                            let prepared = openslot::varac::bbs::prepare_file(&name, &data)?;
                            println!("{} >> file offer <SF:{}>", ts(t0), prepared.offer_tag_value());
                            s.send_file(prepared).await?;
                        }
                        // --park-for CALL: park the VMail at the partner for CALL (relay)
                        if let Some(target) = opt("--park-for") {
                            let now = chrono::Utc::now();
                            let vm = openslot::varac::messages::VMail {
                                guid: String::new(), from: call.clone(), to: target.to_uppercase(),
                                subject: "Parked via NR5P".into(),
                                body: "Relayed VMail test from Openslot".into(),
                                compose_time: now.format("%Y-%m-%d %H:%M:%S").to_string(),
                                timestamp: now, urgent: false, is_parked: true, has_attachment: false, reply_to: None, raw: false,
                            };
                            println!("{} >> parked vmail {}", ts(t0), vm.to_wire());
                            s.send_vmail(&vm).await?;
                        }
                        if let (Some(spec), Some(to)) = (opt("--vmail"), connect.clone()) {
                            let (subject, body) = spec.split_once('|').unwrap_or((spec.as_str(), ""));
                            let now = chrono::Utc::now();
                            let vm = openslot::varac::messages::VMail {
                                guid: String::new(),
                                from: call.clone(),
                                to,
                                subject: subject.into(),
                                body: body.into(),
                                compose_time: now.format("%Y-%m-%d %H:%M:%S").to_string(),
                                timestamp: now,
                                urgent: args.iter().any(|a| a == "--urgent"),
                                is_parked: false,
                                has_attachment: false,
                                reply_to: None,
                                raw: false,
                            };
                            println!("{} >> vmail {}", ts(t0), vm.to_wire());
                            s.send_vmail(&vm).await?;
                        }
                        next_chat_at = Some(Instant::now() + Duration::from_secs(5));
                        continue;
                    }
                    SessionEvent::Message(openslot::varac::messages::IncomingMessage::VMailAttachmentOffer(offer))
                    | SessionEvent::Message(openslot::varac::messages::IncomingMessage::BbsFileOffer(offer)) => {
                        println!("{} << file offer {} ({} bytes compressed, md5 {}) -> accepting", ts(t0), offer.name, offer.compressed_size, offer.md5_hex);
                        file_offer = Some(offer.clone());
                        s.send_file_ready().await?;
                        continue;
                    }
                    SessionEvent::Message(openslot::varac::messages::IncomingMessage::BbsFilePayload(data)) => {
                        let b64 = String::from_utf8_lossy(data).into_owned();
                        match file_offer.as_ref().map(|o| openslot::varac::bbs::receive_file(b64.trim(), &o.md5_hex)) {
                            Some(Ok(bytes)) => {
                                println!("{} << file OK: {} bytes, md5 verified: {:?}", ts(t0), bytes.len(), String::from_utf8_lossy(&bytes[..bytes.len().min(120)]));
                                if let Some(dir) = opt("--save-dir") {
                                    let name = file_offer.as_ref().map(|o| openslot::varac::bbs::strip_gzip_suffix(&o.name).to_string()).unwrap_or("file".into());
                                    std::fs::write(std::path::Path::new(&dir).join(&name), &bytes)?;
                                    println!("{} << saved {dir}/{name}", ts(t0));
                                }
                                s.send_file_ok().await?;
                            }
                            other => {
                                println!("{} << file payload ({} bytes b64) could not be decoded: {other:?}", ts(t0), b64.len());
                                s.send_file_abort().await?;
                            }
                        }
                        continue;
                    }
                    SessionEvent::Message(openslot::varac::messages::IncomingMessage::VMail(vm)) => {
                        println!("{} << VMail from {} to {}: [{}] {}", ts(t0), vm.from, vm.to, vm.subject, vm.body);
                        // (the session already acknowledged it with <SMR>)
                        continue;
                    }
                    _ => {}
                }
                println!("{} << {ev:?}", ts(t0));
            }
            Ok(None) => {}
            Err(_) => {}
        }
    }
    if connect.is_some() {
        println!("{} >> disconnect", ts(t0));
        let _ = s.send_disconnect().await;
        let end = Instant::now() + Duration::from_secs(20);
        while Instant::now() < end {
            if let Ok(Some(ev)) = poll_briefly(&mut s).await {
                if !matches!(ev, SessionEvent::Ptt(_) | SessionEvent::BufferUpdate(_)) {
                    println!("{} << {ev:?}", ts(t0));
                }
                if matches!(ev, SessionEvent::Disconnected) {
                    break;
                }
            }
        }
    }
    Ok(())
}
