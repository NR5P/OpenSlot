pub mod commands;
pub mod connection;
pub mod events;
pub mod kiss;
pub mod launch;

/// Connect to `host:port`, trying each address it resolves to in turn. On Windows
/// "localhost" resolves to ::1 first, while rigctld, flrig and VARA often listen on
/// 127.0.0.1 only, so the first address alone is not enough.
pub fn connect_any(host: &str, port: u16, timeout: std::time::Duration) -> std::io::Result<std::net::TcpStream> {
    use std::net::ToSocketAddrs;
    let mut last = std::io::Error::new(std::io::ErrorKind::NotFound, format!("could not resolve {host}"));
    for addr in (host, port).to_socket_addrs()? {
        match std::net::TcpStream::connect_timeout(&addr, timeout) {
            Ok(s) => return Ok(s),
            Err(e) => last = e,
        }
    }
    Err(last)
}
