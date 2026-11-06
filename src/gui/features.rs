//! Windows for VarAC's extra features: HamPlay (game center and boards) and blocking a
//! callsign.

use super::*;
use crate::varac::hamplay::{Board, Game};

/// `f()` at most every `secs` seconds, or again when `key` changes (windows drawn every
/// frame mustn't query the database or read files each time).
fn cached<T: Clone + Send + Sync + 'static>(ctx: &egui::Context, id: &str, key: &str, secs: f64, f: impl FnOnce() -> T) -> T {
    let mid = egui::Id::new(("feature_cache", id));
    let now = ctx.input(|i| i.time);
    if let Some((k, t, v)) = ctx.data(|d| d.get_temp::<(String, f64, T)>(mid)) {
        if k == key && now - t < secs {
            return v;
        }
    }
    let v = f();
    ctx.data_mut(|d| d.insert_temp(mid, (key.to_string(), now, v.clone())));
    v
}

/// Drop a cached value (after a change), so the next frame reloads it.
fn uncache(ctx: &egui::Context, id: &str) {
    // (the time first: asking the context for it inside data_mut waits on the lock data_mut
    // holds, and the app freezes)
    let now = ctx.input(|i| i.time);
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(("feature_cache_gen", id)), now));
}

/// VarAC's game center: pick a game to invite the QSO partner to.
pub fn draw_hamplay_center(ctx: &egui::Context, state: &mut GuiState) {
    if !state.hamplay_center_open {
        return;
    }
    let mut open = true;
    egui::Window::new("HamPlay")
        .open(&mut open)
        .resizable(false)
        .collapsible(false)
        .show(ctx, |ui| {
            ui.label(RichText::new("Play a game with your QSO partner").strong());
            ui.label(RichText::new("Double-click a game (or click Invite) to send the invitation.").small().color(GRAY));
            ui.add_space(6.0);
            let connected = state.session_state == SessionState::Connected;
            for game in [Game::TicTacToe, Game::ConnectFour] {
                ui.horizontal(|ui| {
                    let r = ui.add(egui::Label::new(RichText::new(game.name()).strong()).sense(egui::Sense::click()));
                    ui.label(RichText::new(game.description()).color(GRAY));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let invite = ui.add_enabled(connected, egui::Button::new("Invite")).clicked();
                        if invite || r.double_clicked() {
                            state.hamplay_invite(game);
                            if state.hamplay.is_some() {
                                state.hamplay_center_open = false;
                            }
                        }
                    });
                });
            }
            if !connected {
                ui.add_space(4.0);
                ui.colored_label(YELLOW, "You must connect to someone first.");
            }
            ui.add_space(4.0);
            ui.checkbox(&mut state.settings_form.hamplay_enabled, "Accept HamPlay invitations")
                .on_hover_text("Off: invitations are rejected (<PJ>)");
        });
    state.hamplay_center_open = open && state.hamplay_center_open;
}

/// The game board (tic-tac-toe / Connect Four, as in VarAC).
pub fn draw_hamplay_game(ctx: &egui::Context, state: &mut GuiState) {
    let Some(g) = state.hamplay.as_ref() else { return };
    if !g.open {
        return;
    }
    let game = g.board.game;
    let me = state.my_callsign.clone();
    let (p1, p2) = if g.me == 1 { (me.clone(), g.opponent.clone()) } else { (g.opponent.clone(), me.clone()) };
    let title = format!("HamPlay: {}", game.name());
    let mut open = true;
    let mut click: Option<(usize, usize)> = None;
    let (mut accept, mut rematch) = (false, false);
    egui::Window::new(title)
        .id(egui::Id::new("hamplay_game"))
        .open(&mut open)
        .resizable(false)
        .collapsible(false)
        .show(ctx, |ui| {
            let Some(g) = state.hamplay.as_ref() else { return };
            ui.label(RichText::new(format!("{p1} ({}) vs. {p2} ({})", Board::symbol(game, 1), Board::symbol(game, 2))).strong());
            ui.add_space(4.0);
            let cell = match game {
                Game::TicTacToe => 64.0,
                Game::ConnectFour => 40.0,
            };
            let my_turn = g.my_turn();
            egui::Grid::new("hamplay_board").spacing([4.0, 4.0]).show(ui, |ui| {
                for (r, row) in g.board.cells.iter().enumerate() {
                    for (c, &v) in row.iter().enumerate() {
                        let (rect, resp) = ui.allocate_exact_size(Vec2::splat(cell), egui::Sense::click());
                        let p = ui.painter();
                        let hover = my_turn && resp.hovered() && g.board.move_for_click(r, c).is_some();
                        let bg = if hover { theme::SURFACE_3 } else { theme::SURFACE_2 };
                        p.rect_filled(rect, 6.0, bg);
                        let last = g.board.last == Some((r, c));
                        let center = rect.center();
                        match (game, v) {
                            (Game::TicTacToe, 1) => {
                                let d = cell * 0.28;
                                let s = Stroke::new(5.0_f32, theme::ACCENT);
                                p.line_segment([center + Vec2::new(-d, -d), center + Vec2::new(d, d)], s);
                                p.line_segment([center + Vec2::new(-d, d), center + Vec2::new(d, -d)], s);
                            }
                            (Game::TicTacToe, 2) => {
                                p.circle_stroke(center, cell * 0.3, Stroke::new(5.0_f32, Color32::from_rgb(249, 115, 22)));
                            }
                            (Game::ConnectFour, 1) => {
                                p.circle_filled(center, cell * 0.4, Color32::from_rgb(37, 99, 235));
                            }
                            (Game::ConnectFour, 2) => {
                                p.circle_filled(center, cell * 0.4, Color32::from_rgb(249, 115, 22));
                            }
                            _ => {
                                if game == Game::ConnectFour {
                                    p.circle_stroke(center, cell * 0.4, Stroke::new(1.0_f32, theme::BORDER));
                                }
                            }
                        }
                        if last {
                            p.rect_stroke(rect, 6.0, Stroke::new(2.0_f32, theme::YELLOW));
                        }
                        if resp.clicked() {
                            click = Some((r, c));
                        }
                    }
                    ui.end_row();
                }
            });
            ui.add_space(6.0);
            let color = if my_turn { theme::GREEN } else { theme::TEXT };
            for line in g.status.lines() {
                ui.label(RichText::new(line).strong().color(color));
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if g.invited && ui.add(theme::accent_button("ACCEPT", theme::GREEN)).clicked() {
                    accept = true;
                }
                let over = g.board.outcome.is_some() || (!g.active && !g.invited && g.board.last.is_some());
                if ui.add_enabled(over && state.session_state == SessionState::Connected, egui::Button::new("REMATCH")).clicked() {
                    rematch = true;
                }
            });
        });
    if let Some((r, c)) = click {
        state.hamplay_click(r, c);
    }
    if accept {
        state.hamplay_accept();
    }
    if rematch {
        state.hamplay_rematch();
    }
    if !open {
        state.hamplay_close();
    }
}

/// VarAC's "Block callsign" question (No is the default answer).
pub fn draw_block_confirm(ctx: &egui::Context, state: &mut GuiState, station_config: &StationConfig) {
    let Some(call) = state.block_confirm.clone() else { return };
    let mut answer = None;
    egui::Window::new("Block callsign")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
        .show(ctx, |ui| {
            ui.label(RichText::new(format!("Block {call}?")).strong().color(theme::YELLOW));
            ui.add_space(4.0);
            ui.label("Blocking it will prevent any future connections, and you will no longer see new traffic \
                      from this callsign in your beacons, broadcasts, or CQ lists.");
            ui.add_space(4.0);
            ui.label("You can always undo this action by going to the settings menu, under the QSO tab, and \
                      editing your blocked callsigns list.");
            ui.add_space(4.0);
            ui.label("Are you sure you want to block this callsign?");
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let no = ui.button("No");
                no.request_focus();
                if no.clicked() {
                    answer = Some(false);
                }
                if ui.button("Yes").clicked() {
                    answer = Some(true);
                }
            });
        });
    match answer {
        Some(true) => {
            state.block_confirm = None;
            block_callsign(state, station_config, &call);
        }
        Some(false) => state.block_confirm = None,
        None => {}
    }
}

/// Add `call` to the block list (Settings → QSO) and apply it now.
pub fn block_callsign(state: &mut GuiState, station_config: &StationConfig, call: &str) {
    let call = call.trim().to_uppercase();
    if call.is_empty() || state.block_list.contains(&call) {
        return;
    }
    state.block_list.push(call.clone());
    state.settings_form.callsigns_block_list = state.block_list.join(",");
    let list = state.settings_form.callsigns_block_list.clone();
    crate::config::update_config_file(|c| c.callsigns_block_list = Some(list));
    station_config.policy.set_blocked(state.block_list.clone());
    state.add_log(format!("{call} added to the block list"));
    state.status_msg = format!("{call} blocked");
}

fn band_of(band: &Option<String>, freq: i64) -> String {
    band.clone().filter(|b| !b.is_empty())
        .or_else(|| freq_to_band(freq.max(0) as u64).map(str::to_string))
        .unwrap_or_default()
}

fn small_table(ui: &mut egui::Ui, id: &str, headers: &[&str], rows: &[Vec<String>], height: f32) -> Option<usize> {
    let mut clicked = None;
    ui.push_id(id, |ui| {
        egui::ScrollArea::vertical().max_height(height).auto_shrink([false, true]).show(ui, |ui| {
            egui::Grid::new(id).striped(true).spacing([12.0, 2.0]).show(ui, |ui| {
                for h in headers {
                    ui.label(RichText::new(*h).small().strong().color(GRAY));
                }
                ui.end_row();
                for (i, row) in rows.iter().enumerate() {
                    for cell in row {
                        let r = ui.add(egui::Label::new(RichText::new(cell).small()).sense(egui::Sense::click()));
                        if r.clicked() {
                            clicked = Some(i);
                        }
                    }
                    ui.end_row();
                }
            });
            if rows.is_empty() {
                ui.label(RichText::new("—").small().color(GRAY));
            }
        });
    });
    clicked
}

/// VarAC's "Callsign history": a station's beacons and CQs of the last year, its
/// broadcasts, our QSOs with it and their chat, and its contact card.
pub fn draw_callsign_history(ctx: &egui::Context, state: &mut GuiState, db: &Storage) {
    let Some(call) = state.history_call.clone() else { return };
    let mut open = true;
    let fmt = |t: chrono::DateTime<Utc>| t.format("%Y-%m-%d %H:%M").to_string().to_uppercase();
    egui::Window::new(format!("Callsign history: {call}"))
        .id(egui::Id::new("callsign_history"))
        .open(&mut open)
        .default_size([760.0, 620.0])
        .resizable(true)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.add(TextEdit::singleline(&mut state.history_input).desired_width(120.0).hint_text("Callsign"));
                if ui.button("History lookup").clicked() && !state.history_input.trim().is_empty() {
                    state.history_call = Some(state.history_input.trim().to_uppercase());
                    state.history_qso = None;
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("PSK Reporter Lookup").clicked() {
                        open_psk_lookup(&call);
                    }
                    if ui.button("QRZ Lookup").clicked() {
                        let _ = open::that(format!("https://qrz.com/lookup/{call}"));
                    }
                    if ui.button("Block callsign").clicked() {
                        state.block_confirm = Some(call.clone());
                    }
                });
            });
            // (loaded once a few seconds, not every frame)
            let ctx2 = ui.ctx().clone();
            let generation = ctx2.data(|d| d.get_temp::<f64>(egui::Id::new(("feature_cache_gen", "history")))).unwrap_or(0.0);
            let hkey = format!("{call}|{generation}");
            let (contact, frames, bcs_raw, qsos) = cached(&ctx2, "history", &hkey, 5.0, || (
                db.get_contact(&call).ok().flatten(),
                db.cqframes_from(&call, Utc::now() - chrono::Duration::hours(8760)).unwrap_or_default(),
                db.broadcasts_from(&call).unwrap_or_default(),
                db.qsos_with(&call).unwrap_or_default(),
            ));
            // Contact card
            if let Some(c) = contact {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(&call).strong().color(theme::ACCENT));
                    for (k, v) in [("Name", &c.name), ("QTH", &c.qth), ("Locator", &c.locator)] {
                        if let Some(v) = v.as_ref().filter(|v| !v.is_empty()) {
                            ui.label(RichText::new(format!("{k}: {v}")).small());
                        }
                    }
                    if let Some(loc) = c.locator.as_deref() {
                        if let Some(d) = grid_distance_str(&state.settings_form.locator, loc) {
                            ui.label(RichText::new(format!("Distance: {d}")).small());
                        }
                    }
                    if let Some(n) = c.notes.as_ref().filter(|n| !n.is_empty()) {
                        ui.label(RichText::new(format!("Comments: {n}")).small().color(GRAY));
                    }
                });
            }
            ui.separator();
            let beacons: Vec<Vec<String>> = frames.iter().filter(|f| f.cqframe_type_id == 2)
                .map(|f| vec![band_of(&f.band, f.frequency), fmt(f.cqframe_time), f.bandwidth.clone(), f.snr.map(|s| format!("{s:+}")).unwrap_or_default()])
                .collect();
            let cqs: Vec<Vec<String>> = frames.iter().filter(|f| f.cqframe_type_id != 2)
                .map(|f| vec![band_of(&f.band, f.frequency), fmt(f.cqframe_time), f.bandwidth.clone(), f.locator.clone().unwrap_or_default(),
                    f.slot.filter(|s| *s > 0).map(|s| format!("SLOT {s}")).unwrap_or_else(|| "CQ".into()), f.snr.map(|s| format!("{s:+}")).unwrap_or_default()])
                .collect();
            ui.columns(2, |cols| {
                cols[0].label(RichText::new(format!("Beacons history (1 year) — {}", beacons.len())).strong());
                small_table(&mut cols[0], "hist_beacons", &["Bnd", "Time", "BW", "SNR"], &beacons, 110.0);
                cols[1].label(RichText::new(format!("CQ calls history (1 year) — {}", cqs.len())).strong());
                small_table(&mut cols[1], "hist_cqs", &["Bnd", "Time", "BW", "LOC", "Type", "SNR"], &cqs, 110.0);
            });
            ui.separator();
            let bcs: Vec<Vec<String>> = bcs_raw.iter()
                .map(|b| vec![band_of(&b.band, b.frequency), fmt(b.broadcast_time), b.from_callsign.clone(), b.to_callsign.clone(),
                    b.via_callsign.clone().unwrap_or_default(), b.broadcast_message.chars().take(80).collect()])
                .collect();
            ui.label(RichText::new("Broadcasts history").strong());
            small_table(ui, "hist_bcs", &["Bnd", "Time", "From", "To", "Via", "Broadcast message"], &bcs, 90.0);
            ui.separator();
            let rows: Vec<Vec<String>> = qsos.iter().map(|q| {
                let mins = q.endtime.map(|e| (e - q.starttime).num_minutes().max(0).to_string()).unwrap_or_default();
                vec![fmt(q.starttime), q.band.clone(), format!("{:.3}", q.frequency as f64 / 1e6),
                    q.snr_sent.map(|s| format!("{s:+}")).unwrap_or_default(), q.snr_received.map(|s| format!("{s:+}")).unwrap_or_default(), mins]
            }).collect();
            ui.label(RichText::new(format!("Previous QSOs — {}", qsos.len())).strong());
            if let Some(i) = small_table(ui, "hist_qsos", &["Time", "Band", "Frequency", "SNR-S", "SNR-R", "Duration (min)"], &rows, 110.0) {
                state.history_qso = Some(qsos[i].guid.clone());
            }
            if let Some(q) = state.history_qso.as_ref().and_then(|g| qsos.iter().find(|q| &q.guid == g)) {
                if ui.button("SNR CHART").on_hover_text("SNR over the selected QSO").clicked() {
                    state.snr_chart = Some(Some(q.guid.clone()));
                }
                if ui.button("QSL CARD").on_hover_text("Make a QSL card for the selected QSO").clicked() {
                    let fmt_snr = |v: Option<i32>| v.map(|s| format!("{s:+}")).unwrap_or_default();
                    let (c, s, r, b, t, n) = (q.callsign.clone(), fmt_snr(q.snr_sent), fmt_snr(q.snr_received), q.band.clone(), q.starttime, q.name.clone().unwrap_or_default());
                    state.open_qsl(&c, &s, &r, &b, t, &n);
                }
            }
            ui.separator();
            ui.label(RichText::new("Chat history").strong());
            match state.history_qso.as_deref() {
                None => {
                    ui.label(RichText::new("Select a QSO from above to view its contents").small().color(GRAY));
                }
                Some(guid) => {
                    let mut chat = cached(&ctx2, "history_chat", guid, 5.0, || db.chat_history(guid, 2000).unwrap_or_default());
                    chat.reverse();
                    let rows: Vec<Vec<String>> = chat.iter()
                        .map(|c| vec![c.creation_time.format("%H:%M:%S").to_string(), c.callsign.clone(), c.entry.replace("//n", " ")])
                        .collect();
                    small_table(ui, "hist_chat", &["Time", "Callsign", "Message"], &rows, 160.0);
                }
            }
        });
    if !open {
        state.history_call = None;
        state.history_qso = None;
    }
}

/// Settings → AI / HamPlay (VarAC [AI] and [HAMPLAY]).
pub fn draw_ai_settings(ui: &mut egui::Ui, f: &mut state::SettingsForm) {
    ui.label(RichText::new("HamPlay").strong());
    ui.checkbox(&mut f.hamplay_enabled, "Accept HamPlay game invitations")
        .on_hover_text("Tic-Tac-Toe and Connect Four with your QSO partner (Tools → HamPlay). Off: invitations are rejected.");
    ui.add_space(8.0);
    ui.label(RichText::new("AI gateway").strong());
    ui.label(RichText::new(
        "Stations connected to you can ask questions by typing 'AI:' followed by the question. \
         Your station asks Google Gemini with your own API key and sends back the answer.",
    ).small().color(GRAY));
    ui.checkbox(&mut f.ai_enabled, "Enable the AI gateway");
    ui.add_enabled_ui(f.ai_enabled, |ui| {
        egui::Grid::new("ai_grid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
            ui.label("Gemini API key:");
            ui.add(TextEdit::singleline(&mut f.ai_api_key).password(true).desired_width(260.0));
            ui.end_row();
            ui.label("Model:");
            ui.add(TextEdit::singleline(&mut f.ai_model).desired_width(180.0).hint_text(crate::varac::ai_gateway::DEFAULT_MODEL));
            ui.end_row();
            ui.label("Timeout (seconds):");
            ui.add(TextEdit::singleline(&mut f.ai_timeout_secs).desired_width(50.0));
            ui.end_row();
            ui.label("Answer length limit (characters):");
            ui.add(TextEdit::singleline(&mut f.ai_limit_chars).desired_width(50.0));
            ui.end_row();
            ui.label("Questions per QSO (0 = no limit):");
            ui.add(TextEdit::singleline(&mut f.ai_limit_per_qso).desired_width(50.0));
            ui.end_row();
        });
        ui.checkbox(&mut f.ai_chat_context, "Remember the QSO's earlier questions and answers");
        ui.checkbox(&mut f.ai_announce, "Announce the gateway (beacons and the 'A' service at connect)");
        ui.label("Additional instructions for the AI:");
        ui.add(TextEdit::multiline(&mut f.ai_customization).desired_rows(3).desired_width(f32::INFINITY));
    });
}

/// VarAC's "In queue": what waits to be sent, with CLR and EDIT.
pub fn draw_queue_window(ctx: &egui::Context, state: &mut GuiState) {
    if !state.queue_open {
        return;
    }
    let mut open = true;
    egui::Window::new("In queue")
        .open(&mut open)
        .default_width(420.0)
        .show(ctx, |ui| {
            if state.queue_lines.is_empty() {
                ui.label(RichText::new("Nothing waiting to be sent").italics().color(GRAY));
            }
            egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                for line in &state.queue_lines {
                    ui.label(RichText::new(line).monospace().small());
                }
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("CLR").on_hover_text("Empty the queue from messages that are waiting to be sent").clicked() {
                    state.send_cmd(GuiCommand::ClearQueue { edit: false });
                }
                if ui.button("EDIT").on_hover_text("Take the queued messages back to the 'new message' section so you can edit them.").clicked() {
                    state.send_cmd(GuiCommand::ClearQueue { edit: true });
                }
                if state.buffer_bytes > 0 && ui.button(format!("CLEAR MODEM BUFFER ({})", state.buffer_bytes)).clicked() {
                    state.send_cmd(GuiCommand::ClearTxBuffer);
                }
            });
        });
    state.queue_open = open && state.queue_open;
}

/// VarAC's Alert center: Incoming / Sent / Archived alerts.
pub fn draw_alert_center(ctx: &egui::Context, state: &mut GuiState, db: &Storage) {
    if !state.alert_center_open {
        return;
    }
    let folders = [(1i64, "Incoming"), (3, "Sent"), (2, "Archived")];
    let mut open = true;
    egui::Window::new("Alert center")
        .open(&mut open)
        .default_size([820.0, 420.0])
        .resizable(true)
        .show(ctx, |ui| {
            let ctx2 = ui.ctx().clone();
            let generation = ctx2.data(|d| d.get_temp::<f64>(egui::Id::new(("feature_cache_gen", "alerts")))).unwrap_or(0.0);
            let all = cached(&ctx2, "alerts", &generation.to_string(), 3.0, || {
                [1i64, 3, 2].map(|f| db.alerts(f).unwrap_or_default())
            });
            ui.horizontal(|ui| {
                for (i, (_folder, name)) in folders.iter().enumerate() {
                    let n = all[i].len();
                    if ui.selectable_label(state.alert_tab == i, format!("{name} ({n})")).clicked() {
                        state.alert_tab = i;
                        state.alert_selected = None;
                    }
                }
            });
            let folder = folders[state.alert_tab.min(2)].0;
            let alerts = all[state.alert_tab.min(2)].clone();
            let mut acted = false;
            ui.separator();
            egui::ScrollArea::vertical().max_height(280.0).auto_shrink([false, true]).show(ui, |ui| {
                egui::Grid::new("alerts_grid").striped(true).spacing([10.0, 2.0]).show(ui, |ui| {
                    for h in ["Status", "Alert time", "From", "To", "Frequency", "Band", "Tag", "Alert name", "Source", "Source text"] {
                        ui.label(RichText::new(h).small().strong().color(GRAY));
                    }
                    ui.end_row();
                    for a in &alerts {
                        let sel = state.alert_selected.as_deref() == Some(a.guid.as_str());
                        let status = if a.read { "READ" } else { "NEW" };
                        let r = ui.selectable_label(sel, RichText::new(status).small().strong()
                            .color(if a.read { GRAY } else { theme::RED }));
                        if r.clicked() {
                            state.alert_selected = Some(a.guid.clone());
                        }
                        if r.double_clicked() {
                            let _ = db.mark_alert_read(Some(&a.guid), folder);
                            acted = true;
                        }
                        ui.label(RichText::new(a.alert_time.format("%Y-%m-%d %H:%M").to_string()).small());
                        ui.label(RichText::new(&a.from_callsign).small().strong());
                        ui.label(RichText::new(&a.to_callsign).small());
                        ui.label(RichText::new(if a.frequency > 0 { format!("{:.3}", a.frequency as f64 / 1e6) } else { String::new() }).small());
                        ui.label(RichText::new(freq_to_band(a.frequency.max(0) as u64).unwrap_or("")).small());
                        ui.label(RichText::new(&a.alert_tag).small().color(theme::YELLOW));
                        ui.label(RichText::new(&a.alert_name).small());
                        ui.label(RichText::new(crate::varac::alerts::Source::from_id(a.source_id).label()).small());
                        ui.label(RichText::new(a.source_text.chars().take(60).collect::<String>()).small());
                        ui.end_row();
                    }
                });
                if alerts.is_empty() {
                    ui.label(RichText::new("No alerts").italics().color(GRAY));
                }
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("SETUP ALERTS").clicked() {
                    state.alert_setup = Some(state.alert_tags.clone());
                }
                let sel = state.alert_selected.clone();
                if folder != 2 && ui.add_enabled(sel.is_some(), egui::Button::new("ARCHIVE")).clicked() {
                    let _ = db.mark_alert_read(sel.as_deref(), folder);
                    let _ = db.move_alert(sel.as_deref(), folder, 2);
                    state.alert_selected = None;
                }
                if ui.add_enabled(sel.is_some(), egui::Button::new("DELETE")).clicked() {
                    let _ = db.delete_alert(sel.as_deref(), folder);
                    state.alert_selected = None;
                }
                if folder == 1 && ui.button("ARCHIVE ALL ALERTS").clicked() {
                    let _ = db.mark_alert_read(None, 1);
                    let _ = db.move_alert(None, 1, 2);
                }
                if folder == 1 && ui.button("MARK ALL AS READ").clicked() {
                    let _ = db.mark_alert_read(None, 1);
                }
                if folder == 2 && ui.button("DELETE ALL").on_hover_text("Deleting all archived alerts").clicked() {
                    let _ = db.delete_alert(None, 2);
                }
                acted |= ui.ctx().input(|i| i.pointer.any_released()) && ui.ui_contains_pointer();
            });
            if acted {
                uncache(&ctx2, "alerts");
            }
        });
    state.unread_alerts = cached(ctx, "alert_count", "", 2.0, || db.unread_alert_count().unwrap_or(0));
    state.alert_center_open = open && state.alert_center_open;
}

/// SETUP ALERTS: VarAC's alert tag editor (up to 20 tags).
pub fn draw_alert_setup(ctx: &egui::Context, state: &mut GuiState) {
    let Some(mut tags) = state.alert_setup.take() else { return };
    let mut open = true;
    let mut save = false;
    egui::Window::new("Alert tags")
        .open(&mut open)
        .default_width(860.0)
        .show(ctx, |ui| {
            ui.label(RichText::new("A tag fires when a beacon, CQ, broadcast or chat line contains it (case-sensitive).").small().color(GRAY));
            egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                egui::Grid::new("alert_tags_grid").spacing([8.0, 4.0]).show(ui, |ui| {
                    for h in ["Tag", "Description", "Font", "Background", "Beacons", "CQs", "Broadcasts", "Chat", "Emails (a|b)", ""] {
                        ui.label(RichText::new(h).small().strong().color(GRAY));
                    }
                    ui.end_row();
                    let mut remove = None;
                    for (i, t) in tags.iter_mut().enumerate() {
                        ui.add(TextEdit::singleline(&mut t.tag).desired_width(90.0));
                        ui.add(TextEdit::singleline(&mut t.description).desired_width(150.0));
                        for (j, c) in [&mut t.font_color, &mut t.back_color].into_iter().enumerate() {
                            egui::ComboBox::from_id_salt(("alert_color", i, j)).width(90.0).selected_text(c.clone()).show_ui(ui, |ui| {
                                for n in crate::varac::alerts::COLOR_NAMES {
                                    ui.selectable_value(c, n.to_string(), *n);
                                }
                            });
                        }
                        ui.checkbox(&mut t.beacons, "");
                        ui.checkbox(&mut t.cqs, "");
                        ui.checkbox(&mut t.broadcasts, "");
                        ui.checkbox(&mut t.datastream, "");
                        let mut emails = t.emails.join("|");
                        if ui.add(TextEdit::singleline(&mut emails).desired_width(140.0)).changed() {
                            t.emails = emails.split('|').map(|e| e.trim().to_string()).filter(|e| !e.is_empty()).collect();
                        }
                        if ui.small_button("×").clicked() {
                            remove = Some(i);
                        }
                        ui.end_row();
                    }
                    if let Some(i) = remove {
                        tags.remove(i);
                    }
                });
            });
            ui.horizontal(|ui| {
                if tags.len() < crate::varac::alerts::MAX_TAGS && ui.button("Add tag").clicked() {
                    tags.push(crate::varac::alerts::AlertTag::default());
                }
                if ui.add(theme::accent_button("SAVE", GREEN)).clicked() {
                    save = true;
                }
            });
        });
    if save {
        let text = crate::varac::alerts::serialize(&tags);
        state.alert_tags = crate::varac::alerts::parse(&text);
        match state::alert_tags_path() {
            Some(p) => match std::fs::write(&p, text) {
                Ok(()) => state.status_msg = format!("Alert tags saved ({})", state.alert_tags.len()),
                Err(e) => state.status_msg = format!("Alert tags not saved: {e}"),
            },
            None => state.status_msg = "Alert tags: no config folder".into(),
        }
    } else if open {
        state.alert_setup = Some(tags);
    }
}

/// "Confirm QSO" (LOG with Auto log off) and the "AUTO QSO LOG IS OFF" notice at the end
/// of a QSO, which closes itself after 10 seconds (VarAC).
pub fn draw_qso_confirm(ctx: &egui::Context, state: &mut GuiState) {
    if let Some(text) = state.confirm_qso.clone() {
        let mut answer = None;
        egui::Window::new("Confirm QSO")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label(RichText::new(text).monospace());
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Yes").clicked() {
                        answer = Some(true);
                    }
                    if ui.button("No").clicked() {
                        answer = Some(false);
                    }
                });
            });
        if let Some(yes) = answer {
            state.confirm_qso = None;
            state.confirm_qso_yes = yes;
        }
    }
    if let Some(until) = state.auto_log_off_notice {
        if Utc::now() >= until {
            state.auto_log_off_notice = None;
            return;
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
        let mut ok = false;
        egui::Window::new("AUTO QSO LOG IS OFF")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label("This QSO will not be automatically logged.");
                ui.label("Click 'LOG' to manually log it or enable auto-logging");
                ui.add_space(6.0);
                ui.label(RichText::new(format!("This message will be automatically closed in {} seconds.", (until - Utc::now()).num_seconds().max(0) + 1)).small().color(GRAY));
                ok = ui.button("OK").clicked();
            });
        if ok {
            state.auto_log_off_notice = None;
        }
    }
}

/// "DX Cluster spot": check the spot, then send it to the telnet cluster (Settings → QSO).
pub fn draw_dx_spot(ctx: &egui::Context, state: &mut GuiState) {
    if let Some(r) = state.dx_spot_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
        state.dx_spot_rx = None;
        match r {
            Ok(echo) => {
                state.add_log(format!("DX cluster spot sent {echo}"));
                state.status_msg = "DX cluster spot sent".into();
            }
            Err(e) => {
                state.add_log(format!("DX cluster spot failed: {e}"));
                state.add_system_chat(format!("DX cluster spot failed: {e}"));
            }
        }
    }
    let Some((mut call, mut khz, mut comment)) = state.dx_spot.take() else { return };
    let mut open = true;
    let mut send = false;
    egui::Window::new("DX Cluster spot")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            egui::Grid::new("dx_spot_grid").num_columns(2).show(ui, |ui| {
                ui.label("Callsign:");
                ui.add(TextEdit::singleline(&mut call).desired_width(110.0));
                ui.end_row();
                ui.label("Frequency (kHz):");
                ui.add(TextEdit::singleline(&mut khz).desired_width(110.0));
                ui.end_row();
                ui.label("Comment:");
                ui.add(TextEdit::singleline(&mut comment).desired_width(220.0));
                ui.end_row();
            });
            send = ui.add(theme::accent_button("SEND SPOT", GREEN)).clicked();
        });
    if send {
        let hz = (khz.trim().parse::<f64>().unwrap_or(0.0) * 1000.0).round() as u64;
        let f = &state.settings_form;
        let (host, port, user, pass) = (f.dxcluster_host.clone(), f.dxcluster_port.trim().parse().unwrap_or(7373), f.dxcluster_username.clone(), f.dxcluster_password.clone());
        let user = if user.trim().is_empty() { state.my_callsign.clone() } else { user };
        let line = crate::varac::dxcluster::spot_line(hz, &call, &comment);
        state.add_log(format!("DX cluster {host}:{port}: {line}"));
        let (tx, rx) = std::sync::mpsc::channel();
        state.dx_spot_rx = Some(rx);
        std::thread::spawn(move || {
            let _ = tx.send(crate::varac::dxcluster::send_spot(&host, port, &user, &pass, &line));
        });
    } else if open {
        state.dx_spot = Some((call, khz, comment));
    }
}

/// VarAC's frequency scheduler editor: the schedule (UTC time → frequency, up to
/// 30 rules) and the scanner list (up to 15 frequencies with wait, beacon, tune, window).
pub fn draw_sched_editor(ctx: &egui::Context, state: &mut GuiState) {
    use crate::varac::scheduler as sch;
    let Some(mut ed) = state.sched_editor.take() else { return };
    let mut open = true;
    let mut save = false;
    egui::Window::new("Frequency scheduler")
        .open(&mut open)
        .default_size([760.0, 560.0])
        .resizable(true)
        .show(ctx, |ui| {
            ui.columns(2, |cols| {
                let ui = &mut cols[0];
                ui.label(RichText::new("Frequency schedule").strong());
                ui.label(RichText::new("UTC time (hour:minute) and frequency (14.105.000). At that minute the radio moves there.").small().color(GRAY));
                egui::ScrollArea::vertical().id_salt("sched_rules").max_height(380.0).show(ui, |ui| {
                    let mut remove = None;
                    for (i, (t, f)) in ed.rules.iter_mut().enumerate() {
                        ui.horizontal(|ui| {
                            ui.add(TextEdit::singleline(t).desired_width(50.0).hint_text("06:00"));
                            ui.add(TextEdit::singleline(f).desired_width(100.0).hint_text("14.105.000"));
                            if ui.small_button("×").clicked() {
                                remove = Some(i);
                            }
                        });
                    }
                    if let Some(i) = remove {
                        ed.rules.remove(i);
                    }
                });
                if ed.rules.len() < sch::MAX_RULES && ui.button("Add rule").clicked() {
                    ed.rules.push((String::new(), sch::dotted(state.dial())));
                }

                let ui = &mut cols[1];
                ui.label(RichText::new("Frequency scanner").strong());
                ui.label(RichText::new("Wait = seconds on each frequency. Start/End (UTC) empty = always.").small().color(GRAY));
                egui::ScrollArea::vertical().id_salt("sched_scan").max_height(380.0).show(ui, |ui| {
                    egui::Grid::new("scan_grid").spacing([4.0, 3.0]).show(ui, |ui| {
                        for h in ["On", "Frequency", "Wait", "Beacon", "Tune", "Start", "End", ""] {
                            ui.label(RichText::new(h).small().strong().color(GRAY));
                        }
                        ui.end_row();
                        let mut remove = None;
                        for (i, r) in ed.scan.iter_mut().enumerate() {
                            ui.checkbox(&mut r.enabled, "");
                            ui.add(TextEdit::singleline(&mut r.freq).desired_width(90.0));
                            egui::ComboBox::from_id_salt(("scan_wait", i)).width(48.0).selected_text(r.wait.to_string()).show_ui(ui, |ui| {
                                for w in sch::WAIT_CHOICES {
                                    ui.selectable_value(&mut r.wait, *w, w.to_string());
                                }
                            });
                            ui.checkbox(&mut r.beacon, "");
                            ui.checkbox(&mut r.tune, "").on_hover_text("Start the antenna tuner when arriving (rigctld)");
                            ui.add(TextEdit::singleline(&mut r.start).desired_width(42.0).hint_text("hh:mm"));
                            ui.add(TextEdit::singleline(&mut r.end).desired_width(42.0).hint_text("hh:mm"));
                            if ui.small_button("×").clicked() {
                                remove = Some(i);
                            }
                            ui.end_row();
                        }
                        if let Some(i) = remove {
                            ed.scan.remove(i);
                        }
                    });
                });
                if ed.scan.len() < sch::MAX_SCAN && ui.button("Add frequency").clicked() {
                    ed.scan.push(state::ScanRow { freq: sch::dotted(state.dial()), wait: 30, enabled: true, ..Default::default() });
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label("Busy hold (seconds):");
                    let mut hold: u32 = state.settings_form.scanner_busy_hold_secs.trim().parse().unwrap_or(0);
                    egui::ComboBox::from_id_salt("scan_hold").width(48.0).selected_text(hold.to_string()).show_ui(ui, |ui| {
                        for h in sch::HOLD_CHOICES {
                            ui.selectable_value(&mut hold, *h, h.to_string());
                        }
                    });
                    state.settings_form.scanner_busy_hold_secs = hold.to_string();
                });
                ui.checkbox(&mut state.settings_form.scanner_announce, "Announce scanning in my beacons")
                    .on_hover_text("Other VarAC stations then show that you scan several frequencies");
                ui.add_space(6.0);
                ui.label(RichText::new("ALE").strong());
                let f = &mut state.settings_form;
                ui.checkbox(&mut f.ale_enabled, "Enable ALE (a mode after FREQ SCANNER)")
                    .on_hover_text("Connecting first picks the best scanner frequency from the station's beacon history, and calls it there");
                ui.add_enabled_ui(f.ale_enabled, |ui| {
                    ui.checkbox(&mut f.ale_skip_approval, "Skip the connect approval (call at once)");
                    ui.horizontal(|ui| {
                        ui.label("Suggest a QSY when the link SNR is below");
                        egui::ComboBox::from_id_salt("ale_thr").width(48.0).selected_text(f.ale_snr_threshold.clone()).show_ui(ui, |ui| {
                            for v in (-24..=0).rev() {
                                ui.selectable_value(&mut f.ale_snr_threshold, v.to_string(), v.to_string());
                            }
                        });
                        ui.label("dB");
                    });
                    ui.horizontal(|ui| {
                        ui.label("History: last");
                        ui.add(TextEdit::singleline(&mut f.ale_recent_minutes).desired_width(32.0));
                        ui.label("min, then");
                        ui.add(TextEdit::singleline(&mut f.ale_history_days).desired_width(24.0));
                        ui.label("days");
                    });
                });
            });
            ui.separator();
            ui.horizontal(|ui| {
                ui.label("At startup:");
                for (k, label) in [("LAST_STATE", "Last state"), ("SCHEDULER", "Scheduler"), ("SCANNER", "Scanner"), ("INACTIVE", "Off")] {
                    ui.radio_value(&mut state.settings_form.schedule_start_mode, k.to_string(), label);
                }
            });
            if ui.add(theme::accent_button("SAVE AND EXIT", GREEN)).clicked() {
                save = true;
            }
        });
    if save {
        match state.save_sched_editor(&ed) {
            Ok(()) => {
                let f = &state.settings_form;
                let (hold, announce, start) = (f.scanner_busy_hold_secs.trim().parse().ok(), f.scanner_announce, f.schedule_start_mode.clone());
                let (ale_on, ale_skip, ale_thr, ale_min, ale_days) = (f.ale_enabled, f.ale_skip_approval,
                    f.ale_snr_threshold.trim().parse().ok(), f.ale_recent_minutes.trim().parse().ok(), f.ale_history_days.trim().parse().ok());
                crate::config::update_config_file(|c| {
                    c.scanner_busy_hold_secs = hold;
                    c.scanner_announce = Some(announce);
                    c.schedule_start_mode = Some(start);
                    c.ale_enabled = Some(ale_on);
                    c.ale_skip_approval = Some(ale_skip);
                    c.ale_snr_threshold = ale_thr;
                    c.ale_recent_minutes = ale_min;
                    c.ale_history_days = ale_days;
                });
                if !ale_on && state.freq_mode == crate::varac::scheduler::Mode::Ale {
                    state.set_freq_mode(crate::varac::scheduler::Mode::Inactive);
                }
                state.status_msg = format!("Frequency schedule saved ({} rules, {} scanner frequencies)", state.schedule_rules.len(), state.scan_list.len());
            }
            Err(e) => state.status_msg = format!("Frequency schedule not saved: {e}"),
        }
    } else if open {
        state.sched_editor = Some(ed);
    }
}

/// VarAC's image shrinker: scale and recompress a picture before sending it.
pub fn draw_shrinker(ctx: &egui::Context, state: &mut GuiState) {
    let Some(mut sh) = state.shrinker.take() else { return };
    let mut open = true;
    let mut action: Option<bool> = None; // Some(true) = compressed, Some(false) = original
    let mut cancel = false;
    let his = state.connected_to.clone().unwrap_or_default();
    let overlay_text = crate::varac::images::overlay_text(
        &state.my_callsign, &his,
        state.qso_log.snr_sent.trim().parse().ok(), state.qso_log.snr_rcvd.trim().parse().ok(),
    );
    if sh.dirty {
        sh.dirty = false;
        sh.result = Some(crate::varac::images::shrink(&sh.original, sh.pct, sh.quality, sh.overlay.then_some(overlay_text.as_str())));
    }
    let (limit, snr, bw, fm) = state.send_conditions();
    let gz = |d: &[u8]| crate::varac::bbs::compress(d).map(|c| c.len()).unwrap_or(d.len());
    let air = |d: &[u8]| crate::varac::images::air_bytes(gz(d));

    egui::Window::new("Image shrinker")
        .open(&mut open)
        .resizable(false)
        .show(ctx, |ui| {
            ui.label(RichText::new(sh.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()).strong());
            ui.label(RichText::new(format!("Original: {} Bytes, {} X {} — {}", sh.original.len(), sh.size.0, sh.size.1,
                // (a picture hardly compresses: its own size, not gzipping megabytes every frame)
                crate::varac::images::time_estimate(crate::varac::images::air_bytes(sh.original.len()), snr, bw, fm))).small().color(GRAY));
            ui.label(RichText::new(format!(
                "Starts at the size that sends in {} (Settings → Files)",
                crate::varac::images::photo_minutes_label(state.settings_form.photo_send_minutes)
            )).small().color(GRAY));
            ui.add_space(6.0);
            // (recompressed when the slider is let go, not on every step of a drag)
            let r = ui.add(egui::Slider::new(&mut sh.pct, 1..=100).text("Resolution %"));
            if r.drag_stopped() || (r.changed() && !r.dragged()) {
                sh.dirty = true;
            }
            let r = ui.add(egui::Slider::new(&mut sh.quality, 0..=50).step_by(2.0).text("Quality"));
            if r.drag_stopped() || (r.changed() && !r.dragged()) {
                sh.dirty = true;
            }
            if ui.checkbox(&mut sh.overlay, format!("Add \"{overlay_text}\" on top")).changed() {
                sh.dirty = true;
            }
            ui.add_space(4.0);
            match &sh.result {
                Some(Ok((jpg, w, h))) => {
                    ui.label(RichText::new(format!("Compressed: {} Bytes, {w} X {h} — {}", jpg.len(),
                        crate::varac::images::time_estimate(air(jpg), snr, bw, fm))).strong().color(theme::GREEN));
                    if let Some(l) = limit.filter(|l| gz(jpg) >= *l) {
                        ui.colored_label(RED, format!("Too big: VARA HF allows under {} KB compressed at {bw} Hz. Shrink it more.", l / 1000));
                    }
                }
                Some(Err(e)) => {
                    ui.colored_label(RED, format!("Can't compress: {e}"));
                }
                None => {}
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let under = |n: usize| limit.is_none_or(|l| n < l);
                if ui.add_enabled(under(sh.original.len()), egui::Button::new("Send original")).clicked() {
                    action = Some(false);
                }
                let ok = matches!(&sh.result, Some(Ok((jpg, ..))) if under(gz(jpg)));
                if ui.add_enabled(ok, egui::Button::new(RichText::new("Send compressed").strong())).clicked() {
                    action = Some(true);
                }
                if ui.button("Save compressed…").clicked() {
                    if let Some(Ok((jpg, _, _))) = &sh.result {
                        let stem = sh.path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "image".into());
                        if let Some(p) = super::dialog::FileDialog::new().set_file_name(format!("{stem}_resized.jpg")).save_file() {
                            let _ = std::fs::write(p, jpg);
                        }
                    }
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });
    if cancel {
        open = false;
    }
    match action {
        Some(false) => {
            state.sent_files.push(sh.path.clone());
            let name = sh.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            state.tx_file_next = Some((name, sh.original.len()));
            state.send_cmd(GuiCommand::SendFile(sh.path.clone()));
            state.status_msg = format!("Sending file: {}", sh.path.display());
        }
        Some(true) => {
            if let Some(Ok((jpg, _, _))) = &sh.result {
                // <name>_resized_<yyyyMMdd_HHmmss>.jpg (UTC), next to the original if we may
                let stem = sh.path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "image".into());
                let name = format!("{stem}_resized_{}.jpg", Utc::now().format("%Y%m%d_%H%M%S"));
                let beside = sh.path.with_file_name(&name);
                let target = if std::fs::write(&beside, jpg).is_ok() {
                    beside
                } else {
                    let p = std::env::temp_dir().join(&name);
                    let _ = std::fs::write(&p, jpg);
                    p
                };
                state.sent_files.push(target.clone());
                state.status_msg = format!("Sending file: {}", target.display());
                state.tx_file_next = Some((name, gz(jpg)));
                state.send_cmd(GuiCommand::SendFile(target));
            }
        }
        None if open => state.shrinker = Some(sh),
        None => {}
    }
}

/// VarAC's gallery: files sent and received, newest first, pictures as thumbnails.
pub fn draw_gallery(ctx: &egui::Context, state: &mut GuiState, db: &Storage) {
    if !state.gallery_open {
        return;
    }
    let mut open = true;
    egui::Window::new("Gallery")
        .open(&mut open)
        .default_size([820.0, 560.0])
        .resizable(true)
        .show(ctx, |ui| {
            let files = cached(ui.ctx(), "gallery", "", 5.0, || db.logged_files().unwrap_or_default());
            if files.is_empty() {
                ui.label(RichText::new("No files sent or received yet").italics().color(GRAY));
            }
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for (path, call, time, outgoing) in files {
                        let p = std::path::PathBuf::from(&path);
                        let name: String = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                        let short = if name.chars().count() > 25 { format!("{}...", name.chars().take(25).collect::<String>()) } else { name };
                        Frame::none().fill(theme::SURFACE_2).rounding(egui::Rounding::same(6.0)).inner_margin(Margin::same(6.0)).show(ui, |ui| {
                            ui.set_width(180.0);
                            ui.vertical_centered(|ui| {
                                let r = if crate::varac::images::is_image(&p) && p.exists() {
                                    ui.add(egui::Image::new(format!("file://{}", p.display())).max_height(120.0).max_width(170.0).sense(egui::Sense::click()))
                                } else {
                                    ui.add(egui::Label::new(RichText::new("📄").size(48.0)).sense(egui::Sense::click()))
                                };
                                if r.on_hover_text(&path).clicked() {
                                    let _ = open::that(&p);
                                }
                                ui.label(RichText::new(short).small());
                                let dir = if outgoing { "to" } else { "from" };
                                ui.label(RichText::new(format!("{dir} {call}, {}", time.format("%b %d, %Y"))).small().color(GRAY));
                            });
                        });
                    }
                });
            });
        });
    state.gallery_open = open && state.gallery_open;
}

/// VarAC's QSL card generator: the card for a QSO on your picture; save it as JPEG or
/// open an email for it.
pub fn draw_qsl(ctx: &egui::Context, state: &mut GuiState) {
    let Some(mut q) = state.qsl.take() else { return };
    if q.dirty {
        q.dirty = false;
        let bg = (!q.background.trim().is_empty()).then(|| std::fs::read(q.background.trim()).ok()).flatten();
        match crate::varac::images::render_qsl(&q.card, bg.as_deref(), q.fade) {
            Ok(img) => {
                let size = [img.width() as usize, img.height() as usize];
                let ci = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
                q.texture = Some(ctx.load_texture("qsl_card", ci, Default::default()));
                q.error = None;
            }
            Err(e) => q.error = Some(e),
        }
    }
    let mut open = true;
    egui::Window::new("QSL card generator")
        .open(&mut open)
        .default_width(720.0)
        .resizable(true)
        .show(ctx, |ui| {
            if let Some(t) = &q.texture {
                ui.add(egui::Image::new(t).max_width(ui.available_width()).max_height(420.0));
            }
            if let Some(e) = &q.error {
                ui.colored_label(RED, e);
            }
            ui.separator();
            egui::Grid::new("qsl_fields").num_columns(4).spacing([8.0, 4.0]).show(ui, |ui| {
                let c = &mut q.card;
                let mut changed = false;
                for (label, v) in [("CALLSIGN", &mut c.call), ("RST-S", &mut c.rst_sent), ("RST-R", &mut c.rst_rcvd), ("BAND", &mut c.band)] {
                    ui.label(label);
                    changed |= ui.add(TextEdit::singleline(v).desired_width(90.0)).lost_focus();
                }
                ui.end_row();
                ui.label("TIME UTC");
                changed |= ui.add(TextEdit::singleline(&mut c.time_utc).desired_width(170.0)).lost_focus();
                ui.label("POWER (W)");
                changed |= ui.add(TextEdit::singleline(&mut c.power).desired_width(60.0)).lost_focus();
                ui.end_row();
                ui.label("MESSAGE");
                changed |= ui.add(TextEdit::singleline(&mut c.message).desired_width(300.0)).lost_focus();
                ui.end_row();
                if changed {
                    q.dirty = true;
                }
            });
            ui.horizontal(|ui| {
                ui.label("Picture:");
                if ui.add(TextEdit::singleline(&mut q.background).desired_width(280.0).hint_text("JPG/PNG (empty = white)")).lost_focus() {
                    q.dirty = true;
                }
                if ui.small_button("Browse…").clicked() {
                    if let Some(p) = super::dialog::FileDialog::new().add_filter("Image file (JPG/PNG)", &["jpg", "jpeg", "png"]).pick_file() {
                        q.background = p.display().to_string();
                        q.dirty = true;
                    }
                }
                if ui.checkbox(&mut q.fade, "Fade to black at the bottom").changed() {
                    q.dirty = true;
                }
            });
            ui.horizontal(|ui| {
                if ui.add(theme::accent_button("SAVE AS JPEG", GREEN)).clicked() {
                    let bg = (!q.background.trim().is_empty()).then(|| std::fs::read(q.background.trim()).ok()).flatten();
                    match crate::varac::images::render_qsl(&q.card, bg.as_deref(), q.fade).and_then(|i| crate::varac::images::to_jpeg(&i, 90)) {
                        Ok(jpg) => {
                            if let Some(p) = super::dialog::FileDialog::new().set_file_name(crate::varac::images::qsl_file_name(&q.card)).save_file() {
                                match std::fs::write(&p, jpg) {
                                    Ok(()) => state.status_msg = format!("QSL card saved: {}", p.display()),
                                    Err(e) => q.error = Some(e.to_string()),
                                }
                            }
                        }
                        Err(e) => q.error = Some(e),
                    }
                }
                if ui.button("Email…").on_hover_text("Opens your mail program; attach the saved card").clicked() {
                    let subject = format!("QSL Card from {}", q.card.my_call);
                    let body = format!("Here is a QSL card from our VARA QSO on {}.\n73s, de {}\n", q.card.time_utc, q.card.my_call);
                    let enc = |s: &str| s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect::<String>();
                    let _ = open::that(format!("mailto:?subject={}&body={}", enc(&subject), enc(&body)));
                }
                if ui.button("Keep these settings").on_hover_text("Picture, message and fade for the next cards").clicked() {
                    let (bg, fade) = (q.background.clone(), q.fade);
                    crate::config::update_config_file(|c| {
                        c.qsl_image_file = Some(bg).filter(|b| !b.trim().is_empty());
                        c.qsl_fade = Some(fade);
                    });
                }
            });
        });
    if open {
        state.qsl = Some(q);
    }
}

/// VarAC's map: stations heard and worked lately, on a world grid of Maidenhead fields
/// (equirectangular, as VarAC projects it). Beacons red, CQs orange, broadcasts magenta,
/// QSOs lime; lines from us to them.
pub fn draw_map(ctx: &egui::Context, state: &mut GuiState, db: &Storage) {
    if !state.map_open {
        return;
    }
    let mut open = true;
    egui::Window::new("Map")
        .open(&mut open)
        .default_size([900.0, 520.0])
        .resizable(true)
        .show(ctx, |ui| {
            let mut lines = ui.ctx().data(|d| d.get_temp::<bool>(egui::Id::new("map_lines"))).unwrap_or(true);
            let mut labels = ui.ctx().data(|d| d.get_temp::<bool>(egui::Id::new("map_labels"))).unwrap_or(true);
            ui.horizontal(|ui| {
                ui.label("Last");
                egui::ComboBox::from_id_salt("map_minutes").selected_text(match state.map_minutes {
                    15 => "15 Minutes", 60 => "1 Hour", 180 => "3 Hours", 720 => "12 Hours", 1440 => "24 Hours",
                    10080 => "1 Week", 43200 => "1 Month", _ => "1 Year",
                }).show_ui(ui, |ui| {
                    for (m, t) in [(15, "15 Minutes"), (60, "1 Hour"), (180, "3 Hours"), (720, "12 Hours"), (1440, "24 Hours"), (10080, "1 Week"), (43200, "1 Month"), (525600, "1 Year")] {
                        ui.selectable_value(&mut state.map_minutes, m, t);
                    }
                });
                ui.checkbox(&mut lines, "Links");
                ui.checkbox(&mut labels, "Callsigns");
                for (c, t) in [(Color32::RED, "Beacon"), (Color32::from_rgb(255, 165, 0), "CQ"), (Color32::from_rgb(255, 0, 255), "Broadcast"), (Color32::from_rgb(0, 255, 0), "QSO")] {
                    ui.colored_label(c, format!("● {t}"));
                }
            });
            ui.ctx().data_mut(|d| {
                d.insert_temp(egui::Id::new("map_lines"), lines);
                d.insert_temp(egui::Id::new("map_labels"), labels);
            });
            let (rect, resp) = ui.allocate_exact_size(ui.available_size().max(Vec2::new(300.0, 160.0)), egui::Sense::hover());
            let p = ui.painter_at(rect);
            p.rect_filled(rect, 4.0, Color32::from_rgb(14, 30, 52));
            let to_screen = |lat: f64, lon: f64| egui::pos2(
                rect.left() + ((lon + 180.0) / 360.0) as f32 * rect.width(),
                rect.top() + ((90.0 - lat) / 180.0) as f32 * rect.height(),
            );
            // Maidenhead fields: 20° × 10°
            let grid = Stroke::new(0.5_f32, Color32::from_rgb(40, 70, 105));
            for i in 0..=18 {
                let x = rect.left() + i as f32 / 18.0 * rect.width();
                p.line_segment([egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())], grid);
                let y = rect.top() + i as f32 / 18.0 * rect.height();
                p.line_segment([egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)], grid);
            }
            for fx in 0..18u8 {
                for fy in 0..18u8 {
                    let c = to_screen(-90.0 + fy as f64 * 10.0 + 5.0, -180.0 + fx as f64 * 20.0 + 10.0);
                    p.text(c, egui::Align2::CENTER_CENTER, format!("{}{}", (b'A' + fx) as char, (b'A' + fy) as char),
                        egui::FontId::proportional(9.0), Color32::from_rgb(55, 90, 130));
                }
            }
            let me = maidenhead_to_latlon(&state.settings_form.locator).map(|(a, b)| to_screen(a, b));
            let stations = cached(ui.ctx(), "map", &state.map_minutes.to_string(), 10.0, || db.map_stations(state.map_minutes).unwrap_or_default());
            // one dot per station and kind
            let mut seen = std::collections::HashSet::new();
            let mut hover: Option<String> = None;
            let mouse = resp.hover_pos();
            for (call, loc, kind) in stations {
                if !seen.insert((call.clone(), kind)) {
                    continue;
                }
                let Some((lat, lon)) = maidenhead_to_latlon(&loc) else { continue };
                let pt = to_screen(lat, lon);
                let col = match kind {
                    1 => Color32::RED,
                    2 => Color32::from_rgb(255, 165, 0),
                    3 => Color32::from_rgb(255, 0, 255),
                    _ => Color32::from_rgb(0, 255, 0),
                };
                if let (true, Some(m)) = (lines, me) {
                    p.line_segment([m, pt], Stroke::new(1.0_f32, col.gamma_multiply(0.45)));
                }
                p.circle_filled(pt, 4.0, col);
                if labels {
                    p.text(pt + Vec2::new(6.0, -6.0), egui::Align2::LEFT_BOTTOM, &call, egui::FontId::proportional(10.0), Color32::WHITE);
                }
                if mouse.is_some_and(|m| m.distance(pt) < 6.0) {
                    hover = Some(format!("{call}  {loc}"));
                }
            }
            if let Some(m) = me {
                p.circle_filled(m, 5.0, Color32::WHITE);
                p.circle_stroke(m, 8.0, Stroke::new(1.5_f32, theme::ACCENT));
            }
            if let Some(h) = hover {
                resp.on_hover_text(h);
            }
        });
    state.map_open = open && state.map_open;
}

/// VarAC's SNR chart: our SNR at the partner and theirs at us over the QSO — live
/// (`None`) or a past QSO (its guid).
pub fn draw_snr_chart(ctx: &egui::Context, state: &mut GuiState, db: &Storage) {
    let Some(which) = state.snr_chart.clone() else { return };
    let (his, mine, title) = match &which {
        None => (state.snr_his_series.clone(), state.snr_my_series.clone(),
            format!("Link with {} on {:.3} MHz", state.connected_to.clone().unwrap_or_else(|| "—".into()), state.dial() as f64 / 1e6)),
        Some(g) => {
            let (h, m) = cached(ctx, "snr_chart", g, 30.0, || db.snr_series(g).unwrap_or_default());
            (h, m, "Link (from the log)".to_string())
        }
    };
    if which.is_none() {
        ctx.request_repaint_after(std::time::Duration::from_secs(1));
    }
    let mut open = true;
    egui::Window::new("SNR chart")
        .open(&mut open)
        .default_size([640.0, 320.0])
        .resizable(true)
        .show(ctx, |ui| {
            ui.label(RichText::new(&title).strong());
            ui.horizontal(|ui| {
                ui.colored_label(Color32::from_rgb(30, 144, 255), format!("● Mine ({})", state.my_callsign));
                ui.colored_label(Color32::from_rgb(255, 140, 0), "● Partner");
                if ui.small_button("Copy data").clicked() {
                    let list = |s: &[(chrono::DateTime<Utc>, f32)]| s.iter().map(|(_, v)| format!("{v:+.0}")).collect::<Vec<_>>().join(",");
                    ui.output_mut(|o| o.copied_text = format!("{title}\nMy SNR report: {}\nPartner SNR report: {}", list(&mine), list(&his)));
                }
            });
            let (rect, _) = ui.allocate_exact_size(ui.available_size().max(Vec2::new(300.0, 160.0)), egui::Sense::hover());
            let p = ui.painter_at(rect);
            p.rect_filled(rect, 4.0, theme::SURFACE_2);
            let all: Vec<&(chrono::DateTime<Utc>, f32)> = his.iter().chain(mine.iter()).collect();
            if all.is_empty() {
                p.text(rect.center(), egui::Align2::CENTER_CENTER, "No SNR reports yet", egui::FontId::proportional(13.0), GRAY);
                return;
            }
            let t0 = all.iter().map(|x| x.0).min().unwrap_or_else(Utc::now);
            let t1 = all.iter().map(|x| x.0).max().unwrap_or_else(Utc::now).max(t0 + chrono::Duration::seconds(30));
            let (lo, hi) = all.iter().fold((f32::MAX, f32::MIN), |(a, b), x| (a.min(x.1), b.max(x.1)));
            let (lo, hi) = ((lo - 3.0).floor().min(-5.0), (hi + 3.0).ceil().max(5.0));
            let inner = rect.shrink2(Vec2::new(36.0, 18.0));
            let x_of = |t: chrono::DateTime<Utc>| inner.left() + (t - t0).num_milliseconds() as f32 / (t1 - t0).num_milliseconds().max(1) as f32 * inner.width();
            let y_of = |v: f32| inner.bottom() - (v - lo) / (hi - lo) * inner.height();
            let mut v = lo;
            while v <= hi {
                let y = y_of(v);
                p.line_segment([egui::pos2(inner.left(), y), egui::pos2(inner.right(), y)], Stroke::new(0.5_f32, theme::BORDER));
                p.text(egui::pos2(rect.left() + 4.0, y), egui::Align2::LEFT_CENTER, format!("{v:+.0}"), egui::FontId::proportional(10.0), GRAY);
                v += ((hi - lo) / 6.0).ceil().max(1.0);
            }
            p.text(egui::pos2(inner.left(), rect.bottom() - 2.0), egui::Align2::LEFT_BOTTOM, t0.format("%H:%M:%S").to_string(), egui::FontId::proportional(10.0), GRAY);
            p.text(egui::pos2(inner.right(), rect.bottom() - 2.0), egui::Align2::RIGHT_BOTTOM, t1.format("%H:%M:%S").to_string(), egui::FontId::proportional(10.0), GRAY);
            for (series, col) in [(&mine, Color32::from_rgb(30, 144, 255)), (&his, Color32::from_rgb(255, 140, 0))] {
                let pts: Vec<egui::Pos2> = series.iter().map(|(t, v)| egui::pos2(x_of(*t), y_of(*v))).collect();
                if pts.len() > 1 {
                    p.add(egui::Shape::line(pts.clone(), Stroke::new(2.0_f32, col)));
                }
                for pt in pts {
                    p.circle_filled(pt, 2.5, col);
                }
            }
        });
    if !open {
        state.snr_chart = None;
    }
}

/// Settings → Appearance and Sounds: VarAC's per-event sounds and the narrator.
pub fn draw_sound_settings(ui: &mut egui::Ui) {
    use crate::gui::sound::{self, Sound};
    let generation = ui.ctx().data(|d| d.get_temp::<f64>(egui::Id::new(("feature_cache_gen", "sound_cfg")))).unwrap_or(0.0);
    let (f, n) = cached(ui.ctx(), "sound_cfg", &generation.to_string(), 30.0, || (crate::config::read_config_file_pub(), crate::config::narrator()));
    let mut mute = f.mute_all_sounds.unwrap_or(false);
    let mut off = f.sounds_off.clone().unwrap_or_default();
    let mut changed = false;
    ui.label(RichText::new("Sounds").strong());
    changed |= ui.checkbox(&mut mute, "Mute all sounds").changed();
    egui::Grid::new("sounds_grid").num_columns(4).spacing([12.0, 2.0]).show(ui, |ui| {
        for (i, s) in Sound::ALL.iter().enumerate() {
            let mut on = !off.iter().any(|k| k == s.key());
            ui.horizontal(|ui| {
                if ui.add_enabled(!mute, egui::Checkbox::new(&mut on, s.label())).changed() {
                    off.retain(|k| k != s.key());
                    if !on {
                        off.push(s.key().to_string());
                    }
                    changed = true;
                }
                if ui.small_button("▶").on_hover_text("Play").clicked() {
                    sound::play_now(*s);
                }
            });
            if i % 2 == 1 {
                ui.end_row();
            }
        }
    });
    ui.add_space(6.0);
    ui.label(RichText::new("Narrator").strong());
    let mut nn = n;
    ui.horizontal(|ui| {
        changed |= ui.checkbox(&mut nn.on, "Speak events").changed();
        changed |= ui.radio_value(&mut nn.female, false, "Male voice").changed();
        changed |= ui.radio_value(&mut nn.female, true, "Female voice").changed();
        if ui.small_button("Test").clicked() {
            sound::say_test(&format!("This is {}", sound::phonetic(&f.callsign.clone().unwrap_or_default())));
        }
    });
    ui.add_enabled_ui(nn.on, |ui| {
        ui.horizontal_wrapped(|ui| {
            changed |= ui.checkbox(&mut nn.incoming, "Incoming messages").changed();
            changed |= ui.checkbox(&mut nn.alerts, "Alerts and notifications").changed();
            changed |= ui.checkbox(&mut nn.cqs, "CQs").changed();
            changed |= ui.checkbox(&mut nn.beacons, "Beacons").changed();
            changed |= ui.checkbox(&mut nn.my_beacons, "My beacons").changed();
            changed |= ui.checkbox(&mut nn.broadcasts, "Broadcasts").changed();
            changed |= ui.checkbox(&mut nn.my_broadcasts, "My broadcasts").changed();
        });
    });
    if changed {
        crate::config::update_config_file(|c| {
            c.mute_all_sounds = Some(mute);
            c.sounds_off = Some(off.clone());
            c.narrator = Some(nn.on);
            c.narrator_female = Some(nn.female);
            c.narrate_incoming = Some(nn.incoming);
            c.narrate_alerts = Some(nn.alerts);
            c.narrate_cqs = Some(nn.cqs);
            c.narrate_beacons = Some(nn.beacons);
            c.narrate_my_beacons = Some(nn.my_beacons);
            c.narrate_broadcasts = Some(nn.broadcasts);
            c.narrate_my_broadcasts = Some(nn.my_broadcasts);
        });
        sound::reload();
        uncache(ui.ctx(), "sound_cfg");
    }
}

/// VarAC's "Switch profile": every config file next to ours is a profile (all
/// settings: station, rig, VARA…). Switching restarts the app with that file.
pub fn draw_profiles(ctx: &egui::Context, state: &mut GuiState) {
    if !state.profiles_open {
        return;
    }
    let active = crate::config::active_config_path();
    let generation = ctx.data(|d| d.get_temp::<f64>(egui::Id::new(("feature_cache_gen", "profile_ptr")))).unwrap_or(0.0);
    let pointer = cached(ctx, "profile_ptr", &generation.to_string(), 5.0, || crate::config::default_profile_pointer().and_then(|p| std::fs::read_to_string(p).ok()).map(|s| s.trim().to_string()));
    let mut open = true;
    let mut switch_to: Option<std::path::PathBuf> = None;
    egui::Window::new("Switch profile")
        .open(&mut open)
        .default_width(560.0)
        .show(ctx, |ui| {
            ui.label(RichText::new("A profile is a whole settings file. Double-click one to switch to it (the app restarts).").small().color(GRAY));
            egui::Grid::new("profiles_grid").striped(true).spacing([14.0, 4.0]).show(ui, |ui| {
                for h in ["Creation time", "File name", "Status"] {
                    ui.label(RichText::new(h).small().strong().color(GRAY));
                }
                ui.end_row();
                let generation = ui.ctx().data(|d| d.get_temp::<f64>(egui::Id::new(("feature_cache_gen", "profiles")))).unwrap_or(0.0);
                for (p, created) in cached(ui.ctx(), "profiles", &generation.to_string(), 5.0, crate::config::profiles) {
                    let is_active = active.as_ref().is_some_and(|a| a == &p);
                    let t = created.map(|t| chrono::DateTime::<Utc>::from(t).format("%Y-%m-%d %H:%M").to_string()).unwrap_or_default();
                    ui.label(RichText::new(t).small());
                    let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    let r = ui.add(egui::Label::new(RichText::new(&name).strong()).sense(egui::Sense::click()));
                    if r.double_clicked() && !is_active {
                        switch_to = Some(p.clone());
                    }
                    ui.label(if is_active { RichText::new("Active").color(theme::GREEN) } else { RichText::new("") });
                    ui.end_row();
                }
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("New profile from the current settings:");
                ui.add(TextEdit::singleline(&mut state.profile_new_name).desired_width(140.0).hint_text("e.g. IC7300"));
                if ui.button("Create").clicked() && !state.profile_new_name.trim().is_empty() {
                    let name: String = state.profile_new_name.trim().chars().filter(|c| c.is_ascii_alphanumeric() || "-_".contains(*c)).collect();
                    if let (Some(dir), Some(src)) = (crate::config::config_dir(), active.as_ref()) {
                        let dst = dir.join(format!("openslot_{name}.toml"));
                        match std::fs::copy(src, &dst) {
                            Ok(_) => {
                                state.status_msg = format!("Profile created: {}", dst.display());
                                uncache(ui.ctx(), "profiles");
                            }
                            Err(e) => state.status_msg = format!("Profile not created: {e}"),
                        }
                        state.profile_new_name.clear();
                    }
                }
            });
            let mut remember = pointer.is_some();
            if ui.checkbox(&mut remember, "Load the last used profile on startup").changed() {
                crate::config::set_default_profile(if remember { active.as_deref() } else { None });
                uncache(ui.ctx(), "profile_ptr");
            }
        });
    if let Some(p) = switch_to {
        let busy = state.session_state != SessionState::Idle || state.ptt;
        if busy {
            state.add_system_chat("Operation can not be performed now: you are currently in connection, beaconing, or trying to connect a station.".into());
        } else {
            if crate::config::default_profile_pointer().is_some() {
                crate::config::set_default_profile(Some(&p));
            }
            // the same command line, with this config file
            let mut args: Vec<String> = Vec::new();
            let mut it = std::env::args().skip(1);
            while let Some(a) = it.next() {
                if a == "--config" {
                    it.next();
                } else if !a.starts_with("--config=") {
                    args.push(a);
                }
            }
            args.push("--config".into());
            args.push(p.display().to_string());
            match std::env::current_exe().and_then(|exe| std::process::Command::new(exe).args(&args).spawn()) {
                Ok(_) => {
                    crate::vara::launch::close();
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                Err(e) => state.status_msg = format!("Could not restart with {}: {e}", p.display()),
            }
        }
    }
    state.profiles_open = open && state.profiles_open;
}

/// VarAC asks about a VMail whose attachment is over the incoming files size limit.
pub fn draw_attachment_ask(ctx: &egui::Context, state: &mut GuiState) {
    let Some(size) = state.attachment_ask else { return };
    let from = state.connected_to.clone().unwrap_or_default();
    let mut answer = None;
    egui::Window::new("Incoming VMail with attachment")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
        .show(ctx, |ui| {
            ui.label(format!("{from} is sending you a VMail with an attachment of {size} bytes, over your incoming files size limit."));
            ui.label(RichText::new("Rejecting it rejects the whole VMail.").color(theme::YELLOW));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.add(theme::accent_button("ACCEPT", GREEN)).clicked() {
                    answer = Some(true);
                }
                if ui.add(theme::accent_button("REJECT", RED)).clicked() {
                    answer = Some(false);
                }
            });
        });
    if let Some(a) = answer {
        state.attachment_ask = None;
        state.send_cmd(GuiCommand::AnswerAttachment(a));
        if !a {
            state.pending_file_offer = None;
            state.pending_vmail_attachment = false;
        }
    }
}

/// VarAC's "ALE - Connect" window and its mid-link "ALE - QSY SUGGESTION".
pub fn draw_ale(ctx: &egui::Context, state: &mut GuiState) {
    use crate::varac::ale::{age_text, fmt_freq, Phase, RowState};
    let hms = Utc::now().format("%H:%M:%S").to_string();
    let mut actions = Vec::new();
    if let Some(w) = state.ale.as_mut() {
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
        let mut open = true;
        egui::Window::new("ALE - Connect")
            .open(&mut open)
            .default_width(640.0)
            .collapsible(false)
            .show(ctx, |ui| {
                let color = match w.phase {
                    Phase::Success(_) => Color32::from_rgb(50, 205, 50),
                    Phase::Finished => RED,
                    Phase::Calling => Color32::from_rgb(30, 144, 255),
                    Phase::Approval(_) if w.attempts > 0 || w.status.starts_with("Waiting") => Color32::from_rgb(255, 140, 0),
                    _ => theme::TEXT,
                };
                ui.label(RichText::new(&w.status).strong().size(14.0).color(color));
                ui.add_space(4.0);
                egui::Grid::new("ale_grid").striped(true).spacing([12.0, 3.0]).show(ui, |ui| {
                    for h in ["", "Frequency", "Avg SNR (dB)", "Beacons", "Last heard", "Basis"] {
                        ui.label(RichText::new(h).small().strong().color(GRAY));
                    }
                    ui.end_row();
                    for i in 0..w.rows.len() {
                        let r = w.rows[i].clone();
                        let icon_color = match r.state {
                            RowState::Calling => Color32::from_rgb(30, 144, 255),
                            RowState::Failed => Color32::RED,
                            RowState::Connected => Color32::from_rgb(50, 205, 50),
                            _ => GRAY,
                        };
                        let grey = r.state == RowState::Blocked;
                        let txt = |s: String| if grey { RichText::new(s).color(GRAY) } else { RichText::new(s) };
                        ui.label(RichText::new(r.state.icon()).color(icon_color));
                        let sel = i == w.selected;
                        if ui.selectable_label(sel, txt(fmt_freq(r.freq_hz)).strong()).clicked() {
                            actions.extend(w.click_row(i, &hms));
                        }
                        match &r.candidate {
                            Some(c) => {
                                let snr_col = if c.avg_snr > -10.0 { theme::GREEN } else if c.avg_snr > -15.0 { theme::YELLOW } else { theme::RED };
                                ui.label(if grey { txt(format!("{:.1}", c.avg_snr)) } else { RichText::new(format!("{:.1}", c.avg_snr)).color(snr_col) });
                                ui.label(txt(c.beacons.to_string()));
                                ui.label(txt(age_text(c.minutes_since_heard)));
                                ui.label(txt(if c.reason.is_empty() { c.basis.clone() } else { c.reason.clone() }));
                            }
                            None => {
                                for v in ["-", "-", "-", "Stay on current frequency"] {
                                    ui.label(v);
                                }
                            }
                        }
                        ui.end_row();
                    }
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    match w.phase {
                        Phase::Approval(t) => {
                            let label = match t { Some(t) => format!("CONNECT ({})", t.div_ceil(10)), None => "CONNECT".into() };
                            if ui.add(theme::accent_button(label, GREEN)).clicked() {
                                actions.extend(w.start_attempt(&hms));
                            }
                        }
                        Phase::Finished => {
                            if ui.button("TRY AGAIN").clicked() {
                                w.try_again();
                            }
                            if ui.button(format!("RETURN TO {}", fmt_freq(w.start_freq))).clicked() {
                                actions.extend(w.return_to_start());
                            }
                        }
                        _ => {}
                    }
                    if ui.button(w.cancel_label()).clicked() {
                        actions.extend(w.cancel());
                    }
                });
                ui.separator();
                egui::ScrollArea::vertical().max_height(120.0).stick_to_bottom(true).show(ui, |ui| {
                    for l in &w.timeline {
                        ui.label(RichText::new(l).small().monospace().color(GRAY));
                    }
                });
            });
        if !open {
            // closing during a call aborts it
            actions.extend(if w.phase == Phase::Calling { w.cancel() } else { Vec::new() });
            actions.push(crate::varac::ale::Action::Close);
        }
    }
    // (the app carries these out on its next tick)
    state.ale_actions.extend(actions);

    if let Some((freq, until)) = state.ale_suggestion {
        let left = (until - Utc::now()).num_seconds();
        let call = state.connected_to.clone().unwrap_or_default();
        let mut answer: Option<bool> = if left < 0 || state.session_state != SessionState::Connected { Some(false) } else { None };
        egui::Window::new("ALE - QSY SUGGESTION")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label(RichText::new("QSY NOW").strong().size(16.0).color(theme::YELLOW));
                ui.label(format!("Link quality is deteriorating.\n\nBest known alternative for {call} is {}.\n\nClick ACCEPT to send a QSY invitation.", fmt_freq(freq)));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.add(theme::accent_button("QSY", GREEN)).clicked() {
                        answer = Some(true);
                    }
                    if ui.button(format!("REJECT ({})", left.max(0))).clicked() {
                        answer = Some(false);
                    }
                });
            });
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
        if let Some(yes) = answer {
            state.ale_suggestion = None;
            if yes && state.session_state == SessionState::Connected {
                // VarAC: <Q:(new − current)/10> to the partner
                let offset = (freq as i64 - state.dial() as i64) / 10 * 10;
                state.send_cmd(GuiCommand::QsyRequest { offset_hz: offset as i32 });
                state.add_log(format!("ALE: QSY invitation to {freq} sent"));
            } else {
                state.ale_declined_at = Some(Utc::now());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Dropping a cached value after a settings change must not freeze the app (it once
    /// deadlocked on egui's context lock).
    #[test]
    fn uncache_does_not_deadlock() {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let ctx = egui::Context::default();
            let _ = ctx.run(Default::default(), |ctx| {
                let a = cached(ctx, "t", "0", 30.0, || 1);
                uncache(ctx, "t");
                let generation = ctx.data(|d| d.get_temp::<f64>(egui::Id::new(("feature_cache_gen", "t")))).unwrap_or(-1.0);
                let b = cached(ctx, "t", &generation.to_string(), 30.0, || 2);
                let _ = tx.send((a, b));
            });
        });
        assert_eq!(rx.recv_timeout(std::time::Duration::from_secs(5)), Ok((1, 2)), "uncache froze or didn't reload");
    }
}
