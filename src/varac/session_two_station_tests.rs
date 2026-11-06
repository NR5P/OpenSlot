//! Two real sessions talking over a pretend VARA link: every combination of encryption,
//! signing, AES and shared keys (and all of them off), both ways, with and without delay
//! on the air. What the stations see is checked against what went over the air.

use super::*;
use crate::vara::commands::VaraCommand;
use crate::varac::secure::{self, KeyTrust, SecureNote, SecurePolicy, SigCheck};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::OwnedWriteHalf;
use tokio::net::{TcpListener, TcpStream};

type Writer = Arc<tokio::sync::Mutex<OwnedWriteHalf>>;

/// One side's modem, as the air holds it.
struct Side {
    data_w: Writer,
}

/// The pretend air: CONNECT / DISCONNECT on either modem link both, what one sends reaches
/// the other (after `delay`), then the sender's modem reports its buffer empty.
struct Air {
    heard: Arc<Mutex<Vec<String>>>,
    a: Side,
    b: Side,
}

impl Air {
    fn heard(&self) -> Vec<String> {
        self.heard.lock().unwrap().clone()
    }

    /// A frame put on the air as if the other station sent it.
    async fn inject_to(&self, to_a: bool, text: &str) {
        let side = if to_a { &self.a } else { &self.b };
        side.data_w.lock().await.write_all(&framing::encode_frame(text)).await.unwrap();
    }
}

async fn modem() -> (VaraConnection, TcpStream, TcpStream) {
    let cmd = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let data = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let (cp, dp) = (cmd.local_addr().unwrap().port(), data.local_addr().unwrap().port());
    let accept = tokio::spawn(async move { (cmd.accept().await.unwrap().0, data.accept().await.unwrap().0) });
    let vara = VaraConnection::connect("127.0.0.1", cp, dp).await.unwrap();
    let (c, d) = accept.await.unwrap();
    (vara, c, d)
}

fn config(call: &str, name: &str) -> StationConfig {
    let cfg = StationConfig {
        callsign: call.into(),
        name: Some(name.into()),
        qth: Some("Testville, TX".into()),
        locator: Some("EM12kp".into()),
        services: ServiceFlags(ServiceFlags::VMAIL),
        bbs_dir: None,
        email_gate: None,
        bbs_gate: None,
        policy: Default::default(),
    };
    cfg.policy.on_calling_frequency.store(false, std::sync::atomic::Ordering::Relaxed);
    // (take offered files without asking, as the operator would)
    cfg.policy.auto_file.store(true, std::sync::atomic::Ordering::Relaxed);
    cfg
}

/// How the pretend air behaves.
#[derive(Clone, Copy)]
struct Opts {
    /// The two stations' callsigns (as configured, SSID and all)
    calls: (&'static str, &'static str),
    /// Hand each frame over in small pieces, a moment apart (as VARA's data port does)
    chunked: bool,
    /// The link's width on CONNECTED: VARA HF's bandwidth in Hz, or VARA FM's NARROW / WIDE
    bw: &'static str,
}

const PLAIN: Opts = Opts { calls: ("N0AAA", "N0BBB"), chunked: false, bw: "500" };

/// Two stations, N0AAA and N0BBB, on one air. `delay_ab` / `delay_ba`: how long a frame
/// takes each way.
async fn two_stations(
    sa: impl FnOnce(&SecurePolicy),
    sb: impl FnOnce(&SecurePolicy),
    delay_ab: Duration,
    delay_ba: Duration,
) -> (VaracSession, VaracSession, Air) {
    two_stations_with(PLAIN, sa, sb, delay_ab, delay_ba).await
}

async fn two_stations_with(
    opts: Opts,
    sa: impl FnOnce(&SecurePolicy),
    sb: impl FnOnce(&SecurePolicy),
    delay_ab: Duration,
    delay_ba: Duration,
) -> (VaracSession, VaracSession, Air) {
    let (va, ac, ad) = modem().await;
    let (vb, bc, bd) = modem().await;
    let (ca, cb) = (config(opts.calls.0, "Alice"), config(opts.calls.1, "Bob"));
    sa(&ca.policy.secure);
    sb(&cb.policy.secure);
    let (a, b) = (VaracSession::new(va, ca), VaracSession::new(vb, cb));

    let heard = Arc::new(Mutex::new(Vec::new()));
    let (acr, acw) = ac.into_split();
    let (bcr, bcw) = bc.into_split();
    let (adr, adw) = ad.into_split();
    let (bdr, bdw) = bd.into_split();
    let (acw, bcw, adw, bdw): (Writer, Writer, Writer, Writer) = (
        Arc::new(tokio::sync::Mutex::new(acw)),
        Arc::new(tokio::sync::Mutex::new(bcw)),
        Arc::new(tokio::sync::Mutex::new(adw)),
        Arc::new(tokio::sync::Mutex::new(bdw)),
    );
    // commands
    let bw = opts.bw;
    for mut r in [acr, bcr] {
        let (acw, bcw) = (acw.clone(), bcw.clone());
        tokio::spawn(async move {
            let mut buf = vec![0u8; 4096];
            let mut line = String::new();
            while let Ok(n @ 1..) = r.read(&mut buf).await {
                line.push_str(&String::from_utf8_lossy(&buf[..n]));
                while let Some(i) = line.find('\r') {
                    let cmd: String = line.drain(..=i).collect();
                    let cmd = cmd.trim();
                    let reply = if let Some(rest) = cmd.strip_prefix("CONNECT ") {
                        Some(format!("CONNECTED {rest} {bw}\rSN 12.0\r"))
                    } else if cmd == "DISCONNECT" || cmd == "ABORT" {
                        Some("DISCONNECTED\r".to_string())
                    } else {
                        None
                    };
                    if let Some(reply) = reply {
                        for w in [&acw, &bcw] {
                            let _ = w.lock().await.write_all(reply.as_bytes()).await;
                        }
                    }
                }
            }
        });
    }
    // data, each way in order, after the delay
    for (label, mut from, to, sender_cmd, delay) in [("A>", adr, bdw.clone(), acw.clone(), delay_ab), ("B>", bdr, adw.clone(), bcw.clone(), delay_ba)] {
        let h = heard.clone();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(tokio::time::Instant, Vec<u8>)>();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 1 << 20];
            while let Ok(n @ 1..) = from.read(&mut buf).await {
                h.lock().unwrap().push(format!("{label}{}", String::from_utf8_lossy(&buf[..n])));
                let _ = tx.send((tokio::time::Instant::now() + delay, buf[..n].to_vec()));
            }
        });
        let chunked = opts.chunked;
        tokio::spawn(async move {
            let mut rng = Rng(0x2545_F491_4F6C_DD1D);
            while let Some((at, bytes)) = rx.recv().await {
                tokio::time::sleep_until(at).await;
                let mut rest = &bytes[..];
                let mut ok = true;
                while !rest.is_empty() && ok {
                    // (a few bytes at a time, a few ms apart: each lands as its own read)
                    // (a big frame in proportionally bigger pieces, so it isn't slowed to a crawl)
                    let n = if chunked { (1 + rng.next() as usize % (bytes.len() / 20).max(40)).min(rest.len()) } else { rest.len() };
                    ok = to.lock().await.write_all(&rest[..n]).await.is_ok();
                    rest = &rest[n..];
                    if chunked && !rest.is_empty() {
                        tokio::time::sleep(Duration::from_millis(3)).await;
                    }
                }
                if !ok || sender_cmd.lock().await.write_all(b"BUFFER 0\r").await.is_err() {
                    break;
                }
            }
        });
    }
    let air = Air { heard, a: Side { data_w: adw }, b: Side { data_w: bdw } };
    (a, b, air)
}

/// What the app does on its own: accept a QSY countdown, take an offered file, confirm it.
async fn react(s: &mut VaracSession, ev: &SessionEvent) {
    crate::varac::session::accept_qsy_for_test(s, ev).await;
    if let SessionEvent::Message(IncomingMessage::BbsFilePayload(_)) = ev {
        let _ = s.send_file_ok().await;
    }
}

/// Both sessions for `ms`, every input handled to the end.
async fn run2(a: &mut VaracSession, b: &mut VaracSession, ms: u64) -> (Vec<SessionEvent>, Vec<SessionEvent>) {
    let (mut ea, mut eb) = (Vec::new(), Vec::new());
    let deadline = tokio::time::Instant::now() + Duration::from_millis(ms);
    loop {
        enum Got {
            A(Option<SessionInput>),
            B(Option<SessionInput>),
            Done,
        }
        let got = tokio::select! {
            i = a.next_input() => Got::A(i),
            i = b.next_input() => Got::B(i),
            _ = tokio::time::sleep_until(deadline) => Got::Done,
        };
        match got {
            Got::A(Some(i)) => {
                if let Some(ev) = a.handle_input(i).await {
                    react(a, &ev).await;
                    ea.push(ev);
                }
            }
            Got::B(Some(i)) => {
                if let Some(ev) = b.handle_input(i).await {
                    react(b, &ev).await;
                    eb.push(ev);
                }
            }
            Got::A(None) | Got::B(None) => tokio::time::sleep(Duration::from_millis(10)).await,
            Got::Done => break,
        }
    }
    (ea, eb)
}

/// Run both until each has finished its handshake (or `ms` passed).
async fn until_handshakes(a: &mut VaracSession, b: &mut VaracSession, ms: u64) -> (Vec<SessionEvent>, Vec<SessionEvent>) {
    let (mut ea, mut eb) = (Vec::new(), Vec::new());
    let done = |e: &[SessionEvent]| e.iter().any(|e| matches!(e, SessionEvent::HandshakeComplete));
    let start = tokio::time::Instant::now();
    while (!done(&ea) || !done(&eb)) && start.elapsed() < Duration::from_millis(ms) {
        let (x, y) = run2(a, b, 100).await;
        ea.extend(x);
        eb.extend(y);
    }
    (ea, eb)
}

fn texts(events: &[SessionEvent]) -> Vec<String> {
    events.iter().filter_map(|e| match e {
        SessionEvent::Message(IncomingMessage::RawText(t)) => Some(t.clone()),
        _ => None,
    }).collect()
}

fn vmails(events: &[SessionEvent]) -> Vec<VMail> {
    events.iter().filter_map(|e| match e {
        SessionEvent::Message(IncomingMessage::VMail(v)) => Some(v.clone()),
        _ => None,
    }).collect()
}

fn notes(events: &[SessionEvent]) -> Vec<SecureNote> {
    events.iter().filter_map(|e| match e {
        SessionEvent::Secure(n) => Some(n.clone()),
        _ => None,
    }).collect()
}

fn vmail(subject: &str, body: &str) -> VMail {
    VMail {
        guid: "t".into(), from: String::new(), to: String::new(), subject: subject.into(), body: body.into(),
        compose_time: String::new(), timestamp: Utc::now(), urgent: false, is_parked: false,
        has_attachment: false, reply_to: None, raw: false,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Mode {
    Off,
    Sign,
    Enc,
    EncSign,
    EncAes,
    /// Shared key: 0 and 1 are different keys
    Shared(u8),
    SharedAes(u8),
}

const KEYS: [&str; 2] = ["ABCD-EFGH-JKLM-NPQR-STUV-WXYZ-2345", "ZZZZ-YYYY-XXXX-WWWW-VVVV-UUUU-TTTT"];

impl Mode {
    fn apply(self, p: &SecurePolicy) {
        match self {
            Mode::Off => {}
            Mode::Sign => p.set(false, true),
            Mode::Enc => p.set(true, false),
            Mode::EncSign => p.set(true, true),
            Mode::EncAes => {
                p.set(true, false);
                p.set_aes(true);
            }
            Mode::Shared(k) | Mode::SharedAes(k) => {
                p.set_shared(true);
                p.set_aes(matches!(self, Mode::SharedAes(_)));
                p.add_shared(secure::SharedKey::new("net", "*", KEYS[k as usize]).unwrap());
            }
        }
    }

    fn encrypts(self) -> bool {
        matches!(self, Mode::Enc | Mode::EncSign | Mode::EncAes)
    }

    fn signs(self) -> bool {
        matches!(self, Mode::Sign | Mode::EncSign)
    }

    fn shared(self) -> Option<u8> {
        match self {
            Mode::Shared(k) | Mode::SharedAes(k) => Some(k),
            _ => None,
        }
    }
}

/// Should a link between these two be encrypted?
/// Does this station want what it writes encrypted?
fn wants(m: Mode) -> bool {
    m.encrypts() || m.shared().is_some()
}

fn expect_encrypted(a: Mode, b: Mode) -> bool {
    (a.encrypts() && b.encrypts()) || matches!((a.shared(), b.shared()), (Some(x), Some(y)) if x == y)
}

/// A whole QSO between `ma` (the caller) and `mb`: chat both ways (some typed before the
/// keys can have crossed), a multi-line message, a VMail each way. Everything must arrive
/// once and as sent, the link must be what both settings say, nothing secret may be heard
/// on the air when it's encrypted, and no false warnings.
async fn qso(ma: Mode, mb: Mode, delay_ab: u64, delay_ba: u64) {
    qso_with(PLAIN, ma, mb, delay_ab, delay_ba).await;
}

async fn qso_with(opts: Opts, ma: Mode, mb: Mode, delay_ab: u64, delay_ba: u64) {
    let tag = format!("{ma:?} -> {mb:?} (delay {delay_ab}/{delay_ba} ms, {:?}, chunked {})", opts.calls, opts.chunked);
    let (mut a, mut b, air) = two_stations_with(opts, |p| ma.apply(p), |p| mb.apply(p), Duration::from_millis(delay_ab), Duration::from_millis(delay_ba)).await;
    a.vara.send_command(&VaraCommand::Connect { source: opts.calls.0.into(), destination: opts.calls.1.into() }).await.unwrap();
    // (typed as soon as the link is up, before anything came from the other side)
    let (mut ea, mut eb) = (Vec::new(), Vec::new());
    while a.state != SessionState::Connected || b.state != SessionState::Connected {
        let (x, y) = run2(&mut a, &mut b, 20).await;
        ea.extend(x);
        eb.extend(y);
    }
    // (a station with encryption on that ends up on a clear link sends nothing it wrote)
    let enc = expect_encrypted(ma, mb);
    let (a_blocked, b_blocked) = (wants(ma) && !enc, wants(mb) && !enc);
    // (refused at once when the link has already settled in the clear, else held and then
    // withheld: either way the operator is told and gets it back)
    assert!(a.send_typed("alpha one").await.is_ok() || a_blocked, "{tag}");
    assert!(b.send_typed("bravo one").await.is_ok() || b_blocked, "{tag}");
    let (x, y) = until_handshakes(&mut a, &mut b, 15_000).await;
    ea.extend(x);
    eb.extend(y);
    assert!(ea.iter().any(|e| matches!(e, SessionEvent::HandshakeComplete)), "{tag}: A's handshake: {ea:?}");
    assert!(eb.iter().any(|e| matches!(e, SessionEvent::HandshakeComplete)), "{tag}: B's handshake: {eb:?}");

    for (r, blocked) in [
        (a.send_typed("alpha two\r\nsecond line").await, a_blocked),
        (a.send_vmail(&vmail("Meeting", "At the hall, 7 pm. Bring the antenna.")).await, a_blocked),
        (b.send_typed("bravo two").await, b_blocked),
        (b.send_vmail(&vmail("Re: Meeting", "Copy, will be there")).await, b_blocked),
    ] {
        assert_eq!(r.is_err(), blocked, "{tag}: {r:?}");
    }
    let (x, y) = run2(&mut a, &mut b, 6000 + 4 * (delay_ab + delay_ba)).await;
    ea.extend(x);
    eb.extend(y);

    // everything arrived, once, as sent (lines of one frame show as one message) — or,
    // from a station that withholds, nothing did, and it was told
    let (ta, tb) = (texts(&ea).join("\n"), texts(&eb).join("\n"));
    for (got, want, blocked) in [(&tb, "alpha one", a_blocked), (&tb, "alpha two\nsecond line", a_blocked), (&ta, "bravo one", b_blocked), (&ta, "bravo two", b_blocked)] {
        assert_eq!(got.matches(want).count(), usize::from(!blocked), "{tag}: {want:?} in {got:?}\nA: {ea:?}\nB: {eb:?}");
        assert!(!got.contains(secure::SIG_TAG) && !got.contains("<OS"), "{tag}: a tag shows: {got:?}");
    }
    let (va, vb) = (vmails(&ea), vmails(&eb));
    let want_b: &[(&str, &str)] = if a_blocked { &[] } else { &[("Meeting", "At the hall, 7 pm. Bring the antenna.")] };
    let want_a: &[(&str, &str)] = if b_blocked { &[] } else { &[("Re: Meeting", "Copy, will be there")] };
    assert_eq!(vb.iter().map(|v| (v.subject.as_str(), v.body.as_str())).collect::<Vec<_>>(), want_b, "{tag}: {eb:?}");
    assert_eq!(va.iter().map(|v| (v.subject.as_str(), v.body.as_str())).collect::<Vec<_>>(), want_a, "{tag}: {ea:?}");
    for (who, e, blocked) in [("A", &ea, a_blocked), ("B", &eb, b_blocked)] {
        let withheld: Vec<String> = notes(e).into_iter().filter_map(|n| match n {
            SecureNote::Withheld { typed, .. } => Some(typed),
            _ => None,
        }).flatten().collect();
        let want: Vec<String> = match (blocked, who) {
            (false, _) => vec![],
            (true, "A") => vec!["alpha one".into(), "alpha two\r\nsecond line".into()],
            (true, _) => vec!["bravo one".into(), "bravo two".into()],
        };
        assert_eq!(withheld, want, "{tag}: {who} was given back {withheld:?}");
    }
    let heard_now = air.heard().join("|");
    for (secret, blocked) in [("alpha", a_blocked), ("At the hall", a_blocked), ("bravo", b_blocked), ("Copy, will", b_blocked)] {
        assert!(!(blocked && heard_now.contains(secret)), "{tag}: {secret:?} went out unencrypted: {heard_now}");
    }

    // the link is what both settings say, on both sides
    assert_eq!((a.secure.is_encrypted(), b.secure.is_encrypted()), (enc, enc), "{tag}");
    let (na, nb) = (notes(&ea), notes(&eb));
    for (who, n, mine) in [("A", &na, ma), ("B", &nb, mb)] {
        let links: Vec<bool> = n.iter().filter_map(|n| match n {
            SecureNote::Link { encrypted, .. } => Some(*encrypted),
            _ => None,
        }).collect();
        // (a station with nothing on, or only signing, says nothing about the link unless
        // the other side encrypts)
        if enc {
            assert_eq!(links, [true], "{tag}: {who}'s notes {n:?}");
        } else if mine.encrypts() || mine.shared().is_some() {
            assert_eq!(links, [false], "{tag}: {who}'s notes {n:?}");
        } else {
            assert!(links.iter().all(|e| !e), "{tag}: {who}'s notes {n:?}");
        }
        let false_alarms: Vec<_> = n.iter().filter(|n| matches!(n, SecureNote::Unencrypted(_) | SecureNote::Warning(_))).collect();
        assert!(false_alarms.is_empty(), "{tag}: {who} warned: {false_alarms:?}");
    }

    // signatures: on a clear link, what a signing station sent checks out at the other end
    for (who, n, sender) in [("B", &nb, ma), ("A", &na, mb)] {
        let checks: Vec<SigCheck> = n.iter().filter_map(|n| match n {
            SecureNote::Signed { check, .. } => Some(*check),
            _ => None,
        }).collect();
        if sender.signs() && !enc && !wants(sender) {
            // (three signed messages: two typed, one VMail)
            assert_eq!(checks.len(), 3, "{tag}: {who} checked {checks:?}");
            assert!(checks.iter().all(|c| matches!(c, SigCheck::Valid(KeyTrust::FirstContact | KeyTrust::Known))), "{tag}: {who} checked {checks:?}");
        } else {
            assert!(checks.is_empty(), "{tag}: {who} checked {checks:?}");
        }
    }

    // the air
    let heard = air.heard();
    let all = heard.join("|");
    if enc {
        for secret in ["alpha", "bravo", "second line", "Meeting", "hall", "Copy, will"] {
            assert!(!all.contains(secret), "{tag}: {secret:?} readable on the air: {heard:?}");
        }
        assert!(!all.contains(secure::SIG_TAG), "{tag}: {heard:?}");
    }
    if ma == Mode::Off && mb == Mode::Off {
        assert!(!all.contains("<OS"), "{tag}: Openslot tags on the air with everything off: {heard:?}");
        assert!(na.is_empty() && nb.is_empty(), "{tag}: {na:?} {nb:?}");
    }
    if ma.shared().is_some() || mb.shared().is_some() {
        assert!(!all.contains(secure::KEY_TAG) || !(ma.shared().is_some() && mb.shared().is_some()), "{tag}: {heard:?}");
    }
    // both count the frames the same way (VarAC's line numbers)
    assert_eq!(a.chat_id, b.chat_id, "{tag}: frame numbers differ");
}

const ALL: [Mode; 9] = [Mode::Off, Mode::Sign, Mode::Enc, Mode::EncSign, Mode::EncAes, Mode::Shared(0), Mode::Shared(1), Mode::SharedAes(0), Mode::Off];

async fn row(ma: Mode, delay_ab: u64, delay_ba: u64) {
    for mb in &ALL[..8] {
        qso(ma, *mb, delay_ab, delay_ba).await;
    }
}

#[tokio::test]
async fn matrix_off() {
    row(Mode::Off, 0, 0).await;
}

#[tokio::test]
async fn matrix_sign() {
    row(Mode::Sign, 0, 0).await;
}

#[tokio::test]
async fn matrix_enc() {
    row(Mode::Enc, 0, 0).await;
}

#[tokio::test]
async fn matrix_enc_sign() {
    row(Mode::EncSign, 0, 0).await;
}

#[tokio::test]
async fn matrix_enc_aes() {
    row(Mode::EncAes, 0, 0).await;
}

#[tokio::test]
async fn matrix_shared() {
    row(Mode::Shared(0), 0, 0).await;
}

#[tokio::test]
async fn matrix_shared_other_key() {
    row(Mode::Shared(1), 0, 0).await;
}

#[tokio::test]
async fn matrix_shared_aes() {
    row(Mode::SharedAes(0), 0, 0).await;
}

/// The caller's frames take a while (its key reaches the callee after the callee's opening
/// has gone out), and the other way round.
#[tokio::test]
async fn slow_air_caller_to_callee() {
    for (ma, mb) in [(Mode::Enc, Mode::Enc), (Mode::EncSign, Mode::Enc), (Mode::Shared(0), Mode::Shared(0)), (Mode::Off, Mode::Enc), (Mode::Enc, Mode::Off), (Mode::Sign, Mode::Off)] {
        qso(ma, mb, 2500, 0).await;
    }
}

#[tokio::test]
async fn slow_air_callee_to_caller() {
    for (ma, mb) in [(Mode::Enc, Mode::Enc), (Mode::Enc, Mode::EncAes), (Mode::Shared(0), Mode::Shared(0)), (Mode::Off, Mode::Enc), (Mode::Enc, Mode::Off), (Mode::Off, Mode::Sign)] {
        qso(ma, mb, 0, 2500).await;
    }
}

#[tokio::test]
async fn slow_air_both_ways() {
    for (ma, mb) in [(Mode::Enc, Mode::Enc), (Mode::SharedAes(0), Mode::Shared(0)), (Mode::Off, Mode::Off), (Mode::Sign, Mode::Sign)] {
        qso(ma, mb, 1500, 1500).await;
    }
}

/// Connected on a calling frequency: the callee offers a QSY first, the caller picks a
/// slot, both move, then the openings. Encrypted the whole way when both have it on.
#[tokio::test]
async fn calling_frequency_handshake() {
    for (ma, mb) in [(Mode::Enc, Mode::Enc), (Mode::Off, Mode::Off), (Mode::Enc, Mode::Off), (Mode::Shared(0), Mode::Shared(0))] {
        let tag = format!("{ma:?} -> {mb:?} on a calling frequency");
        let (mut a, mut b, air) = two_stations(|p| ma.apply(p), |p| mb.apply(p), Duration::ZERO, Duration::ZERO).await;
        for s in [&a, &b] {
            s.config.policy.on_calling_frequency.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        a.vara.send_command(&VaraCommand::Connect { source: "N0AAA".into(), destination: "N0BBB".into() }).await.unwrap();
        let (mut ea, mut eb) = until_handshakes(&mut a, &mut b, 30_000).await;
        assert!(ea.iter().any(|e| matches!(e, SessionEvent::HandshakeComplete)), "{tag}: A {ea:?}");
        assert!(eb.iter().any(|e| matches!(e, SessionEvent::HandshakeComplete)), "{tag}: B {eb:?}");
        let enc = expect_encrypted(ma, mb);
        let blocked = wants(ma) && !enc;
        assert_eq!(a.send_typed("after the move").await.is_err(), blocked, "{tag}");
        let (x, y) = run2(&mut a, &mut b, 3000).await;
        ea.extend(x);
        eb.extend(y);
        assert_eq!(texts(&eb), if blocked { vec![] } else { vec!["after the move".to_string()] }, "{tag}: {eb:?}");
        assert_eq!((a.secure.is_encrypted(), b.secure.is_encrypted()), (enc, enc), "{tag}");
        for (who, e) in [("A", &ea), ("B", &eb)] {
            let alarms: Vec<_> = notes(e).into_iter().filter(|n| matches!(n, SecureNote::Unencrypted(_) | SecureNote::Warning(_))).collect();
            assert!(alarms.is_empty(), "{tag}: {who} warned {alarms:?}");
        }
        if enc {
            assert!(!air.heard().join("|").contains("after the move"), "{tag}");
        }
        assert_eq!(a.chat_id, b.chat_id, "{tag}: frame numbers differ");
    }
}

/// A second link between the same two: the keys are known now, new link keys, counters
/// from the start, and the old link leaves nothing behind.
#[tokio::test]
async fn second_link_knows_the_key() {
    for (ma, mb) in [(Mode::Enc, Mode::Enc), (Mode::Sign, Mode::Sign), (Mode::EncAes, Mode::Enc)] {
        let tag = format!("{ma:?} -> {mb:?}");
        let (mut a, mut b, air) = two_stations(|p| ma.apply(p), |p| mb.apply(p), Duration::ZERO, Duration::ZERO).await;
        let mut first_sealed = Vec::new();
        for round in 0..2 {
            a.vara.send_command(&VaraCommand::Connect { source: "N0AAA".into(), destination: "N0BBB".into() }).await.unwrap();
            let (_, eb) = until_handshakes(&mut a, &mut b, 15_000).await;
            a.send_typed(&format!("round {round}")).await.unwrap();
            let (_, eb2) = run2(&mut a, &mut b, 2500).await;
            let eb: Vec<_> = eb.into_iter().chain(eb2).collect();
            assert_eq!(texts(&eb), [format!("round {round}")], "{tag} round {round}: {eb:?}");
            let n = notes(&eb);
            let trust = if round == 0 { "first contact" } else { "known" };
            if ma.encrypts() && mb.encrypts() {
                assert!(matches!(&n[..], [SecureNote::Link { encrypted: true, text }] if text.contains(trust)), "{tag} round {round}: {n:?}");
            }
            if ma.signs() {
                let want = if round == 0 { KeyTrust::FirstContact } else { KeyTrust::Known };
                assert!(n.iter().any(|n| matches!(n, SecureNote::Signed { check: SigCheck::Valid(t), .. } if *t == want)), "{tag} round {round}: {n:?}");
            }
            let sealed: Vec<String> = air.heard().into_iter().filter(|f| f.contains(secure::ENC_TAG)).collect();
            if round == 0 {
                first_sealed = sealed;
            } else {
                // (a fresh key: no frame of the second link repeats one of the first)
                for f in sealed.iter().skip(first_sealed.len()) {
                    assert!(!first_sealed.contains(f), "{tag}: repeated ciphertext {f}");
                }
            }
            a.send_disconnect().await.unwrap();
            run2(&mut a, &mut b, 500).await;
            assert_eq!((a.state, b.state), (SessionState::Idle, SessionState::Idle), "{tag}");
            assert_eq!(a.secure.state(), secure::LinkState::Off);
        }
        // B saved A's key once
        if ma.encrypts() || ma.signs() {
            assert_eq!(b.config.policy.secure.stations().len(), 1, "{tag}");
        }
    }
}

/// A file both ways round: offered, taken, sent, confirmed, with the bytes intact; on an
/// encrypted link nothing of it is readable on the air.
#[tokio::test]
async fn file_transfer() {
    // (text that doesn't compress to nothing)
    let mut data = Vec::new();
    for i in 0..3000u32 {
        data.extend_from_slice(format!("line {i}: {}\n", i.wrapping_mul(2_654_435_761)).as_bytes());
    }
    for (ma, mb) in [(Mode::Off, Mode::Off), (Mode::Enc, Mode::Enc), (Mode::EncAes, Mode::EncAes), (Mode::Sign, Mode::Off), (Mode::Shared(0), Mode::Shared(0))] {
        let tag = format!("{ma:?} -> {mb:?}");
        let (mut a, mut b, air) = two_stations(|p| ma.apply(p), |p| mb.apply(p), Duration::ZERO, Duration::ZERO).await;
        a.vara.send_command(&VaraCommand::Connect { source: "N0AAA".into(), destination: "N0BBB".into() }).await.unwrap();
        until_handshakes(&mut a, &mut b, 15_000).await;
        let prepared = crate::varac::bbs::prepare_file("notes.txt", &data).unwrap();
        let payload = prepared.base64_payload.clone();
        a.send_file(prepared).await.unwrap();
        let (ea, eb) = run2(&mut a, &mut b, 6000).await;
        let got: Vec<&Vec<u8>> = eb.iter().filter_map(|e| match e {
            SessionEvent::Message(IncomingMessage::BbsFilePayload(d)) => Some(d),
            _ => None,
        }).collect();
        assert_eq!(got.len(), 1, "{tag}: {eb:?}");
        assert_eq!(got[0].as_slice(), payload.as_bytes(), "{tag}: the file changed on the way");
        assert!(ea.iter().any(|e| matches!(e, SessionEvent::Message(IncomingMessage::BbsFileOk))), "{tag}: no <SFOK>: {ea:?}");
        assert!(!a.file_in_progress, "{tag}");
        if expect_encrypted(ma, mb) {
            let all = air.heard().join("|");
            assert!(!all.contains(&payload[..64]) && !all.contains("notes.txt"), "{tag}: the file is readable on the air");
        }
        let alarms: Vec<_> = notes(&ea).into_iter().chain(notes(&eb)).filter(|n| matches!(n, SecureNote::Unencrypted(_) | SecureNote::Warning(_))).collect();
        assert!(alarms.is_empty(), "{tag}: {alarms:?}");
    }
}

/// A VMail with an attachment, encrypted and not.
#[tokio::test]
async fn vmail_with_attachment() {
    for (ma, mb) in [(Mode::Off, Mode::Off), (Mode::Enc, Mode::Enc), (Mode::Sign, Mode::Sign)] {
        let tag = format!("{ma:?} -> {mb:?}");
        let (mut a, mut b, air) = two_stations(|p| ma.apply(p), |p| mb.apply(p), Duration::ZERO, Duration::ZERO).await;
        a.vara.send_command(&VaraCommand::Connect { source: "N0AAA".into(), destination: "N0BBB".into() }).await.unwrap();
        let (_, mut eb) = until_handshakes(&mut a, &mut b, 15_000).await;
        let mut v = vmail("Photos", "See attached");
        v.has_attachment = true;
        a.send_vmail_with_attachments(&v, &[("map.txt".into(), b"grid EM12kp, antenna on the north side".to_vec())]).await.unwrap();
        let (_, y) = run2(&mut a, &mut b, 6000).await;
        eb.extend(y);
        let got = vmails(&eb);
        assert_eq!(got.len(), 1, "{tag}: {eb:?}");
        assert_eq!((got[0].subject.as_str(), got[0].body.as_str()), ("Photos", "See attached"), "{tag}");
        if expect_encrypted(ma, mb) {
            assert!(!air.heard().join("|").contains("Photos"), "{tag}");
        }
        let alarms: Vec<_> = notes(&eb).into_iter().filter(|n| matches!(n, SecureNote::Unencrypted(_) | SecureNote::Warning(_))).collect();
        assert!(alarms.is_empty(), "{tag}: {alarms:?}");
    }
}

/// Someone else on the air: a replayed key announcement, an old encrypted frame again, a
/// forged one and a frame in the clear, all on an encrypted link. The link stays encrypted
/// (and says so), each is refused or flagged, and the QSO carries on.
#[tokio::test]
async fn interference_on_an_encrypted_link() {
    let (mut a, mut b, air) = two_stations(|p| Mode::Enc.apply(p), |p| Mode::Enc.apply(p), Duration::ZERO, Duration::ZERO).await;
    a.vara.send_command(&VaraCommand::Connect { source: "N0AAA".into(), destination: "N0BBB".into() }).await.unwrap();
    until_handshakes(&mut a, &mut b, 15_000).await;
    b.send_typed("first").await.unwrap();
    run2(&mut a, &mut b, 2500).await;
    let heard = air.heard();
    // (what B sent: "B>" then the frame's length and text)
    let from_b: Vec<String> = heard.iter().filter_map(|f| f.strip_prefix("B>")).map(|f| f.split_once(' ').map(|(_, t)| t.to_string()).unwrap_or_default()).collect();
    let b_key = from_b.iter().find(|t| t.starts_with(secure::KEY_TAG)).unwrap().clone();
    let last_sealed = from_b.iter().filter(|t| t.starts_with(secure::ENC_TAG)).last().unwrap().clone();

    for bad in [b_key.as_str(), last_sealed.as_str(), "<OSE:AAAAAQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA>", "pay no attention"] {
        air.inject_to(true, bad).await;
    }
    let (ea, _) = run2(&mut a, &mut b, 1000).await;
    let n = notes(&ea);
    assert!(a.secure.is_encrypted(), "{n:?}");
    assert!(!n.iter().any(|n| matches!(n, SecureNote::Link { encrypted: false, .. })), "A was told the link isn't encrypted: {n:?}");
    assert!(n.iter().any(|n| matches!(n, SecureNote::Unencrypted(_))), "the clear frame is flagged: {n:?}");
    assert!(n.iter().filter(|n| matches!(n, SecureNote::Warning(_))).count() >= 2, "the replayed and forged frames are refused: {n:?}");
    assert!(!texts(&ea).contains(&"first".to_string()), "the replayed frame isn't shown again: {ea:?}");

    b.send_typed("still here").await.unwrap();
    let (ea, _) = run2(&mut a, &mut b, 2500).await;
    assert_eq!(texts(&ea), ["still here"], "{ea:?}");
}

// === Round two: odd inputs, changes mid-link, saved keys, the rarer parts of a QSO ===

/// A connected QSO between the two (caller A), handshakes done.
async fn connect(a: &mut VaracSession, b: &mut VaracSession, to: &str) -> (Vec<SessionEvent>, Vec<SessionEvent>) {
    let me = a.config.callsign.clone();
    a.vara.send_command(&VaraCommand::Connect { source: me, destination: to.into() }).await.unwrap();
    until_handshakes(a, b, 15_000).await
}

fn alarms(events: &[SessionEvent]) -> Vec<SecureNote> {
    notes(events).into_iter().filter(|n| matches!(n, SecureNote::Unencrypted(_) | SecureNote::Warning(_))).collect()
}

/// A small, repeatable random source (no extra crate).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.next() as u8).collect()
    }
}

/// Junk on the air in every Openslot tag, on an encrypted link and on a clear one: nothing
/// panics, the link stays what it was, and the QSO goes on.
#[tokio::test]
async fn junk_frames_on_the_air() {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    for (ma, mb) in [(Mode::Enc, Mode::Enc), (Mode::Off, Mode::Off), (Mode::Sign, Mode::Sign), (Mode::Shared(0), Mode::Shared(0))] {
        let tag = format!("{ma:?} -> {mb:?}");
        let (mut a, mut b, air) = two_stations(|p| ma.apply(p), |p| mb.apply(p), Duration::ZERO, Duration::ZERO).await;
        connect(&mut a, &mut b, "N0BBB").await;
        let enc = expect_encrypted(ma, mb);
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        // real frames from B, cut short or with a character changed
        let from_b: Vec<String> = air.heard().iter().filter_map(|f| f.strip_prefix("B>"))
            .filter_map(|f| f.split_once(' ').map(|(_, t)| t.to_string())).collect();
        let mut junk: Vec<String> = Vec::new();
        for f in &from_b {
            for cut in [1, f.len() / 2, f.len().saturating_sub(2)] {
                junk.push(f.chars().take(cut).collect());
            }
            let mut flipped: Vec<char> = f.chars().collect();
            if flipped.len() > 8 {
                let i = 6 + (rng.next() as usize) % (flipped.len() - 7);
                flipped[i] = if flipped[i] == 'A' { 'B' } else { 'A' };
                junk.push(flipped.into_iter().collect());
            }
        }
        for t in [secure::KEY_TAG, secure::SHARED_TAG, secure::ENC_TAG, secure::SIG_TAG] {
            for n in [0, 1, 4, 5, 20, 21, 33, 49, 65, 81, 97, 98, 130, 131, 500] {
                junk.push(format!("{t}{}>", b64.encode(rng.bytes(n))));
            }
            junk.push(format!("{t}not base64!>"));
            junk.push(t.to_string());
            junk.push(format!("{t}>>>>"));
            junk.push(format!("hello{t}{}>", b64.encode(rng.bytes(64))));
        }
        for j in &junk {
            air.inject_to(true, j).await;
        }
        let (ea, _) = run2(&mut a, &mut b, 1500).await;
        assert_eq!(a.secure.is_encrypted(), enc, "{tag}: {:?}", notes(&ea));
        assert!(!notes(&ea).iter().any(|n| matches!(n, SecureNote::Link { .. })), "{tag}: the link changed: {:?}", notes(&ea));
        b.send_typed("after the junk").await.unwrap();
        a.send_typed("me too").await.unwrap();
        let (ea, eb) = run2(&mut a, &mut b, 3000).await;
        assert!(texts(&ea).iter().any(|t| t == "after the junk"), "{tag}: {ea:?}");
        assert!(texts(&eb).iter().any(|t| t == "me too"), "{tag}: {eb:?}");
    }
}

/// Encryption or signing switched on or off in the middle of a QSO: the link goes on as it
/// started (encrypted stays encrypted), and the next link follows the new setting.
#[tokio::test]
async fn settings_changed_mid_link() {
    // encryption turned off on both during an encrypted QSO
    let (mut a, mut b, air) = two_stations(|p| Mode::Enc.apply(p), |p| Mode::Enc.apply(p), Duration::ZERO, Duration::ZERO).await;
    connect(&mut a, &mut b, "N0BBB").await;
    a.config.policy.secure.set(false, false);
    b.config.policy.secure.set(false, false);
    a.send_typed("still private").await.unwrap();
    let (ea, eb) = run2(&mut a, &mut b, 2500).await;
    assert_eq!(texts(&eb), ["still private"], "{eb:?}");
    assert!(!air.heard().join("|").contains("still private"), "sent in the clear after the switch");
    assert!(alarms(&ea).is_empty() && alarms(&eb).is_empty());
    a.send_disconnect().await.unwrap();
    run2(&mut a, &mut b, 500).await;
    let (ea, eb) = connect(&mut a, &mut b, "N0BBB").await;
    assert!(!a.secure.is_encrypted() && notes(&ea).is_empty() && notes(&eb).is_empty(), "{ea:?}");

    // signing turned on during a clear QSO: signed from then on; the key isn't known yet
    let (mut a, mut b, _air) = two_stations(|_| {}, |_| {}, Duration::ZERO, Duration::ZERO).await;
    connect(&mut a, &mut b, "N0BBB").await;
    a.config.policy.secure.set(false, true);
    a.send_typed("signed now").await.unwrap();
    let (_, eb) = run2(&mut a, &mut b, 2500).await;
    assert_eq!(texts(&eb), ["signed now"], "{eb:?}");
    let checks: Vec<SigCheck> = notes(&eb).iter().filter_map(|n| match n {
        SecureNote::Signed { check, .. } => Some(*check),
        _ => None,
    }).collect();
    assert_eq!(checks, [SigCheck::NoKey], "{eb:?}");

    // encryption turned on during a clear QSO: the link stays clear, so nothing typed goes
    // out from then on; the next link is encrypted
    let (mut a, mut b, air) = two_stations(|_| {}, |_| {}, Duration::ZERO, Duration::ZERO).await;
    connect(&mut a, &mut b, "N0BBB").await;
    a.config.policy.secure.set(true, false);
    b.config.policy.secure.set(true, false);
    assert!(a.send_typed("not on a clear link").await.is_err());
    let (ea, eb) = run2(&mut a, &mut b, 2500).await;
    assert!(texts(&eb).is_empty(), "{eb:?}");
    assert!(notes(&ea).iter().any(|n| matches!(n, SecureNote::Withheld { .. })), "{ea:?}");
    a.send_disconnect().await.unwrap();
    run2(&mut a, &mut b, 500).await;
    connect(&mut a, &mut b, "N0BBB").await;
    a.send_typed("now private").await.unwrap();
    let (_, eb) = run2(&mut a, &mut b, 2500).await;
    assert_eq!(texts(&eb), ["now private"], "{eb:?}");
    assert!(a.secure.is_encrypted() && !air.heard().join("|").contains("now private"));
}

/// Typed while waiting for the partner's key, then the link drops before it comes: none of
/// it goes out, then or on the next link.
#[tokio::test]
async fn held_text_dies_with_the_link() {
    // (B's frames take 8 s to reach A: A never hears B's key on the first link)
    let (mut a, mut b, air) = two_stations(|p| Mode::Enc.apply(p), |p| Mode::Enc.apply(p), Duration::ZERO, Duration::from_secs(8)).await;
    a.vara.send_command(&VaraCommand::Connect { source: "N0AAA".into(), destination: "N0BBB".into() }).await.unwrap();
    while a.state != SessionState::Connected {
        run2(&mut a, &mut b, 20).await;
    }
    a.send_typed("held secret").await.unwrap();
    a.send_vmail(&vmail("Held", "held body")).await.unwrap();
    run2(&mut a, &mut b, 1500).await;
    assert_eq!(a.secure.state(), secure::LinkState::Waiting);
    assert_eq!(a.queued_texts().len(), 2, "{:?}", a.queued_texts());
    a.send_disconnect().await.unwrap();
    run2(&mut a, &mut b, 500).await;
    assert!(a.queued_texts().is_empty(), "{:?}", a.queued_texts());
    // (the late frames of the old link still land at A: nothing must come of them)
    run2(&mut a, &mut b, 9000).await;
    let heard = air.heard().join("|");
    assert!(!heard.contains("held") && !heard.contains("Held"), "{heard}");
}

/// The saved key file across restarts: first contact, known after a restart, a NEW key
/// flagged when the partner replaces its key (and still encrypted, with the warning), known
/// again once accepted.
#[tokio::test]
async fn saved_keys_across_restarts() {
    let dir = std::env::temp_dir().join(format!("openslot-two-station-{}", std::process::id()));
    let (fa, fb) = (dir.join("a").join("keys.json"), dir.join("b").join("keys.json"));
    for mode in [Mode::Enc, Mode::Sign] {
        let _ = std::fs::remove_dir_all(&dir);
        let mut seen = Vec::new();
        for step in ["first", "restart", "replaced", "still flagged", "accepted"] {
            let (pa, pb) = (fa.clone(), fb.clone());
            let (mut a, mut b, _air) = two_stations(move |p| {
                p.open_keyring(pa);
                mode.apply(p);
            }, move |p| {
                p.open_keyring(pb);
                mode.apply(p);
            }, Duration::ZERO, Duration::ZERO).await;
            if step == "replaced" {
                a.config.policy.secure.replace_identity().unwrap();
            }
            if step == "accepted" {
                b.config.policy.secure.accept_new_key("N0AAA");
            }
            let (_, mut eb) = connect(&mut a, &mut b, "N0BBB").await;
            a.send_typed(step).await.unwrap();
            let (_, y) = run2(&mut a, &mut b, 2500).await;
            eb.extend(y);
            assert_eq!(texts(&eb), [step], "{mode:?} {step}: {eb:?}");
            let n = notes(&eb);
            let what = match (mode, &n[..]) {
                (Mode::Enc, [SecureNote::Link { encrypted: true, text }]) if text.contains("first contact") => "first",
                (Mode::Enc, [SecureNote::Link { encrypted: true, text }]) if text.contains("known since") => "known",
                (Mode::Enc, [SecureNote::Link { encrypted: true, text }]) if text.contains("NEW KEY") => "new",
                (Mode::Sign, [SecureNote::Signed { check: SigCheck::Valid(KeyTrust::FirstContact), .. }]) => "first",
                (Mode::Sign, [SecureNote::Signed { check: SigCheck::Valid(KeyTrust::Known), .. }]) => "known",
                (Mode::Sign, [SecureNote::Warning(_), SecureNote::Signed { check: SigCheck::Valid(KeyTrust::Changed), .. }]) => "new",
                _ => panic!("{mode:?} {step}: {n:?}"),
            };
            seen.push(what);
            assert_eq!(b.config.policy.secure.stations().len(), 1, "{mode:?} {step}");
        }
        assert_eq!(seen, ["first", "known", "new", "new", "known"], "{mode:?}");
        let saved = std::fs::read_to_string(&fb).unwrap();
        assert!(saved.contains("N0AAA"), "{saved}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A ping (the -T alias) with everything on: no key announcement, nothing encrypted.
#[tokio::test]
async fn ping_with_encryption_on() {
    let (mut a, mut b, air) = two_stations(|p| Mode::EncSign.apply(p), |p| Mode::EncSign.apply(p), Duration::ZERO, Duration::ZERO).await;
    a.vara.send_command(&VaraCommand::Connect { source: "N0AAA".into(), destination: "N0BBB-T".into() }).await.unwrap();
    let (ea, eb) = run2(&mut a, &mut b, 3000).await;
    assert!(a.ping && b.ping, "{ea:?} {eb:?}");
    assert!(!air.heard().join("|").contains("<OS"), "{:?}", air.heard());
    assert!(notes(&ea).is_empty() && notes(&eb).is_empty(), "{ea:?} {eb:?}");
    assert!(b.config.policy.secure.stations().is_empty());
}

/// The rest of a QSO: QSY up mid-QSO, is-typing, a like, an info request, a long VMail and
/// text in other scripts. Sealed on an encrypted link, and understood either way.
#[tokio::test]
async fn the_rest_of_a_qso() {
    for (ma, mb) in [(Mode::Enc, Mode::Enc), (Mode::Off, Mode::Off), (Mode::SharedAes(0), Mode::Shared(0))] {
        let tag = format!("{ma:?} -> {mb:?}");
        let enc = expect_encrypted(ma, mb);
        let (mut a, mut b, air) = two_stations(|p| ma.apply(p), |p| mb.apply(p), Duration::ZERO, Duration::ZERO).await;
        let (mut ea, mut eb) = connect(&mut a, &mut b, "N0BBB").await;
        a.send_qsy_step(true, 500).await.unwrap();
        let (x, y) = run2(&mut a, &mut b, 3000).await;
        // (the app accepts a QSY step, as it does on ACCEPT)
        for e in &y {
            if let SessionEvent::QsyInvitation { target: QsyTarget::Up, .. } = e {
                b.answer_qsy(true, false, 500).await.unwrap();
            }
        }
        ea.extend(x);
        eb.extend(y);
        let (x, y) = run2(&mut a, &mut b, 4000).await;
        ea.extend(x);
        eb.extend(y);
        assert!(eb.iter().any(|e| matches!(e, SessionEvent::QsyInvitation { target: QsyTarget::Up, .. })), "{tag}: QSY up not understood: {eb:?}");
        b.send_is_typing().await.unwrap();
        b.send_typed("Ünïcode ✓ 73 — привет 日本").await.unwrap();
        let long: String = (0..400).map(|i| format!("word{i} ")).collect();
        a.send_vmail(&vmail("Long", long.trim_end())).await.unwrap();
        a.send_info_request().await.unwrap();
        let (x, y) = run2(&mut a, &mut b, 5000).await;
        ea.extend(x);
        eb.extend(y);
        assert!(texts(&ea).iter().any(|t| t == "Ünïcode ✓ 73 — привет 日本"), "{tag}: {:?}", texts(&ea));
        assert_eq!(vmails(&eb).iter().map(|v| v.body.clone()).collect::<Vec<_>>(), [long.trim_end().to_string()], "{tag}");
        let liked = a.chat_id;
        b.send_reaction(liked, true).await.unwrap();
        let (x, y) = run2(&mut a, &mut b, 2500).await;
        ea.extend(x);
        eb.extend(y);
        assert!(alarms(&ea).is_empty() && alarms(&eb).is_empty(), "{tag}: {:?} {:?}", alarms(&ea), alarms(&eb));
        assert_eq!(a.chat_id, b.chat_id, "{tag}");
        if enc {
            let heard = air.heard().join("|");
            for t in ["<QSYU>", "&IT&", "&L1", "<INFO", "word1", "Long"] {
                assert!(!heard.contains(t), "{tag}: {t} readable on the air");
            }
        }
    }
}

/// Away and a welcome message on a calling frequency, both encrypted.
#[tokio::test]
async fn away_and_welcome_encrypted() {
    for away in [false, true] {
        let (mut a, mut b, air) = two_stations(|p| Mode::Enc.apply(p), |p| Mode::Enc.apply(p), Duration::ZERO, Duration::ZERO).await;
        for s in [&a, &b] {
            s.config.policy.on_calling_frequency.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        b.config.policy.set_welcome("Welcome to my station <NAME>", true);
        if away {
            b.set_away(Some("gone fishing".into()));
        }
        let (mut ea, eb) = connect(&mut a, &mut b, "N0BBB").await;
        let (x, _) = run2(&mut a, &mut b, 3000).await;
        ea.extend(x);
        assert!(a.secure.is_encrypted() && b.secure.is_encrypted(), "away {away}");
        assert!(alarms(&ea).is_empty() && alarms(&eb).is_empty(), "away {away}: {:?} {:?}", alarms(&ea), alarms(&eb));
        assert!(format!("{ea:?}").contains("Welcome to my station"), "away {away}: no welcome: {ea:?}");
        assert!(!air.heard().join("|").contains("Welcome"), "away {away}: welcome readable on the air");
    }
}

/// What someone types that looks like an Openslot tag still arrives as typed, on a clear
/// link and on an encrypted one.
#[tokio::test]
async fn typed_text_that_looks_like_a_tag() {
    // (text in angle brackets in general is VarAC's tags, read as tags: not tried here)
    let typed = ["<OSK:hello>", "<OSE:abc>", "<OSP:xyz>", "<OSG:", "<OSK:", "<OSE:"];
    for (ma, mb) in [(Mode::Off, Mode::Off), (Mode::Enc, Mode::Enc), (Mode::Sign, Mode::Off)] {
        let tag = format!("{ma:?} -> {mb:?}");
        let (mut a, mut b, _air) = two_stations(|p| ma.apply(p), |p| mb.apply(p), Duration::ZERO, Duration::ZERO).await;
        connect(&mut a, &mut b, "N0BBB").await;
        let mut lost = Vec::new();
        for t in typed {
            a.send_typed(t).await.unwrap();
            let (_, eb) = run2(&mut a, &mut b, 2200).await;
            let got = texts(&eb).join("\n");
            if got != t {
                lost.push(format!("{t:?} -> {got:?} ({:?})", alarms(&eb)));
            }
        }
        assert!(lost.is_empty(), "{tag}: {lost:#?}");
    }
}

/// Encryption on, the partner can't: no file, VMail with attachments or away message of
/// ours goes out either, and a file job doesn't stay stuck. With encryption off, they do.
#[tokio::test]
async fn files_vmail_and_away_withheld_on_a_clear_link() {
    for mb in [Mode::Off, Mode::Sign, Mode::Shared(1)] {
        let tag = format!("Enc -> {mb:?}");
        let (mut a, mut b, air) = two_stations(|p| Mode::Enc.apply(p), |p| mb.apply(p), Duration::ZERO, Duration::ZERO).await;
        connect(&mut a, &mut b, "N0BBB").await;
        assert!(!a.secure.is_encrypted(), "{tag}");
        let prepared = crate::varac::bbs::prepare_file("plan.txt", b"the plan, in full").unwrap();
        assert!(a.send_file(prepared.clone()).await.is_err(), "{tag}");
        assert!(!a.file_in_progress, "{tag}: the file job is stuck");
        let mut v = vmail("Photos", "See attached");
        v.has_attachment = true;
        assert!(a.send_vmail_with_attachments(&v, &[("map.txt".into(), b"grid EM12kp".to_vec())]).await.is_err(), "{tag}");
        assert!(a.send_away(Some("back at 1800z")).await.is_err(), "{tag}");
        let (ea, eb) = run2(&mut a, &mut b, 3000).await;
        let withheld: Vec<String> = notes(&ea).iter().filter_map(|n| match n {
            SecureNote::Withheld { text, .. } => Some(text.clone()),
            _ => None,
        }).collect();
        assert_eq!(withheld.len(), 3, "{tag}: {withheld:?}");
        assert!(withheld[0].contains("a file") && withheld[1].contains("Outbox") && withheld[2].contains("away"), "{tag}: {withheld:?}");
        let heard = air.heard().join("|");
        for secret in ["plan.txt", "Photos", "1800z", "<SF", "<AWAY>"] {
            assert!(!heard.contains(secret), "{tag}: {secret} went out: {heard}");
        }
        assert!(!eb.iter().any(|e| matches!(e, SessionEvent::Message(IncomingMessage::BbsFileOffer(_)))), "{tag}");

        // the operator turns encryption off: now it goes, unencrypted, as asked
        a.config.policy.secure.set(false, false);
        a.send_file(prepared).await.unwrap();
        let (_, eb) = run2(&mut a, &mut b, 4000).await;
        assert!(eb.iter().any(|e| matches!(e, SessionEvent::Message(IncomingMessage::BbsFilePayload(_)))), "{tag}: {eb:?}");
    }
}

// === Round four: frames in pieces, SSIDs ===

/// Frames handed over in small pieces (VARA's data port doesn't keep frame boundaries):
/// every kind of link still reads them whole, both ways, slow air or not.
#[tokio::test]
async fn frames_in_pieces() {
    let opts = Opts { chunked: true, ..PLAIN };
    for (ma, mb, d_ab, d_ba) in [
        (Mode::Off, Mode::Off, 0, 0), (Mode::Sign, Mode::Sign, 0, 0), (Mode::Enc, Mode::Enc, 0, 0),
        (Mode::EncSign, Mode::EncAes, 0, 0), (Mode::Shared(0), Mode::SharedAes(0), 0, 0),
        (Mode::Enc, Mode::Off, 0, 0), (Mode::Off, Mode::Sign, 0, 0), (Mode::Enc, Mode::Enc, 2000, 0),
    ] {
        qso_with(opts, ma, mb, d_ab, d_ba).await;
    }
}

/// A file and a VMail with attachments, encrypted, in pieces: arrives whole, with the
/// progress shown on the way.
#[tokio::test]
async fn encrypted_file_in_pieces() {
    let opts = Opts { chunked: true, ..PLAIN };
    let data: Vec<u8> = (0..1500u32).flat_map(|i| format!("row {i} {}\n", i.wrapping_mul(2_654_435_761)).into_bytes()).collect();
    for (ma, mb) in [(Mode::Enc, Mode::Enc), (Mode::Off, Mode::Off)] {
        let tag = format!("{ma:?} -> {mb:?}");
        let (mut a, mut b, _air) = two_stations_with(opts, |p| ma.apply(p), |p| mb.apply(p), Duration::ZERO, Duration::ZERO).await;
        connect(&mut a, &mut b, "N0BBB").await;
        let prepared = crate::varac::bbs::prepare_file("rows.txt", &data).unwrap();
        let payload = prepared.base64_payload.clone();
        a.send_file(prepared).await.unwrap();
        let (_, eb) = run2(&mut a, &mut b, 12_000).await;
        let got: Vec<&Vec<u8>> = eb.iter().filter_map(|e| match e {
            SessionEvent::Message(IncomingMessage::BbsFilePayload(d)) => Some(d),
            _ => None,
        }).collect();
        assert_eq!(got.len(), 1, "{tag}: {:?}", eb.iter().filter(|e| !matches!(e, SessionEvent::RxProgress { .. })).collect::<Vec<_>>());
        assert_eq!(got[0].as_slice(), payload.as_bytes(), "{tag}");
        let progress = eb.iter().filter(|e| matches!(e, SessionEvent::RxProgress { .. })).count();
        assert!(progress > 3, "{tag}: progress shown {progress} times");
        assert!(alarms(&eb).is_empty(), "{tag}: {:?}", alarms(&eb));
        let mut v = vmail("Photos", "See attached");
        v.has_attachment = true;
        a.send_vmail_with_attachments(&v, &[("map.txt".into(), b"grid EM12kp".to_vec())]).await.unwrap();
        let (_, eb) = run2(&mut a, &mut b, 8000).await;
        assert_eq!(vmails(&eb).iter().map(|v| v.body.as_str()).collect::<Vec<_>>(), ["See attached"], "{tag}");
    }
}

/// Stations with SSIDs: keys, signatures and shared keys go by the base callsign, so it
/// all works whatever SSID either side uses, and a key saved from one SSID is known from
/// another.
#[tokio::test]
async fn stations_with_ssids() {
    let opts = Opts { calls: ("N0AAA-1", "N0BBB-7"), ..PLAIN };
    for (ma, mb) in [(Mode::Enc, Mode::Enc), (Mode::Sign, Mode::Sign), (Mode::EncSign, Mode::Off), (Mode::Off, Mode::Off)] {
        qso_with(opts, ma, mb, 0, 0).await;
    }
    // a shared key typed in for the base callsign, a station that calls with an SSID
    let (mut a, mut b, air) = two_stations_with(opts, |p| {
        p.set_shared(true);
        p.add_shared(secure::SharedKey::new("net", "N0BBB", KEYS[0]).unwrap());
    }, |p| {
        p.set_shared(true);
        p.add_shared(secure::SharedKey::new("net", "n0aaa-3", KEYS[0]).unwrap());
    }, Duration::ZERO, Duration::ZERO).await;
    connect(&mut a, &mut b, "N0BBB-7").await;
    assert!(a.secure.is_encrypted() && b.secure.is_encrypted());
    a.send_typed("by base call").await.unwrap();
    let (_, eb) = run2(&mut a, &mut b, 2500).await;
    assert_eq!(texts(&eb), ["by base call"]);
    assert!(!air.heard().join("|").contains("by base call"));

    // the key saved from N0AAA-1 is the one N0AAA-5 is checked against
    let dir = std::env::temp_dir().join(format!("openslot-ssid-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (fa, fb) = (dir.join("a.json"), dir.join("b.json"));
    for (round, calls) in [("N0AAA-1", "N0BBB-7"), ("N0AAA-5", "N0BBB")].into_iter().enumerate() {
        let (pa, pb) = (fa.clone(), fb.clone());
        let (mut a, mut b, _air) = two_stations_with(Opts { calls, ..PLAIN }, move |p| {
            p.open_keyring(pa);
            p.set(true, false);
        }, move |p| {
            p.open_keyring(pb);
            p.set(true, false);
        }, Duration::ZERO, Duration::ZERO).await;
        let (_, eb) = connect(&mut a, &mut b, calls.1).await;
        let want = if round == 0 { "first contact" } else { "known since" };
        assert!(notes(&eb).iter().any(|n| matches!(n, SecureNote::Link { encrypted: true, text } if text.contains(want))), "round {round}: {:?}", notes(&eb));
        assert_eq!(b.config.policy.secure.stations().iter().map(|s| s.call.clone()).collect::<Vec<_>>(), ["N0AAA"]);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

// === Round five: encryption with what came to main alongside it (the phones' file send
// and Cancel, photos, VARA FM) ===

/// Run both until `done` holds for B's events (or `ms` passed); all events kept.
async fn until_b(a: &mut VaracSession, b: &mut VaracSession, ms: u64, done: impl Fn(&[SessionEvent]) -> bool) -> (Vec<SessionEvent>, Vec<SessionEvent>) {
    let (mut ea, mut eb) = (Vec::new(), Vec::new());
    let start = tokio::time::Instant::now();
    while !done(&eb) && start.elapsed() < Duration::from_millis(ms) {
        let (x, y) = run2(a, b, 250).await;
        ea.extend(x);
        eb.extend(y);
    }
    (ea, eb)
}

fn payloads(events: &[SessionEvent]) -> Vec<Vec<u8>> {
    events.iter().filter_map(|e| match e {
        SessionEvent::Message(IncomingMessage::BbsFilePayload(d)) => Some(d.clone()),
        _ => None,
    }).collect()
}

/// The file as it was before it went: the payload is base64 of the gzip.
fn unpacked(payload: &[u8]) -> Vec<u8> {
    use base64::Engine;
    let gz = base64::engine::general_purpose::STANDARD.decode(payload).expect("base64");
    crate::varac::bbs::decompress(&gz).expect("gzip")
}

/// VARA FM gives its mode on CONNECTED (NARROW / WIDE) where VARA HF gives the bandwidth:
/// every kind of link comes up and carries the QSO the same on FM.
#[tokio::test]
async fn vara_fm_links() {
    for bw in ["NARROW", "WIDE"] {
        let opts = Opts { bw, ..PLAIN };
        for (ma, mb) in [
            (Mode::Enc, Mode::Enc), (Mode::EncAes, Mode::EncSign), (Mode::Shared(0), Mode::SharedAes(0)),
            (Mode::Sign, Mode::Off), (Mode::Enc, Mode::Off), (Mode::Off, Mode::Off),
        ] {
            qso_with(opts, ma, mb, 0, 0).await;
        }
    }
}

/// Our file offer withdrawn before the partner said yes (the phones' Cancel): the partner
/// is told, its late yes gets no data, on an encrypted link nothing of the file or the
/// withdrawal is readable on the air, and the next file goes through whole.
#[tokio::test]
async fn withdrawn_file_offer() {
    use std::sync::atomic::Ordering::Relaxed;
    let first: Vec<u8> = (0..400u32).flat_map(|i| format!("step {i} of the plan\n").into_bytes()).collect();
    let second = b"the second file, sent after the first was withdrawn".to_vec();
    for (ma, mb) in [(Mode::Enc, Mode::Enc), (Mode::EncAes, Mode::EncAes), (Mode::Shared(0), Mode::Shared(0)), (Mode::Sign, Mode::Sign), (Mode::Off, Mode::Off)] {
        let tag = format!("{ma:?} -> {mb:?}");
        let (mut a, mut b, air) = two_stations(|p| ma.apply(p), |p| mb.apply(p), Duration::ZERO, Duration::ZERO).await;
        // (B's operator is asked about files: none is taken on its own)
        b.config.policy.auto_file.store(false, Relaxed);
        b.config.policy.file_size_limit.store(0, Relaxed);
        connect(&mut a, &mut b, "N0BBB").await;
        assert_eq!(a.secure.is_encrypted(), expect_encrypted(ma, mb), "{tag}");

        a.send_file(crate::varac::bbs::prepare_file("secret-plan.txt", &first).unwrap()).await.unwrap();
        let offered = |e: &[SessionEvent]| e.iter().any(|e| matches!(e, SessionEvent::Message(IncomingMessage::BbsFileOffer(_))));
        let (_, eb) = until_b(&mut a, &mut b, 5000, offered).await;
        assert!(offered(&eb), "{tag}: no offer: {eb:?}");
        assert!(a.sending_file() && !a.file_data_going(), "{tag}");

        assert!(a.cancel_outgoing_file().await.unwrap(), "{tag}");
        assert!(!a.sending_file(), "{tag}");
        let aborted = |e: &[SessionEvent]| e.iter().any(|e| matches!(e, SessionEvent::Message(IncomingMessage::BbsFileAborted)));
        let (mut all_a, mut all_b) = until_b(&mut a, &mut b, 5000, aborted).await;
        assert!(aborted(&all_b), "{tag}: B wasn't told: {all_b:?}");

        // B's operator says yes, too late: nothing comes
        b.send_file_ready().await.unwrap();
        let (ea2, eb2) = run2(&mut a, &mut b, 2500).await;
        assert!(payloads(&eb2).is_empty(), "{tag}: the withdrawn file came anyway");
        assert!(!a.sending_file(), "{tag}");

        if expect_encrypted(ma, mb) {
            let heard = air.heard().join("|");
            for secret in ["secret-plan", "<SF", "step 1 of"] {
                assert!(!heard.contains(secret), "{tag}: {secret:?} readable on the air: {heard}");
            }
        }
        all_a.extend(ea2);
        all_b.extend(eb2);
        assert!(alarms(&all_a).is_empty() && alarms(&all_b).is_empty(), "{tag}: {:?} {:?}", alarms(&all_a), alarms(&all_b));

        // the next file goes, whole
        b.config.policy.auto_file.store(true, Relaxed);
        a.send_file(crate::varac::bbs::prepare_file("next.txt", &second).unwrap()).await.unwrap();
        let (ea, eb) = until_b(&mut a, &mut b, 8000, |e| !payloads(e).is_empty()).await;
        let got = payloads(&eb);
        assert_eq!(got.len(), 1, "{tag}: {eb:?}");
        assert_eq!(unpacked(&got[0]), second, "{tag}");
        run2(&mut a, &mut b, 1000).await;
        assert!(!a.file_in_progress, "{tag}: A still thinks it's sending: {ea:?}");
        if expect_encrypted(ma, mb) {
            assert!(!air.heard().join("|").contains("next.txt"), "{tag}");
        }
    }
}

/// A photo as the phones send one: binary, hardly compressible, tens of kB, handed over in
/// pieces. Arrives byte for byte with either cipher and with a shared key, and nothing of
/// it (its name, its bytes) is readable on the air.
#[tokio::test]
async fn encrypted_photo() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut photo = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, b'J', b'F', b'I', b'F', 0x00];
    photo.extend(rng.bytes(40_000));
    photo.extend([0xFF, 0xD9]);
    let opts = Opts { chunked: true, ..PLAIN };
    for (ma, mb) in [(Mode::Enc, Mode::Enc), (Mode::EncAes, Mode::Enc), (Mode::SharedAes(0), Mode::SharedAes(0))] {
        let tag = format!("{ma:?} -> {mb:?}");
        let (mut a, mut b, air) = two_stations_with(opts, |p| ma.apply(p), |p| mb.apply(p), Duration::ZERO, Duration::ZERO).await;
        connect(&mut a, &mut b, "N0BBB").await;
        assert!(a.secure.is_encrypted(), "{tag}");
        let prepared = crate::varac::bbs::prepare_file("IMG_0001.jpg", &photo).unwrap();
        let payload = prepared.base64_payload.clone();
        a.send_file(prepared).await.unwrap();
        let (ea, eb) = until_b(&mut a, &mut b, 30_000, |e| !payloads(e).is_empty()).await;
        let got = payloads(&eb);
        assert_eq!(got.len(), 1, "{tag}: no photo");
        assert_eq!(unpacked(&got[0]), photo, "{tag}: the photo changed on the way");
        let heard = air.heard().join("");
        assert!(!heard.contains("IMG_0001"), "{tag}");
        for at in [0, payload.len() / 2, payload.len() - 40] {
            assert!(!heard.contains(&payload[at..at + 32]), "{tag}: part of the photo is readable on the air");
        }
        assert!(alarms(&ea).is_empty() && alarms(&eb).is_empty(), "{tag}: {:?} {:?}", alarms(&ea), alarms(&eb));
    }
}

/// Files crossing each other on an encrypted link, with chat typed while they go: each
/// direction's counter keeps its own order, so everything arrives once and whole.
#[tokio::test]
async fn files_both_ways_with_chat_encrypted() {
    let fa: Vec<u8> = (0..800u32).flat_map(|i| format!("A{i} {}\n", i.wrapping_mul(2_654_435_761)).into_bytes()).collect();
    let fb: Vec<u8> = (0..800u32).flat_map(|i| format!("B{i} {}\n", i.wrapping_mul(40_503)).into_bytes()).collect();
    for (ma, mb) in [(Mode::Enc, Mode::Enc), (Mode::EncAes, Mode::EncSign), (Mode::Shared(0), Mode::Shared(0))] {
        let tag = format!("{ma:?} -> {mb:?}");
        let opts = Opts { chunked: true, ..PLAIN };
        let (mut a, mut b, air) = two_stations_with(opts, |p| ma.apply(p), |p| mb.apply(p), Duration::from_millis(150), Duration::from_millis(250)).await;
        connect(&mut a, &mut b, "N0BBB").await;
        a.send_file(crate::varac::bbs::prepare_file("from-a.txt", &fa).unwrap()).await.unwrap();
        b.send_file(crate::varac::bbs::prepare_file("from-b.txt", &fb).unwrap()).await.unwrap();
        a.send_typed("chat from A while files go").await.unwrap();
        b.send_typed("chat from B while files go").await.unwrap();
        let (mut ea, mut eb) = (Vec::new(), Vec::new());
        let start = tokio::time::Instant::now();
        while (payloads(&ea).is_empty() || payloads(&eb).is_empty()) && start.elapsed() < Duration::from_secs(30) {
            let (x, y) = run2(&mut a, &mut b, 250).await;
            ea.extend(x);
            eb.extend(y);
        }
        let (x, y) = run2(&mut a, &mut b, 1500).await;
        ea.extend(x);
        eb.extend(y);
        let (pa, pb) = (payloads(&ea), payloads(&eb));
        assert_eq!((pa.len(), pb.len()), (1, 1), "{tag}");
        assert_eq!(unpacked(&pb[0]), fa, "{tag}");
        assert_eq!(unpacked(&pa[0]), fb, "{tag}");
        assert_eq!(texts(&eb).iter().filter(|t| *t == "chat from A while files go").count(), 1, "{tag}: {:?}", texts(&eb));
        assert_eq!(texts(&ea).iter().filter(|t| *t == "chat from B while files go").count(), 1, "{tag}: {:?}", texts(&ea));
        let heard = air.heard().join("");
        for secret in ["from-a.txt", "from-b.txt", "while files go"] {
            assert!(!heard.contains(secret), "{tag}: {secret:?} readable on the air");
        }
        assert!(alarms(&ea).is_empty() && alarms(&eb).is_empty(), "{tag}: {:?} {:?}", alarms(&ea), alarms(&eb));
    }
}
