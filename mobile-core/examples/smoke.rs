//! Runs the core the way the apps do: start XARA, start the engine, connect, send a CQ.
//!   cargo run --example smoke -- DATA_DIR CAPTURE PLAYBACK
use std::time::Duration;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (dir, cap, play) = (&a[0], &a[1], &a[2]);
    let modem = serde_json::json!(["--call", "N0TST", "--hf", "2300", "--audio", "coreaudio",
        "--capture", cap, "--playback", play, "--state-dir", dir]);
    openslot_mobile::modem_start(&modem.to_string()).expect("modem");
    std::thread::sleep(Duration::from_secs(2));
    let cfg = serde_json::json!({"data_dir": dir, "callsign": "N0TST", "name": "Test"});
    openslot_mobile::start(&cfg.to_string()).expect("engine");
    openslot_mobile::command(r#"{"cmd":"connect_modem"}"#).unwrap();
    std::thread::sleep(Duration::from_secs(2));
    openslot_mobile::command(r#"{"cmd":"cq"}"#).unwrap();
    openslot_mobile::command(r#"{"cmd":"vmail","to":"W1AW","subject":"hi","body":"test"}"#).unwrap();
    std::thread::sleep(Duration::from_secs(3));
    while let Some(e) = openslot_mobile::next_event() {
        if !e.contains("\"log\"") { println!("event {e}"); }
    }
    println!("outbox {}", openslot_mobile::query(r#"{"q":"vmails","folder":"outbox"}"#));
    if std::env::var_os("STOP_MODEM_FIRST").is_some() {
        println!("modem exit {}", openslot_mobile::modem_stop());
        std::thread::sleep(Duration::from_secs(1));
        while let Some(e) = openslot_mobile::next_event() { println!("after stop {e}"); }
    } else {
        openslot_mobile::command(r#"{"cmd":"disconnect_modem"}"#).unwrap();
        println!("modem exit {}", openslot_mobile::modem_stop());
    }
    openslot_mobile::stop();
    while let Some(l) = openslot_mobile::modem_next_log() { if l.contains("CQ") || l.contains("xara:") { println!("xara {l}"); } }
}
