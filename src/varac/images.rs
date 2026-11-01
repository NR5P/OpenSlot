//! VarAC's image shrinker and QSL card generator.
//!
//! Shrinker: an image about to be sent is scaled (1–100 %) and saved as JPEG with
//! VarAC's quality setting (0–50, used as the JPEG quality), optionally with a white bar at
//! the top saying `DE MYCALL HISCALL S:+nn R:+nn`; sent as `<name>_resized_<UTC>.jpg`.

use ab_glyph::{Font, FontArc, PxScale, ScaleFont};
use image::{Rgba, RgbaImage};

pub fn is_image(path: &std::path::Path) -> bool {
    let p = path.to_string_lossy().to_uppercase();
    [".JPG", ".JPE", ".JPEG", ".BMP", ".GIF", ".PNG"].iter().any(|e| p.ends_with(e))
}

/// The font for overlays (egui's built-in Ubuntu Light).
pub fn font() -> Option<FontArc> {
    let defs = eframe::egui::FontDefinitions::default();
    let data = defs.font_data.get("Ubuntu-Light")?;
    FontArc::try_from_vec(data.font.to_vec()).ok()
}

/// Draw `text` with its top-left at (x, y), `px` high, in `color`.
pub fn draw_text(img: &mut RgbaImage, font: &FontArc, text: &str, x: f32, y: f32, px: f32, color: [u8; 3]) {
    let scale = PxScale::from(px);
    let sf = font.as_scaled(scale);
    let mut caret = x;
    let (w, h) = (img.width() as i32, img.height() as i32);
    for ch in text.chars() {
        let id = sf.glyph_id(ch);
        let glyph = id.with_scale_and_position(scale, ab_glyph::point(caret, y + sf.ascent()));
        caret += sf.h_advance(id);
        if let Some(out) = font.outline_glyph(glyph) {
            let b = out.px_bounds();
            out.draw(|gx, gy, cov| {
                let (px_, py_) = (b.min.x as i32 + gx as i32, b.min.y as i32 + gy as i32);
                if px_ >= 0 && py_ >= 0 && px_ < w && py_ < h {
                    let p = img.get_pixel_mut(px_ as u32, py_ as u32);
                    for i in 0..3 {
                        p.0[i] = (p.0[i] as f32 * (1.0 - cov) + color[i] as f32 * cov) as u8;
                    }
                }
            });
        }
    }
}

/// Text width in pixels.
#[allow(dead_code)]
pub fn text_width(font: &FontArc, text: &str, px: f32) -> f32 {
    let sf = font.as_scaled(PxScale::from(px));
    text.chars().map(|c| sf.h_advance(sf.glyph_id(c))).sum()
}

/// VarAC's overlay text: "DE MYCALL HISCALL S:+nn R:+nn" (parts left out when unknown).
pub fn overlay_text(my: &str, his: &str, snr_sent: Option<i32>, snr_rcvd: Option<i32>) -> String {
    let mut t = format!("DE {my}");
    if !his.is_empty() {
        t.push_str(&format!(" {his}"));
    }
    if let (Some(s), Some(r)) = (snr_sent, snr_rcvd) {
        t.push_str(&format!(" S:{s:+} R:{r:+}"));
    }
    t
}

/// Scale to `pct` %, add the overlay (when given), encode as JPEG at `quality` (0–50).
pub fn shrink(data: &[u8], pct: u32, quality: u8, overlay: Option<&str>) -> Result<(Vec<u8>, u32, u32), String> {
    let img = image::load_from_memory(data).map_err(|e| e.to_string())?;
    let pct = pct.clamp(1, 100);
    let (w, h) = ((img.width() * pct / 100).max(1), (img.height() * pct / 100).max(1));
    let mut rgba = img.resize_exact(w, h, image::imageops::FilterType::Triangle).to_rgba8();
    if let (Some(text), Some(font)) = (overlay, font()) {
        let bar = (h as f32 * 0.10).max(12.0) as u32;
        for y in 0..bar.min(h) {
            for x in 0..w {
                rgba.put_pixel(x, y, Rgba([255, 255, 255, 255]));
            }
        }
        draw_text(&mut rgba, &font, text, 10.0, 10.0_f32.min(bar as f32 * 0.2), (h as f32 * 0.05).max(9.0), [0, 0, 0]);
    }
    let rgb = image::DynamicImage::ImageRgba8(rgba).to_rgb8();
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality.clamp(1, 100))
        .encode_image(&rgb)
        .map_err(|e| e.to_string())?;
    Ok((out, w, h))
}

/// A rough (low, high) transfer speed in bps at `snr`. VARA's speed climbs about
/// exponentially with SNR, from a few tens of bps on a weak signal to the modem's top
/// speed (VARA HF about 7050 bps at 2300 Hz and 1500 at 500 Hz, VARA FM narrow about
/// 12750); half of that is what a file gets after acks and retries, and the low end
/// allows for a worse channel. Bandwidths other than 2300 use the 500 Hz figures.
pub fn speed_bps(snr: i32, bandwidth: u32, fm: bool) -> (u32, u32) {
    // (SNR where the slowest speed starts, SNR of the top speed, slowest, top)
    let (snr_lo, snr_hi, slow, top): (i32, i32, f64, f64) = if fm {
        (-20, 20, 300.0, 12_750.0)
    } else if bandwidth == 2300 {
        (-20, 15, 20.0, 7_050.0)
    } else {
        (-20, 15, 20.0, 1_500.0)
    };
    let x = f64::from(snr.clamp(snr_lo, snr_hi) - snr_lo) / f64::from(snr_hi - snr_lo);
    let hi = (slow * (top / slow).powf(x) / 2.0).round().max(1.0);
    ((hi * 0.65).round().max(1.0) as u32, hi as u32)
}

/// "a-b min at +NNdB" (whole minutes; b is at least a+1).
pub fn time_estimate(bytes: usize, snr: i32, bandwidth: u32, fm: bool) -> String {
    let (lo, hi) = speed_bps(snr, bandwidth, fm);
    let fast = bytes as u64 * 8 / hi.max(1) as u64 / 60;
    let mut slow = bytes as u64 * 8 / lo.max(1) as u64 / 60;
    if slow == fast {
        slow = fast + 1;
    }
    format!("{fast}-{slow} min at {}dB", crate::varac::messages::snr_text(snr))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = RgbaImage::from_pixel(w, h, Rgba([30, 120, 200, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img).write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn shrinks_with_overlay() {
        let src = png(400, 300);
        let (jpg, w, h) = shrink(&src, 50, 50, Some("DE W5TST NR5P S:+10 R:-3")).unwrap();
        assert_eq!((w, h), (200, 150));
        assert_eq!(&jpg[..2], &[0xFF, 0xD8], "a JPEG");
        let back = image::load_from_memory(&jpg).unwrap().to_rgb8();
        // the white bar on top, the picture below
        let top = back.get_pixel(150, 2);
        assert!(top.0.iter().all(|&c| c > 200), "{top:?}");
        let low = back.get_pixel(100, 120);
        assert!(low.0[2] > 150 && low.0[0] < 80, "{low:?}");
        // some dark text pixels in the bar
        assert!((0..200).any(|x| (0..15).any(|y| back.get_pixel(x, y).0[0] < 100)));
        assert!(is_image(std::path::Path::new("/a/b.JPeG")));
        assert!(!is_image(std::path::Path::new("/a/b.txt")));
        assert_eq!(overlay_text("W5TST", "NR5P", Some(10), Some(-3)), "DE W5TST NR5P S:+10 R:-3");
        assert_eq!(overlay_text("W5TST", "", None, None), "DE W5TST");
        assert_eq!(time_estimate(30_000, 0, 2300, false), "14-21 min at +00dB");
        assert_eq!(speed_bps(10, 500, false), (263, 405));
        // top speeds at a strong signal (halved), never zero on a weak one
        assert_eq!(speed_bps(25, 2300, false).1, 3525);
        assert_eq!(speed_bps(25, 500, false).1, 750);
        assert_eq!(speed_bps(25, 0, true).1, 6375);
        assert!(speed_bps(-30, 500, false).0 >= 1);
    }
}

/// What goes on a QSL card.
#[derive(Debug, Clone, Default)]
pub struct QslCard {
    pub my_call: String,
    /// "Name: …", "QTH: …", "Locator: …", "Rig: …", "Antenna: …" (empty ones left out)
    pub my_lines: Vec<String>,
    pub call: String,
    pub rst_sent: String,
    pub rst_rcvd: String,
    pub band: String,
    /// "MMM dd, yyyy HH:mm:ss"
    pub time_utc: String,
    pub power: String,
    pub message: String,
}

/// VarAC's default message: "TKS for QSO <HNAME>, 73, <NAME>."
pub fn qsl_message(template: &str, his_name: &str, my_name: &str) -> String {
    template.replace("<HNAME>", his_name).replace("<NAME>", my_name)
}

/// Draw the card on `background` (or plain white), VarAC style: the texts on
/// semi-transparent white boxes, optionally fading to black at the bottom.
pub fn render_qsl(card: &QslCard, background: Option<&[u8]>, fade: bool) -> Result<RgbaImage, String> {
    let mut img = match background.map(image::load_from_memory) {
        Some(Ok(i)) => {
            let i = i.to_rgba8();
            // keep big photos manageable
            if i.width() > 1600 {
                image::imageops::resize(&i, 1600, i.height() * 1600 / i.width(), image::imageops::FilterType::Triangle)
            } else {
                i
            }
        }
        Some(Err(e)) => return Err(format!("background: {e}")),
        None => RgbaImage::from_pixel(900, 560, Rgba([255, 255, 255, 255])),
    };
    let (w, h) = (img.width() as f32, img.height() as f32);
    if fade {
        for y in (h as u32 / 2)..h as u32 {
            let k = (y as f32 - h / 2.0) / (h / 2.0);
            for x in 0..w as u32 {
                let p = img.get_pixel_mut(x, y);
                for c in 0..3 {
                    p.0[c] = (p.0[c] as f32 * (1.0 - k)) as u8;
                }
            }
        }
    }
    let font = font().ok_or("no font")?;
    let boxed = |img: &mut RgbaImage, x: f32, y: f32, bw: f32, bh: f32| {
        for yy in y.max(0.0) as u32..((y + bh).min(h)) as u32 {
            for xx in x.max(0.0) as u32..((x + bw).min(w)) as u32 {
                let p = img.get_pixel_mut(xx, yy);
                for c in 0..3 {
                    p.0[c] = ((p.0[c] as u32 * 59 + 255 * 196) / 255) as u8;
                }
            }
        }
    };
    let unit = h / 28.0;
    let m = unit;
    // my callsign, big, with my details under it
    let big = unit * 3.0;
    let cw = text_width(&font, &card.my_call, big) + m;
    boxed(&mut img, m * 0.5, m * 0.5, cw + m, big * 1.25);
    draw_text(&mut img, &font, &card.my_call, m, m * 0.6, big, [0, 0, 0]);
    let mut y = m * 0.5 + big * 1.4;
    let small = unit * 1.1;
    if !card.my_lines.is_empty() {
        let lw = card.my_lines.iter().map(|l| text_width(&font, l, small)).fold(0.0, f32::max) + m;
        boxed(&mut img, m * 0.5, y, lw + m, small * 1.35 * card.my_lines.len() as f32 + m * 0.3);
        for l in &card.my_lines {
            draw_text(&mut img, &font, l, m, y + m * 0.15, small, [0, 0, 0]);
            y += small * 1.35;
        }
    }
    // the QSO table near the bottom
    let heads = ["CALLSIGN", "RST-S", "RST-R", "2 WAY", "BAND", "TIME UTC", "POWER (W)"];
    let vals = [card.call.as_str(), &card.rst_sent, &card.rst_rcvd, "VARA", &card.band, &card.time_utc, &card.power];
    let cell = unit * 1.2;
    let widths: Vec<f32> = heads.iter().zip(vals.iter())
        .map(|(a, b)| text_width(&font, a, cell * 0.8).max(text_width(&font, b, cell)) + m)
        .collect();
    let total: f32 = widths.iter().sum();
    let ty = h - unit * 8.0;
    let tx = ((w - total) / 2.0).max(m * 0.5);
    boxed(&mut img, tx - m * 0.3, ty - m * 0.3, total + m * 0.6, cell * 3.0);
    let mut x = tx;
    for ((head, val), cw) in heads.iter().zip(vals.iter()).zip(widths.iter()) {
        draw_text(&mut img, &font, head, x, ty, cell * 0.8, [60, 60, 60]);
        draw_text(&mut img, &font, val, x, ty + cell * 1.1, cell, [0, 0, 0]);
        x += cw;
    }
    // the message
    let msg = format!("MESSAGE : {}", card.message);
    let mw = text_width(&font, &msg, cell);
    let my = h - unit * 3.6;
    boxed(&mut img, tx - m * 0.3, my - m * 0.3, mw.max(total) + m * 0.6, cell * 1.6);
    draw_text(&mut img, &font, &msg, tx, my, cell, [0, 0, 0]);
    Ok(img)
}

pub fn to_jpeg(img: &RgbaImage, quality: u8) -> Result<Vec<u8>, String> {
    let rgb = image::DynamicImage::ImageRgba8(img.clone()).to_rgb8();
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality).encode_image(&rgb).map_err(|e| e.to_string())?;
    Ok(out)
}

/// VarAC's file name: `<call>_DE_<mycall>_<time>.jpg` ('/' → '-', spaces and ':' → '_').
pub fn qsl_file_name(card: &QslCard) -> String {
    format!("{}_DE_{}_{}.jpg", card.call, card.my_call, card.time_utc)
        .replace('/', "-")
        .replace([' ', ':'], "_")
        .replace(',', "")
}

#[cfg(test)]
mod qsl_tests {
    use super::*;

    #[test]
    fn renders_a_card() {
        let card = QslCard {
            my_call: "W5TST".into(),
            my_lines: vec!["Name: Joe".into(), "QTH: Austin".into()],
            call: "NR5P".into(),
            rst_sent: "+10".into(),
            rst_rcvd: "-3".into(),
            band: "20m".into(),
            time_utc: "Oct 30, 2026 18:05:00".into(),
            power: "100".into(),
            message: qsl_message("TKS for QSO <HNAME>, 73, <NAME>.", "Bob", "Joe"),
        };
        assert_eq!(card.message, "TKS for QSO Bob, 73, Joe.");
        let img = render_qsl(&card, None, true).unwrap();
        assert_eq!((img.width(), img.height()), (900, 560));
        // dark text was drawn, and the bottom faded
        assert!(img.pixels().any(|p| p.0[0] < 40 && p.0[1] < 40));
        let jpg = to_jpeg(&img, 90).unwrap();
        assert_eq!(&jpg[..2], &[0xFF, 0xD8]);
        assert_eq!(qsl_file_name(&card), "NR5P_DE_W5TST_Oct_30_2026_18_05_00.jpg");
    }
}
