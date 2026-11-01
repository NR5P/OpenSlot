//! Start VARA with the modem connection, as VarAC does: set VARA's
//! own ini next to the exe (`[Setup] TCP Command Port`, `Enable KISS=1`, `KISS Port`,
//! `[Monitor] Monitor Mode=0`), start it, then wait the launch delay before connecting.
//! VARA is only started when nothing answers on its command port yet. On Linux and macOS
//! a `.exe` is started through Wine.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

static CHILD: Mutex<Option<std::process::Child>> = Mutex::new(None);

/// Set `key=value` in `[section]` of an ini text (added when missing), keeping the rest.
pub fn ini_set(text: &str, section: &str, key: &str, value: &str) -> String {
    let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut out: Vec<String> = Vec::new();
    let (mut in_section, mut done, mut seen_section) = (false, false, false);
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') && t.ends_with(']') {
            if in_section && !done {
                out.push(format!("{key}={value}"));
                done = true;
            }
            in_section = t[1..t.len() - 1].eq_ignore_ascii_case(section);
            seen_section |= in_section;
        } else if in_section && !done && t.split_once('=').is_some_and(|(k, _)| k.trim().eq_ignore_ascii_case(key)) {
            out.push(format!("{key}={value}"));
            done = true;
            continue;
        }
        out.push(line.to_string());
    }
    if !done {
        if !seen_section {
            out.push(format!("[{section}]"));
        }
        out.push(format!("{key}={value}"));
    }
    out.join(nl) + nl
}

/// VARA's ini for this exe (VARA.ini / VARAFM.ini / VARASAT.ini next to it).
pub fn ini_path(exe: &Path) -> PathBuf {
    let stem = exe.file_stem().map(|s| s.to_string_lossy().to_uppercase()).unwrap_or_else(|| "VARA".into());
    let name = match stem.as_str() {
        "VARAFM" => "VARAFM.ini",
        "VARASAT" => "VARASAT.ini",
        _ => "VARA.ini",
    };
    exe.parent().unwrap_or(Path::new(".")).join(name)
}

/// Is something listening on VARA's command port already?
pub fn modem_answers(host: &str, port: u16) -> bool {
    super::connect_any(host, port, std::time::Duration::from_secs(1)).is_ok()
}

/// Start VARA unless it already runs. Returns a line for the log, or None when nothing
/// was done.
pub fn launch(exe: &str, host: &str, cmd_port: u16, kiss_port: u16) -> Option<Result<String, String>> {
    let exe = exe.trim();
    // the VARA we started is still coming up (Wine can take a while): don't start another
    let ours_running = CHILD.lock().ok().is_some_and(|mut c| c.as_mut().is_some_and(|ch| matches!(ch.try_wait(), Ok(None))));
    if exe.is_empty() || ours_running || modem_answers(host, cmd_port) {
        return None;
    }
    let path = PathBuf::from(exe);
    if !path.exists() {
        return Some(Err(format!("VARA not found at {exe}")));
    }
    let ini = ini_path(&path);
    // VARA.ini is ANSI (cp1252, e.g. sound card names with accents): edit it byte for byte
    // (Latin-1 round trip), and leave it alone if it can't be read
    let bytes = match std::fs::read(&ini) {
        Ok(b) => Some(b),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(Vec::new()),
        Err(_) => None,
    };
    if let Some(bytes) = bytes {
        let text: String = bytes.iter().map(|&b| b as char).collect();
        let mut t = ini_set(&text, "Monitor", "Monitor Mode", "0");
        t = ini_set(&t, "Setup", "Enable KISS", "1");
        t = ini_set(&t, "Setup", "TCP Command Port", &cmd_port.to_string());
        t = ini_set(&t, "Setup", "KISS Port", &kiss_port.to_string());
        let out: Vec<u8> = t.chars().map(|c| c as u32 as u8).collect();
        let _ = std::fs::write(&ini, out);
    }
    let is_exe = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe"));
    let mut cmd = if is_exe && !cfg!(windows) {
        let mut c = std::process::Command::new("wine");
        c.arg(&path);
        c
    } else {
        std::process::Command::new(&path)
    };
    if let Some(dir) = path.parent() {
        cmd.current_dir(dir);
    }
    cmd.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    match cmd.spawn() {
        Ok(child) => {
            if let Ok(mut c) = CHILD.lock() {
                *c = Some(child);
            }
            Some(Ok(format!("VARA started: {exe}")))
        }
        Err(e) => Some(Err(format!("VARA could not be started ({exe}): {e}"))),
    }
}

/// Close the VARA we started (VarAC closes it with the modem connection / at exit).
pub fn close() {
    if let Ok(mut c) = CHILD.lock() {
        if let Some(mut child) = c.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_varas_ini() {
        let text = "[Setup]\r\nRetries=15\r\nTCP Command Port=8300\r\n[Monitor]\r\nMonitor Mode=1\r\n";
        let t = ini_set(text, "Setup", "TCP Command Port", "8400");
        let t = ini_set(&t, "Setup", "KISS Port", "8200");
        let t = ini_set(&t, "Monitor", "Monitor Mode", "0");
        assert_eq!(t, "[Setup]\r\nRetries=15\r\nTCP Command Port=8400\r\nKISS Port=8200\r\n[Monitor]\r\nMonitor Mode=0\r\n");
        assert_eq!(ini_set("", "Setup", "Enable KISS", "1"), "[Setup]\nEnable KISS=1\n");
        assert_eq!(ini_path(Path::new("/x/VARAFM.exe")), PathBuf::from("/x/VARAFM.ini"));
        assert_eq!(ini_path(Path::new("/x/VARA.exe")), PathBuf::from("/x/VARA.ini"));
    }
}
