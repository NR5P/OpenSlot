//! DX cluster spots, as VarAC sends them: telnet to the
//! cluster, answer the login prompt ("login:" / "Please enter your call:") with the user
//! name and a password prompt with the password, then
//! `DX <kHz> <CALL> (<software> VARA HF Digi Mode) <comment>`.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

/// VarAC's frequency text: Hz with a dot before the last three digits (14105.000).
pub fn khz(freq_hz: u64) -> String {
    format!("{}.{:03}", freq_hz / 1000, freq_hz % 1000)
}

pub fn spot_line(freq_hz: u64, call: &str, comment: &str) -> String {
    format!("DX {} {} (Openslot VARA HF Digi Mode) {}", khz(freq_hz), call.trim().to_uppercase(), comment.trim())
        .trim_end()
        .to_string()
}

fn read_until(s: &mut TcpStream, buf: &mut String, wanted: &[&str], timeout: Duration) -> Option<usize> {
    let end = Instant::now() + timeout;
    let mut chunk = [0u8; 1024];
    loop {
        if let Some(i) = wanted.iter().position(|w| buf.to_lowercase().contains(&w.to_lowercase())) {
            return Some(i);
        }
        if Instant::now() >= end {
            return None;
        }
        match s.read(&mut chunk) {
            Ok(0) => return None,
            Ok(n) => buf.push_str(&String::from_utf8_lossy(&chunk[..n])),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(_) => return None,
        }
    }
}

/// Log in and send one spot. Blocking (run it on a thread).
pub fn send_spot(host: &str, port: u16, user: &str, password: &str, line: &str) -> Result<String, String> {
    let mut s = crate::vara::connect_any(host, port, Duration::from_secs(10)).map_err(|e| e.to_string())?;
    s.set_read_timeout(Some(Duration::from_millis(500))).ok();
    let mut buf = String::new();
    if read_until(&mut s, &mut buf, &["login:", "please enter your call:"], Duration::from_secs(15)).is_none() {
        return Err("no login prompt from the cluster".into());
    }
    s.write_all(format!("{}\r\n", user.trim()).as_bytes()).map_err(|e| e.to_string())?;
    buf.clear();
    if read_until(&mut s, &mut buf, &["password"], Duration::from_secs(4)).is_some() {
        s.write_all(format!("{password}\r\n").as_bytes()).map_err(|e| e.to_string())?;
        std::thread::sleep(Duration::from_secs(1));
    }
    s.write_all(format!("{line}\r\n").as_bytes()).map_err(|e| e.to_string())?;
    buf.clear();
    let _ = read_until(&mut s, &mut buf, &[line.split_whitespace().nth(2).unwrap_or("DX")], Duration::from_secs(5));
    let _ = s.write_all(b"bye\r\n");
    Ok(buf.lines().last().unwrap_or("").trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spot_text() {
        assert_eq!(khz(14_105_000), "14105.000");
        assert_eq!(khz(7_105_500), "7105.500");
        assert_eq!(spot_line(14_105_000, "nr5p", " Good sig "), "DX 14105.000 NR5P (Openslot VARA HF Digi Mode) Good sig");
    }

    #[test]
    fn talks_to_a_cluster() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let t = std::thread::spawn(move || {
            let (mut c, _) = l.accept().unwrap();
            c.write_all(b"Welcome\r\nlogin: ").unwrap();
            let mut got = String::new();
            let mut b = [0u8; 256];
            c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            while !got.contains("DX ") || !got.contains("\r\n") || got.matches("\r\n").count() < 2 {
                let n = c.read(&mut b).unwrap();
                if n == 0 { break; }
                got.push_str(&String::from_utf8_lossy(&b[..n]));
                if got == "W5TST\r\n" {
                    c.write_all(b"Hello W5TST\r\nW5TST de K1TTT >\r\n").unwrap();
                }
            }
            c.write_all(b"DX de W5TST: 14105.0 NR5P\r\n").unwrap();
            got
        });
        let r = send_spot("127.0.0.1", port, "W5TST", "", &spot_line(14_105_000, "NR5P", "test"));
        let got = t.join().unwrap();
        assert!(r.is_ok(), "{r:?}");
        assert!(got.starts_with("W5TST\r\nDX 14105.000 NR5P (Openslot VARA HF Digi Mode) test\r\n"), "{got:?}");
    }
}
