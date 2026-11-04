//! Just enough HTTP and WebSocket (RFC 6455) for the remote server and client: one port
//! serves the phone page and the WebSocket every client uses, desktop or browser alike.

use std::io;

use base64::Engine;
use sha1::{Digest, Sha1};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Largest message accepted (commands and state are a few kB; nothing legitimate is near).
pub const MAX_MESSAGE: usize = 1 << 20;
/// What a remote accepts from its station: frames of the station's screen, with its
/// pictures, are bigger.
pub const MAX_FROM_STATION: usize = 64 << 20;
const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opcode {
    Continuation,
    Text,
    Binary,
    Close,
    Ping,
    Pong,
}

impl Opcode {
    fn from_u8(b: u8) -> io::Result<Self> {
        Ok(match b {
            0 => Opcode::Continuation,
            1 => Opcode::Text,
            2 => Opcode::Binary,
            8 => Opcode::Close,
            9 => Opcode::Ping,
            10 => Opcode::Pong,
            _ => return Err(bad(format!("unknown WebSocket opcode {b}"))),
        })
    }

    fn to_u8(self) -> u8 {
        match self {
            Opcode::Continuation => 0,
            Opcode::Text => 1,
            Opcode::Binary => 2,
            Opcode::Close => 8,
            Opcode::Ping => 9,
            Opcode::Pong => 10,
        }
    }
}

/// A whole message (fragments joined).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    Text(String),
    Binary(Vec<u8>),
    Ping(Vec<u8>),
    Pong(Vec<u8>),
    Close,
}

fn bad(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

/// An HTTP request's first line and headers (names lower-cased).
#[derive(Debug, Default)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }

    pub fn is_upgrade(&self) -> bool {
        self.header("upgrade").is_some_and(|v| v.eq_ignore_ascii_case("websocket"))
    }
}

/// Largest HTTP head read: a browser's is well under 2 kB.
const MAX_HEAD: usize = 8192;

/// Reads a request head (up to 8 kB), or the status line and headers of a response.
pub async fn read_head<R: AsyncBufRead + Unpin>(r: &mut R) -> io::Result<Request> {
    let mut req = Request::default();
    let mut total = 0;
    let mut first = true;
    loop {
        let mut line = String::new();
        // (each line only as long as what's left of the 8 kB: a line that never ends must
        // not be buffered without limit)
        let left = (MAX_HEAD - total) as u64 + 1;
        let n = (&mut *r).take(left).read_line(&mut line).await?;
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "connection closed in the HTTP head"));
        }
        total += n;
        if total > MAX_HEAD {
            return Err(bad("HTTP head too long"));
        }
        if !line.ends_with('\n') {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "connection closed in the HTTP head"));
        }
        let line = line.trim_end();
        if first {
            first = false;
            let mut parts = line.split_whitespace();
            req.method = parts.next().unwrap_or("").to_string();
            req.path = parts.next().unwrap_or("").to_string();
            continue;
        }
        if line.is_empty() {
            return Ok(req);
        }
        if let Some((n, v)) = line.split_once(':') {
            req.headers.push((n.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
    }
}

pub fn accept_key(key: &str) -> String {
    let mut h = Sha1::new();
    h.update(key.trim().as_bytes());
    h.update(GUID.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(h.finalize())
}

/// Answers a WebSocket upgrade request.
pub async fn accept<W: AsyncWrite + Unpin>(w: &mut W, req: &Request) -> io::Result<()> {
    let key = req.header("sec-websocket-key").ok_or_else(|| bad("no Sec-WebSocket-Key"))?;
    let reply = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
        accept_key(key)
    );
    w.write_all(reply.as_bytes()).await?;
    w.flush().await
}

/// A plain HTTP response (the phone page, a 404), then the connection is closed.
pub async fn respond<W: AsyncWrite + Unpin>(w: &mut W, status: &str, content_type: &str, body: &[u8]) -> io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    w.write_all(head.as_bytes()).await?;
    w.write_all(body).await?;
    w.flush().await
}

/// Opens a WebSocket as a client (`path` on `host`); the reader then carries frames.
pub async fn connect<R, W>(r: &mut R, w: &mut W, host: &str, path: &str) -> io::Result<()>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let nonce: [u8; 16] = *uuid::Uuid::new_v4().as_bytes();
    let key = base64::engine::general_purpose::STANDARD.encode(nonce);
    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
    );
    w.write_all(req.as_bytes()).await?;
    w.flush().await?;
    let head = read_head(r).await?;
    // (read_head puts the status code where a request has its path)
    if head.path != "101" {
        return Err(io::Error::new(io::ErrorKind::ConnectionRefused, format!("not an Openslot remote server (HTTP {})", head.path)));
    }
    if head.header("sec-websocket-accept") != Some(accept_key(&key).as_str()) {
        return Err(bad("bad WebSocket handshake"));
    }
    Ok(())
}

/// Writes one frame. Clients mask what they send (the RFC requires it); servers don't.
pub async fn write_frame<W: AsyncWrite + Unpin>(w: &mut W, op: Opcode, payload: &[u8], mask: bool) -> io::Result<()> {
    let mut head = Vec::with_capacity(14);
    head.push(0x80 | op.to_u8());
    let m = if mask { 0x80 } else { 0 };
    match payload.len() {
        n if n < 126 => head.push(m | n as u8),
        n if n <= u16::MAX as usize => {
            head.push(m | 126);
            head.extend_from_slice(&(n as u16).to_be_bytes());
        }
        n => {
            head.push(m | 127);
            head.extend_from_slice(&(n as u64).to_be_bytes());
        }
    }
    if mask {
        let key: [u8; 4] = uuid::Uuid::new_v4().as_bytes()[..4].try_into().unwrap_or([0x5a; 4]);
        head.extend_from_slice(&key);
        let masked: Vec<u8> = payload.iter().enumerate().map(|(i, b)| b ^ key[i % 4]).collect();
        w.write_all(&head).await?;
        w.write_all(&masked).await?;
    } else {
        w.write_all(&head).await?;
        w.write_all(payload).await?;
    }
    w.flush().await
}

pub async fn write_message<W: AsyncWrite + Unpin>(w: &mut W, msg: &Message, mask: bool) -> io::Result<()> {
    match msg {
        Message::Text(t) => write_frame(w, Opcode::Text, t.as_bytes(), mask).await,
        Message::Binary(b) => write_frame(w, Opcode::Binary, b, mask).await,
        Message::Ping(b) => write_frame(w, Opcode::Ping, b, mask).await,
        Message::Pong(b) => write_frame(w, Opcode::Pong, b, mask).await,
        Message::Close => write_frame(w, Opcode::Close, &[], mask).await,
    }
}

async fn read_frame<R: AsyncRead + Unpin>(r: &mut R, max: usize) -> io::Result<(bool, Opcode, Vec<u8>)> {
    let mut h = [0u8; 2];
    r.read_exact(&mut h).await?;
    let fin = h[0] & 0x80 != 0;
    let op = Opcode::from_u8(h[0] & 0x0f)?;
    let masked = h[1] & 0x80 != 0;
    let len = match h[1] & 0x7f {
        126 => {
            let mut b = [0u8; 2];
            r.read_exact(&mut b).await?;
            u16::from_be_bytes(b) as usize
        }
        127 => {
            let mut b = [0u8; 8];
            r.read_exact(&mut b).await?;
            usize::try_from(u64::from_be_bytes(b)).unwrap_or(usize::MAX)
        }
        n => n as usize,
    };
    if len > max {
        return Err(bad("WebSocket message too big"));
    }
    let mut key = [0u8; 4];
    if masked {
        r.read_exact(&mut key).await?;
    }
    let mut payload = vec![0u8; len];
    r.read_exact(&mut payload).await?;
    if masked {
        for (i, b) in payload.iter_mut().enumerate() {
            *b ^= key[i % 4];
        }
    }
    Ok((fin, op, payload))
}

/// Reads the next whole message, joining fragments.
pub async fn read_message<R: AsyncRead + Unpin>(r: &mut R) -> io::Result<Message> {
    read_message_max(r, MAX_MESSAGE).await
}

/// [`read_message`], up to `max` bytes.
pub async fn read_message_max<R: AsyncRead + Unpin>(r: &mut R, max: usize) -> io::Result<Message> {
    let mut data: Option<(Opcode, Vec<u8>)> = None;
    loop {
        let (fin, op, payload) = read_frame(r, max).await?;
        match op {
            // (a ping or pong may come between a message's fragments, RFC 6455 5.4: ours are
            // JSON pings, so one there is just skipped and the message kept)
            Opcode::Ping | Opcode::Pong if data.is_some() => continue,
            Opcode::Ping => return Ok(Message::Ping(payload)),
            Opcode::Pong => return Ok(Message::Pong(payload)),
            Opcode::Close => return Ok(Message::Close),
            Opcode::Continuation => {
                let (_, buf) = data.as_mut().ok_or_else(|| bad("WebSocket continuation without a start"))?;
                buf.extend_from_slice(&payload);
                if buf.len() > max {
                    return Err(bad("WebSocket message too big"));
                }
            }
            Opcode::Text | Opcode::Binary => data = Some((op, payload)),
        }
        if fin {
            let (op, buf) = data.take().ok_or_else(|| bad("WebSocket frame out of order"))?;
            return Ok(match op {
                Opcode::Text => Message::Text(String::from_utf8(buf).map_err(|_| bad("WebSocket text isn't UTF-8"))?),
                _ => Message::Binary(buf),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 6455's own example.
    #[test]
    fn accept_key_matches_the_rfc() {
        assert_eq!(accept_key("dGhlIHNhbXBsZSBub25jZQ=="), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    }

    #[tokio::test]
    async fn frames_round_trip_masked_and_not() {
        for mask in [false, true] {
            for len in [0usize, 5, 125, 126, 70_000] {
                let payload: Vec<u8> = (0..len).map(|i| i as u8).collect();
                let mut buf = Vec::new();
                write_frame(&mut buf, Opcode::Binary, &payload, mask).await.unwrap();
                let mut r = buf.as_slice();
                assert_eq!(read_message(&mut r).await.unwrap(), Message::Binary(payload));
            }
        }
    }

    #[tokio::test]
    async fn a_ping_between_fragments_keeps_the_message() {
        // "Hel", a ping, then "lo"
        let raw = [0x01u8, 0x03, b'H', b'e', b'l', 0x89, 0x00, 0x80, 0x02, b'l', b'o'];
        let mut r = raw.as_slice();
        assert_eq!(read_message(&mut r).await.unwrap(), Message::Text("Hello".into()));
    }

    #[tokio::test]
    async fn a_header_line_that_never_ends_is_refused_at_8_kb() {
        use tokio::io::AsyncWriteExt;
        let (mut tx, rx) = tokio::io::duplex(1 << 16);
        let writer = tokio::spawn(async move {
            let _ = tx.write_all(b"GET / HTTP/1.1\r\nX: ").await;
            let chunk = vec![b'A'; 1 << 16];
            // (far more than the limit: the reader must stop long before this ends)
            for _ in 0..1024 {
                if tx.write_all(&chunk).await.is_err() {
                    return;
                }
            }
        });
        let mut r = tokio::io::BufReader::new(rx);
        let e = read_head(&mut r).await.unwrap_err();
        assert!(e.to_string().contains("too long"), "{e}");
        drop(r);
        let _ = writer.await;
    }

    #[tokio::test]
    async fn fragments_are_joined() {
        // "Hel" + "lo", unmasked (RFC 6455 5.7)
        let raw = [0x01u8, 0x03, b'H', b'e', b'l', 0x80, 0x02, b'l', b'o'];
        let mut r = raw.as_slice();
        assert_eq!(read_message(&mut r).await.unwrap(), Message::Text("Hello".into()));
    }

    #[tokio::test]
    async fn handshake_both_ways() {
        let (client, server) = tokio::io::duplex(4096);
        let srv = tokio::spawn(async move {
            let (r, mut w) = tokio::io::split(server);
            let mut r = tokio::io::BufReader::new(r);
            let req = read_head(&mut r).await.unwrap();
            assert!(req.is_upgrade());
            assert_eq!(req.path, "/ws");
            accept(&mut w, &req).await.unwrap();
            write_message(&mut w, &Message::Text("hi".into()), false).await.unwrap();
            assert_eq!(read_message(&mut r).await.unwrap(), Message::Text("there".into()));
        });
        let (r, mut w) = tokio::io::split(client);
        let mut r = tokio::io::BufReader::new(r);
        connect(&mut r, &mut w, "localhost", "/ws").await.unwrap();
        assert_eq!(read_message(&mut r).await.unwrap(), Message::Text("hi".into()));
        write_message(&mut w, &Message::Text("there".into()), true).await.unwrap();
        srv.await.unwrap();
    }
}
