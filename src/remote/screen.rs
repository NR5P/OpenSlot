//! The station's screen on a remote: a desktop remote shows the station's own main window,
//! menus and windows, exactly as they look at the station.
//!
//! The station draws its screen a second time for each remote, in an egui context of that
//! remote's size ([`Renderer`]), and sends what egui painted: shapes, the text as layout jobs
//! (laid out again at the remote, with the same fonts: the glyph atlas differs from one
//! context to another) and pictures. The remote paints it ([`Viewer`]) and sends back its
//! mouse and keyboard as egui events, which the station's next pass takes as its own input.
//! Every button, menu and window is the station's, working on the station's state, logs and
//! settings, just as if pressed there.
//!
//! On the WebSocket: the remote sends `{"type":"screen", …}` ([`ScreenInput`]) with its size
//! (the first one starts the screen) and its input; the station answers with binary
//! messages, byte 0 = [`SCREEN`], then a [`Frame`] as deflated JSON.

use std::collections::HashMap;
use std::hash::Hasher;
use std::io::{Read, Write};
use std::time::{Duration, Instant};

use eframe::egui;
use egui::epaint::{
    self, CircleShape, ClippedShape, Color32, CubicBezierShape, EllipseShape, Mesh, PathShape, PathStroke,
    QuadraticBezierShape, RectShape, Shape, Stroke, TextShape, TextureId,
};
use egui::{Pos2, Rect, Vec2};
use serde::{Deserialize, Serialize};

/// Binary message type: a frame of the station's screen.
pub const SCREEN: u8 = 2;
/// A remote's window can't make the station draw anything bigger than this.
const MAX_SIDE: f32 = 8192.0;

/// What a remote sends: its window, and the input since the last one.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ScreenInput {
    /// The area the station's screen is shown in, in points
    pub size: [f32; 2],
    /// The remote's points per pixel (text is laid out for it)
    pub ppp: f32,
    #[serde(default)]
    pub events: Vec<egui::Event>,
    #[serde(default)]
    pub modifiers: egui::Modifiers,
}

/// One frame of the station's screen.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Frame {
    pub size: [f32; 2],
    pub shapes: Vec<WireShape>,
    /// Pictures to load (or update), by the station's texture key
    #[serde(default)]
    pub textures: Vec<WireTexture>,
    #[serde(default)]
    pub free: Vec<u64>,
    #[serde(default)]
    pub cursor: egui::CursorIcon,
    /// Copied at the station: goes to the remote's clipboard
    #[serde(default)]
    pub copied: String,
    #[serde(default)]
    pub open_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireShape {
    pub clip: Rect,
    pub shape: Piece,
}

/// A shape as sent: egui's, except that text goes as its layout job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Piece {
    Circle(CircleShape),
    Ellipse(EllipseShape),
    Line { points: [Pos2; 2], stroke: PathStroke },
    Path(PathShape),
    Rect(RectShape),
    Text {
        pos: Pos2,
        job: epaint::text::LayoutJob,
        underline: Stroke,
        fallback: Color32,
        #[serde(default)]
        override_color: Option<Color32>,
        opacity: f32,
        angle: f32,
    },
    Mesh(Mesh),
    Quadratic(QuadraticBezierShape),
    Cubic(CubicBezierShape),
}

/// A picture (or the changed part of one), as RGBA bytes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireTexture {
    pub key: u64,
    pub size: [usize; 2],
    /// Where the part goes; None = the whole picture
    pub pos: Option<[usize; 2]>,
    /// base64 of the premultiplied RGBA bytes
    pub rgba: String,
    pub options: epaint::textures::TextureOptions,
}

/// The station's texture id as a key (User ids in the top half).
fn key_of(id: TextureId) -> u64 {
    match id {
        TextureId::Managed(n) => n & (u64::MAX >> 1),
        TextureId::User(n) => n | (1 << 63),
    }
}

/// The font atlas: each side has its own (text is laid out again at the remote), and its
/// top-left texel is white in both, which plain shapes use.
const FONT_KEY: u64 = 0;

/// JSON has no infinity (serde writes null, which then doesn't read back), and egui uses it
/// for "no limit": an unclipped layer, text that doesn't wrap. A huge number does the same.
fn finite(x: f32) -> f32 {
    if x.is_finite() {
        x
    } else if x.is_nan() {
        0.0
    } else {
        x.signum() * 1e30
    }
}

fn finite_rect(r: Rect) -> Rect {
    Rect::from_min_max(Pos2::new(finite(r.min.x), finite(r.min.y)), Pos2::new(finite(r.max.x), finite(r.max.y)))
}

/// The text's colors as painted: a disabled button, a faded window, recolour the laid-out
/// glyphs, not the job they were laid out from. Each section takes the colour its first
/// glyph was painted in (each visible glyph is four vertices, in order, from
/// glyph_vertex_range).
fn painted_colors(galley: &epaint::Galley, job: &mut epaint::text::LayoutJob) {
    let mut done = vec![false; job.sections.len()];
    for row in &galley.rows {
        let mut v = row.visuals.glyph_vertex_range.start;
        for g in &row.glyphs {
            if g.uv_rect.is_nothing() {
                continue;
            }
            let i = g.section_index as usize;
            if let (Some(false), Some(vertex)) = (done.get(i).copied(), row.visuals.mesh.vertices.get(v)) {
                job.sections[i].format.color = vertex.color;
                done[i] = true;
            }
            v += 4;
        }
    }
}

fn flatten(shape: Shape, clip: Rect, out: &mut Vec<WireShape>) {
    let clip = finite_rect(clip);
    let piece = match shape {
        Shape::Noop | Shape::Callback(_) => return,
        Shape::Vec(v) => {
            for s in v {
                flatten(s, clip, out);
            }
            return;
        }
        Shape::Circle(c) => Piece::Circle(c),
        Shape::Ellipse(e) => Piece::Ellipse(e),
        Shape::LineSegment { points, stroke } => Piece::Line { points, stroke },
        Shape::Path(p) => Piece::Path(p),
        Shape::Rect(mut r) => {
            r.rect = finite_rect(r.rect);
            Piece::Rect(r)
        }
        Shape::Text(t) => Piece::Text {
            pos: t.pos,
            job: {
                let mut job = (*t.galley.job).clone();
                painted_colors(&t.galley, &mut job);
                job.wrap.max_width = finite(job.wrap.max_width);
                job.first_row_min_height = finite(job.first_row_min_height);
                job
            },
            underline: t.underline,
            fallback: t.fallback_color,
            override_color: t.override_text_color,
            opacity: t.opacity_factor,
            angle: t.angle,
        },
        Shape::Mesh(m) => Piece::Mesh(m),
        Shape::QuadraticBezier(q) => Piece::Quadratic(q),
        Shape::CubicBezier(c) => Piece::Cubic(c),
    };
    out.push(WireShape { clip, shape: piece });
}

fn wire_texture(id: TextureId, delta: &epaint::ImageDelta) -> Option<WireTexture> {
    use base64::Engine;
    let epaint::ImageData::Color(img) = &delta.image else { return None };
    let mut rgba = Vec::with_capacity(img.pixels.len() * 4);
    for p in &img.pixels {
        rgba.extend_from_slice(&p.to_array());
    }
    Some(WireTexture {
        key: key_of(id),
        size: img.size,
        pos: delta.pos,
        rgba: base64::engine::general_purpose::STANDARD.encode(rgba),
        options: delta.options,
    })
}

/// Hashes what goes into a writer (to tell whether a frame changed without keeping it).
struct HashWriter(std::collections::hash_map::DefaultHasher);

impl Write for HashWriter {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.write(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn encode(frame: &Frame) -> Vec<u8> {
    let mut z = flate2::write::DeflateEncoder::new(vec![SCREEN], flate2::Compression::fast());
    let _ = serde_json::to_writer(&mut z, frame);
    z.finish().unwrap_or_default()
}

/// Reads a frame (the binary message, type byte included).
pub fn decode(msg: &[u8]) -> Option<Frame> {
    let body = msg.strip_prefix(&[SCREEN])?;
    let mut json = Vec::new();
    // (bounded: a frame is far smaller; nothing makes a remote inflate gigabytes)
    flate2::read::DeflateDecoder::new(body).take(256 << 20).read_to_end(&mut json).ok()?;
    serde_json::from_slice(&json).ok()
}

// ---------------------------------------------------------------- the station's side

/// The station's screen for one remote: its own egui context (so its windows, scrolling and
/// typing are its own), drawn with the station's drawing code.
pub struct Renderer {
    pub ctx: egui::Context,
    start: Instant,
    input: ScreenInput,
    last_pass: Option<Instant>,
    next_due: Instant,
    /// The last frame sent (shapes and size), to send only changes
    last_hash: Option<u64>,
    pending_hash: Option<u64>,
    /// Copied at the station, or a link to open, not yet taken by the remote
    copied: String,
    open_url: Option<String>,
    /// Pictures not yet taken by the remote: they go with the next frame sent
    textures: Vec<WireTexture>,
    free: Vec<u64>,
    /// What the draw code's theme was last set to here
    pub theme: Option<(bool, f32, bool)>,
}

/// A remote's screen at most this often (and at least this often while it changes).
const MIN_GAP: Duration = Duration::from_millis(40);
const MAX_GAP: Duration = Duration::from_millis(500);

impl Renderer {
    pub fn new(setup: impl FnOnce(&egui::Context)) -> Self {
        let ctx = egui::Context::default();
        // (the chat and broadcast pop-out windows become windows inside the screen)
        ctx.set_embed_viewports(true);
        setup(&ctx);
        Self {
            ctx,
            start: Instant::now(),
            input: ScreenInput { size: [1360.0, 880.0], ppp: 1.0, ..Default::default() },
            last_pass: None,
            next_due: Instant::now(),
            last_hash: None,
            pending_hash: None,
            copied: String::new(),
            open_url: None,
            textures: Vec::new(),
            free: Vec::new(),
            theme: None,
        }
    }

    /// Takes the remote's input. True when it's time to draw the screen again.
    pub fn take_input(&mut self, inputs: Vec<ScreenInput>, now: Instant) -> bool {
        let mut fresh = false;
        for i in inputs {
            let size = [i.size[0].clamp(100.0, MAX_SIDE), i.size[1].clamp(100.0, MAX_SIDE)];
            let ppp = if i.ppp.is_finite() { i.ppp.clamp(0.5, 4.0) } else { 1.0 };
            fresh |= !i.events.is_empty() || size != self.input.size || ppp != self.input.ppp;
            self.input.size = size;
            self.input.ppp = ppp;
            self.input.modifiers = i.modifiers;
            self.input.events.extend(i.events);
        }
        let gap_ok = self.last_pass.is_none_or(|t| now.duration_since(t) >= MIN_GAP);
        gap_ok && (fresh || now >= self.next_due)
    }

    /// Draws the screen with `draw`. Returns the frame to send, or None when nothing changed.
    /// The frame's pictures stay pending until [`Renderer::sent`].
    pub fn pass(&mut self, draw: impl FnMut(&egui::Context)) -> Option<Vec<u8>> {
        let now = Instant::now();
        let size = Vec2::new(self.input.size[0], self.input.size[1]);
        let mut raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
            time: Some(self.start.elapsed().as_secs_f64()),
            modifiers: self.input.modifiers,
            events: std::mem::take(&mut self.input.events),
            focused: true,
            max_texture_side: Some(8192),
            ..Default::default()
        };
        if let Some(vp) = raw.viewports.get_mut(&egui::ViewportId::ROOT) {
            vp.native_pixels_per_point = Some(self.input.ppp);
            vp.focused = Some(true);
        }
        let out = self.ctx.run(raw, draw);
        self.last_pass = Some(now);
        let delay = out.viewport_output.get(&egui::ViewportId::ROOT).map(|v| v.repaint_delay).unwrap_or(MAX_GAP);
        self.next_due = now + delay.clamp(MIN_GAP, MAX_GAP);

        for (id, delta) in &out.textures_delta.set {
            if key_of(*id) != FONT_KEY {
                if let Some(t) = wire_texture(*id, delta) {
                    self.textures.push(t);
                }
            }
        }
        self.free.extend(out.textures_delta.free.iter().map(|id| key_of(*id)).filter(|k| *k != FONT_KEY));

        let mut shapes = Vec::with_capacity(out.shapes.len());
        for ClippedShape { clip_rect, shape } in out.shapes {
            flatten(shape, clip_rect, &mut shapes);
        }
        let p = out.platform_output;
        if !p.copied_text.is_empty() {
            self.copied = p.copied_text;
        }
        if let Some(u) = p.open_url {
            self.open_url = Some(u.url);
        }
        let frame = Frame {
            size: self.input.size,
            shapes,
            textures: self.textures.clone(),
            free: self.free.clone(),
            cursor: p.cursor_icon,
            copied: self.copied.clone(),
            open_url: self.open_url.clone(),
        };
        let mut h = HashWriter(Default::default());
        let _ = serde_json::to_writer(&mut h, &(&frame.size, &frame.shapes, frame.cursor));
        let hash = h.0.finish();
        let news = !frame.textures.is_empty() || !frame.free.is_empty() || !frame.copied.is_empty() || frame.open_url.is_some();
        if !news && self.last_hash == Some(hash) {
            return None;
        }
        self.pending_hash = Some(hash);
        Some(encode(&frame))
    }

    /// The last frame from [`Renderer::pass`] went out: its pictures and clipboard are delivered.
    pub fn sent(&mut self) {
        self.last_hash = self.pending_hash.take();
        self.textures.clear();
        self.free.clear();
        self.copied.clear();
        self.open_url = None;
    }
}

// ---------------------------------------------------------------- the remote's side

/// The station's screen as the remote shows it.
#[derive(Default)]
pub struct Viewer {
    /// The station's pictures, loaded here
    textures: HashMap<u64, egui::TextureHandle>,
    /// The last frame, ready to paint (text laid out here)
    shapes: Vec<ClippedShape>,
    pub size: [f32; 2],
    pub cursor: egui::CursorIcon,
    pub frames: u64,
    /// What was last sent to the station (to send the size again only when it changes)
    sent_size: Option<([f32; 2], f32)>,
}

impl Viewer {
    pub fn has_frame(&self) -> bool {
        self.frames > 0
    }

    /// Takes a frame: loads its pictures and lays out its text for painting.
    pub fn apply(&mut self, ctx: &egui::Context, frame: Frame) {
        use base64::Engine;
        for k in &frame.free {
            self.textures.remove(k);
        }
        for t in frame.textures {
            let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(&t.rgba) else { continue };
            if bytes.len() != t.size[0] * t.size[1] * 4 {
                continue;
            }
            let pixels = bytes.chunks_exact(4).map(|p| Color32::from_rgba_premultiplied(p[0], p[1], p[2], p[3])).collect();
            let img = egui::ColorImage { size: t.size, pixels };
            match (t.pos, self.textures.get_mut(&t.key)) {
                (Some(pos), Some(h)) => h.set_partial(pos, img, t.options),
                (Some(_), None) => {} // (a part of a picture we never had)
                (None, Some(h)) => h.set(img, t.options),
                (None, None) => {
                    let h = ctx.load_texture(format!("station-{}", t.key), img, t.options);
                    self.textures.insert(t.key, h);
                }
            }
        }
        if !frame.copied.is_empty() {
            ctx.copy_text(frame.copied);
        }
        if let Some(url) = frame.open_url {
            ctx.open_url(egui::OpenUrl::new_tab(url));
        }
        let remap = |id: TextureId| -> TextureId {
            let k = key_of(id);
            if k == FONT_KEY {
                return TextureId::default();
            }
            self.textures.get(&k).map(|h| h.id()).unwrap_or_default()
        };
        let mut shapes = Vec::with_capacity(frame.shapes.len());
        for WireShape { clip, shape } in frame.shapes {
            let shape = match shape {
                Piece::Circle(c) => Shape::Circle(c),
                Piece::Ellipse(e) => Shape::Ellipse(e),
                Piece::Line { points, stroke } => Shape::LineSegment { points, stroke },
                Piece::Path(p) => Shape::Path(p),
                Piece::Rect(mut r) => {
                    r.fill_texture_id = remap(r.fill_texture_id);
                    Shape::Rect(r)
                }
                Piece::Text { pos, job, underline, fallback, override_color, opacity, angle } => {
                    let galley = ctx.fonts(|f| f.layout_job(job));
                    Shape::Text(TextShape {
                        pos,
                        galley,
                        underline,
                        fallback_color: fallback,
                        override_text_color: override_color,
                        opacity_factor: opacity,
                        angle,
                    })
                }
                Piece::Mesh(mut m) => {
                    m.texture_id = remap(m.texture_id);
                    Shape::Mesh(m)
                }
                Piece::Quadratic(q) => Shape::QuadraticBezier(q),
                Piece::Cubic(c) => Shape::CubicBezier(c),
            };
            shapes.push(ClippedShape { clip_rect: clip, shape });
        }
        self.shapes = shapes;
        self.size = frame.size;
        self.cursor = frame.cursor;
        self.frames += 1;
    }

    /// Paints the last frame with its top-left corner at `origin`.
    pub fn paint(&self, painter: &egui::Painter, origin: Pos2) {
        let d = origin.to_vec2();
        for ClippedShape { clip_rect, shape } in &self.shapes {
            let mut s = shape.clone();
            s.translate(d);
            painter.with_clip_rect(clip_rect.translate(d).intersect(painter.clip_rect())).add(s);
        }
    }

    /// The remote's input for the station, from this frame's events, for a screen shown at
    /// `area`. None when there's nothing to tell it.
    pub fn input(&mut self, ctx: &egui::Context, area: Rect) -> Option<ScreenInput> {
        let d = area.min.to_vec2();
        let size = [area.width().round(), area.height().round()];
        let ppp = ctx.pixels_per_point();
        let (events, modifiers) = ctx.input(|i| {
            let events: Vec<egui::Event> = i
                .events
                .iter()
                .filter_map(|e| {
                    use egui::Event as E;
                    Some(match e.clone() {
                        E::PointerMoved(p) => E::PointerMoved(p - d),
                        E::PointerButton { pos, button, pressed, modifiers } => E::PointerButton { pos: pos - d, button, pressed, modifiers },
                        E::Touch { device_id, id, phase, pos, force } => E::Touch { device_id, id, phase, pos: pos - d, force },
                        E::Copy | E::Cut | E::Paste(_) | E::Text(_) | E::Key { .. } | E::MouseMoved(_) | E::PointerGone
                        | E::Zoom(_) | E::Ime(_) | E::MouseWheel { .. } | E::WindowFocused(_) => e.clone(),
                        // (the station takes its own screenshots, and no screen reader is there)
                        _ => return None,
                    })
                })
                .collect();
            (events, i.modifiers)
        });
        let changed = self.sent_size != Some((size, ppp));
        if events.is_empty() && !changed {
            return None;
        }
        self.sent_size = Some((size, ppp));
        Some(ScreenInput { size, ppp, events, modifiers })
    }

    /// Asks for the whole size again (after a reconnect: the station starts afresh).
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn station_screen(ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("NR5P");
            if ui.button("CALL CQ").clicked() {
                ui.label("clicked");
            }
            // (loaded once, as the app's pictures are)
            let id = egui::Id::new("pic");
            let tex = ui.ctx().data_mut(|d| d.get_temp::<egui::TextureHandle>(id)).unwrap_or_else(|| {
                let t = ui.ctx().load_texture("pic", egui::ColorImage::new([4, 4], Color32::RED), Default::default());
                ui.ctx().data_mut(|d| d.insert_temp(id, t.clone()));
                t
            });
            ui.image((tex.id(), Vec2::splat(16.0)));
        });
    }

    /// The station's screen comes out at the remote with the same text, positions and
    /// pictures, and a click at the remote is a click at the station.
    #[test]
    fn a_frame_round_trip() {
        let mut r = Renderer::new(|_| {});
        assert!(r.take_input(vec![ScreenInput { size: [400.0, 300.0], ppp: 1.0, ..Default::default() }], Instant::now()));
        let msg = r.pass(station_screen).expect("a first frame");
        let frame = decode(&msg).expect("decodes");
        assert_eq!(frame.size, [400.0, 300.0]);
        let texts: Vec<String> = frame.shapes.iter().filter_map(|s| match &s.shape { Piece::Text { job, .. } => Some(job.text.clone()), _ => None }).collect();
        assert!(texts.iter().any(|t| t == "NR5P") && texts.iter().any(|t| t == "CALL CQ"), "{texts:?}");
        assert!(!frame.textures.is_empty(), "the picture goes along");
        r.sent();
        assert!(r.pass(station_screen).is_none(), "nothing changed: nothing sent");

        // painted at the remote: text laid out again, the picture loaded
        let remote = egui::Context::default();
        let mut v = Viewer::default();
        let _ = remote.run(Default::default(), |ctx| v.apply(ctx, frame.clone()));
        assert!(v.has_frame());
        assert_eq!(v.textures.len(), 1);
        let station_text = frame.shapes.iter().find_map(|s| match &s.shape { Piece::Text { pos, job, .. } if job.text == "CALL CQ" => Some(*pos), _ => None }).unwrap();
        let shown = v.shapes.iter().find_map(|s| match &s.shape { Shape::Text(t) if t.galley.job.text == "CALL CQ" => Some((t.pos, t.galley.size())), _ => None }).unwrap();
        assert_eq!(shown.0, station_text);
        assert!(shown.1.x > 10.0, "laid out with real glyphs");

        // a click on the button, sent from a remote whose screen area starts at (0, 30)
        let button = frame.shapes.iter().find_map(|s| match &s.shape { Piece::Text { pos, job, .. } if job.text == "CALL CQ" => Some(*pos + Vec2::new(5.0, 5.0)), _ => None }).unwrap();
        let at = button + Vec2::new(0.0, 30.0);
        let area = Rect::from_min_size(Pos2::new(0.0, 30.0), Vec2::new(400.0, 300.0));
        let raw = egui::RawInput {
            events: vec![
                egui::Event::PointerMoved(at),
                egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() },
                egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() },
            ],
            ..Default::default()
        };
        let mut input = None;
        let _ = remote.run(raw, |ctx| input = v.input(ctx, area));
        let input = input.expect("the click goes to the station");
        assert!(matches!(input.events[0], egui::Event::PointerMoved(p) if p == button), "{:?}", input.events);
        assert!(r.take_input(vec![input], Instant::now() + MIN_GAP));
        let msg = r.pass(station_screen).expect("the click changed the screen");
        let frame = decode(&msg).unwrap();
        assert!(frame.shapes.iter().any(|s| matches!(&s.shape, Piece::Text { job, .. } if job.text == "clicked")), "the station saw the click");
    }

    /// A disabled button's text is greyed at the station, so it is at the remote too.
    #[test]
    fn disabled_text_stays_grey() {
        let screen = |ctx: &egui::Context| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.add_enabled(true, egui::Button::new("ON"));
                ui.add_enabled(false, egui::Button::new("OFF"));
            });
        };
        let mut r = Renderer::new(|_| {});
        let frame = decode(&r.pass(screen).unwrap()).unwrap();
        let color = |want: &str| frame.shapes.iter().find_map(|s| match &s.shape {
            Piece::Text { job, fallback, .. } if job.text == want => {
                let c = job.sections[0].format.color;
                Some(if c == Color32::PLACEHOLDER { *fallback } else { c })
            }
            _ => None,
        }).unwrap();
        let (on, off) = (color("ON"), color("OFF"));
        assert_ne!(on, off, "the disabled one is painted differently");
        assert!(off.a() < on.a() || off.r() < on.r(), "and dimmer: {on:?} {off:?}");
    }

    /// egui's "no limit" (infinity) in clip rects and text widths: JSON can't carry it, so a
    /// frame with it once never decoded at the remote ("Waiting for the station's screen").
    #[test]
    fn unlimited_sizes_still_decode() {
        let ctx = egui::Context::default();
        let mut job = epaint::text::LayoutJob::simple_singleline("no wrap".into(), egui::FontId::proportional(14.0), Color32::WHITE);
        job.wrap.max_width = f32::INFINITY;
        let mut galley = None;
        let _ = ctx.run(Default::default(), |ctx| galley = Some(ctx.fonts(|f| f.layout_job(job.clone()))));
        let mut shapes = Vec::new();
        flatten(Shape::galley(Pos2::new(5.0, 5.0), galley.unwrap(), Color32::WHITE), Rect::EVERYTHING, &mut shapes);
        flatten(Shape::rect_filled(Rect::EVERYTHING, 0.0, Color32::RED), Rect::EVERYTHING, &mut shapes);
        let msg = encode(&Frame { size: [100.0, 100.0], shapes, ..Default::default() });
        let frame = decode(&msg).expect("decodes");
        assert_eq!(frame.shapes.len(), 2);
        assert!(frame.shapes[0].clip.contains(Pos2::new(1e6, -1e6)), "still unclipped");
        assert!(matches!(&frame.shapes[0].shape, Piece::Text { job, .. } if job.wrap.max_width > 1e29));
    }

    #[test]
    fn junk_is_no_frame() {
        assert!(decode(&[]).is_none());
        assert!(decode(&[SCREEN, 1, 2, 3]).is_none());
        assert!(decode(&[1, 0, 0]).is_none(), "an audio frame isn't a screen");
    }
}
