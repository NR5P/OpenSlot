//! Main-window layout, arranged like the original VarAC so operators find
//! controls where they expect them:
//!
//! ```text
//! ┌ menu ────────────────────────────────────── UTC date · time ┐
//! │ FREQUENCY + controls │ BEACONS            │ CQ CALLS          │
//! │ (modem, tune, CQ…)   │ In QSO with · QRZ  │ SNR last/avg/mine │
//! ├ BROADCASTS ───────────────────────────────────┬ options      ┤
//! ├ CHAT                                          │ QSY, VMAIL…  │
//! │                                               │ SNR | INFO   │
//! ├ CALLSIGN  SNR  BAND  NAME  LOC  QTH  START    │ LOG · canned │
//! ├ New message                                   │ SEND         │
//! └ TX RX state call grid … status ─────────── VMAIL · RIG ─────┘
//! ```
//!
//! The right-hand column never scrolls: in a short window its buttons pair up instead.

use super::*;

const SIDE_COL_W: f32 = 176.0;
const CONTROL_COL_W: f32 = 320.0;

/// Draws everything except the menu bar and modal windows.
/// Returns true when the band/frequency was changed from the UI (caller tunes the rig).
pub(super) fn draw_main_window(
    ctx: &egui::Context,
    state: &mut GuiState,
    db: &Storage,
    qso_guid: &Option<String>,
) -> bool {
    let mut band_changed = false;

    // eframe's very first frame can have a near-zero window; fixed-width columns would go negative.
    let screen = ctx.screen_rect();
    if screen.width() < SIDE_COL_W + CONTROL_COL_W + 300.0 || screen.height() < 300.0 {
        egui::CentralPanel::default().show(ctx, |_| {});
        return false;
    }

    draw_status_bar(ctx, state);

    // Top section: controls | beacons | CQ calls
    egui::TopBottomPanel::top("top_section")
        .frame(Frame::none().fill(theme::BG).inner_margin(Margin { left: 10.0, right: 10.0, top: 8.0, bottom: 4.0 }))
        .resizable(true)
        .default_height(236.0)
        .min_height(200.0)
        .show_separator_line(false)
        .show(ctx, |ui| {
            let h = ui.available_height();
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(Vec2::new(CONTROL_COL_W, h), Layout::top_down(Align::LEFT), |ui| {
                    band_changed = draw_control_column(ui, state, db);
                });
                let w = ((ui.available_width() - 8.0) / 2.0).max(100.0);
                ui.allocate_ui_with_layout(Vec2::new(w, h), Layout::top_down(Align::LEFT), |ui| draw_beacons_card(ui, state, db));
                ui.allocate_ui_with_layout(Vec2::new(w, h), Layout::top_down(Align::LEFT), |ui| draw_cq_card(ui, state, db));
            });
        });

    // One right-hand column from the broadcasts down to the QSO row, as in VarAC:
    // options, then QSY / VMail / broadcast / file / path finder / in-QSO requests.
    // Everything in it fits without scrolling (the window's minimum height allows for it).
    egui::SidePanel::right("side_column")
        .frame(Frame::none().fill(theme::BG).inner_margin(Margin { left: 0.0, right: 10.0, top: 4.0, bottom: 8.0 }))
        .exact_width(SIDE_COL_W + 10.0)
        .resizable(false)
        .show_separator_line(false)
        .show(ctx, |ui| draw_side_column(ui, state));

    // QSO log row + message box (bottom, like VarAC), left of the column's LOG / SEND
    egui::TopBottomPanel::bottom("composer")
        .frame(Frame::none().fill(theme::BG).inner_margin(Margin { left: 10.0, right: 8.0, top: 4.0, bottom: 8.0 }))
        .show_separator_line(false)
        .show(ctx, |ui| {
            draw_qso_row(ui, state);
            ui.add_space(2.0);
            draw_composer(ui, state, db, qso_guid);
        });

    // Broadcasts strip
    egui::TopBottomPanel::top("broadcasts")
        .frame(Frame::none().fill(theme::BG).inner_margin(Margin { left: 10.0, right: 8.0, top: 4.0, bottom: 4.0 }))
        .resizable(true)
        // (a laptop screen leaves the chat barely two lines otherwise)
        .default_height(if screen.height() < 760.0 { 64.0 } else { 96.0 })
        .min_height(56.0)
        .show_separator_line(false)
        .show(ctx, |ui| draw_broadcasts_card(ui, state));

    // Chat
    egui::CentralPanel::default()
        .frame(Frame::none().fill(theme::BG).inner_margin(Margin { left: 10.0, right: 8.0, top: 4.0, bottom: 4.0 }))
        .show(ctx, |ui| draw_chat_area(ui, state));

    draw_vmail_window(ctx, state, db);

    band_changed
}

fn full_width(ui: &egui::Ui) -> Vec2 {
    Vec2::new(ui.available_width(), 26.0)
}

// ---------------------------------------------------------------- top-left controls

fn draw_control_column(ui: &mut egui::Ui, state: &mut GuiState, db: &Storage) -> bool {
    let mut band_changed = false;

    // FREQUENCY
    theme::card().inner_margin(Margin::symmetric(12.0, 6.0)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            theme::caption(ui, "Frequency");
            if !state.current_band.is_empty() {
                theme::pill(ui, state.current_band.clone(), theme::ACCENT).on_hover_text("Band");
            }
            if let Some(m) = state.rig_mode {
                theme::pill(ui, m.to_string(), theme::PURPLE).on_hover_text("Radio mode");
            }
            if state.freq_mode.scanning() {
                theme::pill(ui, if state.freq_mode == crate::varac::scheduler::Mode::Ale { "ALE" } else { "SCANNING" }, theme::YELLOW);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                rig_pill(ui, state);
                if state.modem_connected {
                    theme::status_pill(ui, "VARA", theme::GREEN).on_hover_text("VARA modem connected");
                } else {
                    theme::status_pill(ui, "VARA", theme::RED).on_hover_text("VARA modem not connected");
                }
            });
        });
        ui.horizontal(|ui| {
            let hz = state.rig_freq.filter(|_| state.rig_connected).unwrap_or(state.current_frequency);
            // The readout is the frequency picker: click it for the calling frequencies,
            // your own list, or to type any frequency
            let readout = {
                let mut job = egui::text::LayoutJob::default();
                let fmt = |size: f32, color: Color32, mono: bool| egui::TextFormat {
                    font_id: if mono { egui::FontId::monospace(size) } else { egui::FontId::proportional(size) },
                    color,
                    ..Default::default()
                };
                job.append(&format!("{}.{:03}", hz / 1_000_000, (hz / 1000) % 1000), 0.0, fmt(24.0, theme::TEXT, true));
                job.append(&format!(".{:03}", hz % 1000), 0.0, fmt(16.0, theme::MUTED, true));
                // (room for the painted ▾: egui's bundled fonts have no triangle glyphs)
                job.append("    ", 0.0, fmt(14.0, theme::MUTED, false));
                job
            };
            let resp = ui.add(egui::Button::new(readout).frame(false))
                .on_hover_text("Change frequency: calling frequencies, your own list, or type one");
            {
                let c = egui::pos2(resp.rect.right() - 9.0, resp.rect.center().y + 2.0);
                let color = if resp.hovered() { theme::TEXT } else { theme::MUTED };
                let points = vec![egui::pos2(c.x - 4.0, c.y - 2.5), egui::pos2(c.x + 4.0, c.y - 2.5), egui::pos2(c.x, c.y + 2.5)];
                ui.painter().add(egui::Shape::convex_polygon(points, color, Stroke::NONE));
            }
            let popup = ui.make_persistent_id("freq_picker");
            if resp.clicked() {
                ui.memory_mut(|m| m.toggle_popup(popup));
            }
            let mut pick: Option<u64> = None;
            egui::popup_below_widget(ui, popup, &resp, egui::PopupCloseBehavior::CloseOnClickOutside, |ui| {
                ui.set_min_width(300.0);
                theme::caption(ui, "Enter a frequency");
                ui.horizontal(|ui| {
                    let te = ui.add(egui::TextEdit::singleline(&mut state.freq_entry)
                        .hint_text("14.1055 MHz or 14105.5 kHz").desired_width(180.0));
                    let enter = te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if ui.button("Tune").clicked() || enter {
                        match messages::parse_frequency_input(&state.freq_entry) {
                            Some(f) => {
                                pick = Some(f);
                                state.freq_entry.clear();
                            }
                            None => state.status_msg = format!("Not a frequency: {}", state.freq_entry.trim()),
                        }
                    }
                });
                ui.separator();
                theme::caption(ui, "VarAC calling frequencies");
                egui::Grid::new("cf_grid").num_columns(2).spacing([14.0, 2.0]).show(ui, |ui| {
                    for (i, &(freq, band)) in messages::CALLING_FREQUENCIES_HZ.iter().enumerate() {
                        let label = RichText::new(format!("{band:>4}  {:.3}", freq as f64 / 1e6)).monospace();
                        if ui.selectable_label(state.current_frequency == freq, label).clicked() {
                            pick = Some(freq);
                        }
                        if i % 2 == 1 {
                            ui.end_row();
                        }
                    }
                });
                ui.separator();
                theme::caption(ui, "My frequencies");
                if state.my_frequencies.is_empty() {
                    ui.label(RichText::new("None yet: add the frequency you're on below.").small().italics().color(theme::MUTED));
                }
                let mut remove = None;
                for (i, sf) in state.my_frequencies.iter().enumerate() {
                    ui.horizontal(|ui| {
                        let band = freq_to_band(sf.hz).unwrap_or("");
                        let label = RichText::new(format!("{:>9.4}  {band:>4}  {}", sf.hz as f64 / 1e6, sf.name)).monospace();
                        if ui.selectable_label(state.current_frequency == sf.hz, label).clicked() {
                            pick = Some(sf.hz);
                        }
                        if ui.small_button("✕").on_hover_text("Remove from my frequencies").clicked() {
                            remove = Some(i);
                        }
                    });
                }
                if let Some(i) = remove {
                    state.my_frequencies.remove(i);
                    state.save_my_frequencies();
                }
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut state.freq_entry_name).hint_text("Name (optional)").desired_width(120.0));
                    let here = format!("{}.{:03}.{:03}", hz / 1_000_000, (hz / 1000) % 1000, hz % 1000);
                    let known = state.my_frequencies.iter().any(|f| f.hz == hz);
                    if ui.add_enabled(!known, egui::Button::new(format!("Add {here}")))
                        .on_hover_text("Keep the frequency you're on in this list").clicked()
                    {
                        state.my_frequencies.push(crate::config::SavedFrequency { hz, name: state.freq_entry_name.trim().to_string() });
                        state.my_frequencies.sort_by_key(|f| f.hz);
                        state.freq_entry_name.clear();
                        state.save_my_frequencies();
                    }
                });
            });
            if let Some(f) = pick {
                ui.memory_mut(|m| m.close_popup());
                // (also when the radio has moved off it, e.g. to a slot)
                if f != state.current_frequency || f != hz {
                    state.current_frequency = f;
                    state.current_band = freq_to_band(f).unwrap_or("").to_string();
                    band_changed = true;
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                // VarAC's frequency schedule button: OFF → SCHEDULE → SCANNER → OFF
                use crate::varac::scheduler::Mode;
                let rig_ok = state.rig_backend != crate::rig::RigBackend::None;
                let mode = state.freq_mode;
                let text = if rig_ok { mode.button() } else { "FREQ SCHEDULE N/A" };
                let b = if mode == Mode::Inactive { egui::Button::new(text) } else { theme::accent_button(text, theme::YELLOW) };
                let r = ui.add_enabled(rig_ok, b.small())
                    .on_hover_text("Frequency scheduler / scanner. Right-click to edit the lists.");
                if r.clicked() {
                    let next = mode.next_with_ale(!state.schedule_rules.is_empty(), state.scan_list.iter().any(|e| e.enabled), state.settings_form.ale_enabled);
                    if next == Mode::Inactive && mode == Mode::Inactive {
                        state.add_system_chat("No frequency schedule rules configured: right-click the FREQ SCHEDULE button to open the editor.".into());
                        state.sched_editor = Some(state.new_sched_editor());
                    } else {
                        state.set_freq_mode(next);
                    }
                }
                r.context_menu(|ui| {
                    if ui.button("Frequency scheduler / scanner editor...").clicked() {
                        state.sched_editor = Some(state.new_sched_editor());
                        ui.close_menu();
                    }
                });
            });
        });
        draw_slot_row(ui, state);
    });

    ui.add_space(6.0);

    // Two button columns, as in VarAC: left = modem/tune, right = connect/abort/CQ/beacon
    ui.columns(2, |cols| {
        let ui = &mut cols[0];
        ui.spacing_mut().item_spacing.y = 5.0;
        if state.modem_connected {
            let btn = egui::Button::new(RichText::new("DISCONNECT MODEM").size(12.0).color(theme::YELLOW).strong())
                .fill(theme::tint(theme::YELLOW, 30))
                .stroke(Stroke::new(1.0_f32, theme::tint(theme::YELLOW, 110)))
                .wrap_mode(egui::TextWrapMode::Truncate);
            if ui.add_sized(full_width(ui), btn).clicked() {
                state.send_cmd(GuiCommand::DisconnectModem);
            }
        } else if ui.add_sized(full_width(ui), theme::primary_button("CONNECT MODEM", theme::GREEN)).clicked() {
            state.send_cmd(GuiCommand::ConnectModem);
        }
        ui.horizontal(|ui| {
            let w = (ui.available_width() - 6.0) / 2.0;
            if ui.add_sized([w, 26.0], egui::Button::new("TUNE")).on_hover_text("Send a tuning carrier (opens the drive level control)").clicked()
                && state.modem_connected
            {
                state.send_cmd(GuiCommand::Tune);
                state.tune_window = true;
            }
            if ui.add_sized([w, 26.0], egui::Button::new("STOP")).clicked() {
                state.send_cmd(GuiCommand::TuneStop);
            }
        });
        let w = full_width(ui).x;
        egui::ComboBox::from_id_salt("listen_mode")
            .selected_text(format!("Listen: {}", state.listen_mode))
            .width(w)
            .show_ui(ui, |ui| {
                for (label, mode, tip) in [
                    ("ON", ListenMode::On, "Answer connects and decode CQs"),
                    ("CQ", ListenMode::Cq, "Decode CQs only"),
                    ("OFF", ListenMode::Off, "Don't listen"),
                ] {
                    if ui.selectable_label(state.listen_mode == label, label).on_hover_text(tip).clicked() {
                        state.listen_mode = label.to_string();
                        state.send_cmd(GuiCommand::SetListen(mode));
                    }
                }
            });
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let w = ui.available_width() - 44.0;
            // VarAC's beacon button shows the countdown while beaconing
            let label = if state.freq_mode.scanning() {
                "BEACONS MANAGED BY SCANNER".to_string()
            } else if state.beacon_auto_enabled {
                let interval: i64 = state.settings_form.beacon_interval.parse().unwrap_or(15);
                let left = state.last_beacon_sent.map(|t| interval * 60 - (Utc::now() - t).num_seconds()).unwrap_or(0);
                if left <= 0 {
                    "NEXT BEACON: SOON".to_string()
                } else {
                    format!("NEXT BEACON: {:02}:{:02}:{:02}", left / 3600, left / 60 % 60, left % 60)
                }
            } else {
                "SEND BEACONS".to_string()
            };
            let tip = if state.settings_form.unlimited_beacon_time {
                "Send a beacon now"
            } else {
                "Send a beacon now. The automatic beacon shuts down within 72 hours; you'll need to re-enable it to continue beaconing."
            };
            let beacon_click = ui.add_sized([w, 26.0], theme::accent_button(label, theme::PURPLE)).on_hover_text(tip).clicked();
            if beacon_click && state.freq_mode.scanning() {
                // VarAC: pressing BEACON while the scanner runs switches the scanner off
                state.set_freq_mode(crate::varac::scheduler::Mode::Inactive);
            } else if beacon_click
                && state.modem_connected
                && !state.refuse_wide_on_calling_freq()
            {
                let source = state.beacon_source();
                let bandwidth = state.cq_bandwidth();
                state.send_cmd(GuiCommand::SendCq { source, bandwidth });
                state.last_beacon_sent = Some(Utc::now());
            }
            auto_toggle(ui, &mut state.beacon_auto_enabled, "Send beacons automatically".into());
        });
        if state.buffer_bytes > 0 {
            if ui.add_sized(full_width(ui), theme::accent_button(format!("CLEAR BUF ({})", state.buffer_bytes), theme::RED)).clicked() {
                state.send_cmd(GuiCommand::ClearTxBuffer);
            }
        }

        let ui = &mut cols[1];
        ui.spacing_mut().item_spacing.y = 5.0;
        draw_callsign_box(ui, state, db);
        let in_qso = matches!(state.session_state, SessionState::Connected | SessionState::AwaitingDisconnect);
        if in_qso {
            if ui.add_sized(full_width(ui), theme::accent_button("DISCONNECT", theme::RED)).clicked() {
                state.send_cmd(GuiCommand::Disconnect);
            }
        } else if ui.add_sized(full_width(ui), theme::primary_button("CONNECT", theme::ACCENT)).clicked() {
            try_connect(state);
        }
        ui.horizontal(|ui| {
            let w = (ui.available_width() - 6.0) / 2.0;
            if ui.add_sized([w, 26.0], egui::Button::new("ABORT")).clicked() {
                state.send_cmd(GuiCommand::Abort);
            }
            if in_qso && state.buffer_bytes > 0 {
                if ui.add_sized([w, 26.0], theme::accent_button("QUICK DISC", theme::RED))
                    .on_hover_text("Clear buffer and disconnect immediately").clicked()
                {
                    state.send_cmd(GuiCommand::ClearTxBuffer);
                    state.send_cmd(GuiCommand::Disconnect);
                }
            } else if ui.add_sized([w, 26.0], egui::Button::new("PING")).on_hover_text("Ping the callsign above").clicked()
                && state.modem_connected
                && !state.connect_callsign.is_empty()
                && !state.refuse_wide_on_calling_freq()
            {
                let dest = state.connect_callsign.to_uppercase();
                state.add_log(format!("Pinging {dest}..."));
                state.status_msg = format!("Pinging {dest}...");
                state.send_cmd(GuiCommand::Ping(dest));
            }
        });
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let w = ui.available_width() - 44.0;
            if state.cq_active || state.slot_wait.is_some() {
                if ui.add_sized([w, 26.0], theme::accent_button("END CQ", theme::RED)).clicked() {
                    state.cq_active = false;
                    state.send_cmd(GuiCommand::Abort);
                    if state.slot_wait.is_some() || state.pending_slot_qsy.is_some() {
                        state.return_to_cf();
                    }
                    state.status_msg = "CQ stopped".into();
                }
            } else if ui.add_sized([w, 26.0], theme::accent_button("CALL CQ", theme::PURPLE)).clicked() && state.modem_connected {
                // VarAC: choose the slot to stand by on, unless the slot system is off
                if state.settings_form.skip_cq_slot_selector {
                    state.call_cq_on_slot(0);
                } else {
                    state.cq_slot_selector_open = true;
                }
            }
            let tip = format!("Repeat CQ every {}s", state.auto_cq_interval_secs);
            auto_toggle(ui, &mut state.auto_cq_enabled, tip);
        });
    });

    band_changed
}

/// VarAC's SLOT / CF readouts: which slot the radio is on, the calling frequency, slot
/// up/down, back to CF; and while standing by after a CQ, the countdown back to the CF.
fn draw_slot_row(ui: &mut egui::Ui, state: &mut GuiState) {
    use crate::varac::slots;
    let step = state.slot_step();
    let cf = state.slot_wait.map(|(_, cf, _)| cf).unwrap_or(state.current_frequency);
    let dial = state.dial();
    let slot = slots::slot_at(cf, dial, step);
    ui.horizontal(|ui| {
        theme::caption(ui, "SLOT");
        let text = match slot {
            Some(0) => "CF".to_string(),
            Some(s) => s.to_string(),
            None => "—".into(),
        };
        ui.label(RichText::new(text).monospace().strong().color(if slot.is_some_and(|s| s != 0) { theme::YELLOW } else { theme::TEXT }))
            .on_hover_text("The slot you're on (VarAC slots are 750 Hz steps around the calling frequency)");
        let rig = state.rig_connected;
        let cur = slot.unwrap_or(0);
        if arrow_button(ui, rig && slots::neighbour(cur, -1).is_some(), false)
            .on_hover_text("Move down a slot").clicked()
        {
            if let Some(f) = slots::neighbour(cur, -1).and_then(|s| slots::frequency(cf, s, step)) {
                state.tune_to = Some(f);
            }
        }
        if arrow_button(ui, rig && slots::neighbour(cur, 1).is_some(), true)
            .on_hover_text("Move up a slot").clicked()
        {
            if let Some(f) = slots::neighbour(cur, 1).and_then(|s| slots::frequency(cf, s, step)) {
                state.tune_to = Some(f);
            }
        }
        ui.add_space(8.0);
        theme::caption(ui, "CF");
        ui.label(RichText::new(format!("{:.3}", cf as f64 / 1e6)).monospace().color(theme::MUTED))
            .on_hover_text("The calling frequency: CQ there, then QSO on a slot");
        if dial != cf && ui.add_enabled(rig, egui::Button::new("CF").small())
            .on_hover_text("Return to the calling frequency").clicked()
        {
            state.return_to_cf();
        }
    });
    // VarAC's bandwidth buttons (VARA HF): the modem's speed; 500 Hz on calling frequencies
    // (a row of their own: beside the slot controls they ran over the CF label)
    if state.modem_type() == crate::vara::commands::ModemType::Hf {
        ui.horizontal(|ui| {
            theme::caption(ui, "BW");
            ui.spacing_mut().item_spacing.x = 4.0;
            for hz in ["500", "2300", "2750"] {
                let on = state.settings_form.bandwidth.trim() == hz;
                if ui.add_enabled(state.modem_connected, egui::SelectableLabel::new(on, RichText::new(hz).small().monospace()))
                    .on_hover_text(format!("VARA bandwidth {hz} Hz"))
                    .clicked() && !on
                {
                    state.set_bandwidth(hz);
                }
            }
        });
    }
    if let Some((s, _, until)) = state.slot_wait {
        let left = (until - Utc::now()).num_seconds().max(0);
        ui.horizontal(|ui| {
            theme::pill(ui, format!("Waiting on slot {s} for a reply · back to CF in {left}s"), theme::YELLOW);
            if ui.small_button("Return now").clicked() {
                state.return_to_cf();
            }
        });
    }
}

/// TUNE's window: the modem's TX drive level, changed live while the carrier is on (VarAC's
/// tune slider: DRIVELEVEL -30…0 dB), so the radio's ALC can be set; STOP ends the carrier.
pub fn draw_tune_window(ctx: &egui::Context, state: &mut GuiState) {
    if !state.tune_window {
        return;
    }
    let mut open = true;
    egui::Window::new("Tune")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_width(360.0)
        .show(ctx, |ui| {
            let on_air = state.ptt;
            ui.horizontal(|ui| {
                let (text, color) = if on_air { ("TRANSMITTING TUNE CARRIER", theme::RED) } else { ("NOT TRANSMITTING", theme::MUTED) };
                ui.label(RichText::new(text).strong().color(color));
            });
            ui.add_space(4.0);
            theme::caption(ui, "Modem drive level");
            let mut db = state.drive_level.unwrap_or(-5);
            let before = db;
            ui.horizontal(|ui| {
                if ui.add_enabled(db > -30, egui::Button::new(RichText::new(" - ").strong())).on_hover_text("1 dB down").clicked() {
                    db -= 1;
                }
                ui.add(egui::Slider::new(&mut db, -30..=0).suffix(" dB"));
                if ui.add_enabled(db < 0, egui::Button::new(RichText::new(" + ").strong())).on_hover_text("1 dB up").clicked() {
                    db += 1;
                }
            });
            if db != before {
                state.drive_level = Some(db);
                state.send_cmd(GuiCommand::DriveLevel(db));
            }
            if state.drive_level.is_none() {
                ui.label(RichText::new("(the modem hasn't said its level yet: moving it sets it)").small().italics().color(theme::MUTED));
            }
            ui.label(RichText::new("Watch the radio's ALC: lower the level until the ALC barely moves. \
                Set the output power on the radio. Use a clear frequency or a dummy load.").small().color(theme::MUTED));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if on_air {
                    if ui.add(theme::accent_button("STOP", theme::RED)).clicked() {
                        state.send_cmd(GuiCommand::TuneStop);
                    }
                } else if ui.add_enabled(state.modem_connected, theme::accent_button("TUNE AGAIN", theme::PURPLE)).clicked() {
                    state.send_cmd(GuiCommand::Tune);
                }
                if ui.button("Close").clicked() {
                    state.send_cmd(GuiCommand::TuneStop);
                    state.tune_window = false;
                }
            });
        });
    if !open {
        state.send_cmd(GuiCommand::TuneStop);
        state.tune_window = false;
    }
}

/// A small button with a painted triangle (egui's bundled fonts have no ▲▼ glyphs).
fn arrow_button(ui: &mut egui::Ui, enabled: bool, up: bool) -> egui::Response {
    let resp = ui.add_enabled(enabled, egui::Button::new("").min_size(Vec2::new(24.0, 20.0)));
    let c = resp.rect.center();
    let color = if enabled { ui.style().interact(&resp).fg_stroke.color } else { theme::tint(theme::MUTED, 90) };
    let (tip, base) = if up { (c.y - 3.0, c.y + 2.5) } else { (c.y + 3.0, c.y - 2.5) };
    let points = vec![egui::pos2(c.x - 4.5, base), egui::pos2(c.x + 4.5, base), egui::pos2(c.x, tip)];
    ui.painter().add(egui::Shape::convex_polygon(points, color, Stroke::NONE));
    resp
}

/// The small AUTO toggle beside CALL CQ and SEND BEACONS.
fn auto_toggle(ui: &mut egui::Ui, on: &mut bool, tip: String) {
    let text = RichText::new("AUTO").size(10.0).strong().color(if *on { theme::ACCENT } else { theme::MUTED });
    let mut btn = egui::Button::new(text).min_size(Vec2::new(40.0, 26.0));
    ui.spacing_mut().button_padding.x = 4.0;
    if *on {
        btn = btn.fill(theme::tint(theme::ACCENT, 34)).stroke(Stroke::new(1.0_f32, theme::tint(theme::ACCENT, 120)));
    }
    if ui.add(btn).on_hover_text(tip).clicked() {
        *on = !*on;
    }
}

/// VarAC's "CQ Slot selector": pick the slot you'll wait on after the CQ, then call.
pub fn draw_cq_slot_selector(ctx: &egui::Context, state: &mut GuiState) {
    use crate::varac::slots;
    if !state.cq_slot_selector_open {
        end_cq_sniff(state);
        return;
    }
    let step = state.slot_step();
    let cf = state.current_frequency;
    let mut open = true;
    let mut call = false;
    egui::Window::new("CQ slot selector")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label(RichText::new(
                "VarAC has one calling frequency per band. Your CQ names a slot around it where you'll \
                 wait for replies; after the CQ the radio moves there.",
            ).small().color(theme::MUTED));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("CQ type").strong());
                let mut ty = state.cq_type.clone();
                let custom = !matches!(ty.as_str(), "STANDARD" | "SOS" | "EMRG" | "HELP" | "CHAT" | "INFO" | "DATA");
                egui::ComboBox::from_id_salt("cq_type")
                    .selected_text(if custom { "Custom".to_string() } else { ty.clone() })
                    .show_ui(ui, |ui| {
                        for t in ["STANDARD", "SOS", "EMRG", "HELP", "CHAT", "INFO", "DATA"] {
                            ui.selectable_value(&mut ty, t.to_string(), t);
                        }
                        if ui.selectable_label(custom, "Custom").clicked() {
                            ty = String::new();
                        }
                    });
                if custom {
                    ui.add(TextEdit::singleline(&mut ty).desired_width(50.0).hint_text("4 chars"));
                    ty = ty.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == ' ').take(4).collect();
                }
                state.cq_type = ty;
            });
            if state.cq_type != "STANDARD" {
                ui.label(RichText::new(if state.settings_form.locator.trim().is_empty() {
                    "The call will include locator — set your locator in Settings first."
                } else {
                    "The call will include locator."
                }).small().color(theme::MUTED));
            }
            ui.add_space(4.0);
            ui.label(RichText::new("Select a slot").strong());
            // (VarAC: on a 60 m calling frequency only slots 1 and 11, to stay in the allocation)
            let on_60m_cf = crate::varac::messages::CALLING_FREQUENCIES_HZ.iter().any(|(f, b)| *f == cf && *b == "60m");
            ui.horizontal(|ui| {
                for s in slots::SLOTS_BY_FREQUENCY {
                    if on_60m_cf && !matches!(s, 0 | 1 | 11) {
                        continue;
                    }
                    let label = if s == 0 { "CF".to_string() } else { s.to_string() };
                    let sel = state.cq_slot == s;
                    let btn = egui::Button::new(RichText::new(label).strong()).selected(sel).min_size(Vec2::new(30.0, 26.0));
                    if ui.add(btn).on_hover_text(slots::label(s, step)).clicked() {
                        state.cq_slot = s;
                    }
                }
            });
            let f = slots::frequency(cf, state.cq_slot, step).unwrap_or(cf);
            ui.horizontal(|ui| {
                theme::caption(ui, "Calling frequency");
                ui.label(RichText::new(format!("{:.3}", cf as f64 / 1e6)).monospace());
                ui.add_space(12.0);
                theme::caption(ui, "Selected slot");
                ui.label(RichText::new(format!("{:.3}", f as f64 / 1e6)).monospace().strong().color(theme::YELLOW));
            });
            if state.cq_slot == 0 {
                ui.colored_label(theme::YELLOW, "Slot 0 = the calling frequency itself. QSOs on VarAC calling frequencies are prohibited.");
            } else {
                ui.colored_label(theme::MUTED, RichText::new("Make sure the slot is not out of band, and is free.").small());
            }
            if !state.rig_connected {
                ui.colored_label(theme::YELLOW, RichText::new("No rig control: you'll have to retune to the slot yourself after the CQ.").small());
            }
            if on_60m_cf {
                ui.colored_label(theme::RED, RichText::new("Some slots were disabled to avoid conflict with other band allocaitons").small());
            }
            // VarAC's slot sniffer: hold to listen on the slot (8 s); from 2 s on it shows when
            // the modem hears the slot busy. Beacons pause meanwhile; release = back to the CF.
            ui.horizontal(|ui| {
                let can = state.rig_connected && state.cq_slot != 0;
                let resp = ui.add_enabled(can, egui::Button::new("SLOT SNIFFER"));
                // Held = pressed on the button and the mouse button still down, wherever the
                // pointer is now (the window may shift under it as the status appears)
                let held = resp.is_pointer_button_down_on()
                    || (state.cq_sniff.is_some() && ui.input(|i| i.pointer.primary_down()));
                match (held, state.cq_sniff) {
                    (true, None) => {
                        state.cq_sniff = Some((cf, Utc::now(), false));
                        state.tune_to = Some(f);
                        state.add_log(format!("SLOT sniffer: listening on slot {} ({:.3} MHz)", state.cq_slot, f as f64 / 1e6));
                    }
                    (false, Some(_)) => end_cq_sniff(state),
                    _ => {}
                }
                // (fixed width, so the window doesn't change size while sniffing)
                ui.allocate_ui(Vec2::new(220.0, 20.0), |ui| ui.horizontal(|ui| match state.cq_sniff {
                    Some((_, since, heard_busy)) => {
                        let ms = (Utc::now() - since).num_milliseconds();
                        ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
                        if ms >= 2000 && state.busy && !heard_busy {
                            state.cq_sniff = Some((cf, since, true));
                        }
                        if ms < 8000 {
                            ui.add(egui::ProgressBar::new(1.0 - ms as f32 / 8000.0).desired_width(90.0));
                        } else {
                            ui.label(RichText::new("Done").small().color(theme::MUTED));
                        }
                        if ms >= 2000 && state.busy {
                            ui.label(RichText::new("FREQUENCY IS BUSY").strong().color(theme::RED));
                        } else if ms >= 2000 && !heard_busy {
                            ui.label(RichText::new("seems free").small().color(theme::GREEN));
                        }
                    }
                    None if !state.rig_connected => {
                        ui.label(RichText::new("Available only with CAT freq. control").small().color(theme::MUTED));
                    }
                    None if state.cq_slot == 0 => {
                        ui.label(RichText::new("Select a slot to check").small().color(theme::MUTED));
                    }
                    None => {
                        ui.label(RichText::new("Click and hold").small().color(theme::MUTED));
                    }
                }));
            });
            ui.add_space(6.0);
            let on_official_cf = crate::varac::messages::is_calling_frequency(cf);
            let typed_without_locator = state.cq_type != "STANDARD" && state.settings_form.locator.trim().is_empty();
            if ui.add_enabled(state.cq_sniff.is_none(), theme::accent_button("CALL CQ", theme::PURPLE)).clicked() {
                if state.cq_slot == 0 && on_official_cf {
                    state.tx_alert = Some(("Slot 0 selected".into(),
                        "You are on an official VarAC calling frequency, so you must choose a slot.".into()));
                } else if typed_without_locator {
                    state.tx_alert = Some(("Locator needed".into(),
                        "A CQ of this type includes your locator: set it in Settings → Station first.".into()));
                } else {
                    call = true;
                }
            }
        });
    if call {
        let slot = state.cq_slot;
        state.call_cq_on_slot(slot);
        state.cq_slot_selector_open = false;
    } else if !open {
        end_cq_sniff(state);
        state.cq_slot_selector_open = false;
    }
}

/// Let go of the CQ selector's sniffer: back to the calling frequency.
fn end_cq_sniff(state: &mut GuiState) {
    if let Some((cf, _, _)) = state.cq_sniff.take() {
        state.tune_to = Some(cf);
        state.add_log("SLOT sniffer: Changing frequency back to calling frequency".into());
    }
}

/// VarAC: "This station called CQ on a slot N minutes ago. Most stations won't wait for
/// over 5 minutes on a slot…"
pub fn draw_stale_cq_confirm(ctx: &egui::Context, state: &mut GuiState) {
    let Some((call, slot, mins)) = state.stale_cq_confirm.clone() else { return };
    let mut answer = None;
    egui::Window::new("Old CQ call")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label(format!(
                "{call} called CQ on slot {slot} {mins} minutes ago. Most stations won't wait for over 5 minutes on a slot. Connect anyway?"
            ));
            ui.horizontal(|ui| {
                if ui.add(theme::accent_button("CONNECT", theme::GREEN)).clicked() {
                    answer = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    answer = Some(false);
                }
            });
        });
    if let Some(go) = answer {
        state.stale_cq_confirm = None;
        if go {
            state.connect_on_slot(&call, slot);
        }
    }
}

fn rig_pill(ui: &mut egui::Ui, state: &mut GuiState) {
    let (color, tip) = match (&state.rig_backend, state.rig_connected) {
        (crate::rig::RigBackend::None, _) => (theme::MUTED, "No rig control — click to set up".to_string()),
        (be, true) => (theme::GREEN, format!("{be}{}", if state.rig_settings_ptt { " · PTT via CAT" } else { "" })),
        (be, false) => (theme::YELLOW, format!("{be}: {}", state.rig_error.as_deref().unwrap_or("connecting…"))),
    };
    let (label, color) = if state.rig_ptt { ("RIG TX", theme::TX_RED) } else { ("RIG", color) };
    if theme::status_pill(ui, label, color).interact(egui::Sense::click()).on_hover_text(tip).clicked() {
        state.settings_open = true;
        state.settings_form.settings_tab = 5;
    }
}

fn draw_callsign_box(ui: &mut egui::Ui, state: &mut GuiState, db: &Storage) {
    let resp = ui.add(
        TextEdit::singleline(&mut state.connect_callsign)
            .desired_width(ui.available_width())
            .font(egui::TextStyle::Monospace)
            .margin(Margin::symmetric(8.0, 4.0))
            .hint_text("Callsign…"),
    );
    resp.context_menu(|ui| {
        ui.label(RichText::new("Recent Stations").strong());
        ui.separator();
        if let Ok(qsos) = db.recent_qsos(50) {
            let mut seen = std::collections::HashSet::new();
            for qso in &qsos {
                if seen.insert(&qso.callsign) {
                    let label = match qso.name {
                        Some(ref name) => format!("{} ({})", qso.callsign, name),
                        None => qso.callsign.clone(),
                    };
                    if ui.button(&label).clicked() {
                        state.connect_callsign = qso.callsign.clone();
                        ui.close_menu();
                    }
                    if seen.len() >= 15 {
                        break;
                    }
                }
            }
            if seen.is_empty() {
                ui.label("No recent QSOs");
            }
        }
    });
    if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        try_connect(state);
    }
}

fn try_connect(state: &mut GuiState) {
    if state.connect_callsign.is_empty() || !state.modem_connected {
        if !state.modem_connected {
            state.status_msg = "Modem not connected".into();
        }
        return;
    }
    if state.refuse_wide_on_calling_freq() {
        return;
    }
    // VARA FM: "CALL VIA DIGI1 DIGI2" connects through digipeaters (VarAC lets FM users type
    // spaces there); everywhere else the callsign alone
    let typed = state.connect_callsign.trim().to_uppercase();
    let dest = if state.modem_type() == crate::vara::commands::ModemType::Fm {
        typed.split_whitespace().collect::<Vec<_>>().join(" ")
    } else {
        typed.split_whitespace().collect::<String>()
    };
    let call_only = dest.split(" VIA ").next().unwrap_or(&dest).to_string();
    if !messages::varac_callsign_ok(&call_only) {
        state.status_msg = format!("Invalid callsign: {dest}");
        return;
    }
    if call_only.split('-').next() == state.my_callsign.split('-').next() {
        state.status_msg = format!("{dest} is your own callsign");
        return;
    }
    state.add_log(format!("Connecting to {dest}..."));
    state.status_msg = format!("Connecting to {dest}...");
    state.connect_started = Some(Utc::now());
    state.last_connect_callsign = Some(dest.clone());
    state.send_cmd(GuiCommand::Connect(dest));
    state.connect_callsign.clear();
}

// ---------------------------------------------------------------- beacons / CQ

fn section_header(ui: &mut egui::Ui, title: &str, count: usize, color: Color32) {
    ui.label(RichText::new(title).strong().size(12.5).color(color));
    if count > 0 {
        theme::pill(ui, count.to_string(), color);
    }
}

fn draw_beacons_card(ui: &mut egui::Ui, state: &mut GuiState, db: &Storage) {
    theme::card().show(ui, |ui| {
        ui.set_min_size(ui.available_size().max(Vec2::ZERO));
        ui.horizontal(|ui| {
            section_header(ui, "BEACONS", state.beacon_entries.len(), BEACON_PURPLE);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if !state.beacon_entries.is_empty() && ui.small_button("Clear").clicked() {
                    state.beacon_entries.clear();
                }
            });
        });
        let table_h = (ui.available_height() - 40.0).max(60.0);
        ScrollArea::vertical()
            .id_salt("beacons_scroll")
            .max_height(table_h)
            .min_scrolled_height(table_h)
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                let my_loc = state.settings_form.locator.clone();
                match draw_station_table(ui, &state.beacon_entries, &state.callsign_tags, &my_loc, "", db) {
                    Some(StationAction::ClearList) => state.beacon_entries.clear(),
                    Some(action) => handle_station_action(action, state),
                    None => {}
                }
            });

        // "In QSO with / Duration / QRZ? / History" row, under the beacon list as in VarAC
        ui.with_layout(Layout::bottom_up(Align::LEFT), |ui| {
            ui.horizontal(|ui| {
                theme::caption(ui, "In QSO with");
                let call = state.connected_to.clone();
                match call {
                    Some(ref c) => {
                        ui.label(RichText::new(c).strong().size(15.0).color(theme::GREEN));
                        if let Some(start) = state.connection_start {
                            let secs = (Utc::now() - start).num_seconds().max(0);
                            ui.label(RichText::new(format!("{}:{:02}", secs / 60, secs % 60)).monospace().color(theme::MUTED));
                        }
                    }
                    None => {
                        ui.label(RichText::new("—").color(theme::MUTED));
                    }
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.small_button("History").on_hover_text("QSO log").clicked() {
                        state.qso_log_open = true;
                    }
                    let qrz_target = call.or_else(|| (!state.connect_callsign.is_empty()).then(|| state.connect_callsign.to_uppercase()));
                    if ui.add_enabled(qrz_target.is_some(), egui::Button::new("QRZ?").small()).clicked() {
                        if let Some(c) = qrz_target {
                            open_qrz_lookup(&c);
                        }
                    }
                });
            });
        });
    });
}

fn draw_cq_card(ui: &mut egui::Ui, state: &mut GuiState, db: &Storage) {
    theme::card().show(ui, |ui| {
        ui.set_min_size(ui.available_size().max(Vec2::ZERO));
        ui.horizontal(|ui| {
            section_header(ui, "CQ CALLS", state.cq_entries.len(), CYAN);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if !state.cq_entries.is_empty() && ui.small_button("Clear").clicked() {
                    state.cq_entries.clear();
                }
                ui.add(TextEdit::singleline(&mut state.cq_filter).desired_width(80.0).hint_text("Filter…"));
            });
        });
        let table_h = (ui.available_height() - 40.0).max(60.0);
        ScrollArea::vertical()
            .id_salt("cq_scroll")
            .max_height(table_h)
            .min_scrolled_height(table_h)
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                let my_loc = state.settings_form.locator.clone();
                let filt = state.cq_filter.clone();
                match draw_station_table(ui, &state.cq_entries, &state.callsign_tags, &my_loc, &filt, db) {
                    Some(StationAction::ClearList) => state.cq_entries.clear(),
                    Some(action) => handle_station_action(action, state),
                    None => {}
                }
            });

        // SNR: Last / Avg / Mine (under CQ calls, as in VarAC)
        ui.with_layout(Layout::bottom_up(Align::LEFT), |ui| {
            ui.horizontal(|ui| {
                let snr_box = |ui: &mut egui::Ui, label: &str, v: Option<f32>, tip: &str| {
                    theme::caption(ui, label);
                    let (text, c) = match v {
                        Some(v) => (
                            format!("{v:+.0}"),
                            if v >= 10.0 { theme::GREEN } else if v >= 0.0 { theme::YELLOW } else { theme::RED },
                        ),
                        None => ("—".into(), theme::MUTED),
                    };
                    Frame::none()
                        .fill(theme::SURFACE_2)
                        .rounding(egui::Rounding::same(6.0))
                        .inner_margin(Margin::symmetric(8.0, 2.0))
                        .show(ui, |ui| {
                            ui.set_min_width(30.0);
                            ui.label(RichText::new(text).monospace().strong().color(c));
                        })
                        .response
                        .on_hover_text(tip);
                };
                theme::caption(ui, "SNR");
                if let Some(last) = state.snr_last {
                    theme::snr_meter(ui, last);
                }
                snr_box(ui, "Last", state.snr_last, "Last SNR of the other station (dB)");
                snr_box(ui, "Avg", state.snr_avg, "Average SNR this QSO (dB)");
                snr_box(ui, "Mine", state.snr_mine, "How the other station hears you (dB)");
            });
        });
    });
}

// ---------------------------------------------------------------- broadcasts row

fn draw_broadcasts_card(ui: &mut egui::Ui, state: &mut GuiState) {
    theme::card().inner_margin(Margin::symmetric(10.0, 4.0)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            section_header(ui, "BROADCASTS", state.broadcasts.len(), YELLOW);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if !state.broadcasts.is_empty() && ui.small_button("Clear").clicked() {
                    state.broadcasts.clear();
                }
            });
        });
        ScrollArea::vertical()
            .id_salt("bc_scroll")
            .max_height((ui.available_height() - 4.0).max(20.0))
            .auto_shrink([false, true])
            .show(ui, |ui| {
                let bc_snapshot: Vec<_> = state.broadcasts.iter().rev().take(50)
                    .map(|bc| (bc.time.clone(), bc.from.clone(), bc.message.clone(), bc.to.clone(), bc.via.clone(), bc.qsy_hz, bc.alert))
                    .collect();
                for (time, from, message, to, via, qsy_hz, alert) in &bc_snapshot {
                    let resp = ui.horizontal(|ui| {
                        ui.label(RichText::new(time).small().monospace().color(theme::MUTED));
                        ui.label(RichText::new(from).strong().color(callsign_color(from)));
                        // Addressed ones show the addressee (and digipeaters), as VarAC's To/Via columns
                        if !to.eq_ignore_ascii_case("ALL") || via.is_some() {
                            let mut addr = format!("→ {to}");
                            if let Some(v) = via {
                                addr.push_str(&format!(" via {v}"));
                            }
                            ui.label(RichText::new(addr).small().color(theme::MUTED));
                        }
                        match alert {
                            Some(((fr, fg, fb), (br, bg, bb))) => ui.label(RichText::new(message)
                                .color(Color32::from_rgb(*fr, *fg, *fb)).background_color(Color32::from_rgb(*br, *bg, *bb))),
                            None => ui.label(RichText::new(message).color(theme::TEXT)),
                        };
                        if let Some(f) = qsy_hz {
                            if ui.small_button("QSY").on_hover_text(format!("Go to {:.3} MHz", *f as f64 / 1e6)).clicked() {
                                state.current_frequency = *f;
                                state.tune_to = Some(*f);
                                state.add_log(format!("QSY to {:.3} MHz (broadcast)", *f as f64 / 1e6));
                            }
                        }
                    }).response;
                    resp.context_menu(|ui| {
                        if ui.button(format!("Connect to {from}")).clicked() && state.modem_connected {
                            state.add_log(format!("Connecting to {from}..."));
                            state.status_msg = format!("Connecting to {from}...");
                            state.connect_started = Some(Utc::now());
                            state.send_cmd(GuiCommand::Connect(from.clone()));
                            ui.close_menu();
                        }
                        if ui.button(format!("Send VMail to {from}")).clicked() {
                            state.compose_open = true;
                            state.compose_to = from.clone();
                            ui.close_menu();
                        }
                        if ui.button("Copy message").clicked() {
                            ui.output_mut(|o| o.copied_text = format!("{from}: {message}"));
                            ui.close_menu();
                        }
                        if ui.button(format!("Lookup {from} on QRZ")).clicked() {
                            open_qrz_lookup(from);
                            ui.close_menu();
                        }
                        let mut action = None;
                        station_menu_extras(ui, from, &mut action);
                        if let Some(a) = action {
                            handle_station_action(a, state);
                        }
                    });
                }
                if state.broadcasts.is_empty() {
                    ui.label(RichText::new("No broadcasts heard yet").small().italics().color(theme::MUTED));
                }
            });
    });
}

// ---------------------------------------------------------------- right-hand column

const BTN_H: f32 = 24.0;

/// VarAC's right-hand column, top to bottom: the options ("Send 'is typing'", "I'm away"…),
/// QSY, SEND VMAIL, BROADCAST, SEND FILE, PATH FINDER, SNR | INFO; and at the bottom,
/// Auto log QSO, LOG, the canned messages, Enter to send and SEND. Nothing in it scrolls.
fn draw_side_column(ui: &mut egui::Ui, state: &mut GuiState) {
    let connected = state.session_state == SessionState::Connected;
    // Height of the LOG … SEND group at the bottom, measured last frame
    let bottom_id = ui.id().with("side_bottom_h");
    let bottom_h = ui.ctx().data(|d| d.get_temp::<f32>(bottom_id)).unwrap_or(140.0);
    ui.spacing_mut().item_spacing = Vec2::new(4.0, 4.0);
    ui.spacing_mut().button_padding.x = 4.0;
    let w = ui.available_width();
    let half = Vec2::new((w - 4.0) / 2.0, BTN_H);
    let label = |text: &str| RichText::new(text).size(12.5);

    draw_options_card(ui, state);
    ui.add_space(4.0);

    // Offers waiting on an answer go first, where the eye already is
    if state.pending_qsy.is_some() {
        ui.horizontal(|ui| {
            if ui.add_sized(half, theme::primary_button(label("ACCEPT QSY"), theme::GREEN)).clicked() {
                state.send_cmd(GuiCommand::QsyAccept);
                state.pending_qsy = None;
                state.status_msg = "QSY accepted — change frequency".into();
            }
            if ui.add_sized(half, theme::accent_button(label("REJECT"), theme::RED)).clicked() {
                state.send_cmd(GuiCommand::QsyReject);
                state.pending_qsy = None;
                state.status_msg = "QSY rejected".into();
            }
        });
    }
    if state.pending_file_offer.is_some() && !state.pending_vmail_attachment {
        ui.horizontal(|ui| {
            if state.file_accepted {
                ui.add_enabled(false, egui::Button::new(label("Receiving…")).min_size(half));
            } else if ui.add_sized(half, theme::primary_button(label("ACCEPT FILE"), theme::GREEN)).clicked() {
                // One <SFRD> per offer (a second click sent it twice)
                state.file_accepted = true;
                state.send_cmd(GuiCommand::FileReady);
            }
            if ui.add_sized(half, theme::accent_button(label("DECLINE"), theme::RED)).clicked() {
                state.pending_file_offer = None;
                state.send_cmd(GuiCommand::FileAbort);
            }
        });
    }

    // VarAC gives each of these a row of its own. When the window is too short for that,
    // pair them up from the bottom (SEND FILE | PATH FINDER first), only as many as needed.
    let row = BTN_H + 4.0;
    let spare = ui.available_height() - 2.0 * row - bottom_h - 4.0;
    let rows_fit = (spare / row).floor().max(0.0) as usize;
    let pairs = SIDE_BUTTONS.saturating_sub(rows_fit).min(SIDE_BUTTONS / 2);
    let singles = SIDE_BUTTONS - 2 * pairs;
    // Still short with everything paired (a 768 px screen): make these rows lower rather
    // than run into LOG / SEND below, by the overlap measured last frame
    let rows = (singles + pairs) as f32 + 2.0 + if state.settings_form.emcomm_mode { 1.0 } else { 0.0 };
    let shrink_id = ui.id().with("side_shrink");
    let shrink = ui.ctx().data(|d| d.get_temp::<f32>(shrink_id)).unwrap_or(0.0);
    let base = ui.spacing().interact_size.y;
    let bh = (base - shrink).clamp(18.0, base);
    let full = Vec2::new(w, bh);
    let half = Vec2::new((w - 4.0) / 2.0, bh);
    // (a horizontal row is at least interact_size high, whatever its buttons ask for)
    let (saved_interact, saved_pad) = (ui.spacing().interact_size.y, ui.spacing().button_padding.y);
    ui.spacing_mut().interact_size.y = bh;
    if bh < base {
        ui.spacing_mut().button_padding.y = ((bh - 16.0) / 2.0).clamp(1.0, saved_pad);
    }
    // VarAC shows its check-in request button only in EmComm mode
    if state.settings_form.emcomm_mode {
        if ui.add_sized(full, theme::accent_button(RichText::new("CHECK-IN REQUEST").size(12.5), theme::RED))
            .on_hover_text("Ask EmComm stations to beacon so you see who is on (<EC> to EMCOMM)")
            .clicked()
        {
            let dial = state.dial();
            if crate::varac::messages::is_calling_frequency(dial) {
                state.status_msg = "Can not send a check-in request on a VarAC CF".into();
            } else {
                state.send_cmd(GuiCommand::EmCommCheckIn);
                state.status_msg = "EmComm check-in request queued".into();
            }
        }
    }
    for i in 0..singles {
        side_button(ui, state, i, full, connected);
    }
    for i in (singles..SIDE_BUTTONS).step_by(2) {
        ui.horizontal(|ui| {
            side_button(ui, state, i, half, connected);
            side_button(ui, state, i + 1, half, connected);
        });
    }

    // In-QSO requests, paired as in VarAC (SNR | INFO, LH.P | FS.P)
    ui.horizontal(|ui| {
        if centered_button(ui, connected, half, label("SNR"))
            .on_hover_text("Ask for a signal report").clicked()
        {
            state.send_cmd(GuiCommand::SnrRequest);
        }
        if centered_button(ui, connected, half, label("INFO"))
            .on_hover_text("Ask for their station info").clicked()
        {
            state.send_cmd(GuiCommand::InfoRequest);
        }
    });
    ui.horizontal(|ui| {
        if centered_button(ui, connected, half, label("BBS LIST"))
            .on_hover_text("List the messages on their BBS").clicked()
        {
            match state.bbs_refusal(false) {
                Some(why) => state.tx_alert = Some(why),
                None => state.send_cmd(GuiCommand::RequestBbsList),
            }
        }
        // VarAC's INQUIRE window, as a menu: questions the partner's VarAC answers itself
        ui.add_enabled_ui(connected, |ui| {
            egui::menu::menu_custom_button(ui, egui::Button::new(label("INQUIRE…")).min_size(half), |ui| {
                let ask = |ui: &mut egui::Ui, label: &str, tip: &str, tag: String| {
                    if ui.button(label).on_hover_text(tip).clicked() {
                        state.send_cmd(GuiCommand::SendInquiry(tag));
                        ui.close_menu();
                    }
                };
                ask(ui, "Last heard (all)", "Stations they heard in the last 24 h (<LHR>)", "<LHR>".into());
                let call = state.connect_callsign.trim().to_uppercase();
                if !call.is_empty() {
                    ask(ui, &format!("Last heard {call}"), "When did they last hear this station (<LHC:>)", format!("<LHC:{call}>"));
                }
                ask(ui, "Parked VMail status", "Were your VMails parked there relayed? (<VRP>)", "<VRP>".into());
                ask(ui, "Locator", "Their locator (<LOCR>)", "<LOCR>".into());
                ask(ui, "GPS position", "Their GPS position, if they share it (<GPSR>)", "<GPSR>".into());
                ask(ui, "Version", "Their software and modem versions (<VER>)", "<VER>".into());
                ask(ui, "Frequency schedule", "Their frequency scheduler plan (<FSR>)", "<FSR>".into());
                ask(ui, "Last connections (EmComm)", "Their last 5 QSOs; they must be in EmComm mode (<LCR>)", "<LCR>".into());
                if ui.button("Email gateway address").on_hover_text("<EGA>").clicked() {
                    state.send_cmd(GuiCommand::GatewayAddressRequest);
                    ui.close_menu();
                }
            });
        });
    });

    ui.spacing_mut().interact_size.y = saved_interact;
    ui.spacing_mut().button_padding.y = saved_pad;
    let top_end = ui.cursor().top();
    // Bottom of the column, beside the QSO row and the message box
    let bottom = ui.with_layout(Layout::bottom_up(Align::LEFT), |ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        if ui.add_sized([w, 30.0], theme::primary_button("SEND", theme::GREEN)).clicked() {
            state.send_requested = true;
        }
        ui.spacing_mut().interact_size.y = 20.0;
        let mut enter_to_send = !state.multiline_input;
        if ui.checkbox(&mut enter_to_send, label("Enter to send"))
            .on_hover_text("Off: Enter makes a new line, Ctrl+Enter sends")
            .changed()
        {
            state.multiline_input = !enter_to_send;
        }
        ui.spacing_mut().interact_size.y = BTN_H;
        egui::ComboBox::from_id_salt("canned_pick")
            .selected_text(RichText::new("Canned msg (F1–F12)").size(12.5))
            .width(w)
            .show_ui(ui, |ui| {
                for i in 0..12 {
                    let cm = &state.canned_messages[i];
                    if cm.text.is_empty() {
                        continue;
                    }
                    let label = if cm.label.is_empty() { format!("F{}", i + 1) } else { format!("F{}  {}", i + 1, cm.label) };
                    if ui.add_enabled(connected, egui::SelectableLabel::new(false, label)).clicked() {
                        pick_canned(state, i);
                    }
                }
                ui.separator();
                if ui.button("Edit canned messages…").clicked() {
                    state.canned_editor_open = true;
                }
            });
        let tip = if state.unlogged_qso.is_some() { "Write this QSO to the log" } else { "Open the QSO log" };
        let log_btn = if state.unlogged_qso.is_some() { theme::accent_button(label("LOG QSO"), theme::GREEN) } else { egui::Button::new(label("LOG")) };
        if ui.add_sized(full, log_btn).on_hover_text(tip).clicked() {
            if state.unlogged_qso.is_some() {
                state.log_qso_requested = true;
            } else {
                state.qso_log_open = true;
            }
        }
        ui.spacing_mut().interact_size.y = 20.0;
        if ui.checkbox(&mut state.auto_log_qso, label("Auto log QSO")).changed() {
            state.settings_form.auto_log_qso = state.auto_log_qso;
        }
    });
    ui.ctx().data_mut(|d| d.insert_temp(bottom_id, bottom.response.rect.height()));
    // Rows above overlapping the bottom group: lower them next frame (and give it back
    // when there's room again)
    let overlap = top_end - bottom.response.rect.top();
    let next = if overlap > 0.5 { shrink + overlap / rows } else { (shrink + (overlap + 2.0) / rows).max(0.0).min(shrink) };
    if (next - shrink).abs() > 0.1 {
        ui.ctx().data_mut(|d| d.insert_temp(shrink_id, next.clamp(0.0, base - 18.0)));
        ui.ctx().request_repaint();
    }
}

const SIDE_BUTTONS: usize = 6;

/// A fixed-size button with its label centred, enabled or not.
fn centered_button(ui: &mut egui::Ui, enabled: bool, size: Vec2, text: RichText) -> egui::Response {
    ui.add_enabled_ui(enabled, |ui| ui.add_sized(size, egui::Button::new(text))).inner
}

/// The right-hand column's buttons in VarAC's order: QSY, SEND VMAIL, BROADCAST,
/// VMAIL (folders), SEND FILE, PATH FINDER.
fn side_button(ui: &mut egui::Ui, state: &mut GuiState, i: usize, size: Vec2, connected: bool) {
    let label = |text: &str| RichText::new(text).size(12.5);
    match i {
        0 => {
            if centered_button(ui, connected && state.pending_qsy.is_none(), size, label("QSY"))
                .on_hover_text("Ask the other station to change frequency")
                .clicked()
            {
                state.qsy_invite_open = Some(Utc::now());
                state.qsy_invite_mid = true;
                state.qsy_invite_target = None;
            }
        }
        1 => {
            if ui.add_sized(size, theme::accent_button(label("SEND VMAIL"), theme::YELLOW)).clicked() {
                // VarAC: To = the station we're connected to
                if state.compose_to.is_empty() && state.compose_editing.is_none() {
                    if let Some(c) = state.connected_to.clone().filter(|_| state.session_state == SessionState::Connected) {
                        state.compose_to = c;
                    }
                }
                state.compose_open = true;
            }
        }
        2 => {
            if ui.add_sized(size, theme::accent_button(label("BROADCAST"), theme::YELLOW)).clicked() {
                state.broadcast_compose_open = true;
            }
        }
        3 => {
            let text = if state.unread_vmails > 0 { format!("VMAIL ({})", state.unread_vmails) } else { "VMAIL".into() };
            let btn = if state.unread_vmails > 0 { theme::accent_button(label(&text), theme::YELLOW) } else { egui::Button::new(label(&text)) };
            if ui.add_sized(size, btn).on_hover_text("VMail folders").clicked() {
                state.vmail_window_open = !state.vmail_window_open;
            }
        }
        4 => {
            if centered_button(ui, connected, size, label("SEND FILE")).clicked() {
                state.file_send_open = true;
                state.file_path_input.clear();
            }
        }
        _ => {
            if ui.add_sized(size, egui::Button::new(label("PATH FINDER"))).on_hover_text("Find a relay path to a station").clicked() {
                state.path_finder_open = true;
            }
        }
    }
}

/// The option checkboxes at the top of VarAC's right-hand column.
fn draw_options_card(ui: &mut egui::Ui, state: &mut GuiState) {
    theme::card().inner_margin(Margin::symmetric(8.0, 6.0)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 2.0;
        ui.spacing_mut().interact_size.y = 20.0;
        let label = |text: &str| RichText::new(text).size(12.5);

        ui.checkbox(&mut state.is_typing_enabled, label("Send 'is typing'"));
        let was_away = state.away_active;
        ui.checkbox(&mut state.away_active, label("I'm away"));
        if state.away_active {
            ui.add(TextEdit::singleline(&mut state.away_message).desired_width(f32::INFINITY).hint_text("Away message"));
        }
        if state.away_active != was_away {
            // (VarAC: away counts as idle for the auto-away time; back counts as activity)
            let mins: i64 = state.settings_form.auto_away_minutes.parse().unwrap_or(10);
            state.last_activity = if state.away_active { Utc::now() - chrono::Duration::minutes(mins) } else { Utc::now() };
            if state.away_active {
                // Some(..) = away (None means "back"), so an empty message becomes "Away"
                let msg = if state.away_message.trim().is_empty() { "Away".to_string() } else { state.away_message.clone() };
                state.status_msg = "Away status active".into();
                state.send_cmd(GuiCommand::SetAway(Some(msg)));
            } else {
                state.away_message.clear();
                state.status_msg = "Away status cleared".into();
                state.send_cmd(GuiCommand::SetAway(None));
            }
        }
        if ui.checkbox(&mut state.unattended_links, label("Unattended links"))
            .on_hover_text("While away, the radio may answer links and send automatic replies (beacons are always allowed)")
            .changed()
        {
            let on = state.unattended_links;
            state.send_cmd(GuiCommand::SetUnattended(on));
            crate::config::update_config_file(|c| c.unattended_links = Some(on));
        }
        let connected = state.session_state == SessionState::Connected;
        let mut vsnr = state.verbose_snr;
        if ui.add_enabled(connected || vsnr, egui::Checkbox::new(&mut vsnr, label("Verbose SNR")))
            .on_hover_text("Exchange SNR reports every minute with your partner (invites them with <VSI>)")
            .changed()
        {
            state.verbose_snr = vsnr;
            state.send_cmd(GuiCommand::VerboseSnr(vsnr));
        }
    });
}

// ---------------------------------------------------------------- QSO log row

/// VarAC's QSO log row: CALLSIGN, SNR-S / SNR-R / BAND (pick lists), NAME, LOC, QTH,
/// MyPWR, START / END (with "now"), CLR. It fills itself during the QSO and is what LOG
/// (or Auto log at the end of the QSO) writes to the ADIF log and loggers.
fn draw_qso_row(ui: &mut egui::Ui, state: &mut GuiState) {
    const SNRS: [&str; 51] = [
        "+25", "+24", "+23", "+22", "+21", "+20", "+19", "+18", "+17", "+16", "+15", "+14", "+13", "+12", "+11", "+10",
        "+09", "+08", "+07", "+06", "+05", "+04", "+03", "+02", "+01", "+00", "-01", "-02", "-03", "-04", "-05", "-06",
        "-07", "-08", "-09", "-10", "-11", "-12", "-13", "-14", "-15", "-16", "-17", "-18", "-19", "-20", "-21", "-22",
        "-23", "-24", "-25",
    ];
    const BANDS: [&str; 19] = [
        "1.2cm", "3cm", "5cm", "13cm", "23cm", "70cm", "2m", "6m", "8m", "10m", "12m", "15m", "17m", "20m", "30m",
        "40m", "60m", "80m", "160m",
    ];
    let column = |ui: &mut egui::Ui, label: &str, add: &mut dyn FnMut(&mut egui::Ui)| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            theme::caption(ui, label);
            add(ui);
        });
    };
    let edit = |ui: &mut egui::Ui, v: &mut String, w: f32| {
        ui.add(TextEdit::singleline(v).desired_width(w).margin(Margin::symmetric(4.0, 3.0)));
    };
    let pick = |ui: &mut egui::Ui, id: &str, v: &mut String, items: &[&str], w: f32| {
        egui::ComboBox::from_id_salt(id).width(w).selected_text(v.clone()).show_ui(ui, |ui| {
            for it in items {
                ui.selectable_value(v, it.to_string(), *it);
            }
        });
    };
    let now = || Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 5.0;
        let fixed = 88.0 + 58.0 * 2.0 + 64.0 + 100.0 + 58.0 + 40.0 + 150.0 * 2.0 + 36.0 + 8.0 * 18.0;
        let qth_w = (ui.available_width() - fixed).max(60.0);
        let l = &mut state.qso_log;
        column(ui, "Callsign", &mut |ui| edit(ui, &mut l.call, 80.0));
        column(ui, "SNR-S", &mut |ui| pick(ui, "qso_snr_s", &mut l.snr_sent, &SNRS, 46.0));
        column(ui, "SNR-R", &mut |ui| pick(ui, "qso_snr_r", &mut l.snr_rcvd, &SNRS, 46.0));
        column(ui, "Band", &mut |ui| pick(ui, "qso_band", &mut l.band, &BANDS, 52.0));
        column(ui, "Name", &mut |ui| edit(ui, &mut l.name, 92.0));
        column(ui, "Loc", &mut |ui| edit(ui, &mut l.locator, 50.0));
        column(ui, "QTH", &mut |ui| edit(ui, &mut l.qth, qth_w));
        column(ui, "MyPWR", &mut |ui| {
            edit(ui, &mut l.power, 32.0);
            l.power.retain(|c| c.is_ascii_digit() || c == '.');
        });
        column(ui, "Start", &mut |ui| ui.horizontal(|ui| {
            edit(ui, &mut l.start, 118.0);
            if ui.small_button("now").on_hover_text("Set the QSO start time to now (UTC)").clicked() {
                l.start = now();
            }
        }).response.mark_changed());
        column(ui, "End", &mut |ui| ui.horizontal(|ui| {
            edit(ui, &mut l.end, 118.0);
            if ui.small_button("now").on_hover_text("Manually set QSO end time to NOW").clicked() {
                l.end = now();
            }
        }).response.mark_changed());
        column(ui, " ", &mut |ui| {
            if ui.small_button("CLR").on_hover_text("Clear the QSO fields").clicked() {
                *l = crate::gui::state::QsoLogFields::default();
            }
        });
    });
}

// ---------------------------------------------------------------- composer

/// The reply being composed, if any: (wire prefix `<C:n>`, quote for our line, n).
fn take_reply(state: &mut GuiState) -> (String, Option<String>, Option<u32>) {
    match state.reply_to_chat.take() {
        Some((id, quote)) => (format!("<C:{id}>"), Some(quote), Some(id)),
        None => (String::new(), None, None),
    }
}

/// A canned message, the VarAC way: its text goes into the message box (after what's
/// there); with `<SND>` in it, it's sent at once. (`<DISC>`: disconnect once it's sent.)
pub(super) fn pick_canned(state: &mut GuiState, i: usize) {
    let Some(cm) = state.canned_messages.get(i).cloned() else { return };
    if cm.text.is_empty() {
        return;
    }
    if !state.input_text.is_empty() {
        state.input_text.push('\n');
    }
    state.input_text.push_str(&cm.text.replace("//n", "\n"));
    if cm.disconnect_after && !state.input_text.contains("<DISC>") {
        state.input_text.push_str("<SND><DISC>"); // (Openslot's older "disconnect after" box)
    }
    if state.input_text.contains("<SND>") {
        state.input_text = state.input_text.replace("<SND>", "");
        if state.session_state == SessionState::Connected {
            state.send_requested = true;
        }
    }
}

fn draw_composer(ui: &mut egui::Ui, state: &mut GuiState, db: &Storage, qso_guid: &Option<String>) {
    let mut do_send = std::mem::take(&mut state.send_requested);
    if state.compose_open {
        draw_compose_window(ui, state, db);
        return;
    }
    let connected = state.session_state == SessionState::Connected;

    // VarAC's "REPLYING TO:" box above the input
    if let Some((_, quote)) = state.reply_to_chat.clone() {
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("REPLYING TO: {quote}")).small().color(theme::ACCENT));
            if ui.small_button("✕").on_hover_text("Cancel the reply").clicked() {
                state.reply_to_chat = None;
            }
        });
    }
    ui.horizontal_top(|ui| {
        // desired_width excludes the TextEdit's own margins (2×10px)
        let text_w = (ui.available_width() - 20.0).max(100.0);
        let hint = if connected {
            if state.multiline_input { "Type a message · Ctrl+Enter to send" } else { "Type a message · Enter to send" }
        } else {
            "Not connected — type /help for commands"
        };
        let resp = ui.add(
            TextEdit::multiline(&mut state.input_text)
                .desired_width(text_w)
                .desired_rows(3)
                .margin(Margin::symmetric(10.0, 8.0))
                .hint_text(hint),
        );
        // VarAC: "is typing" when the text changes, is over 10 characters, once a minute at
        // most, and only with nothing waiting to go out
        // (and, with SuppressIsTypingInLowSNR, not when either side is below -14 dB)
        let low_snr = state.settings_form.suppress_typing_low_snr
            && state.snr_last.into_iter().chain(state.snr_mine).any(|s| s < -14.0);
        if resp.changed() && connected && state.is_typing_enabled && state.input_text.chars().count() > 10
            && state.queue_count == 0 && state.buffer_bytes == 0 && !low_snr
        {
            let due = state.last_typing_sent.map(|t| (Utc::now() - t).num_seconds() >= 60).unwrap_or(true);
            if due {
                state.last_typing_sent = Some(Utc::now());
                state.send_cmd(GuiCommand::SendIsTyping);
            }
        }
        if resp.changed() {
            state.last_activity = Utc::now();
        }
        // "Enter to send" (VarAC's checkbox) = single-line behaviour; otherwise Ctrl+Enter
        if resp.has_focus() {
            let (enter, ctrl) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.modifiers.ctrl));
            if enter && (ctrl || !state.multiline_input) {
                // Drop the newline the TextEdit just inserted
                if state.input_text.ends_with('\n') {
                    state.input_text.pop();
                }
                do_send = true;
            }
        }
    });

    // VarAC: no messages on a ping link (the text stays in the box)
    if do_send && connected && state.ping_active && !state.input_text.trim().is_empty() {
        do_send = false;
        state.tx_alert = Some((
            "Can't send during ping".into(),
            "You are currently in a ping session.\n\nMessaging is disabled while you are performing a ping or being pinged by another station.".into(),
        ));
    }
    if do_send && !state.input_text.trim().is_empty() {
        let typed = std::mem::take(&mut state.input_text);
        // VarAC expands its tags in everything it sends (<NAME>, <RIG>, <HCALL>, <UTCDT> …)
        let mut text = if connected { state.expand_tags(&typed) } else { typed };
        // AI MODE (VarAC): everything we type goes to the partner's AI gateway
        if connected && state.ai_mode && !text.trim_start().to_uppercase().starts_with("AI:") {
            text = format!("AI:{text}");
        }
        if connected {
            let disconnect = text.contains("<DISC>");
            let (prefix, quote, reply_id) = take_reply(state);
            // (VarAC sends the message box's line breaks as they are: CRLF)
            state.send_cmd(GuiCommand::SendChat(format!("{prefix}{}", text.replace("\r\n", "\n").replace('\n', "\r\n"))));
            crate::gui::sound::play(crate::gui::sound::Sound::OutgoingChat);
            // Our own lines with an alert tag go to the Alert center's Sent folder (VarAC)
            for t in crate::varac::alerts::matches(&state.alert_tags, crate::varac::alerts::Source::Datastream, &[&text]) {
                let _ = db.insert_alert(&crate::storage::AlertRecord {
                    guid: uuid::Uuid::new_v4().to_string(),
                    source_id: 4,
                    from_callsign: state.my_callsign.clone(),
                    to_callsign: state.connected_to.clone().unwrap_or_default(),
                    alert_tag: t.tag.clone(),
                    alert_name: t.description.clone(),
                    source_text: text.clone(),
                    alert_time: Utc::now(),
                    read: true,
                    folder_id: 3,
                    frequency: state.dial() as i64,
                });
            }
            let ts = state.format_time();
            state.add_chat(ChatLine {
                time: ts,
                callsign: state.my_callsign.clone(),
                text: text.clone(),
                is_outgoing: true,
                is_system: false,
                reply_to: reply_id.map(|i| i.to_string()),
                reply_quote: quote,
                tx: Some(crate::gui::state::TxStage::Queued),
                ..Default::default()
            });
            if let Some(guid) = qso_guid {
                let _ = db.insert_chat(&crate::storage::ChatRecord {
                    guid: uuid::Uuid::new_v4().to_string(),
                    entry_type: 2,
                    qso_guid: guid.clone(),
                    callsign: state.my_callsign.clone(),
                    entry: text,
                    creation_time: Utc::now(),
                    chat_id: None,
                    reply_on_chat_id: reply_id.map(i64::from),
                });
            }
            // <DISC> goes out with the message; then the link is closed (VarAC)
            // (VarAC's <DISC> tag: the message goes out, then the link ends)
            if disconnect {
                state.send_cmd(GuiCommand::DisconnectAfterSent);
            }
        } else if text.starts_with('/') {
            handle_chat_slash_command(state, &text);
        } else {
            state.input_text = text;
            state.status_msg = "Not connected — use Connect first".into();
        }
    }
}

// ---------------------------------------------------------------- status bar

/// VarAC's "PTT Disabled": listen only. Switching it on pauses beacons, ends a CQ and
/// stops a tune, as VarAC does; it's saved at once.
fn set_ptt_disabled(state: &mut GuiState, on: bool) {
    state.ptt_disabled = on;
    state.send_cmd(GuiCommand::SetPttDisabled(on));
    crate::config::update_config_file(|c| c.ptt_disabled = Some(on));
    if on {
        state.cq_active = false;
        state.send_cmd(GuiCommand::TuneStop);
    }
    state.status_msg = if on { "PTT disabled: listening only".into() } else { "PTT enabled".into() };
}

fn draw_status_bar(ctx: &egui::Context, state: &mut GuiState) {
    egui::TopBottomPanel::bottom("status_bar")
        .frame(Frame::none().fill(theme::SURFACE).inner_margin(Margin::symmetric(10.0, 4.0)).stroke(Stroke::new(1.0_f32, theme::BORDER)))
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;

                // TX / RX lamps (bottom-left in VarAC)
                let lamp = |ui: &mut egui::Ui, label: &str, on: bool, color: Color32| {
                    let (fill, text) = if on { (color, Color32::WHITE) } else { (theme::SURFACE_2, theme::MUTED) };
                    Frame::none().fill(fill).rounding(egui::Rounding::same(4.0)).inner_margin(Margin::symmetric(8.0, 1.0))
                        .show(ui, |ui| ui.label(RichText::new(label).strong().size(12.0).color(text)));
                };
                lamp(ui, "TX", state.ptt, theme::TX_RED);
                lamp(ui, "RX", !state.ptt && state.modem_connected, Color32::from_rgb(22, 163, 74));
                // VarAC's PTT Disabled / Ignore DCD, always in sight
                let toggle = |ui: &mut egui::Ui, text: &str, on: bool, color: Color32, tip: &str| {
                    let (fill, fg) = if on { (color, Color32::WHITE) } else { (theme::SURFACE_2, theme::MUTED) };
                    ui.add(egui::Button::new(RichText::new(text).strong().size(11.5).color(fg)).fill(fill).small())
                        .on_hover_text(tip)
                        .clicked()
                };
                if toggle(ui, "PTT DISABLED", state.ptt_disabled, theme::TX_RED,
                    "Listen only: the radio is never keyed (Openslot ignores the modem's PTT). Click to switch.")
                {
                    set_ptt_disabled(state, !state.ptt_disabled);
                }
                if toggle(ui, "IGNORE DCD", state.ignore_dcd, theme::YELLOW,
                    "Transmit even when the channel seems busy (switches itself off after 2 hours). Click to switch.")
                {
                    let on = !state.ignore_dcd;
                    state.ignore_dcd = on;
                    state.ignore_dcd_since = on.then(Utc::now);
                    state.send_cmd(GuiCommand::SetIgnoreDcd(on));
                    crate::config::update_config_file(|c| c.ignore_dcd = Some(on));
                }
                if state.ptt {
                    ui.ctx().request_repaint();
                }

                let (s, c) = match state.session_state {
                    SessionState::Idle => ("IDLE", theme::MUTED),
                    SessionState::Connecting => ("CONNECTING", theme::YELLOW),
                    SessionState::Connected => ("CONNECTED", theme::GREEN),
                    SessionState::AwaitingDisconnect => ("DISCONNECTING", theme::YELLOW),
                };
                ui.label(RichText::new(s).strong().size(12.0).color(c));
                ui.separator();
                ui.label(RichText::new(&state.my_callsign).strong().size(12.0).color(theme::ACCENT));
                if !state.settings_form.locator.is_empty() {
                    ui.label(RichText::new(&state.settings_form.locator).size(12.0).color(theme::MUTED));
                }
                ui.separator();

                if state.auto_cq_enabled && state.cq_active {
                    let next_in = state.last_cq_sent
                        .map(|t| (state.auto_cq_interval_secs as i64 - (Utc::now() - t).num_seconds()).max(0))
                        .unwrap_or(0);
                    theme::pill(ui, format!("AUTO-CQ {next_in}s"), theme::ACCENT);
                }
                // VarAC's ALERT: red while there are unread alerts; opens the Alert center
                let (afill, afg) = if state.unread_alerts > 0 { (theme::RED, Color32::WHITE) } else { (theme::SURFACE_2, theme::MUTED) };
                let alabel = if state.unread_alerts > 0 { format!("ALERT ({})", state.unread_alerts) } else { "ALERT".into() };
                if ui.add(egui::Button::new(RichText::new(alabel).strong().size(11.5).color(afg)).fill(afill).small())
                    .on_hover_text("Alert center").clicked()
                {
                    state.alert_center_open = true;
                }
                if state.away_active {
                    theme::pill(ui, "AWAY", theme::YELLOW);
                }
                if state.ai_mode
                    && ui.add(egui::Button::new(RichText::new("AI MODE").strong().size(11.5).color(Color32::WHITE)).fill(theme::ACCENT).small())
                        .on_hover_text("Your messages go to the partner's AI gateway. Click to exit AI mode.")
                        .clicked()
                {
                    state.ai_mode = false;
                    state.add_system_chat("EXITING AI MODE.".into());
                }
                if let Some(g) = state.hamplay.as_ref().filter(|g| !g.open && (g.active || g.invited)) {
                    let label = if g.invited { "HAMPLAY INVITE" } else { "HAMPLAY" };
                    if ui.add(egui::Button::new(RichText::new(label).strong().size(11.5).color(Color32::WHITE)).fill(theme::GREEN).small()).clicked() {
                        if let Some(g) = state.hamplay.as_mut() {
                            g.open = true;
                        }
                    }
                }
                if state.relay_notification_count > 0
                    && ui.add(egui::Button::new(RichText::new(format!("RELAY ({})", state.relay_notification_count))
                        .strong().size(11.5).color(Color32::WHITE)).fill(theme::PURPLE).small())
                        .on_hover_text("Stations holding VMail for you: click to see them and collect it")
                        .clicked()
                {
                    state.relay_notifications_open = true;
                }
                if state.busy {
                    theme::pill(ui, "BUSY", theme::RED);
                }
                if state.buffer_bytes > 0 {
                    theme::pill(ui, format!("BUF {}", state.buffer_bytes), theme::YELLOW);
                }
                if let Some(ref sending) = state.currently_sending {
                    theme::pill(ui, format!("Sending {sending}"), theme::YELLOW);
                }
                if state.queue_count > 0
                    && theme::pill(ui, format!("In queue {}", state.queue_count), theme::YELLOW).interact(egui::Sense::click())
                        .on_hover_text("Messages waiting to be sent: click to see them (CLR / EDIT)").clicked()
                {
                    state.queue_open = true;
                }
                ui.add(egui::Label::new(RichText::new(&state.status_msg).size(12.0).color(theme::MUTED)).truncate());

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let (vtxt, vcol) = if state.unread_vmails > 0 {
                        (format!("{} NEW VMAIL", state.unread_vmails), theme::YELLOW)
                    } else {
                        ("NO NEW VMAIL".to_string(), theme::MUTED)
                    };
                    if theme::pill(ui, vtxt, vcol).interact(egui::Sense::click()).on_hover_text("Open VMail").clicked() {
                        state.vmail_window_open = true;
                        state.vmail_folder_tab = 0;
                    }
                    // VarAC's Parking / Outbox counters open the mailbox on those folders
                    if ui.add(egui::Label::new(RichText::new(format!("Parking: {}", state.parking_count)).size(11.5).color(theme::MUTED)).sense(egui::Sense::click()))
                        .on_hover_text("VMail parked here for other stations").clicked()
                    {
                        state.vmail_window_open = true;
                        state.vmail_folder_tab = 3;
                    }
                    if ui.add(egui::Label::new(RichText::new(format!("Outbox: {}", state.outbox_count)).size(11.5).color(theme::MUTED)).sense(egui::Sense::click()))
                        .on_hover_text("VMail waiting to be sent").clicked()
                    {
                        state.vmail_window_open = true;
                        state.vmail_folder_tab = 2;
                    }
                    if let Some((on, suspended)) = state.email_gate_pill {
                        let (fill, fg, tip) = if suspended {
                            (Color32::from_rgb(234, 88, 12), Color32::WHITE, "The Email gateway is INACTIVE due to repeated failed attempts.\n\nCLICK TO ACTIVATE")
                        } else if on {
                            (theme::GREEN, Color32::WHITE, "The Email gateway is ACTIVE\n\nCLICK TO DEACTIVATE")
                        } else {
                            (theme::SURFACE_2, theme::MUTED, "The Email gateway is INACTIVE.\n\nCLICK TO ACTIVATE")
                        };
                        if ui.add(egui::Button::new(RichText::new("EMAIL GATE").strong().size(11.5).color(fg)).fill(fill).small()).on_hover_text(tip).clicked() {
                            state.email_gate_toggle = true;
                        }
                    }
                    if let Some(e) = state.rig_error.clone().filter(|_| !state.rig_connected && state.rig_backend != crate::rig::RigBackend::None) {
                        ui.add(egui::Label::new(RichText::new(format!("Rig: {e}")).size(11.0).color(theme::RED)).truncate())
                            .on_hover_text(e);
                    }
                });
            });
        });
}

// ---------------------------------------------------------------- VMail folders window

fn draw_vmail_window(ctx: &egui::Context, state: &mut GuiState, db: &Storage) {
    if !state.vmail_window_open {
        return;
    }
    let mut open = true;
    egui::Window::new("VMail")
        .open(&mut open)
        .default_size([520.0, 320.0])
        .show(ctx, |ui| draw_vmail_folders(ui, state, db));
    state.vmail_window_open = open && state.vmail_window_open;
}

// ---------------------------------------------------------------- QSY invitation

const QSY_INVITE_TIMEOUT_SECS: i64 = 80;

/// VarAC's "Send QSY Invitation", opened by the partner's <Q>/<AWQ> in the
/// opening handshake or by the QSY button during a QSO. STEP 1: a slot (5…1 below, 11…15
/// above the calling frequency; on 60 m only 1 and 11) or a custom frequency. STEP 2:
/// the frequency sniffer (hold to listen there with PTT off; needs rig control). STEP 3:
/// SEND QSY INVITATION — `<Q:n>`, n = the offset from our frequency in tens of Hz.
pub(super) fn draw_qsy_invitation_window(ctx: &egui::Context, state: &mut GuiState) {
    let Some(opened) = state.qsy_invite_open else {
        end_sniff(state);
        return;
    };
    let mid = state.qsy_invite_mid;
    let left = QSY_INVITE_TIMEOUT_SECS - (Utc::now() - opened).num_seconds();
    let dial = state.dial();
    let cf = state.current_frequency;
    let step = state.slot_step() as i32;
    let on_60m_cf = crate::varac::messages::CALLING_FREQUENCIES_HZ.iter().any(|(f, b)| *f == cf && *b == "60m");
    let slot_ok = |k: i32| !on_60m_cf || k == 1;
    let mut send: Option<i32> = None;
    let mut stay = !mid && left <= 0;
    let mut close = false;
    egui::Window::new("Send QSY Invitation")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
        .show(ctx, |ui| {
            if !mid {
                ui.label(RichText::new("Your QSO partner allows auto-QSY.").strong());
                ui.label("Please send a QSY invitation to clear the calling frequency.");
                ui.add_space(6.0);
            }
            theme::caption(ui, "Step 1: select a slot or a frequency");
            let target = state.qsy_invite_target;
            let pick = |ui: &mut egui::Ui, slot: i32, offset: i32, state: &mut GuiState| {
                let f = (cf as i64 + offset as i64).max(0) as u64;
                let selected = target == Some(f);
                if ui.add_sized([34.0, 28.0], egui::SelectableLabel::new(selected, slot.to_string()))
                    .on_hover_text(format!("{offset:+} Hz → {:.3} kHz", f as f64 / 1000.0))
                    .clicked()
                {
                    state.qsy_invite_target = Some(f);
                    state.qsy_custom_khz.clear();
                }
            };
            ui.horizontal(|ui| {
                theme::caption(ui, "Below");
                for k in (1..=5).rev() {
                    if slot_ok(k) {
                        pick(ui, k, -k * step, state);
                    }
                }
                ui.add_space(8.0);
                ui.label(RichText::new(format!("{:.3}", cf as f64 / 1000.0)).monospace().strong())
                    .on_hover_text("Calling frequency");
                ui.add_space(8.0);
                for k in 1..=5 {
                    if slot_ok(k) {
                        pick(ui, 10 + k, k * step, state);
                    }
                }
                theme::caption(ui, "Above");
            });
            ui.label(RichText::new("Preferred: 1 and 11").small().color(theme::MUTED));
            if on_60m_cf {
                ui.label(RichText::new("Some slots were disabled to avoid conflict with other band allocaitons").small().color(theme::RED));
            }
            ui.horizontal(|ui| {
                ui.label("Custom (kHz):");
                if ui.add(TextEdit::singleline(&mut state.qsy_custom_khz).desired_width(110.0).hint_text("e.g. 14108.250")).changed() {
                    state.qsy_invite_target = state.qsy_custom_khz.trim().replace(',', ".").parse::<f64>().ok()
                        .filter(|k| *k > 0.0).map(|k| (k * 1000.0).round() as u64);
                }
                ui.label(RichText::new("Make sure the slot is not out of band!").small().color(theme::RED));
            });
            ui.label(RichText::new(format!("Current frequency {:.3} kHz", dial as f64 / 1000.0)).small().color(theme::MUTED));
            ui.add_space(6.0);

            theme::caption(ui, "Step 2: check that the frequency is free");
            ui.horizontal(|ui| {
                let can_sniff = state.rig_connected && state.qsy_invite_target.is_some_and(|t| t != dial);
                let resp = ui.add_enabled(can_sniff, egui::Button::new("FREQUENCY SNIFFER"));
                let held = resp.is_pointer_button_down_on()
                    || (state.qsy_sniff.is_some() && ui.input(|i| i.pointer.primary_down()));
                match (held, state.qsy_sniff) {
                    (true, None) => {
                        if let Some(t) = state.qsy_invite_target {
                            // VarAC: retune there with PTT off while the button is held
                            let we_disable = !state.ptt_disabled;
                            if we_disable {
                                state.send_cmd(GuiCommand::SetPttDisabled(true));
                            }
                            state.qsy_sniff = Some((dial, Utc::now(), we_disable));
                            state.tune_to = Some(t);
                            state.add_log(format!("SLOT sniffer: Changing frequency to {:.3} kHz", t as f64 / 1000.0));
                        }
                    }
                    (false, Some(_)) => end_sniff(state),
                    _ => {}
                }
                let text = match state.qsy_sniff {
                    Some((_, since, _)) if (Utc::now() - since).num_milliseconds() < 5000 => {
                        ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
                        format!("Keep holding... {}", 5 - (Utc::now() - since).num_seconds())
                    }
                    Some(_) => "Done".to_string(),
                    None if state.rig_connected => "Click and hold".to_string(),
                    None => "Available only with CAT freq. control".to_string(),
                };
                ui.label(RichText::new(text).small().color(theme::MUTED));
            });
            ui.add_space(6.0);

            theme::caption(ui, "Step 3: send the QSY invitation");
            ui.horizontal(|ui| {
                let target = state.qsy_invite_target;
                if ui.add_enabled(state.qsy_sniff.is_none(), theme::primary_button("SEND QSY INVITATION", theme::GREEN)).clicked() {
                    match target {
                        Some(t) if t != dial => send = Some(((t as i64 - dial as i64) / 10) as i32),
                        _ => state.tx_alert = Some(("Please select a slot".into(),
                            "Select a Slot to invite your QSO partner to QSY.".into())),
                    }
                }
                if mid {
                    if ui.button("CANCEL").clicked() {
                        close = true;
                    }
                } else {
                    if ui.button("Stay on this frequency").clicked() {
                        stay = true;
                    }
                    ui.label(RichText::new(format!("staying automatically in {}s", left.max(0))).small().color(theme::MUTED));
                }
            });
        });
    if send.is_some() || stay || close {
        end_sniff(state);
        state.qsy_invite_open = None;
        state.qsy_invite_target = None;
        state.qsy_custom_khz.clear();
    }
    if let Some(tens) = send {
        if mid {
            state.send_cmd(GuiCommand::QsyRequest { offset_hz: tens * 10 });
        } else {
            state.send_cmd(GuiCommand::SendQsyInvitation(tens));
        }
        state.add_log(format!("QSY invitation sent: {:+} Hz", tens * 10));
    } else if stay {
        state.send_cmd(GuiCommand::SendQsyInvitation(0));
        state.add_log("Staying on the calling frequency (<Q:0>)".into());
    }
}

/// Let go of the frequency sniffer: back to where we were, PTT as it was.
fn end_sniff(state: &mut GuiState) {
    if let Some((back, _, we_disabled)) = state.qsy_sniff.take() {
        state.tune_to = Some(back);
        if we_disabled {
            state.send_cmd(GuiCommand::SetPttDisabled(false));
        }
        state.add_log("SLOT sniffer: Changing frequency back".into());
    }
}
