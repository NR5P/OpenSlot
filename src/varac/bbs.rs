use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use md5::{Digest, Md5};
use std::io::{Read, Write};

use crate::varac::messages::{BBS_MAX_COMPRESSED, BBS_MAX_EXPANDED};

#[derive(Debug, thiserror::Error)]
pub enum BbsError {
    #[error("file too large after compression: {0} bytes (max {BBS_MAX_COMPRESSED})")]
    CompressedTooLarge(usize),
    #[error("decompressed file too large: {0} bytes (max {BBS_MAX_EXPANDED})")]
    ExpandedTooLarge(usize),
    #[error("MD5 mismatch: expected {expected}, got {actual}")]
    Md5Mismatch { expected: String, actual: String },
    #[error("compression error: {0}")]
    Compress(std::io::Error),
    #[error("decompression error: {0}")]
    Decompress(std::io::Error),
    #[error("base64 decode error: {0}")]
    Base64(base64::DecodeError),
}

/// Compress data with gzip for BBS file transfer.
pub fn compress(data: &[u8]) -> Result<Vec<u8>, BbsError> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(data).map_err(BbsError::Compress)?;
    let compressed = encoder.finish().map_err(BbsError::Compress)?;
    if compressed.len() > BBS_MAX_COMPRESSED {
        return Err(BbsError::CompressedTooLarge(compressed.len()));
    }
    Ok(compressed)
}

/// Decompress gzip data from BBS file transfer.
pub fn decompress(data: &[u8]) -> Result<Vec<u8>, BbsError> {
    decompress_max(data, BBS_MAX_EXPANDED)
}

fn decompress_max(data: &[u8], max: usize) -> Result<Vec<u8>, BbsError> {
    // (read at most one byte past the limit: a bomb never gets expanded in full)
    let mut decoder = GzDecoder::new(data).take(max as u64 + 1);
    let mut buf = Vec::new();
    decoder.read_to_end(&mut buf).map_err(BbsError::Decompress)?;
    if buf.len() > max {
        return Err(BbsError::ExpandedTooLarge(buf.len()));
    }
    Ok(buf)
}

/// Windows file types that run when double-clicked: a received file ending in one of these gets
/// `_NNNNNN` appended so it can't be run by a double-click.
pub const DANGEROUS_EXTENSIONS: &[&str] = &[
    ".EXE", ".COM", ".MSI", ".MSC", ".JAR", ".BAT", ".CMD", ".VB", ".VBS", ".VBE", ".JS", ".JSE", ".WS", ".WSF",
    ".WSC", ".WSH", ".PS1", ".PS1XML", ".PS2", ".PS2XML", ".PSC1", ".PSC2", ".MSH", ".MSH1", ".MSH2", ".MSHXML",
    ".MSH1XML", ".MSH2XML", ".SCF", ".LNK", ".INF", ".REG",
];

/// Where a received file goes in `dir`, as VarAC saves it: an existing name becomes
/// `stem_1.ext`, `stem_2.ext`…; a dangerous extension gets `_` + 6 random digits.
pub fn received_path(dir: &std::path::Path, name: &str, random6: u32) -> std::path::PathBuf {
    let mut p = dir.join(name);
    if p.exists() {
        let stem = std::path::Path::new(name).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let ext = std::path::Path::new(name).extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        let mut i = 1;
        while p.exists() {
            p = dir.join(format!("{stem}_{i}{ext}"));
            i += 1;
        }
    }
    let upper = p.to_string_lossy().to_uppercase();
    if DANGEROUS_EXTENSIONS.iter().any(|e| upper.ends_with(e)) {
        p = std::path::PathBuf::from(format!("{}_{:06}", p.display(), random6 % 1_000_000));
    }
    p
}

/// Compute uppercase hex MD5 of data (matching VarAC's format).
pub fn md5_hex(data: &[u8]) -> String {
    let mut hasher = Md5::new();
    hasher.update(data);
    let result = hasher.finalize();
    hex_upper(&result)
}

fn hex_upper(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}

/// Prepare a file for BBS transfer: compress, base64-encode, compute MD5.
/// Returns (base64_payload, compressed_size, md5_hex_of_compressed).
pub fn prepare_file(filename: &str, data: &[u8]) -> Result<PreparedFile, BbsError> {
    let compressed = compress(data)?;
    let md5 = md5_hex(&compressed);
    let b64 = BASE64.encode(&compressed);
    let offer_name = format!("{filename}.VarAC_gzip");
    Ok(PreparedFile {
        offer_name,
        compressed_size: compressed.len(),
        md5_hex: md5,
        base64_payload: b64,
    })
}

#[derive(Debug, Clone)]
pub struct PreparedFile {
    pub offer_name: String,
    pub compressed_size: usize,
    pub md5_hex: String,
    pub base64_payload: String,
}

impl PreparedFile {
    pub fn offer_tag_value(&self) -> String {
        format!("{}|{}|{}", self.offer_name, self.compressed_size, self.md5_hex)
    }
}

/// Receive and verify a BBS file transfer: decode base64, verify MD5, decompress.
pub fn receive_file(
    base64_data: &str,
    expected_md5: &str,
) -> Result<Vec<u8>, BbsError> {
    let compressed = BASE64.decode(base64_data).map_err(BbsError::Base64)?;

    let actual_md5 = md5_hex(&compressed);
    if !actual_md5.eq_ignore_ascii_case(expected_md5) {
        return Err(BbsError::Md5Mismatch {
            expected: expected_md5.to_string(),
            actual: actual_md5,
        });
    }

    decompress(&compressed)
}

/// Strip the `.VarAC_gzip` suffix to recover the original filename.
pub fn strip_gzip_suffix(name: &str) -> &str {
    name.strip_suffix(".VarAC_gzip").unwrap_or(name)
}

/// Resolve a BBS file request to a file inside `bbs_dir`. Only plain file names are
/// allowed: "../", separators and absolute paths would let anyone on the air fetch
/// arbitrary local files.
pub fn resolve_request(bbs_dir: &str, name: &str) -> Option<std::path::PathBuf> {
    let plain = !name.is_empty()
        && !name.contains(['/', '\\', ':'])
        && name != "."
        && name != ".."
        && std::path::Path::new(name).components().count() == 1;
    if !plain {
        return None;
    }
    let dir = std::path::Path::new(bbs_dir).canonicalize().ok()?;
    let path = dir.join(name).canonicalize().ok()?;
    (path.starts_with(&dir) && path.is_file()).then_some(path)
}


/// Files offered by our BBS (name, modification date, size).
pub fn scan_directory(dir: &str) -> Vec<crate::varac::messages::BbsListingEntry> {
    let path = std::path::Path::new(dir);
    if !path.is_dir() {
        return Vec::new();
    }
    let mut entries = Vec::new();
    if let Ok(readdir) = std::fs::read_dir(path) {
        for entry in readdir.flatten() {
            if let Ok(meta) = entry.metadata() {
                // VarAC lists every file, any size, dated by its creation time (local)
                if meta.is_file() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    let date = meta.created().or_else(|_| meta.modified())
                        .map(|t| chrono::DateTime::<chrono::Local>::from(t).format("%Y-%m-%d").to_string())
                        .unwrap_or_default();
                    entries.push(crate::varac::messages::BbsListingEntry {
                        name,
                        date,
                        size: meta.len() as usize,
                    });
                }
            }
        }
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compress_decompress_roundtrip() {
        let data = b"Hello from the BBS! 73 de W1ABC";
        let compressed = compress(data).unwrap();
        let decompressed = decompress(&compressed).unwrap();
        assert_eq!(decompressed, data);
    }

    #[test]
    fn md5_matches_expected() {
        let data = b"test data";
        let hash = md5_hex(data);
        assert_eq!(hash.len(), 32);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(hash, hash.to_uppercase());
    }

    #[test]
    fn prepare_and_receive_roundtrip() {
        let original = b"This is a test file for BBS transfer.\nLine 2.\n";
        let prepared = prepare_file("test.txt", original).unwrap();

        assert_eq!(prepared.offer_name, "test.txt.VarAC_gzip");
        assert!(prepared.compressed_size > 0);
        assert_eq!(prepared.md5_hex.len(), 32);

        let recovered = receive_file(&prepared.base64_payload, &prepared.md5_hex).unwrap();
        assert_eq!(recovered, original);
    }

    #[test]
    fn md5_mismatch_detected() {
        let prepared = prepare_file("x.txt", b"data").unwrap();
        let result = receive_file(&prepared.base64_payload, "DEADBEEF00000000DEADBEEF00000000");
        assert!(matches!(result, Err(BbsError::Md5Mismatch { .. })));
    }

    #[test]
    fn offer_tag_format() {
        let prepared = prepare_file("notes.txt", b"content").unwrap();
        let tag = prepared.offer_tag_value();
        let parts: Vec<&str> = tag.split('|').collect();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0], "notes.txt.VarAC_gzip");
        assert!(parts[1].parse::<usize>().is_ok());
        assert_eq!(parts[2].len(), 32);
    }

    #[test]
    fn received_file_names() {
        let dir = std::env::temp_dir().join(format!("openslot_rx_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(received_path(&dir, "a.txt", 0), dir.join("a.txt"));
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        assert_eq!(received_path(&dir, "a.txt", 0), dir.join("a_1.txt"));
        std::fs::write(dir.join("a_1.txt"), b"x").unwrap();
        assert_eq!(received_path(&dir, "a.txt", 0), dir.join("a_2.txt"));
        assert_eq!(received_path(&dir, "run.exe", 42), dir.join("run.exe_000042"));
        assert_eq!(received_path(&dir, "x.Bat", 7), dir.join("x.Bat_000007"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn strip_suffix() {
        assert_eq!(strip_gzip_suffix("notes.txt.VarAC_gzip"), "notes.txt");
        assert_eq!(strip_gzip_suffix("already_clean.txt"), "already_clean.txt");
    }

    #[test]
    fn expanded_size_limit() {
        let big = vec![0u8; 4097];
        let compressed = {
            let mut enc = GzEncoder::new(Vec::new(), Compression::default());
            enc.write_all(&big).unwrap();
            enc.finish().unwrap()
        };
        assert!(matches!(decompress_max(&compressed, 4096), Err(BbsError::ExpandedTooLarge(_))));
        assert_eq!(decompress_max(&compressed, 4097).unwrap().len(), 4097);
        // VarAC sends files far bigger than our old 1 MiB / 32 KiB caps
        let mut x = 12345u32;
        let two_mb: Vec<u8> = (0..2_000_000).map(|_| { x = x.wrapping_mul(1_103_515_245).wrapping_add(12345); (x >> 16) as u8 }).collect();
        let p = prepare_file("big.bin", &two_mb).unwrap();
        assert!(p.compressed_size > 32_768);
        assert_eq!(receive_file(&p.base64_payload, &p.md5_hex).unwrap(), two_mb);
    }
}
