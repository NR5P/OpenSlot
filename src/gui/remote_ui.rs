//! Remote mode in the GUI (see crate::remote): the Remote control window (serve this
//! station, or control another), the station's side (what it publishes, and doing what
//! remotes ask as its own buttons would), and the console a remote shows in place of the
//! main window while it controls a station.

use eframe::egui::{self, Align, Frame, Layout, Margin, RichText, Vec2};

use super::rig_panel::{draw_rig_controls, RigPanel};
use super::state::GuiState;
use crate::varac::session::SessionState;
use super::theme;
use crate::config::RemoteSettings;
use crate::remote::client::{Phase, RemoteClient};
use crate::remote::{ChatItem, Command, Found, RigView, StationItem, Stations, Status, DEFAULT_PORT};

/// The Remote control window's fields, and what the app reports back into it.
pub struct RemoteForm {
    pub window_open: bool,
    pub settings: RemoteSettings,
    pub port: String,
    pub connect_port: String,
    /// Asked of the app: (re)start or stop serving, connect, disconnect
    pub apply_server: bool,
    pub connect_requested: bool,
    pub disconnect_requested: bool,
    /// Serving: the port, remotes connected, listening, the audio's note; else why not
    pub serving: Option<(u16, usize, usize, String)>,
    pub serve_error: Option<String>,
    pub phone_url: Option<String>,
    pub audio_inputs: Option<Vec<(String, String)>>,
    pub found: Vec<Found>,
    pub searching: Option<std::sync::mpsc::Receiver<Vec<Found>>>,
    // The remote console
    pub input: String,
    pub call: String,
    pub freq_entry: String,
    pub rig_open: bool,
    pub volume: f32,
}

impl Default for RemoteForm {
    fn default() -> Self {
        let settings = crate::config::remote_settings();
        Self {
            port: settings.port.unwrap_or(DEFAULT_PORT).to_string(),
            connect_port: settings.connect_port.unwrap_or(DEFAULT_PORT).to_string(),
            settings,
            window_open: false,
            apply_server: false,
            connect_requested: false,
            disconnect_requested: false,
            serving: None,
            serve_error: None,
            phone_url: None,
            audio_inputs: None,
            found: Vec::new(),
            searching: None,
            input: String::new(),
            call: String::new(),
            freq_entry: String::new(),
            rig_open: false,
            volume: 0.8,
        }
    }
}

// ---------------------------------------------------------------- the station's side

fn link_name(s: SessionState) -> &'static str {
    match s {
        SessionState::Idle => "idle",
        SessionState::Connecting => "connecting",
        SessionState::Connected => "connected",
        SessionState::AwaitingDisconnect => "disconnecting",
    }
}

fn station_item(e: &super::state::CqEntry) -> StationItem {
    StationItem {
        time: e.time.clone(),
        call: e.callsign.clone(),
        snr: e.snr.clone(),
        locator: e.locator.clone(),
        bandwidth: e.bandwidth,
        beacon: e.is_beacon,
    }
}

/// What the station publishes to its remotes, from the main screen's state.
pub fn snapshot(state: &GuiState, server: &crate::remote::server::RemoteServer) -> (Status, Vec<ChatItem>, Stations, RigView) {
    let dial = state.dial();
    let status = Status {
        callsign: state.my_callsign.clone(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        modem: state.modem_connected,
        link: link_name(state.session_state).into(),
        partner: state.connected_to.clone(),
        partner_name: state.partner_name.clone(),
        partner_qth: state.partner_qth.clone(),
        partner_locator: state.partner_locator.clone(),
        tx: state.ptt,
        busy: state.busy,
        snr: state.snr_last.or(state.snr),
        bandwidth: state.settings_form.bandwidth.clone(),
        frequency: dial,
        calling_frequency: state.current_frequency,
        band: state.current_band.clone(),
        on_calling_frequency: crate::varac::messages::is_calling_frequency(dial),
        cq_active: state.cq_active || state.slot_wait.is_some(),
        beacon_auto: state.beacon_auto_enabled,
        buffer: state.buffer_bytes,
        ptt_disabled: state.ptt_disabled,
        unread_vmail: state.unread_vmails,
        status: state.status_msg.clone(),
        audio: server.audio_available(),
        audio_note: server.audio_note(),
    };
    let skip = state.chat_lines.len().saturating_sub(200);
    let chat = state.chat_lines.iter().skip(skip).map(|l| ChatItem {
        time: l.time.clone(),
        call: l.callsign.clone(),
        text: l.text.clone(),
        out: l.is_outgoing,
        system: l.is_system,
    }).collect();
    let stations = Stations {
        cq: state.cq_entries.iter().take(50).map(station_item).collect(),
        beacons: state.beacon_entries.iter().take(50).map(station_item).collect(),
    };
    let rig = RigView {
        backend: state.rig_backend.to_string(),
        connected: state.rig_connected,
        frequency: state.rig_freq,
        mode: state.rig_mode,
        // (keyed through CAT, or VARA keying it by VOX or its own PTT line)
        ptt: state.rig_ptt || state.ptt,
        extras: state.rig_extras.clone(),
        may_transmit: !state.ptt_disabled,
    };
    (status, chat, stations, rig)
}

/// Does what a remote asked, as the station's own controls do. Returns a reason when it
/// can't (for the remotes), and logs what was done.
pub fn apply(state: &mut GuiState, db: &crate::storage::Storage, qso_guid: &Option<String>, who: &str, cmd: Command) -> Option<String> {
    use super::state::GuiCommand;
    let in_qso = matches!(state.session_state, SessionState::Connected | SessionState::AwaitingDisconnect);
    let what = match &cmd {
        Command::Chat { .. } => "chat".to_string(),
        Command::Connect { call } => format!("connect to {call}"),
        Command::Frequency { hz } => format!("tune to {:.4} MHz", *hz as f64 / 1e6),
        Command::Rig { action } => format!("rig: {action:?}"),
        other => format!("{other:?}").to_lowercase(),
    };
    state.add_log(format!("Remote ({who}): {what}"));
    match cmd {
        Command::Chat { text } => {
            if state.session_state != SessionState::Connected {
                return Some("Not in a QSO: connect first".into());
            }
            if state.ping_active {
                return Some("Can't send during a ping".into());
            }
            if text.trim().is_empty() {
                return None;
            }
            super::layout::send_typed(state, db, qso_guid, text);
        }
        Command::Connect { call } => {
            if in_qso {
                return Some("Already in a QSO".into());
            }
            if call.trim().is_empty() {
                return Some("No callsign to connect to".into());
            }
            state.connect_callsign = call.trim().to_uppercase();
            super::layout::try_connect(state);
            if !state.connect_callsign.is_empty() {
                // (refused: try_connect said why in the status line)
                state.connect_callsign.clear();
                return Some(state.status_msg.clone());
            }
        }
        Command::Disconnect => state.send_cmd(GuiCommand::Disconnect),
        Command::Abort => state.send_cmd(GuiCommand::Abort),
        Command::Cq => {
            if !state.modem_connected {
                return Some("Modem not connected".into());
            }
            if state.cq_active || in_qso {
                return Some(if in_qso { "In a QSO".into() } else { "Already calling CQ".into() });
            }
            // (on the slot the operator chose last, which needs the rig to move there)
            let slot = if state.rig_connected { state.cq_slot } else { 0 };
            state.call_cq_on_slot(slot);
        }
        Command::EndCq => {
            state.cq_active = false;
            state.send_cmd(GuiCommand::Abort);
            if state.slot_wait.is_some() || state.pending_slot_qsy.is_some() {
                state.return_to_cf();
            }
            state.status_msg = "CQ stopped".into();
        }
        Command::Beacon => {
            if !state.modem_connected {
                return Some("Modem not connected".into());
            }
            super::layout::beacon_now(state);
        }
        Command::AutoBeacon { on } => state.beacon_auto_enabled = on,
        Command::Frequency { hz } => {
            if !(100_000..=500_000_000).contains(&hz) {
                return Some(format!("Not a frequency: {hz} Hz"));
            }
            if state.rig_backend == crate::rig::RigBackend::None {
                return Some("The station has no rig control".into());
            }
            state.current_frequency = hz;
            state.current_band = super::freq_to_band(hz).unwrap_or("").to_string();
            state.tune_to = Some(hz);
        }
        Command::Rig { action } => {
            if !state.rig_connected {
                return Some("The station's radio isn't connected".into());
            }
            state.rig_actions.push(action);
        }
        // (the server answers these itself)
        Command::Audio { .. } => {}
    }
    None
}

// ---------------------------------------------------------------- the Remote control window

pub fn draw_remote_window(ctx: &egui::Context, state: &mut GuiState, client: Option<&RemoteClient>) {
    let f = &mut state.remote;
    if !f.window_open {
        return;
    }
    if let Some(found) = f.searching.as_ref().and_then(|rx| rx.try_recv().ok()) {
        f.found = found;
        f.searching = None;
    }
    let mut open = true;
    egui::Window::new("Remote control")
        .open(&mut open)
        .default_width(460.0)
        .resizable(false)
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            theme::caption(ui, "This station");
            ui.label(RichText::new("Let other computers and phones on your network run this Openslot: chat, CQ, beacons, \
                the frequency, the rig controls, and listen to the radio.").small().color(theme::MUTED));
            ui.checkbox(&mut f.settings.serve, "Allow remote control of this station");
            egui::Grid::new("remote_serve").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                ui.label("Port");
                ui.add(egui::TextEdit::singleline(&mut f.port).desired_width(70.0));
                ui.end_row();
                ui.label("Password");
                ui.add(egui::TextEdit::singleline(&mut f.settings.password).password(true).desired_width(180.0)
                    .hint_text("needed: a remote can transmit"));
                ui.end_row();
                ui.label("Radio audio");
                if crate::remote::audio::can_capture() {
                    let inputs = f.audio_inputs.get_or_insert_with(crate::remote::audio::inputs).clone();
                    let describe = |id: &str| inputs.iter().find(|(i, _)| i == id).map(|(_, d)| d.clone()).unwrap_or_else(|| id.to_string());
                    let default = crate::remote::audio::default_input();
                    let modem = if default.is_empty() { "None (no audio for remotes)".to_string() } else { format!("The modem's input ({})", describe(&default)) };
                    let shown = if f.settings.audio_device.is_empty() { modem.clone() } else { describe(&f.settings.audio_device) };
                    egui::ComboBox::from_id_salt("remote_audio_in").width(260.0).selected_text(shown).show_ui(ui, |ui| {
                        ui.selectable_value(&mut f.settings.audio_device, String::new(), modem);
                        for (id, label) in &inputs {
                            ui.selectable_value(&mut f.settings.audio_device, id.clone(), label);
                        }
                    });
                } else {
                    ui.label(RichText::new("Not in this build (the Mac, Windows and Linux apps send it)").small().color(theme::MUTED));
                }
                ui.end_row();
            });
            ui.horizontal(|ui| {
                if ui.button(if f.settings.serve { "Apply" } else { "Apply (off)" }).clicked() {
                    f.settings.port = f.port.trim().parse().ok().filter(|&p| p > 0);
                    f.apply_server = true;
                }
                match (&f.serving, &f.serve_error) {
                    (Some((port, n, listening, _)), _) => {
                        let who = match n { 0 => "no remotes connected".to_string(), 1 => "1 remote connected".to_string(), n => format!("{n} remotes connected") };
                        let ears = if *listening > 0 { format!(", {listening} listening") } else { String::new() };
                        ui.label(RichText::new(format!("On, port {port}: {who}{ears}")).color(theme::GREEN));
                    }
                    (None, Some(e)) => {
                        ui.label(RichText::new(e).color(theme::RED));
                    }
                    (None, None) => {
                        ui.label(RichText::new("Off").color(theme::MUTED));
                    }
                }
            });
            if let Some((_, _, _, note)) = &f.serving {
                if let Some(url) = &f.phone_url {
                    ui.horizontal(|ui| {
                        ui.label("On a phone, open");
                        ui.label(RichText::new(url).monospace().color(theme::ACCENT));
                        if ui.small_button("Copy").clicked() {
                            ui.ctx().copy_text(url.clone());
                        }
                    });
                }
                if !note.is_empty() {
                    ui.label(RichText::new(format!("Audio: {note}")).small().color(theme::MUTED));
                }
            }

            ui.add_space(6.0);
            ui.separator();
            theme::caption(ui, "Control another Openslot");
            if let Some(c) = client {
                ui.horizontal(|ui| {
                    let v = c.view();
                    ui.label(format!("{} at {}:{}", if v.name.is_empty() { "Station" } else { &v.name }, c.config.host, c.config.port));
                    phase_label(ui, &v.phase);
                    if ui.button("Disconnect").clicked() {
                        f.disconnect_requested = true;
                    }
                });
            } else {
                ui.horizontal(|ui| {
                    if ui.add_enabled(f.searching.is_none(), egui::Button::new("Find stations")).clicked() {
                        let (tx, rx) = std::sync::mpsc::channel();
                        std::thread::spawn(move || {
                            let _ = tx.send(crate::remote::discover(std::time::Duration::from_millis(1500)));
                        });
                        f.searching = Some(rx);
                    }
                    if f.searching.is_some() {
                        ui.spinner();
                        ui.label(RichText::new("Looking on your network…").color(theme::MUTED));
                    }
                });
                for s in f.found.clone() {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&s.name).strong());
                        ui.label(RichText::new(format!("{}:{}{}", s.host, s.port, if s.audio { " · audio" } else { "" })).monospace().color(theme::MUTED));
                        if ui.small_button("Use").clicked() {
                            f.settings.connect_host = s.host.clone();
                            f.connect_port = s.port.to_string();
                        }
                    });
                }
                egui::Grid::new("remote_connect").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                    ui.label("Address");
                    ui.add(egui::TextEdit::singleline(&mut f.settings.connect_host).hint_text("192.168.1.20").desired_width(180.0));
                    ui.end_row();
                    ui.label("Port");
                    ui.add(egui::TextEdit::singleline(&mut f.connect_port).desired_width(70.0));
                    ui.end_row();
                    ui.label("Password");
                    ui.add(egui::TextEdit::singleline(&mut f.settings.connect_password).password(true).desired_width(180.0));
                    ui.end_row();
                });
                let ready = !f.settings.connect_host.trim().is_empty() && f.connect_port.trim().parse::<u16>().is_ok();
                if ui.add_enabled(ready, theme::primary_button("CONNECT", theme::ACCENT)).clicked() {
                    f.settings.connect_port = f.connect_port.trim().parse().ok();
                    f.connect_requested = true;
                }
            }
        });
    f.window_open = open;
}

fn phase_label(ui: &mut egui::Ui, phase: &Phase) {
    let (text, color) = match phase {
        Phase::Connecting => ("connecting…".to_string(), theme::YELLOW),
        Phase::Connected => ("connected".to_string(), theme::GREEN),
        Phase::Retrying(why) => (format!("reconnecting ({why})"), theme::YELLOW),
        Phase::Denied(why) => (format!("refused: {why}"), theme::RED),
    };
    ui.label(RichText::new(text).color(color));
}

// ---------------------------------------------------------------- the remote console

/// The main window while controlling a station: its essentials, driven over the network.
pub fn draw_console(ctx: &egui::Context, state: &mut GuiState, client: &RemoteClient) {
    let v = client.view();
    let f = &mut state.remote;
    let mut cmds: Vec<Command> = Vec::new();
    let connected = v.phase == Phase::Connected;
    let st = v.status.clone().unwrap_or_default();
    let in_qso = matches!(st.link.as_str(), "connected" | "disconnecting");

    // Who we're controlling, and the audio
    egui::TopBottomPanel::top("remote_banner")
        .frame(Frame::none().fill(theme::tint(theme::ACCENT, 28)).inner_margin(Margin::symmetric(12.0, 8.0)))
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                theme::pill(ui, "REMOTE", theme::ACCENT);
                let name = if v.name.is_empty() { client.config.host.clone() } else { v.name.clone() };
                ui.label(RichText::new(format!("{name}'s Openslot")).strong().size(15.0));
                ui.label(RichText::new(format!("{}:{}", client.config.host, client.config.port)).monospace().color(theme::MUTED));
                phase_label(ui, &v.phase);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("Disconnect").on_hover_text("Stop controlling this station (it carries on by itself)").clicked() {
                        f.disconnect_requested = true;
                    }
                    if v.audio_offered {
                        if v.listening {
                            if ui.add(egui::Slider::new(&mut f.volume, 0.0..=1.0).show_value(false)).on_hover_text("Volume").changed() {
                                client.set_volume(f.volume);
                            }
                        }
                        let b = egui::Button::new(if v.listening { "LISTENING" } else { "LISTEN" }).selected(v.listening);
                        if ui.add_enabled(connected, b).on_hover_text("Hear the station's radio here").clicked() {
                            client.set_listening(!v.listening);
                            client.set_volume(f.volume);
                        }
                    } else if connected {
                        ui.label(RichText::new("no audio").small().color(theme::MUTED))
                            .on_hover_text(if st.audio_note.is_empty() { "The station offers no audio" } else { st.audio_note.as_str() });
                    }
                    if let Some(e) = &v.audio_error {
                        ui.label(RichText::new(e).small().color(theme::RED));
                    }
                });
            });
        });

    // Status line: the station's own, or its last notice
    egui::TopBottomPanel::bottom("remote_status")
        .frame(Frame::none().fill(theme::SURFACE).inner_margin(Margin::symmetric(12.0, 5.0)))
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                let (tx, color) = if st.tx { ("TX", theme::TX_RED) } else if st.busy { ("BUSY", theme::YELLOW) } else { ("RX", theme::GREEN) };
                theme::status_pill(ui, tx, color);
                theme::status_pill(ui, "VARA", if st.modem { theme::GREEN } else { theme::RED });
                if let Some(snr) = st.snr {
                    ui.label(RichText::new(format!("SNR {snr:+.0} dB")).monospace());
                }
                if st.buffer > 0 {
                    ui.label(RichText::new(format!("buffer {}", st.buffer)).monospace().color(theme::YELLOW));
                }
                if st.unread_vmail > 0 {
                    ui.label(RichText::new(format!("{} new VMail", st.unread_vmail)).color(theme::ACCENT));
                }
                let recent = v.notices.back().filter(|(at, _)| at.elapsed().as_secs() < 15).map(|(_, t)| t.clone());
                match recent {
                    Some(n) => ui.label(RichText::new(n).color(theme::YELLOW)),
                    None => ui.label(RichText::new(&st.status).color(theme::MUTED)),
                };
            });
        });

    // Stations heard
    egui::SidePanel::right("remote_stations")
        .frame(Frame::none().fill(theme::BG).inner_margin(Margin::same(10.0)))
        .default_width(300.0)
        .resizable(true)
        .show(ctx, |ui| {
            for (title, list) in [("CQ calls", &v.stations.cq), ("Beacons", &v.stations.beacons)] {
                theme::card().show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    theme::caption(ui, &format!("{title} ({})", list.len()));
                    egui::ScrollArea::vertical().id_salt(title).max_height((ui.available_height() / 2.0 - 30.0).max(120.0)).show(ui, |ui| {
                        if list.is_empty() {
                            ui.label(RichText::new("None heard yet").small().color(theme::MUTED));
                        }
                        for s in list {
                            let text = format!("{:<9} {} {}", s.call, s.snr.as_deref().unwrap_or("   "), s.time);
                            let r = ui.selectable_label(f.call.eq_ignore_ascii_case(&s.call), RichText::new(text).monospace())
                                .on_hover_text(format!("{} Hz{}{}", s.bandwidth, s.locator.as_ref().map(|l| format!(" · {l}")).unwrap_or_default(),
                                    "\nClick to fill in the call, double-click to connect"));
                            if r.clicked() {
                                f.call = s.call.clone();
                            }
                            if r.double_clicked() && connected && !in_qso {
                                cmds.push(Command::Connect { call: s.call.clone() });
                            }
                        }
                    });
                });
                ui.add_space(6.0);
            }
        });

    // Controls: frequency, link, CQ / beacons
    egui::TopBottomPanel::top("remote_controls")
        .frame(Frame::none().fill(theme::BG).inner_margin(Margin { left: 10.0, right: 10.0, top: 8.0, bottom: 4.0 }))
        .show(ctx, |ui| {
            ui.add_enabled_ui(connected, |ui| {
                ui.horizontal_top(|ui| {
                    theme::card().inner_margin(Margin::symmetric(12.0, 8.0)).show(ui, |ui| {
                        ui.set_width(330.0);
                        ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            theme::caption(ui, "Frequency");
                            if !st.band.is_empty() {
                                theme::pill(ui, st.band.clone(), theme::ACCENT);
                            }
                            if let Some(m) = v.rig.as_ref().and_then(|r| r.mode) {
                                theme::pill(ui, m.to_string(), theme::PURPLE);
                            }
                            if st.on_calling_frequency {
                                theme::pill(ui, "CF", theme::YELLOW).on_hover_text("On a calling frequency");
                            }
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                let rig_ok = v.rig.as_ref().is_some_and(|r| r.backend != "None");
                                let b = egui::Button::new(RichText::new("RIG CONTROLS").size(11.0)).small().selected(f.rig_open);
                                if ui.add_enabled(rig_ok, b).on_hover_text("More rig controls").clicked() {
                                    f.rig_open = !f.rig_open;
                                }
                            });
                        });
                        let hz = st.frequency;
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 0.0;
                            ui.label(RichText::new(format!("{}.{:03}", hz / 1_000_000, (hz / 1000) % 1000)).monospace().size(24.0));
                            ui.label(RichText::new(format!(".{:03}", hz % 1000)).monospace().size(16.0).color(theme::MUTED));
                        });
                        ui.horizontal(|ui| {
                            let te = ui.add(egui::TextEdit::singleline(&mut f.freq_entry).hint_text("14.1055 MHz").desired_width(120.0));
                            let enter = te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                            if ui.button("Tune").clicked() || enter {
                                if let Some(hz) = crate::varac::messages::parse_frequency_input(&f.freq_entry) {
                                    cmds.push(Command::Frequency { hz });
                                    f.freq_entry.clear();
                                }
                            }
                            egui::ComboBox::from_id_salt("remote_cf").selected_text("Calling freqs").width(110.0).show_ui(ui, |ui| {
                                for &(hz, band) in crate::varac::messages::CALLING_FREQUENCIES_HZ {
                                    if ui.selectable_label(st.calling_frequency == hz, RichText::new(format!("{band:>4}  {:.3}", hz as f64 / 1e6)).monospace()).clicked() {
                                        cmds.push(Command::Frequency { hz });
                                    }
                                }
                            });
                        });
                        });
                    });

                    theme::card().inner_margin(Margin::symmetric(12.0, 8.0)).show(ui, |ui| {
                        ui.set_width(300.0);
                        ui.vertical(|ui| {
                        theme::caption(ui, if in_qso { "In QSO" } else { "Connect" });
                        if let Some(p) = &st.partner {
                            let mut who = p.clone();
                            for x in [&st.partner_name, &st.partner_qth, &st.partner_locator].into_iter().flatten() {
                                who.push_str(" · ");
                                who.push_str(x);
                            }
                            ui.label(RichText::new(who).strong().color(theme::ACCENT));
                        }
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut f.call).hint_text("Callsign…").font(egui::TextStyle::Monospace).desired_width(120.0));
                            if in_qso {
                                if ui.add(theme::accent_button("DISCONNECT", theme::RED)).clicked() {
                                    cmds.push(Command::Disconnect);
                                }
                            } else if ui.add_enabled(!f.call.trim().is_empty(), theme::primary_button("CONNECT", theme::ACCENT)).clicked() {
                                cmds.push(Command::Connect { call: f.call.trim().to_uppercase() });
                            }
                            if ui.button("ABORT").clicked() {
                                cmds.push(Command::Abort);
                            }
                        });
                        ui.label(RichText::new(match st.link.as_str() {
                            "connecting" => "Connecting…",
                            "connected" => "Linked",
                            "disconnecting" => "Disconnecting…",
                            _ => if st.modem { "Ready" } else { "The station's modem isn't connected" },
                        }).small().color(theme::MUTED));
                        });
                    });

                    theme::card().inner_margin(Margin::symmetric(12.0, 8.0)).show(ui, |ui| {
                        ui.set_width(200.0);
                        ui.vertical(|ui| {
                        theme::caption(ui, "CQ / Beacons");
                        if st.cq_active {
                            if ui.add_sized([180.0, 26.0], theme::accent_button("END CQ", theme::RED)).clicked() {
                                cmds.push(Command::EndCq);
                            }
                        } else if ui.add_enabled(!in_qso, egui::Button::new("CALL CQ").min_size(Vec2::new(180.0, 26.0))).clicked() {
                            cmds.push(Command::Cq);
                        }
                        if ui.add_sized([180.0, 26.0], theme::accent_button("SEND BEACON", theme::PURPLE)).clicked() {
                            cmds.push(Command::Beacon);
                        }
                        let mut auto = st.beacon_auto;
                        if ui.checkbox(&mut auto, "Automatic beacons").changed() {
                            cmds.push(Command::AutoBeacon { on: auto });
                        }
                        });
                    });
                });
            });
        });

    // Message box
    egui::TopBottomPanel::bottom("remote_composer")
        .frame(Frame::none().fill(theme::BG).inner_margin(Margin { left: 10.0, right: 10.0, top: 4.0, bottom: 8.0 }))
        .show(ctx, |ui| {
            ui.add_enabled_ui(connected, |ui| {
                ui.horizontal(|ui| {
                    let w = ui.available_width() - 90.0;
                    let te = ui.add(egui::TextEdit::singleline(&mut f.input).desired_width(w)
                        .hint_text(if in_qso { "Message…" } else { "Connect to a station to chat" }));
                    let enter = te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if (ui.add_enabled(in_qso, theme::primary_button("SEND", theme::ACCENT)).clicked() || (enter && in_qso)) && !f.input.trim().is_empty() {
                        cmds.push(Command::Chat { text: std::mem::take(&mut f.input) });
                        te.request_focus();
                    }
                });
            });
        });

    // Chat
    egui::CentralPanel::default()
        .frame(Frame::none().fill(theme::BG).inner_margin(Margin::symmetric(10.0, 4.0)))
        .show(ctx, |ui| {
            theme::card().show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.set_height(ui.available_height());
                egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
                    if v.chat.is_empty() {
                        ui.label(RichText::new(match &v.phase {
                            Phase::Connected => "No chat yet".to_string(),
                            Phase::Denied(why) => format!("The station refused: {why}. Disconnect, then check the password."),
                            _ => "Reaching the station…".to_string(),
                        }).color(theme::MUTED));
                    }
                    for line in &v.chat {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new(&line.time).monospace().small().color(theme::MUTED));
                            if line.system {
                                ui.label(RichText::new(&line.text).italics().color(theme::MUTED));
                            } else {
                                let color = if line.out { theme::ACCENT } else { theme::GREEN };
                                ui.label(RichText::new(format!("{}:", line.call)).strong().color(color));
                                ui.label(&line.text);
                            }
                        });
                    }
                });
            });
        });

    // The station's rig controls, in their own window as at the station
    if f.rig_open {
        let mut open = true;
        let mut actions = Vec::new();
        egui::Window::new("Rig controls (remote)").open(&mut open).default_width(330.0).resizable(false).show(ctx, |ui| {
            match &v.rig {
                Some(r) => {
                    let panel = RigPanel {
                        connected: r.connected && connected,
                        mode: r.mode,
                        ptt: r.ptt,
                        extras: &r.extras,
                        may_transmit: r.may_transmit,
                        backend: r.backend.clone(),
                    };
                    draw_rig_controls(ui, &panel, &mut actions);
                }
                None => {
                    ui.label(RichText::new("Waiting for the station…").color(theme::MUTED));
                }
            }
        });
        f.rig_open = open;
        cmds.extend(actions.into_iter().map(|action| Command::Rig { action }));
    }

    for c in cmds {
        client.send(c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::state::{ChatLine, GuiCommand};
    use crate::rig::RigAction;

    fn station(modem: bool) -> (GuiState, tokio::sync::mpsc::UnboundedReceiver<GuiCommand>, crate::storage::Storage) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let mut st = GuiState::new("NR5P".into(), modem, tx);
        st.settings_form.bandwidth = "500".into();
        (st, rx, crate::storage::Storage::open_in_memory().unwrap())
    }

    fn sent(rx: &mut tokio::sync::mpsc::UnboundedReceiver<GuiCommand>) -> Vec<GuiCommand> {
        std::iter::from_fn(|| rx.try_recv().ok()).collect()
    }

    /// A remote's commands do what the station's own buttons do, and are refused (with a
    /// reason for the remote) when the buttons would be.
    #[test]
    fn commands_act_like_the_buttons() {
        let (mut st, mut rx, db) = station(false);
        let none = None;
        assert_eq!(apply(&mut st, &db, &none, "test", Command::Connect { call: "W5TST".into() }), Some("Modem not connected".into()));
        assert!(st.connect_callsign.is_empty(), "a refused call doesn't stay in the station's box");
        assert_eq!(apply(&mut st, &db, &none, "test", Command::Cq), Some("Modem not connected".into()));
        assert_eq!(apply(&mut st, &db, &none, "test", Command::Beacon), Some("Modem not connected".into()));
        assert_eq!(apply(&mut st, &db, &none, "test", Command::Chat { text: "hi".into() }), Some("Not in a QSO: connect first".into()));
        assert!(apply(&mut st, &db, &none, "test", Command::Rig { action: RigAction::Tune }).is_some());
        assert!(apply(&mut st, &db, &none, "test", Command::Frequency { hz: 7_105_000 }).is_some(), "no rig control");
        assert!(sent(&mut rx).is_empty(), "nothing went to the modem");

        let (mut st, mut rx, db) = station(true);
        assert_eq!(apply(&mut st, &db, &none, "test", Command::Connect { call: " " .into() }), Some("No callsign to connect to".into()));
        assert_eq!(apply(&mut st, &db, &none, "test", Command::Connect { call: "w5tst".into() }), None);
        assert!(matches!(sent(&mut rx).as_slice(), [GuiCommand::Connect(c)] if c == "W5TST"));
        assert_eq!(apply(&mut st, &db, &none, "test", Command::Connect { call: "NR5P-1".into() }), Some("NR5P-1 is your own callsign".into()));

        assert_eq!(apply(&mut st, &db, &none, "test", Command::Beacon), None);
        assert!(matches!(sent(&mut rx).as_slice(), [GuiCommand::SendCq { source, bandwidth: 500 }] if source == "NR5P-9"));
        assert_eq!(apply(&mut st, &db, &none, "test", Command::AutoBeacon { on: true }), None);
        assert!(st.beacon_auto_enabled);

        assert_eq!(apply(&mut st, &db, &none, "test", Command::Cq), None);
        assert!(st.cq_active);
        assert!(matches!(sent(&mut rx).as_slice(), [GuiCommand::SendCq { .. } | GuiCommand::SendTypedCq { .. }]));
        assert_eq!(apply(&mut st, &db, &none, "test", Command::Cq), Some("Already calling CQ".into()));
        assert_eq!(apply(&mut st, &db, &none, "test", Command::EndCq), None);
        assert!(!st.cq_active);
        assert!(matches!(sent(&mut rx).as_slice(), [GuiCommand::Abort]));

        // In a QSO: chat goes out (logged as ours), connect is refused, disconnect works
        st.session_state = SessionState::Connected;
        st.connected_to = Some("W5TST".into());
        assert_eq!(apply(&mut st, &db, &none, "test", Command::Chat { text: "hello there".into() }), None);
        assert!(matches!(sent(&mut rx).as_slice(), [GuiCommand::SendChat(t)] if t == "hello there"));
        assert!(st.chat_lines.back().is_some_and(|l| l.is_outgoing && l.text == "hello there"));
        assert_eq!(apply(&mut st, &db, &none, "test", Command::Connect { call: "K1ABC".into() }), Some("Already in a QSO".into()));
        st.ping_active = true;
        assert_eq!(apply(&mut st, &db, &none, "test", Command::Chat { text: "x".into() }), Some("Can't send during a ping".into()));
        st.ping_active = false;
        assert_eq!(apply(&mut st, &db, &none, "test", Command::Disconnect), None);
        assert_eq!(apply(&mut st, &db, &none, "test", Command::Abort), None);
        assert!(matches!(sent(&mut rx).as_slice(), [GuiCommand::Disconnect, GuiCommand::Abort]));

        // The radio: tuned through tune_to (as the frequency picker), rig controls queued
        st.rig_backend = crate::rig::RigBackend::Rigctld { host: "127.0.0.1".into(), port: 4532 };
        st.rig_connected = true;
        assert_eq!(apply(&mut st, &db, &none, "test", Command::Frequency { hz: 7_105_000 }), None);
        assert_eq!((st.tune_to, st.current_frequency, st.current_band.as_str()), (Some(7_105_000), 7_105_000, "40m"));
        assert!(apply(&mut st, &db, &none, "test", Command::Frequency { hz: 12 }).is_some());
        let a = RigAction::Level { level: crate::rig::Level::RfPower, value: 0.3 };
        assert_eq!(apply(&mut st, &db, &none, "test", Command::Rig { action: a }), None);
        assert_eq!(st.rig_actions, vec![a]);
        // and each was logged at the station
        assert!(st.log_lines.iter().any(|l| l.contains("Remote (test): tune to 7.1050 MHz")));
    }

    /// What the station publishes: the essentials of its main screen.
    #[test]
    fn snapshot_of_the_station() {
        let (mut st, _rx, _db) = station(true);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let cfg = crate::remote::server::ServerConfig { port: 0, password: "x".into(), name: "NR5P".into(), audio_device: String::new() };
        let srv = crate::remote::server::RemoteServer::start(rt.handle(), cfg).unwrap();
        st.current_frequency = 14_105_000;
        st.current_band = "20m".into();
        st.connected_to = Some("W5TST".into());
        st.session_state = SessionState::Connected;
        st.partner_name = Some("Bob".into());
        for i in 0..250 {
            st.chat_lines.push_back(ChatLine { callsign: "W5TST".into(), text: format!("line {i}"), ..Default::default() });
        }
        let (status, chat, stations, rig) = snapshot(&st, &srv);
        assert_eq!(status.callsign, "NR5P");
        assert_eq!(status.link, "connected");
        assert_eq!(status.partner.as_deref(), Some("W5TST"));
        assert_eq!(status.partner_name.as_deref(), Some("Bob"));
        assert_eq!(status.calling_frequency, 14_105_000);
        assert!(status.on_calling_frequency || status.frequency != 14_105_000);
        assert!(!status.audio, "no capture in tests");
        assert_eq!(chat.len(), 200, "the last 200 lines");
        assert_eq!(chat.last().unwrap().text, "line 249");
        assert!(stations.cq.is_empty());
        assert_eq!(rig.backend, "None");
    }
}
