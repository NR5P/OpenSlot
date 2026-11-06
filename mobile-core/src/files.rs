//! Sending a file from the phones. The file picker's preview (`file_info`) and the
//! `send_file` command both build the file here, so what the operator saw is what goes out.
//!
//! As in the desktop app (and VarAC), a picture can be sent as it is or through the image
//! shrinker: scaled to a percentage and saved as JPEG at VarAC's quality, named
//! `<name>_resized_<yyyyMMdd_HHmmss>.jpg` (UTC). The shrinker starts at the largest size
//! that fits the link (VARA HF: under 100K compressed at 500 Hz, 1M at 2300) and goes out
//! in the operator's send-time choice (about 5 minutes unless set otherwise).

use std::path::Path;

use chrono::Utc;
use serde_json::{Value, json};

use crate::varac::bbs;
use crate::varac::images;
use crate::varac::session::attachment_limit;

/// VarAC's shrinker quality (0–50, used as the JPEG quality).
pub const DEFAULT_QUALITY: u8 = 50;

/// A file ready to offer: its name on the air and its bytes.
pub struct Outgoing {
    pub name: String,
    pub data: Vec<u8>,
}

/// The name a file goes out under. The offer is `<SF:name.VarAC_gzip|size|MD5>`, so a
/// name can't hold `|`, `<` or `>` (nor a path).
pub fn wire_name(path: &Path) -> String {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let name: String = name.chars().map(|c| if matches!(c, '|' | '<' | '>' | '/' | '\\') { '_' } else { c }).collect();
    if name.trim().is_empty() { "file".into() } else { name }
}

/// The shrunk picture's name, as VarAC's shrinker names it.
fn resized_name(path: &Path) -> String {
    let stem = Path::new(&wire_name(path)).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "image".into());
    format!("{stem}_resized_{}.jpg", Utc::now().format("%Y%m%d_%H%M%S"))
}

/// The file as it goes out: as it is, or (a picture, with `shrink` = (percent, quality))
/// through the image shrinker.
pub fn outgoing(path: &Path, shrink: Option<(u32, u8)>) -> Result<Outgoing, String> {
    let data = std::fs::read(path).map_err(|e| format!("can't read {}: {e}", wire_name(path)))?;
    if data.is_empty() {
        return Err(format!("{} is empty", wire_name(path)));
    }
    match shrink {
        Some((pct, quality)) if images::is_image(path) => {
            let (jpg, _, _) = images::shrink(&data, pct, quality, None)?;
            Ok(Outgoing { name: resized_name(path), data: jpg })
        }
        _ => Ok(Outgoing { name: wire_name(path), data }),
    }
}

/// What the preview says about one version of the file.
fn describe(name: &str, data: &[u8], limit: Option<usize>, snr: i32, bandwidth: u32, fm: bool) -> Value {
    let compressed = bbs::compress(data).map(|c| c.len()).unwrap_or(data.len());
    // On the air it's base64 of the gzip
    let air = images::air_bytes(compressed);
    json!({
        "name": name,
        "bytes": data.len(),
        "compressed": compressed,
        "fits": limit.is_none_or(|l| compressed < l),
        "estimate": images::time_estimate(air, snr, bandwidth, fm),
    })
}

/// `file_info {path, bandwidth, fm?, snr?, pct?, quality?, minutes?}`: the file picker's preview.
///   {name, bytes, compressed, fits, estimate, limit, image, w?, h?,
///    shrunk?: {pct, quality, name, bytes, compressed, fits, estimate, w, h}}
/// `limit` is VARA HF's limit on the compressed file at that bandwidth (null: none; VARA FM
/// has none, and `fm` times the send at VARA FM's speeds).
/// For a picture `shrunk` is the shrinker's result: at `pct` if given, else at the largest
/// size that fits and goes in about `minutes` (0: the largest that fits; default 5).
/// `error` when the file can't be read.
pub fn file_info(q: &Value) -> Value {
    let path = Path::new(q["path"].as_str().unwrap_or(""));
    let bandwidth: u32 = match &q["bandwidth"] {
        Value::String(s) => s.trim().parse().unwrap_or(500),
        v => v.as_u64().unwrap_or(500) as u32,
    };
    let snr = q["snr"].as_i64().unwrap_or(0) as i32;
    let fm = q["fm"].as_bool().unwrap_or(false);
    let limit = if fm { None } else { attachment_limit(u16::try_from(bandwidth).ok()) };
    let data = match std::fs::read(path) {
        Ok(d) if !d.is_empty() => d,
        Ok(_) => return json!({"error": format!("{} is empty", wire_name(path))}),
        Err(e) => return json!({"error": format!("can't read {}: {e}", wire_name(path))}),
    };
    let mut info = describe(&wire_name(path), &data, limit, snr, bandwidth, fm);
    info["limit"] = json!(limit);
    info["image"] = json!(false);
    if images::is_image(path) {
        if let Ok(img) = images::load_upright(&data) {
            info["image"] = json!(true);
            info["w"] = json!(img.width());
            info["h"] = json!(img.height());
            let quality = q["quality"].as_u64().map_or(DEFAULT_QUALITY, |v| v.min(50) as u8);
            let shrunk = match q["pct"].as_u64() {
                Some(pct) => {
                    let pct = (pct as u32).clamp(1, 100);
                    images::shrink_image(&img, pct, quality, None).map(|(jpg, w, h)| (pct, jpg, w, h))
                }
                None => {
                    let minutes = q["minutes"].as_u64().map_or(images::PHOTO_MINUTES_DEFAULT, |m| m as u32);
                    images::starting_size(&img, quality, limit, minutes, snr, bandwidth, fm)
                }
            };
            match shrunk {
                Ok((pct, jpg, w, h)) => {
                    let mut s = describe(&resized_name(path), &jpg, limit, snr, bandwidth, fm);
                    s["pct"] = json!(pct);
                    s["quality"] = json!(quality);
                    s["w"] = json!(w);
                    s["h"] = json!(h);
                    info["shrunk"] = s;
                }
                Err(e) => info["shrink_error"] = json!(e),
            }
        }
    }
    info
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str, data: &[u8]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("openslot-files-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        std::fs::write(&p, data).unwrap();
        p
    }

    /// A noisy picture (noise doesn't compress, like a photo).
    fn photo(w: u32, h: u32) -> Vec<u8> {
        let mut seed = 12345u32;
        let img = image::RgbImage::from_fn(w, h, |x, y| {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            let n = (seed >> 16) as u8 / 4;
            image::Rgb([(x % 256) as u8 / 2 + n, (y % 256) as u8 / 2 + n, n * 2])
        });
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 95).encode_image(&img).unwrap();
        out
    }

    #[test]
    fn names_cant_break_the_offer_tag() {
        assert_eq!(wire_name(Path::new("/x/a|b<c>.txt")), "a_b_c_.txt");
        assert_eq!(wire_name(Path::new("/x/notes 1.txt")), "notes 1.txt");
        assert_eq!(wire_name(Path::new("/")), "file");
    }

    #[test]
    fn a_text_file_goes_as_it_is() {
        let p = temp("log.txt", b"hello hello hello hello");
        let o = outgoing(&p, Some((50, 50))).unwrap(); // (not a picture: no shrinking)
        assert_eq!(o.name, "log.txt");
        assert_eq!(o.data, b"hello hello hello hello");
        let i = file_info(&json!({"path": p, "bandwidth": "500", "snr": 5}));
        assert_eq!(i["name"], "log.txt");
        assert_eq!(i["bytes"], 23);
        assert_eq!(i["image"], false);
        assert_eq!(i["limit"], 100_000);
        assert_eq!(i["fits"], true);
        assert!(i["estimate"].as_str().unwrap().contains("min at"), "{i}");
        assert!(i.get("shrunk").is_none());
    }

    #[test]
    fn vara_fm_has_no_size_limit_and_is_faster() {
        let p = temp("IMG_0002.JPG", &photo(1600, 1200));
        let hf = file_info(&json!({"path": p, "bandwidth": 500, "snr": 10}));
        let fm = file_info(&json!({"path": p, "bandwidth": 500, "fm": true, "snr": 10}));
        assert_eq!(fm["limit"], Value::Null, "{fm}");
        assert_eq!(fm["fits"], true, "{fm}");
        // (a picture at its starting size: bigger on FM, where it goes faster and nothing caps it)
        assert!(fm["shrunk"]["pct"].as_u64().unwrap() >= hf["shrunk"]["pct"].as_u64().unwrap(), "{hf}\n{fm}");
        // (the fast end of "a-b min" / "a-b h", in minutes)
        let mins = |v: &Value| -> u64 {
            let e = v["estimate"].as_str().unwrap();
            let a: u64 = e.split(['-', ' ']).next().unwrap().parse().unwrap();
            if e.contains(" h ") { a * 60 } else { a }
        };
        assert!(mins(&fm) < mins(&hf), "{hf}\n{fm}");
        // the shared limit check doesn't stop it either
        assert!(fm["compressed"].as_u64().unwrap() > 100_000, "the test photo should be over HF's 500 Hz limit: {fm}");
    }

    #[test]
    fn empty_and_missing_files_are_errors() {
        let p = temp("empty.bin", b"");
        assert!(outgoing(&p, None).is_err());
        assert!(file_info(&json!({"path": p}))["error"].as_str().unwrap().contains("empty"));
        assert!(file_info(&json!({"path": "/nope/missing.txt"}))["error"].as_str().unwrap().contains("can't read"));
    }

    #[test]
    fn a_big_photo_is_shrunk_to_fit_500_hz() {
        let p = temp("IMG_0001.JPG", &photo(1600, 1200));
        let i = file_info(&json!({"path": p, "bandwidth": 500, "snr": 0}));
        assert_eq!(i["image"], true);
        assert_eq!((i["w"].as_u64(), i["h"].as_u64()), (Some(1600), Some(1200)));
        assert_eq!(i["fits"], false, "the test photo should be too big at 500 Hz: {i}");
        let s = &i["shrunk"];
        assert_eq!(s["fits"], true, "{i}");
        let pct = s["pct"].as_u64().unwrap();
        assert!(pct < 100 && (pct < 20 || pct % 5 == 0), "{s}");
        assert!(s["name"].as_str().unwrap().starts_with("IMG_0001_resized_") && s["name"].as_str().unwrap().ends_with(".jpg"));
        assert!(s["compressed"].as_u64().unwrap() < 100_000);
        // and send_file at that percentage sends that picture (it fits as well)
        let o = outgoing(&p, Some((pct as u32, DEFAULT_QUALITY))).unwrap();
        assert!(o.name.starts_with("IMG_0001_resized_"));
        let prepared = bbs::prepare_file(&o.name, &o.data).unwrap();
        assert!(prepared.compressed_size < 100_000);
        assert_eq!(s["compressed"].as_u64().unwrap() as usize, prepared.compressed_size);
        // the picture is what it should be: a JPEG of the shrunk size
        let back = image::load_from_memory(&o.data).unwrap();
        assert_eq!((back.width() as u64, back.height() as u64), (s["w"].as_u64().unwrap(), s["h"].as_u64().unwrap()));
    }

    #[test]
    fn the_send_time_choice_sets_the_starting_size() {
        let p = temp("IMG_0002.JPG", &photo(1600, 1200));
        let pct = |m: u64| file_info(&json!({"path": p, "bandwidth": "500", "snr": 10, "minutes": m}))["shrunk"]["pct"].as_u64().unwrap();
        let (two, ten, largest) = (pct(2), pct(10), pct(0));
        assert!(two <= ten && ten <= largest, "{two} {ten} {largest}");
        assert!(two < largest, "two minutes should need a smaller picture: {two} {largest}");
        // no choice given: about five minutes
        let default = file_info(&json!({"path": p, "bandwidth": "500", "snr": 10}))["shrunk"]["pct"].as_u64().unwrap();
        assert_eq!(default, pct(5));
    }

    #[test]
    fn the_operator_picks_the_size() {
        let p = temp("pic.png", &{
            let mut out = Vec::new();
            image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(200, 100, image::Rgb([10, 200, 30])))
                .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
                .unwrap();
            out
        });
        let i = file_info(&json!({"path": p, "bandwidth": "2300", "pct": 50, "quality": 20}));
        assert_eq!(i["limit"], 1_000_000);
        let s = &i["shrunk"];
        assert_eq!((s["pct"].as_u64(), s["quality"].as_u64()), (Some(50), Some(20)));
        assert_eq!((s["w"].as_u64(), s["h"].as_u64()), (Some(100), Some(50)));
        // a small picture that fits isn't shrunk unless asked
        let i = file_info(&json!({"path": p, "bandwidth": "500"}));
        assert_eq!(i["shrunk"]["pct"], 100);
    }

    #[test]
    fn no_limit_at_2750() {
        let p = temp("a.txt", b"x");
        assert_eq!(file_info(&json!({"path": p, "bandwidth": "2750"}))["limit"], Value::Null);
    }

    #[test]
    fn a_sideways_photo_is_turned_upright() {
        // A 40x20 JPEG with EXIF orientation 6 (rotate 90° clockwise to view): upright it's 20x40
        let mut jpg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpg, 90)
            .encode_image(&image::RgbImage::from_pixel(40, 20, image::Rgb([200, 0, 0])))
            .unwrap();
        // APP1 Exif segment: big-endian TIFF, one IFD entry, Orientation (0x0112) = 6
        let tiff: [u8; 26] = [
            b'M', b'M', 0, 42, 0, 0, 0, 8, // header, IFD at 8
            0, 1, // one entry
            0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, 6, 0, 0, // Orientation SHORT 1 = 6
            0, 0, 0, 0, // no next IFD
        ];
        let mut app1 = b"Exif\0\0".to_vec();
        app1.extend_from_slice(&tiff);
        let len = (app1.len() + 2) as u16;
        let mut with_exif = vec![0xFF, 0xD8, 0xFF, 0xE1, (len >> 8) as u8, len as u8];
        with_exif.extend_from_slice(&app1);
        with_exif.extend_from_slice(&jpg[2..]);
        let p = temp("side.jpg", &with_exif);
        let i = file_info(&json!({"path": p, "bandwidth": "500", "pct": 100}));
        assert_eq!((i["w"].as_u64(), i["h"].as_u64()), (Some(20), Some(40)), "{i}");
        assert_eq!((i["shrunk"]["w"].as_u64(), i["shrunk"]["h"].as_u64()), (Some(20), Some(40)));
    }
}
