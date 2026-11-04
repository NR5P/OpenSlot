//! The Rig controls window: power, gains, NB/NR, preamp, attenuator, AGC, the mode and the
//! meters, beyond the frequency on the main screen. The same panel drives a remote station
//! (remote_ui.rs), so it only reports what was asked for and never touches the rig itself.

use eframe::egui::{self, Color32, RichText, Stroke, Vec2};

use super::state::GuiState;
use super::theme;
use crate::rig::{Agc, Meter, RadioMode, RigAction, RigExtras};

/// What the panel shows: the radio as last reported.
pub struct RigPanel<'a> {
    pub connected: bool,
    pub mode: Option<RadioMode>,
    pub ptt: bool,
    pub extras: &'a RigExtras,
    /// May key the transmitter (the tuner): not with PTT disabled
    pub may_transmit: bool,
    /// Where the controls come from ("rigctld (127.0.0.1:4532)"), for the empty case
    pub backend: String,
}

/// The local window, opened from the frequency card's CONTROLS button.
pub fn draw_rig_window(ctx: &egui::Context, state: &mut GuiState) {
    if !state.rig_panel_open {
        return;
    }
    let mut open = true;
    let mut actions = Vec::new();
    egui::Window::new("Rig controls")
        .open(&mut open)
        .default_width(330.0)
        .resizable(false)
        .collapsible(true)
        .show(ctx, |ui| {
            let panel = RigPanel {
                connected: state.rig_connected,
                mode: state.rig_mode,
                ptt: state.rig_ptt,
                extras: &state.rig_extras,
                may_transmit: !state.ptt_disabled,
                backend: state.rig_backend.to_string(),
            };
            draw_rig_controls(ui, &panel, &mut actions);
        });
    state.rig_panel_open = open;
    state.rig_actions.extend(actions);
}

pub fn draw_rig_controls(ui: &mut egui::Ui, p: &RigPanel, out: &mut Vec<RigAction>) {
    ui.spacing_mut().item_spacing.y = 6.0;
    if !p.connected {
        ui.label(RichText::new("The radio isn't connected.").color(theme::MUTED));
        if p.backend == "None" {
            ui.label(RichText::new("Set up rig control in Settings → RIG Control.").small().color(theme::MUTED));
        }
        return;
    }
    let x = p.extras;

    // Meters: the S-meter receiving, power / SWR / ALC transmitting
    if p.ptt {
        let mut any = false;
        if let Some(&v) = x.meters.get(&Meter::Power) {
            meter(ui, "PO", v, format!("{:.0} %", v * 100.0), theme::ACCENT);
            any = true;
        }
        if let Some(&v) = x.meters.get(&Meter::Swr) {
            let color = if v >= 2.5 { theme::RED } else if v >= 1.8 { theme::YELLOW } else { theme::GREEN };
            meter(ui, "SWR", ((v - 1.0) / 4.0).clamp(0.0, 1.0), format!("{v:.1}"), color);
            any = true;
        }
        if let Some(&v) = x.meters.get(&Meter::Alc) {
            meter(ui, "ALC", v, format!("{:.0} %", v * 100.0), theme::PURPLE);
            any = true;
        }
        if !any {
            ui.label(RichText::new("Transmitting").color(theme::TX_RED).strong());
        }
    } else if let Some(&db) = x.meters.get(&Meter::Signal) {
        // S0 … S9+60 across the bar, S9 at 60 %
        let frac = if db <= 0.0 { (db + 54.0) / 54.0 * 0.6 } else { 0.6 + db / 60.0 * 0.4 };
        meter(ui, "S", frac.clamp(0.0, 1.0), crate::rig::extras::s_units(db), theme::GREEN);
    }

    // Mode
    theme::caption(ui, "Mode");
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for m in RadioMode::ALL {
            if ui.add(egui::Button::new(RichText::new(m.to_string()).monospace().size(12.0)).selected(p.mode == Some(m)).min_size(Vec2::new(54.0, 22.0))).clicked()
                && p.mode != Some(m)
            {
                out.push(RigAction::Mode { mode: m });
            }
        }
    });

    if !x.probed && x.levels.is_empty() && x.switches.is_empty() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(RichText::new("Asking the radio which controls it has…").color(theme::MUTED));
        });
    }

    // Levels, applied when the slider is let go (a drag would flood the radio)
    if !x.levels.is_empty() {
        egui::Grid::new(ui.id().with("rig_levels")).num_columns(3).spacing([8.0, 6.0]).show(ui, |ui| {
            for (&level, &value) in &x.levels {
                ui.label(level.label());
                let id = ui.id().with(("rig_level", level));
                let mut v = ui.data(|d| d.get_temp::<f32>(id)).unwrap_or(value);
                let r = ui.add_sized([170.0, 18.0], egui::Slider::new(&mut v, 0.0..=1.0).show_value(false));
                if r.dragged() || r.has_focus() {
                    ui.data_mut(|d| d.insert_temp(id, v));
                }
                if r.drag_stopped() || (r.changed() && !r.dragged()) {
                    out.push(RigAction::Level { level, value: v });
                    ui.data_mut(|d| d.remove::<f32>(id));
                }
                ui.label(RichText::new(format!("{:>3.0} %", v * 100.0)).monospace().color(theme::MUTED));
                ui.end_row();
            }
        });
    }

    // On/off switches
    if !x.switches.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for (&sw, &on) in &x.switches {
                if ui.add(egui::Button::new(RichText::new(sw.label()).size(12.5)).selected(on).min_size(Vec2::new(60.0, 24.0)))
                    .on_hover_text(format!("{} {}", sw.tip(), if on { "on: click to switch off" } else { "off: click to switch on" }))
                    .clicked()
                {
                    out.push(RigAction::Switch { switch: sw, on: !on });
                }
            }
        });
    }

    if let Some(agc) = x.agc {
        ui.horizontal(|ui| {
            theme::caption(ui, "AGC");
            ui.spacing_mut().item_spacing.x = 4.0;
            for a in Agc::ALL {
                if ui.add(egui::Button::new(RichText::new(a.label()).size(12.0)).selected(agc == a)).clicked() && agc != a {
                    out.push(RigAction::Agc { agc: a });
                }
            }
        });
    }

    ui.separator();
    ui.horizontal(|ui| {
        let tip = if p.may_transmit {
            "Start the radio's antenna tuner (it transmits while it tunes)"
        } else {
            "PTT is disabled: the tuner would transmit"
        };
        if ui.add_enabled(p.may_transmit && !p.ptt, egui::Button::new("TUNE ATU")).on_hover_text(tip).clicked() {
            out.push(RigAction::Tune);
        }
        if x.probed && x.levels.is_empty() && x.switches.is_empty() && x.agc.is_none() {
            ui.label(RichText::new(format!("No other controls through {}", p.backend)).small().color(theme::MUTED));
        }
    });
}

fn meter(ui: &mut egui::Ui, label: &str, frac: f32, text: String, color: Color32) {
    ui.horizontal(|ui| {
        ui.add_sized([30.0, 16.0], egui::Label::new(RichText::new(label).monospace().color(theme::MUTED)));
        let (rect, _) = ui.allocate_exact_size(Vec2::new(200.0, 12.0), egui::Sense::hover());
        let p = ui.painter();
        p.rect_filled(rect, 3.0, theme::tint(theme::BORDER, 160));
        let mut fill = rect;
        fill.set_width(rect.width() * frac.clamp(0.0, 1.0));
        p.rect_filled(fill, 3.0, color);
        p.rect_stroke(rect, 3.0, Stroke::new(1.0_f32, theme::BORDER));
        ui.label(RichText::new(text).monospace());
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rig::{Level, Switch};

    /// Only the controls the radio answered to are drawn, and nothing is asked for unprompted.
    #[test]
    fn draws_without_asking() {
        let ctx = egui::Context::default();
        let mut x = RigExtras::default();
        x.levels.insert(Level::RfPower, 0.5);
        x.switches.insert(Switch::NoiseBlanker, true);
        x.meters.insert(Meter::Signal, -6.0);
        x.agc = Some(Agc::Fast);
        x.probed = true;
        let mut out = Vec::new();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let p = RigPanel { connected: true, mode: Some(RadioMode::UsbData), ptt: false, extras: &x, may_transmit: true, backend: "rigctld".into() };
                draw_rig_controls(ui, &p, &mut out);
            });
        });
        assert!(out.is_empty());
    }
}
