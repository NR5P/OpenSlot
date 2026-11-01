/// Message framing for the VarAC application protocol.
///
/// Each message is framed as: `<length> <message_text>`, sent as UTF-8.
///
/// The length counts UTF-16 code units (characters), *not* UTF-8 bytes (verified on
/// air against VarAC 16.1.8). For ASCII the two agree; a byte count breaks the link as
/// soon as a frame carries `«`/`»` (VarAC's escaped `<`/`>`) or any other non-ASCII text:
/// VarAC waits for the "missing" characters and glues the next frame on.
///
/// If the first byte of a stream is not an ASCII digit, fall back to plain-text
/// passthrough (no framing). VMail frames may declare a length that omits the
/// closing `>` — the session layer handles this by consuming one trailing byte.
use bytes::BytesMut;

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("incomplete frame: need more data")]
    Incomplete,
    #[error("invalid length prefix")]
    InvalidLength,
    #[error("frame too large: {0} bytes")]
    TooLarge(usize),
}

const MAX_FRAME_SIZE: usize = 1_000_000; // 1MB safety limit

/// Encode a message into the VarAC wire format.
pub fn encode_frame(message: &str) -> Vec<u8> {
    let payload = message.as_bytes();
    let header = format!("{} ", message.encode_utf16().count());
    let mut frame = Vec::with_capacity(header.len() + payload.len());
    frame.extend_from_slice(header.as_bytes());
    frame.extend_from_slice(payload);
    frame
}

/// Try to decode one frame from a buffer.
/// Returns the decoded message and the number of bytes consumed, or a FrameError.
pub fn decode_frame(buf: &BytesMut) -> Result<(String, usize), FrameError> {
    let data = &buf[..];

    // Find the space that separates length from payload
    let space_pos = data
        .iter()
        .position(|&b| b == b' ')
        .ok_or(FrameError::Incomplete)?;

    let len_str =
        std::str::from_utf8(&data[..space_pos]).map_err(|_| FrameError::InvalidLength)?;

    let payload_len: usize = len_str.parse().map_err(|_| FrameError::InvalidLength)?;

    if payload_len > MAX_FRAME_SIZE {
        return Err(FrameError::TooLarge(payload_len));
    }

    // Walk UTF-8 characters until `payload_len` UTF-16 units are covered
    let start = space_pos + 1;
    let (mut p, mut units) = (start, 0usize);
    while units < payload_len {
        let b = *data.get(p).ok_or(FrameError::Incomplete)?;
        let (width, u) = match b {
            0x00..=0x7f => (1, 1),
            0xc0..=0xdf => (2, 1),
            0xe0..=0xef => (3, 1),
            0xf0..=0xf7 => (4, 2), // outside the BMP: a surrogate pair (two UTF-16 units)
            _ => (1, 1),           // stray byte: counts as one (lossy) character
        };
        if p + width > data.len() {
            return Err(FrameError::Incomplete);
        }
        p += width;
        units += u;
    }
    let total_len = p;

    let payload = &data[start..total_len];
    let message = String::from_utf8_lossy(payload).into_owned();

    Ok((message, total_len))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let msg = "Hello de W1ABC";
        let encoded = encode_frame(msg);
        let mut buf = BytesMut::from(&encoded[..]);
        let (decoded, consumed) = decode_frame(&buf).unwrap();
        assert_eq!(decoded, msg);
        assert_eq!(consumed, encoded.len());
        let _ = buf.split_to(consumed);
    }

    #[test]
    fn incomplete() {
        let buf = BytesMut::from("14 Hello");
        assert!(matches!(decode_frame(&buf), Err(FrameError::Incomplete)));
    }

    /// Captured failure: VarAC counted `«with»` as 6 characters, not 8 bytes.
    #[test]
    fn length_counts_characters_like_varac() {
        let msg = "<MSG:a «b» c>";
        let encoded = encode_frame(msg);
        assert!(encoded.starts_with(b"13 "), "{:?}", String::from_utf8_lossy(&encoded));
        let mut two = encoded.clone();
        two.extend_from_slice(b"5 <SMR>");
        let (decoded, consumed) = decode_frame(&BytesMut::from(&two[..])).unwrap();
        assert_eq!(decoded, msg);
        assert_eq!(consumed, encoded.len());
        // partial multi-byte character: wait for more
        assert!(matches!(decode_frame(&BytesMut::from(&encoded[..encoded.len() - 4])), Err(FrameError::Incomplete)));
    }

    #[test]
    fn unicode() {
        let msg = "73 de W1ABC 🎵";
        let encoded = encode_frame(msg);
        let buf = BytesMut::from(&encoded[..]);
        let (decoded, _) = decode_frame(&buf).unwrap();
        assert_eq!(decoded, msg);
    }
}
