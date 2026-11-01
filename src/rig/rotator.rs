//! Antenna rotator through PstRotator, as VarAC turns one ("Rotate Antenna" in the beacon and
//! CQ menus): a UDP datagram naming the azimuth, sent to PstRotator's UDP port (12000 by
//! default). PstRotator then drives whatever rotator it is set up for.

use std::io;
use std::net::UdpSocket;

/// PstRotator's UDP command to turn to `azimuth` degrees.
pub fn message(azimuth: i64) -> String {
    format!("<PST><AZIMUTH>{}</AZIMUTH></PST>", azimuth.rem_euclid(360))
}

/// Asks PstRotator at `host:port` to turn the antenna to `azimuth` degrees.
pub fn turn_to(host: &str, port: u16, azimuth: i64) -> io::Result<()> {
    let socket = UdpSocket::bind(("0.0.0.0", 0))?;
    socket.send_to(message(azimuth).as_bytes(), (host, port))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_names_the_azimuth() {
        assert_eq!(message(45), "<PST><AZIMUTH>45</AZIMUTH></PST>");
        assert_eq!(message(360), "<PST><AZIMUTH>0</AZIMUTH></PST>");
        assert_eq!(message(-10), "<PST><AZIMUTH>350</AZIMUTH></PST>");
    }

    #[test]
    fn turn_to_sends_one_datagram() {
        let rx = UdpSocket::bind("127.0.0.1:0").unwrap();
        rx.set_read_timeout(Some(std::time::Duration::from_secs(2))).unwrap();
        let port = rx.local_addr().unwrap().port();
        turn_to("127.0.0.1", port, 123).unwrap();
        let mut buf = [0u8; 128];
        let n = rx.recv(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"<PST><AZIMUTH>123</AZIMUTH></PST>");
    }
}
