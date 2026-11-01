//! Visual theme: palette, global egui style, and small styled widgets
//! (pills, cards, accent buttons) shared by the GUI.

use eframe::egui;
use egui::{Color32, Frame, Margin, RichText, Rounding, Stroke, Vec2};

// Palette (dark-first; slate background, saturated-but-soft accents)
pub const BG: Color32 = Color32::from_rgb(13, 16, 23);
pub const SURFACE: Color32 = Color32::from_rgb(20, 24, 33);
pub const SURFACE_2: Color32 = Color32::from_rgb(28, 33, 45);
pub const SURFACE_3: Color32 = Color32::from_rgb(38, 44, 59);
pub const BORDER: Color32 = Color32::from_rgb(45, 52, 69);
pub const TEXT: Color32 = Color32::from_rgb(226, 232, 240);
pub const MUTED: Color32 = Color32::from_rgb(140, 151, 171);

pub const ACCENT: Color32 = Color32::from_rgb(56, 189, 248);
pub const GREEN: Color32 = Color32::from_rgb(52, 211, 153);
pub const YELLOW: Color32 = Color32::from_rgb(251, 191, 36);
pub const RED: Color32 = Color32::from_rgb(248, 113, 113);
pub const PURPLE: Color32 = Color32::from_rgb(192, 132, 252);
pub const TX_RED: Color32 = Color32::from_rgb(239, 68, 68);

pub const RADIUS: f32 = 8.0;

/// Translucent version of an accent for fills behind colored text.
pub fn tint(c: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), alpha)
}

/// Install visuals + spacing. Cheap enough per frame, but we only call it on change.
/// VarAC's themes: DEFAULT (dark or light) and SCIFI (always dark: deep blue, cyan text).
pub fn apply_with(ctx: &egui::Context, dark: bool, font_size: f32, scifi: bool) {
    let dark = dark || scifi;
    let mut v = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    let r = Rounding::same(RADIUS);

    if dark {
        v.panel_fill = BG;
        v.window_fill = SURFACE;
        v.extreme_bg_color = Color32::from_rgb(10, 12, 18);
        v.faint_bg_color = SURFACE_2;
        v.code_bg_color = SURFACE_2;
        v.window_stroke = Stroke::new(1.0_f32, BORDER);
        v.override_text_color = Some(TEXT);

        let w = &mut v.widgets;
        w.noninteractive.bg_fill = SURFACE;
        w.noninteractive.weak_bg_fill = SURFACE;
        w.noninteractive.bg_stroke = Stroke::new(1.0_f32, BORDER);
        w.noninteractive.fg_stroke = Stroke::new(1.0_f32, MUTED);

        w.inactive.bg_fill = SURFACE_2;
        w.inactive.weak_bg_fill = SURFACE_2;
        w.inactive.bg_stroke = Stroke::new(1.0_f32, BORDER);
        w.inactive.fg_stroke = Stroke::new(1.0_f32, TEXT);

        w.hovered.bg_fill = SURFACE_3;
        w.hovered.weak_bg_fill = SURFACE_3;
        w.hovered.bg_stroke = Stroke::new(1.0_f32, tint(ACCENT, 140));
        w.hovered.fg_stroke = Stroke::new(1.5_f32, Color32::WHITE);

        w.active.bg_fill = tint(ACCENT, 60);
        w.active.weak_bg_fill = tint(ACCENT, 60);
        w.active.bg_stroke = Stroke::new(1.0_f32, ACCENT);
        w.active.fg_stroke = Stroke::new(1.5_f32, Color32::WHITE);

        w.open.bg_fill = SURFACE_3;
        w.open.weak_bg_fill = SURFACE_3;
        w.open.bg_stroke = Stroke::new(1.0_f32, ACCENT);

        v.selection.bg_fill = tint(ACCENT, 90);
        v.selection.stroke = Stroke::new(1.0_f32, ACCENT);
        v.hyperlink_color = ACCENT;
    } else {
        v.panel_fill = Color32::from_rgb(243, 245, 249);
        v.window_fill = Color32::WHITE;
        v.extreme_bg_color = Color32::WHITE;
        v.faint_bg_color = Color32::from_rgb(234, 238, 244);
        v.window_stroke = Stroke::new(1.0_f32, Color32::from_rgb(214, 220, 230));
        v.selection.bg_fill = tint(Color32::from_rgb(2, 132, 199), 70);
        v.hyperlink_color = Color32::from_rgb(2, 132, 199);
    }

    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.rounding = Rounding::same(4.0);
        w.expansion = 0.0;
    }
    if scifi {
        let cyan = Color32::from_rgb(0, 229, 255);
        v.panel_fill = Color32::from_rgb(10, 16, 34);
        v.window_fill = Color32::from_rgb(14, 24, 48);
        v.extreme_bg_color = Color32::from_rgb(6, 10, 24);
        v.faint_bg_color = Color32::from_rgb(18, 30, 58);
        v.override_text_color = Some(cyan);
        v.window_stroke = Stroke::new(1.0_f32, tint(cyan, 120));
        v.widgets.inactive.bg_fill = Color32::from_rgb(0, 55, 85);
        v.widgets.inactive.weak_bg_fill = Color32::from_rgb(0, 55, 85);
        v.widgets.hovered.bg_fill = Color32::from_rgb(110, 20, 100);
        v.widgets.hovered.weak_bg_fill = Color32::from_rgb(110, 20, 100);
        v.widgets.active.bg_fill = Color32::from_rgb(55, 25, 90);
        v.selection.bg_fill = tint(cyan, 80);
    }
    v.window_rounding = Rounding::same(12.0);
    v.menu_rounding = r;
    v.window_shadow = egui::epaint::Shadow {
        offset: Vec2::new(0.0, 8.0),
        blur: 28.0,
        spread: 0.0,
        color: Color32::from_black_alpha(if dark { 140 } else { 40 }),
    };
    v.popup_shadow = egui::epaint::Shadow {
        offset: Vec2::new(0.0, 4.0),
        blur: 14.0,
        spread: 0.0,
        color: Color32::from_black_alpha(if dark { 120 } else { 30 }),
    };
    v.striped = true;
    v.slider_trailing_fill = true;
    ctx.set_visuals(v);

    let mut style = (*ctx.style()).clone();
    let size = font_size.clamp(10.0, 24.0);
    use egui::{FontFamily::*, FontId, TextStyle};
    style.text_styles = [
        (TextStyle::Heading, FontId::new(size + 6.0, Proportional)),
        (TextStyle::Body, FontId::new(size, Proportional)),
        (TextStyle::Button, FontId::new(size, Proportional)),
        (TextStyle::Monospace, FontId::new(size, Monospace)),
        (TextStyle::Small, FontId::new((size - 3.0).max(9.0), Proportional)),
    ]
    .into();
    let s = &mut style.spacing;
    s.item_spacing = Vec2::new(8.0, 6.0);
    s.button_padding = Vec2::new(10.0, 4.0);
    s.interact_size.y = 26.0;
    s.window_margin = Margin::same(14.0);
    s.menu_margin = Margin::same(6.0);
    s.combo_width = 80.0;
    s.scroll.bar_width = 8.0;
    s.scroll.floating = true;
    ctx.set_style(style);
}

/// A rounded "card" container.
pub fn card() -> Frame {
    Frame::none()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0_f32, BORDER))
        .rounding(Rounding::same(RADIUS + 2.0))
        .inner_margin(Margin::symmetric(10.0, 8.0))
}

/// Small rounded status pill: colored text on a faint tint of the same color.
pub fn pill(ui: &mut egui::Ui, text: impl Into<String>, color: Color32) -> egui::Response {
    Frame::none()
        .fill(tint(color, 34))
        .stroke(Stroke::new(1.0_f32, tint(color, 90)))
        .rounding(Rounding::same(999.0))
        .inner_margin(Margin::symmetric(9.0, 2.0))
        .show(ui, |ui| {
            ui.label(RichText::new(text.into()).color(color).strong().size(12.0))
        })
        .response
}

/// Pill with a leading status dot (green = up, etc.).
pub fn status_pill(ui: &mut egui::Ui, label: &str, color: Color32) -> egui::Response {
    Frame::none()
        .fill(tint(color, 26))
        .stroke(Stroke::new(1.0_f32, tint(color, 80)))
        .rounding(Rounding::same(999.0))
        .inner_margin(Margin::symmetric(9.0, 3.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(8.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 4.0, color);
                ui.painter().circle_stroke(rect.center(), 6.0, Stroke::new(1.0_f32, tint(color, 60)));
                ui.label(RichText::new(label).color(TEXT).strong().size(12.0));
            });
        })
        .response
}

/// Button filled with a translucent accent and colored label.
pub fn accent_button(text: impl Into<RichText>, color: Color32) -> egui::Button<'static> {
    egui::Button::new(text.into().color(color).strong())
        .fill(tint(color, 30))
        .stroke(Stroke::new(1.0_f32, tint(color, 110)))
        .rounding(Rounding::same(6.0))
}

/// Solid primary button (call to action).
pub fn primary_button(text: impl Into<RichText>, color: Color32) -> egui::Button<'static> {
    egui::Button::new(text.into().color(Color32::from_rgb(8, 12, 20)).strong())
        .fill(color)
        .rounding(Rounding::same(6.0))
}

/// Uppercase muted caption used above groups of controls.
pub fn caption(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text.to_uppercase()).color(MUTED).size(10.5).strong());
}

/// Segmented signal meter, S-meter style.
pub fn snr_meter(ui: &mut egui::Ui, snr_db: f32) {
    let segments = 12;
    let norm = ((snr_db + 20.0) / 40.0).clamp(0.0, 1.0);
    let lit = (norm * segments as f32).round() as usize;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(segments as f32 * 6.0, 14.0), egui::Sense::hover());
    let p = ui.painter();
    for i in 0..segments {
        let x = rect.left() + i as f32 * 6.0;
        let seg = egui::Rect::from_min_size(egui::pos2(x, rect.top()), Vec2::new(4.0, rect.height()));
        let frac = i as f32 / segments as f32;
        let c = if frac < 0.33 { RED } else if frac < 0.6 { YELLOW } else { GREEN };
        p.rect_filled(seg, 1.5, if i < lit { c } else { SURFACE_3 });
    }
}
