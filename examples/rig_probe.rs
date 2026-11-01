//! Quick rig-control check without starting the GUI.
//!
//!   cargo run --example rig_probe -- rigctld [HOST] [PORT]          (RigHelm / Hamlib, default 127.0.0.1 4532)
//!   cargo run --example rig_probe -- cat PORT [BAUD] [ftdx3000|yaesu|kenwood]
//!
//! Reads frequency and mode, tunes 1 kHz up and back. Add `--ptt` to key for 1 second
//! (make sure you're on a clear frequency into a dummy load or antenna).

use openslot::rig::{RigBackend, create_rig};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let ptt = args.iter().any(|a| a == "--ptt");
    let args: Vec<&str> = args.iter().filter(|a| *a != "--ptt").map(String::as_str).collect();
    let arg = |i: usize, d: &'static str| args.get(i).copied().unwrap_or(d);

    let backend = match arg(0, "rigctld") {
        "cat" => RigBackend::from_settings("cat", "", "", arg(1, "/dev/ttyUSB0"), arg(2, "38400"), arg(3, "ftdx3000")),
        _ => RigBackend::from_settings("rigctld", arg(1, "127.0.0.1"), arg(2, "4532"), "", "", ""),
    };
    println!("backend: {backend}");
    let mut rig = create_rig(&backend).expect("backend");

    let step = |what: &str, r: Result<String, openslot::rig::RigError>| match r {
        Ok(v) => println!("  ok   {what}: {v}"),
        Err(e) => {
            println!("  FAIL {what}: {e}");
            std::process::exit(1);
        }
    };

    step("connect", rig.connect().map(|_| rig.name().to_string()));
    let f = rig.get_freq();
    let f0 = *f.as_ref().unwrap_or(&0);
    step("get_freq", f.map(|f| format!("{f} Hz")));
    match rig.get_mode() {
        Ok(m) => println!("  ok   get_mode: {m}"),
        Err(e) => println!("  warn get_mode: {e} (non-fatal; Openslot skips mode readback)"),
    }
    step("set_freq +1kHz", rig.set_freq(f0 + 1000).map(|_| String::new()));
    std::thread::sleep(std::time::Duration::from_millis(300));
    step("readback", rig.get_freq().map(|f| {
        if f == f0 + 1000 { format!("{f} Hz") } else { format!("{f} Hz  (expected {}!)", f0 + 1000) }
    }));
    step("restore", rig.set_freq(f0).map(|_| format!("{f0} Hz")));
    if ptt {
        step("PTT on", rig.set_ptt(true).map(|_| String::new()));
        std::thread::sleep(std::time::Duration::from_secs(1));
        step("PTT off", rig.set_ptt(false).map(|_| String::new()));
    }
    rig.disconnect();
    println!("all good");
}
