//! One Openslot phone station for on-air tests: the mobile engine on a VARA/XARA modem
//! (see README "Interop test lab"), driven by a timed script, printing every event.
//!
//!   cargo run --release --example lab_station -- CONFIG_JSON SCRIPT_JSON SECS
//!
//! CONFIG_JSON is the engine config (data_dir, callsign, cmd_port, ...) plus optional
//! "qsy_slot" (answer for our QSY invitation, default 12), "accept_qsy" (default true),
//! "accept_files" (answer to a file offer: true (default), false, or "ignore" to leave it
//! to the script) and
//! "seed_heard": [[call, minutes_ago, snr], ...] (stations put in the heard list first).
//! SCRIPT_JSON is [[seconds, command], ...]; seconds count from the modem link-up, or with
//! "script_from_link": true from the first QSO link (a VARA connect can take minutes);
//! there a negative time counts from start-up (for the connect itself).
use std::time::{Duration, Instant};

use serde_json::{Value, json};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let cfg: Value = serde_json::from_str(&a[0]).expect("config JSON");
    let script: Vec<(f64, Value)> = serde_json::from_str::<Vec<(f64, Value)>>(&a[1]).expect("script JSON");
    let secs: f64 = a[2].parse().expect("seconds");
    let qsy_slot = cfg["qsy_slot"].as_u64().unwrap_or(12);
    let accept_qsy = cfg["accept_qsy"].as_bool().unwrap_or(true);
    // (None: leave the offer to the script)
    let accept_files = if cfg["accept_files"] == "ignore" { None } else { Some(cfg["accept_files"].as_bool().unwrap_or(true)) };
    let from_link = cfg["script_from_link"].as_bool().unwrap_or(false);
    let mut start: Option<Instant> = None;

    // The heard list our last-heard answers come from
    let dir = std::path::PathBuf::from(cfg["data_dir"].as_str().unwrap());
    std::fs::create_dir_all(&dir).unwrap();
    if let Some(seed) = cfg["seed_heard"].as_array() {
        let db = openslot_mobile::storage::Storage::open(&dir.join("varac.db")).unwrap();
        for s in seed {
            let ago = chrono::Duration::minutes(s[1].as_i64().unwrap_or(0));
            db.insert_cqframe(&openslot_mobile::storage::CqFrameRecord {
                guid: uuid::Uuid::new_v4().to_string(),
                cqframe_time: chrono::Utc::now() - ago,
                cqframe_type_id: 1,
                frequency: cfg["frequency"].as_i64().unwrap_or(14_105_000),
                bandwidth: "500".into(),
                from_callsign: s[0].as_str().unwrap().into(),
                snr: s[2].as_i64().map(|v| v as i32),
                slot: None,
                locator: None,
                band: Some(openslot_mobile::engine::band_of(cfg["frequency"].as_u64().unwrap_or(14_105_000)).into()),
            })
            .unwrap();
            db.upsert_heard_station(s[0].as_str().unwrap()).unwrap();
        }
    }

    openslot_mobile::start(&cfg.to_string()).expect("engine");
    let t0 = Instant::now();
    let say = |what: &str, v: &str| println!("{:7.1} {what} {v}", t0.elapsed().as_secs_f64());
    let cmd = |c: &Value| {
        say(">>", &c.to_string());
        if let Err(e) = openslot_mobile::command(&c.to_string()) {
            say("!!", &e);
        }
    };
    cmd(&json!({"cmd": "connect_modem"}));
    if !from_link {
        start = Some(t0);
    }
    let mut queue = script.into_iter().peekable();
    while t0.elapsed().as_secs_f64() < secs {
        while let Some(e) = openslot_mobile::next_event() {
            let v: Value = serde_json::from_str(&e).unwrap_or(Value::Null);
            match v["type"].as_str() {
                Some("busy" | "buffer" | "ptt" | "snr") => continue,
                Some("log") if !v["text"].as_str().unwrap_or("").contains("Call ID") => continue,
                _ => {}
            }
            say("<<", &e);
            if start.is_none() && v["type"] == "connected" {
                start = Some(Instant::now());
            }
            // The prompts a phone's operator would answer
            match v["type"].as_str() {
                Some("qsy_invite_request") => cmd(&json!({"cmd": "qsy_invite", "slot": qsy_slot})),
                Some("qsy_request") => cmd(&json!({"cmd": if accept_qsy { "qsy_accept" } else { "qsy_reject" }})),
                Some("file_offer") => {
                    if let Some(yes) = accept_files {
                        cmd(&json!({"cmd": if yes { "file_accept" } else { "file_decline" }}));
                    }
                }
                _ => {}
            }
        }
        // (a negative time counts from start-up even with script_from_link: the connect)
        while queue.peek().is_some_and(|(t, _)| {
            if *t < 0.0 { t0.elapsed().as_secs_f64() >= -*t } else { start.is_some_and(|s0| s0.elapsed().as_secs_f64() >= *t) }
        }) {
            let (_, c) = queue.next().unwrap();
            if c["q"].is_string() {
                say("??", &format!("{} -> {}", c, openslot_mobile::query(&c.to_string())));
            } else {
                cmd(&c);
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    say("--", "end");
    openslot_mobile::stop();
}
