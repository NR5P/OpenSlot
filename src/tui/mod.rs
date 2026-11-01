mod ui;

use chrono::Utc;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::DefaultTerminal;
use std::collections::VecDeque;
use std::time::Duration;

use crate::storage::Storage;
use crate::vara::commands::{Bandwidth, VaraCommand};
use crate::vara::connection::VaraConnection;
use crate::varac::bbs;
use crate::varac::messages::{BbsFileOffer, IncomingMessage, VMail};
use crate::varac::session::{SessionEvent, SessionState, StationConfig, VaracSession};

const MAX_LOG_LINES: usize = 500;

#[derive(Clone)]
pub struct ModemConfig {
    pub host: String,
    pub cmd_port: u16,
    pub data_port: u16,
    /// VARA KISS port (broadcasts); VarAC's default is 8100 = command port - 200.
    pub kiss_port: u16,
    pub bandwidth: Bandwidth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Chat,
    VMail,
    CqList,
    Broadcasts,
    Log,
}

impl Tab {
    const ALL: &[Tab] = &[Tab::Chat, Tab::VMail, Tab::CqList, Tab::Broadcasts, Tab::Log];

    fn label(&self) -> &'static str {
        match self {
            Tab::Chat => "Chat [F1]",
            Tab::VMail => "VMail [F2]",
            Tab::CqList => "CQ [F3]",
            Tab::Broadcasts => "BC [F4]",
            Tab::Log => "Log [F5]",
        }
    }

    fn next(&self) -> Tab {
        let idx = Self::ALL.iter().position(|t| t == self).unwrap_or(0);
        Self::ALL[(idx + 1) % Self::ALL.len()]
    }

    fn prev(&self) -> Tab {
        let idx = Self::ALL.iter().position(|t| t == self).unwrap_or(0);
        Self::ALL[(idx + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

#[derive(Debug, Clone)]
pub struct ChatLine {
    pub time: String,
    pub callsign: String,
    pub text: String,
    pub is_outgoing: bool,
    pub is_system: bool,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct CqEntry {
    pub time: String,
    pub callsign: String,
    pub bandwidth: u16,
    pub snr: Option<String>,
    pub locator: Option<String>,
    pub is_beacon: bool,
    pub cq_type: Option<String>,
}

#[derive(Debug, Clone)]
pub struct BroadcastEntry {
    pub time: String,
    pub from: String,
    pub to: String,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct VMailEntry {
    pub guid: String,
    pub time: String,
    pub from: String,
    pub to: String,
    pub subject: String,
    pub body: String,
    pub read: bool,
}

/// VMail compose state
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComposeStep {
    To,
    Subject,
    Body,
}

#[derive(Debug, Clone)]
pub struct ComposeState {
    pub step: ComposeStep,
    pub to: String,
    pub subject: String,
    pub body: String,
}

pub struct App {
    pub tab: Tab,
    pub input: String,
    pub cursor_pos: usize,
    pub chat_lines: VecDeque<ChatLine>,
    pub cq_entries: VecDeque<CqEntry>,
    pub broadcasts: VecDeque<BroadcastEntry>,
    pub vmail_list: Vec<VMailEntry>,
    pub log_lines: VecDeque<String>,
    pub connected_to: Option<String>,
    pub session_state: SessionState,
    pub ptt: bool,
    pub busy: bool,
    pub buffer_bytes: u32,
    pub my_callsign: String,
    pub chat_scroll: u16,
    pub quit: bool,
    pub status_msg: String,
    pub unread_vmails: i64,
    pub modem_connected: bool,
    pub compose: Option<ComposeState>,
    pub vmail_selected: usize,
    pub vmail_viewing: bool,
    pub pending_file_offer: Option<BbsFileOffer>,
    pub pending_file_md5: Option<String>,
}

impl App {
    pub fn new(callsign: String, modem_connected: bool) -> Self {
        let status = if modem_connected {
            "VARA modem connected. Ready.".into()
        } else {
            "Offline — /modem to connect".into()
        };
        Self {
            tab: Tab::Chat,
            input: String::new(),
            cursor_pos: 0,
            chat_lines: VecDeque::with_capacity(MAX_LOG_LINES),
            cq_entries: VecDeque::with_capacity(200),
            broadcasts: VecDeque::with_capacity(200),
            vmail_list: Vec::new(),
            log_lines: VecDeque::with_capacity(MAX_LOG_LINES),
            connected_to: None,
            session_state: SessionState::Idle,
            ptt: false,
            busy: false,
            buffer_bytes: 0,
            my_callsign: callsign,
            chat_scroll: 0,
            quit: false,
            status_msg: status,
            unread_vmails: 0,
            modem_connected,
            compose: None,
            vmail_selected: 0,
            vmail_viewing: false,
            pending_file_offer: None,
            pending_file_md5: None,
        }
    }

    pub fn add_log(&mut self, msg: String) {
        let ts = Utc::now().format("%H:%M:%S").to_string();
        self.log_lines.push_back(format!("[{ts}] {msg}"));
        if self.log_lines.len() > MAX_LOG_LINES {
            self.log_lines.pop_front();
        }
    }

    pub fn add_chat(&mut self, line: ChatLine) {
        self.chat_lines.push_back(line);
        if self.chat_lines.len() > MAX_LOG_LINES {
            self.chat_lines.pop_front();
        }
        self.chat_scroll = 0;
    }

    pub fn add_system_chat(&mut self, text: String) {
        let ts = Utc::now().format("%H:%M:%S").to_string();
        self.add_chat(ChatLine {
            time: ts,
            callsign: String::new(),
            text,
            is_outgoing: false,
            is_system: true,
        });
    }

    fn handle_key(&mut self, key: KeyEvent) -> Option<AppAction> {
        // Ctrl shortcuts always work
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('c') | KeyCode::Char('q') => {
                    self.quit = true;
                    return None;
                }
                KeyCode::Char('d') => return Some(AppAction::Disconnect),
                _ => {}
            }
        }

        // Escape cancels compose mode or VMail viewing
        if key.code == KeyCode::Esc {
            if self.compose.is_some() {
                self.compose = None;
                self.input.clear();
                self.cursor_pos = 0;
                self.status_msg = "VMail compose cancelled".into();
                return None;
            }
            if self.vmail_viewing {
                self.vmail_viewing = false;
                return None;
            }
        }

        // VMail tab navigation (when not composing and input is empty)
        if self.tab == Tab::VMail && self.compose.is_none() && self.input.is_empty() {
            match key.code {
                KeyCode::Up => {
                    if self.vmail_viewing {
                        return None; // no-op while viewing
                    }
                    self.vmail_selected = self.vmail_selected.saturating_sub(1);
                    return None;
                }
                KeyCode::Down => {
                    if self.vmail_viewing {
                        return None;
                    }
                    if !self.vmail_list.is_empty() {
                        self.vmail_selected = (self.vmail_selected + 1).min(self.vmail_list.len() - 1);
                    }
                    return None;
                }
                KeyCode::Enter => {
                    if self.vmail_viewing {
                        self.vmail_viewing = false;
                        return None;
                    }
                    if !self.vmail_list.is_empty() {
                        self.vmail_viewing = true;
                        return Some(AppAction::ReadVMail(self.vmail_selected));
                    }
                    return None;
                }
                KeyCode::Char('d') => {
                    if !self.vmail_viewing && !self.vmail_list.is_empty() {
                        return Some(AppAction::DeleteVMail(self.vmail_selected));
                    }
                }
                _ => {}
            }
        }

        match key.code {
            KeyCode::F(1) => self.tab = Tab::Chat,
            KeyCode::F(2) => self.tab = Tab::VMail,
            KeyCode::F(3) => self.tab = Tab::CqList,
            KeyCode::F(4) => self.tab = Tab::Broadcasts,
            KeyCode::F(5) => self.tab = Tab::Log,
            KeyCode::Tab => {
                if self.compose.is_none() {
                    self.tab = self.tab.next();
                }
            }
            KeyCode::BackTab => {
                if self.compose.is_none() {
                    self.tab = self.tab.prev();
                }
            }
            KeyCode::Enter => {
                if !self.input.is_empty() {
                    let text = std::mem::take(&mut self.input);
                    self.cursor_pos = 0;
                    return Some(AppAction::SendInput(text));
                } else if self.compose.is_some() {
                    let text = String::new();
                    return Some(AppAction::SendInput(text));
                }
            }
            KeyCode::Char(c) => {
                self.input.insert(self.cursor_pos, c);
                self.cursor_pos += 1;
            }
            KeyCode::Backspace => {
                if self.cursor_pos > 0 {
                    self.cursor_pos -= 1;
                    self.input.remove(self.cursor_pos);
                }
            }
            KeyCode::Delete => {
                if self.cursor_pos < self.input.len() {
                    self.input.remove(self.cursor_pos);
                }
            }
            KeyCode::Left => {
                if self.cursor_pos > 0 {
                    self.cursor_pos -= 1;
                }
            }
            KeyCode::Right => {
                if self.cursor_pos < self.input.len() {
                    self.cursor_pos += 1;
                }
            }
            KeyCode::Home => self.cursor_pos = 0,
            KeyCode::End => self.cursor_pos = self.input.len(),
            KeyCode::PageUp => {
                self.chat_scroll = self.chat_scroll.saturating_add(10);
            }
            KeyCode::PageDown => {
                self.chat_scroll = self.chat_scroll.saturating_sub(10);
            }
            _ => {}
        }
        None
    }
}

pub enum AppAction {
    SendInput(String),
    Disconnect,
    ReadVMail(usize),
    DeleteVMail(usize),
}

#[allow(dead_code)]
enum SessionResponse {
    SendVMailAck,
    SendInfoResponse,
    SendVersionResponse,
    SendFileReady,
    SendFileOk,
    SendFileAbort,
}

pub async fn run(
    session: Option<VaracSession>,
    station_config: StationConfig,
    db: Storage,
    callsign: String,
    modem_config: ModemConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut terminal = ratatui::init();
    let result = run_inner(
        &mut terminal, session, station_config, &db, callsign, modem_config,
    ).await;
    ratatui::restore();
    result
}

async fn run_inner(
    terminal: &mut DefaultTerminal,
    mut session: Option<VaracSession>,
    mut station_config: StationConfig,
    db: &Storage,
    callsign: String,
    modem_config: ModemConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut app = App::new(callsign.clone(), session.is_some());
    let mut current_qso_guid: Option<String> = None;

    app.unread_vmails = db.unread_vmail_count().unwrap_or(0);
    refresh_vmail_list(&mut app, db);

    loop {
        terminal.draw(|frame| ui::draw(frame, &app))?;

        if app.quit {
            break;
        }

        // Poll session for events, or just sleep if no session
        let session_event = if session.as_ref().is_some_and(|s| s.modem_closed()) {
            // The modem quit or the network dropped
            session = None;
            app.modem_connected = false;
            app.status_msg = "VARA modem closed the connection — /modem to reconnect".into();
            app.add_log("VARA modem closed the connection".into());
            None
        } else if let Some(sess) = session.as_mut() {
            // Only the wait may time out; a received input is always processed fully
            let input = tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(50)) => None,
                input = sess.next_input() => input,
            };
            match input {
                Some(input) => sess.handle_input(input).await,
                None => None,
            }
        } else {
            tokio::time::sleep(Duration::from_millis(50)).await;
            None
        };

        // Handle session event if we got one
        if let Some(evt) = session_event {
            if let Some(sess) = session.as_mut() {
                handle_session_event(&mut app, db, &mut current_qso_guid, sess, evt).await;
            }
        }

        // Process keyboard events
        while event::poll(Duration::ZERO)? {
            if let Event::Key(key) = event::read()? {
                if let Some(action) = app.handle_key(key) {
                    handle_action(
                        &mut app, &mut session, &mut station_config,
                        db, &current_qso_guid, &modem_config, &callsign, action,
                    ).await;
                }
            }
        }
    }

    Ok(())
}

async fn handle_action(
    app: &mut App,
    session: &mut Option<VaracSession>,
    station_config: &mut StationConfig,
    db: &Storage,
    qso_guid: &Option<String>,
    modem_config: &ModemConfig,
    callsign: &str,
    action: AppAction,
) {
    match action {
        AppAction::SendInput(text) => {
            // VMail compose mode
            if let Some(ref mut compose) = app.compose {
                match compose.step {
                    ComposeStep::To => {
                        if text.is_empty() {
                            app.status_msg = "Callsign required".into();
                            return;
                        }
                        compose.to = text.to_uppercase();
                        compose.step = ComposeStep::Subject;
                        app.status_msg = format!("VMail to {} — enter subject:", compose.to);
                    }
                    ComposeStep::Subject => {
                        compose.subject = if text.is_empty() { "(no subject)".into() } else { text };
                        compose.step = ComposeStep::Body;
                        app.status_msg = "Enter message body (//n for newlines):".into();
                    }
                    ComposeStep::Body => {
                        compose.body = text;
                        let vmail = VMail {
                            guid: String::new(),
                            from: callsign.to_string(),
                            to: compose.to.clone(),
                            subject: compose.subject.clone(),
                            body: compose.body.clone(),
                            compose_time: Utc::now().format("%Y-%m-%d %H:%M:%S").to_string(),
                            timestamp: Utc::now(),
                            urgent: false,
                            is_parked: false,
                            has_attachment: false,
                            reply_to: None,
                            raw: false,
                        };
                        app.compose = None;

                        if let Some(sess) = session {
                            if sess.state == SessionState::Connected {
                                match sess.send_vmail(&vmail).await {
                                    Ok(()) => {
                                        app.add_system_chat(format!(
                                            "VMail sent to {} [{}]", vmail.to, vmail.subject
                                        ));
                                        app.status_msg = format!("VMail sent to {}", vmail.to);
                                        // Store in sent folder
                                        let guid = uuid::Uuid::new_v4().to_string();
                                        let _ = db.insert_vmail(&crate::storage::VMailRecord {
                                            guid,
                                            creation_time: vmail.timestamp,
                                            sent_time: Some(Utc::now()),
                                            received_time: None,
                                            folder_id: crate::storage::VMailFolder::Sent as i32,
                                            vmail_to: vmail.to,
                                            vmail_from: vmail.from,
                                            vmail_via: None,
                                            subject: vmail.subject,
                                            msg: vmail.body,
                                            read_status: true,
                                            is_deleted: false,
                                            urgent: false,
                                            delivery_band: None,
                                            delivery_snr: None,
                                            frequency: None,
                                        });
                                    }
                                    Err(e) => {
                                        app.add_log(format!("VMail send error: {e}"));
                                        app.status_msg = "VMail send failed".into();
                                    }
                                }
                            } else {
                                // Store in outbox for later delivery
                                let guid = uuid::Uuid::new_v4().to_string();
                                let _ = db.insert_vmail(&crate::storage::VMailRecord {
                                    guid,
                                    creation_time: vmail.timestamp,
                                    sent_time: None,
                                    received_time: None,
                                    folder_id: crate::storage::VMailFolder::Outbox as i32,
                                    vmail_to: vmail.to.clone(),
                                    vmail_from: vmail.from,
                                    vmail_via: None,
                                    subject: vmail.subject,
                                    msg: vmail.body,
                                    read_status: true,
                                    is_deleted: false,
                                    urgent: false,
                                    delivery_band: None,
                                    delivery_snr: None,
                                    frequency: None,
                                });
                                app.status_msg = format!("VMail to {} queued in outbox", vmail.to);
                            }
                        } else {
                            app.status_msg = "No modem — VMail saved to outbox".into();
                            let guid = uuid::Uuid::new_v4().to_string();
                            let _ = db.insert_vmail(&crate::storage::VMailRecord {
                                guid,
                                creation_time: vmail.timestamp,
                                sent_time: None,
                                received_time: None,
                                folder_id: crate::storage::VMailFolder::Outbox as i32,
                                vmail_to: vmail.to,
                                vmail_from: vmail.from,
                                vmail_via: None,
                                subject: vmail.subject,
                                msg: vmail.body,
                                read_status: true,
                                is_deleted: false,
                                urgent: false,
                                delivery_band: None,
                                delivery_snr: None,
                                frequency: None,
                            });
                        }
                        return;
                    }
                }
                return;
            }

            // Slash commands
            if text.starts_with('/') {
                handle_command(
                    app, session, station_config, db, modem_config, callsign, &text,
                ).await;
                return;
            }

            // Regular chat
            let sess = match session.as_mut() {
                Some(s) => s,
                None => {
                    app.status_msg = "No modem connected — /modem to connect".into();
                    return;
                }
            };

            if sess.state != SessionState::Connected {
                app.status_msg = "Not connected — use /connect CALLSIGN".into();
                return;
            }

            // Convert newlines to VarAC line separator for outgoing
            // (VarAC sends line breaks as they are, CRLF)
            let wire_text = text.replace("\r\n", "\n").replace('\n', "\r\n");
            match sess.send_typed(&wire_text).await {
                Ok(()) => {
                    let ts = Utc::now().format("%H:%M:%S").to_string();
                    app.add_chat(ChatLine {
                        time: ts,
                        callsign: app.my_callsign.clone(),
                        text: text.clone(),
                        is_outgoing: true,
                        is_system: false,
                    });
                    if let Some(guid) = qso_guid {
                        let _ = db.insert_chat(&crate::storage::ChatRecord {
                            guid: uuid::Uuid::new_v4().to_string(),
                            entry_type: 2,
                            qso_guid: guid.clone(),
                            callsign: app.my_callsign.clone(),
                            entry: text,
                            creation_time: Utc::now(),
                            chat_id: None,
                            reply_on_chat_id: None,
                        });
                    }
                }
                Err(e) => {
                    app.add_log(format!("Send error: {e}"));
                }
            }
        }
        AppAction::Disconnect => {
            if let Some(sess) = session {
                if sess.state == SessionState::Connected {
                    if let Err(e) = sess.send_disconnect().await {
                        app.add_log(format!("Disconnect error: {e}"));
                    }
                }
            }
        }
        AppAction::ReadVMail(idx) => {
            if let Some(entry) = app.vmail_list.get(idx) {
                if !entry.read {
                    let _ = db.mark_vmail_read(&entry.guid);
                    app.unread_vmails = db.unread_vmail_count().unwrap_or(0);
                    if let Some(e) = app.vmail_list.get_mut(idx) {
                        e.read = true;
                    }
                }
            }
        }
        AppAction::DeleteVMail(idx) => {
            if let Some(entry) = app.vmail_list.get(idx) {
                let _ = db.mark_vmail_deleted(&entry.guid);
                app.status_msg = format!("VMail from {} deleted", entry.from);
            }
            refresh_vmail_list(app, db);
            app.unread_vmails = db.unread_vmail_count().unwrap_or(0);
            if app.vmail_selected >= app.vmail_list.len() && !app.vmail_list.is_empty() {
                app.vmail_selected = app.vmail_list.len() - 1;
            }
        }
    }
}

async fn handle_command(
    app: &mut App,
    session: &mut Option<VaracSession>,
    station_config: &mut StationConfig,
    db: &Storage,
    modem_config: &ModemConfig,
    callsign: &str,
    text: &str,
) {
    let parts: Vec<&str> = text.splitn(2, ' ').collect();
    let cmd = parts[0].to_lowercase();
    let arg = parts.get(1).copied().unwrap_or("");

    match cmd.as_str() {
        "/modem" | "/reconnect" => {
            app.add_log("Connecting to VARA modem...".into());
            app.status_msg = "Connecting to modem...".into();
            match try_connect_modem(modem_config, callsign, station_config).await {
                Ok(sess) => {
                    *session = Some(sess);
                    app.modem_connected = true;
                    app.status_msg = "VARA modem connected".into();
                    app.add_log("VARA modem connected".into());
                }
                Err(e) => {
                    app.status_msg = format!("Modem connection failed: {e}");
                    app.add_log(format!("Modem error: {e}"));
                }
            }
        }
        "/connect" => {
            if session.is_none() {
                app.status_msg = "No modem — /modem first".into();
                return;
            }
            if arg.is_empty() {
                app.status_msg = "Usage: /connect CALLSIGN".into();
                return;
            }
            let dest = arg.to_uppercase();
            app.add_log(format!("Connecting to {dest}..."));
            app.status_msg = format!("Connecting to {dest}...");
            if let Some(sess) = session {
                if let Err(e) = sess.vara.send_command(
                    &VaraCommand::Connect {
                        source: app.my_callsign.clone(),
                        destination: dest,
                    },
                ).await {
                    app.add_log(format!("Connect error: {e}"));
                }
            }
        }
        "/disconnect" | "/disc" => {
            if let Some(sess) = session {
                if sess.state == SessionState::Connected {
                    if let Err(e) = sess.send_disconnect().await {
                        app.add_log(format!("Disconnect error: {e}"));
                    }
                } else {
                    app.status_msg = "Not connected".into();
                }
            } else {
                app.status_msg = "No modem".into();
            }
        }
        "/cq" => {
            if session.is_none() {
                app.status_msg = "No modem — /modem first".into();
                return;
            }
            if let Some(sess) = session {
                // VarAC: the SSID names the slot you'll wait on (1-5 below, 11-15 above the
                // calling frequency); no slot = the bare callsign
                let slot: u8 = arg.trim().parse().unwrap_or(0);
                let source = crate::varac::slots::cq_source(callsign, slot);
                let bw = match modem_config.bandwidth {
                    Bandwidth::Bw500 => 500,
                    Bandwidth::Bw2300 => 2300,
                    Bandwidth::Bw2750 => 2750,
                };
                match sess.vara.send_command(
                    &VaraCommand::CqFrame { source: source.clone(), bandwidth: bw },
                ).await {
                    Ok(()) => {
                        app.status_msg = format!("CQ sent as {source} @ {bw} Hz");
                        app.add_log(format!("CQ: {source} @ {bw} Hz"));
                    }
                    Err(e) => app.add_log(format!("CQ error: {e}")),
                }
            }
        }
        "/listen" => {
            if let Some(sess) = session {
                let mode = match arg.to_lowercase().as_str() {
                    "cq" => crate::vara::commands::ListenMode::Cq,
                    "off" => crate::vara::commands::ListenMode::Off,
                    _ => crate::vara::commands::ListenMode::On,
                };
                match sess.vara.send_command(&VaraCommand::Listen(mode)).await {
                    Ok(()) => app.status_msg = format!("Listen mode: {mode}"),
                    Err(e) => app.add_log(format!("Listen error: {e}")),
                }
            } else {
                app.status_msg = "No modem".into();
            }
        }
        "/vmail" => {
            if arg == "compose" || arg.starts_with("to ") {
                // Start compose
                let to = arg.strip_prefix("to ").unwrap_or("").to_uppercase();
                if to.is_empty() {
                    app.compose = Some(ComposeState {
                        step: ComposeStep::To,
                        to: String::new(),
                        subject: String::new(),
                        body: String::new(),
                    });
                    app.status_msg = "VMail compose — enter destination callsign:".into();
                } else {
                    app.compose = Some(ComposeState {
                        step: ComposeStep::Subject,
                        to: to.clone(),
                        subject: String::new(),
                        body: String::new(),
                    });
                    app.status_msg = format!("VMail to {to} — enter subject:");
                }
                app.tab = Tab::VMail;
            } else {
                app.tab = Tab::VMail;
                refresh_vmail_list(app, db);
            }
        }
        "/bw" => {
            if let Some(sess) = session {
                let bw = match arg {
                    "500" => Some(Bandwidth::Bw500),
                    "2300" => Some(Bandwidth::Bw2300),
                    "2750" => Some(Bandwidth::Bw2750),
                    _ => None,
                };
                if let Some(bw) = bw {
                    match sess.vara.send_command(&VaraCommand::SetBandwidth(bw)).await {
                        Ok(()) => app.status_msg = format!("Bandwidth set to {bw}"),
                        Err(e) => app.add_log(format!("BW error: {e}")),
                    }
                } else {
                    app.status_msg = "Usage: /bw 500|2300|2750".into();
                }
            } else {
                app.status_msg = "No modem".into();
            }
        }
        "/accept" => {
            if app.pending_file_offer.is_none() {
                app.status_msg = "No pending file offer".into();
                return;
            }
            if let Some(sess) = session {
                match sess.send_file_ready().await {
                    Ok(()) => {
                        app.status_msg = "Accepting file transfer...".into();
                        app.add_log("Sent file ready — waiting for data".into());
                    }
                    Err(e) => app.add_log(format!("Accept error: {e}")),
                }
            } else {
                app.status_msg = "No modem".into();
            }
        }
        "/decline" => {
            if let Some(offer) = app.pending_file_offer.take() {
                let name = bbs::strip_gzip_suffix(&offer.name).to_string();
                if let Some(sess) = session {
                    let _ = sess.send_file_abort().await;
                }
                app.status_msg = format!("Declined file: {name}");
                app.pending_file_md5 = None;
            } else {
                app.status_msg = "No pending file offer".into();
            }
        }
        "/abort" => {
            if let Some(sess) = session {
                match sess.vara.send_command(&VaraCommand::Abort).await {
                    Ok(()) => app.status_msg = "Connection aborted".into(),
                    Err(e) => app.add_log(format!("Abort error: {e}")),
                }
            }
        }
        "/help" => {
            app.add_system_chat("--- Commands ---".into());
            app.add_system_chat("  /connect CALL     Connect to station".into());
            app.add_system_chat("  /disconnect       Disconnect".into());
            app.add_system_chat("  /abort            Immediate disconnect".into());
            app.add_system_chat("  /modem            Connect to VARA modem".into());
            app.add_system_chat("  /cq [SLOT]        Send CQ (optionally naming a VarAC slot: 1-5, 11-15)".into());
            app.add_system_chat("  /listen on|cq|off Set listen mode".into());
            app.add_system_chat("  /vmail            View inbox".into());
            app.add_system_chat("  /vmail compose     Compose VMail".into());
            app.add_system_chat("  /vmail to CALL    Compose to callsign".into());
            app.add_system_chat("  /bw 500|2300|2750 Set bandwidth".into());
            app.add_system_chat("  /accept           Accept file offer".into());
            app.add_system_chat("  /decline          Decline file offer".into());
            app.add_system_chat("  /help             Show this help".into());
            app.add_system_chat("--- Keys ---".into());
            app.add_system_chat("  Ctrl+C/Q  Quit    Ctrl+D  Disconnect".into());
            app.add_system_chat("  F1-F5     Tabs    Tab/Shift+Tab  Cycle".into());
            app.add_system_chat("  PgUp/PgDn Scroll  Esc  Cancel compose".into());
            app.tab = Tab::Chat;
        }
        _ => {
            app.status_msg = format!("Unknown command: {cmd} — /help for commands");
        }
    }
}

async fn try_connect_modem(
    cfg: &ModemConfig,
    callsign: &str,
    station_config: &StationConfig,
) -> Result<VaracSession, Box<dyn std::error::Error>> {
    let mut vara = VaraConnection::connect(&cfg.host, cfg.cmd_port, cfg.data_port).await?;
    vara.initialize(callsign, cfg.bandwidth).await?;
    let mut sess = VaracSession::new(vara, station_config.clone());
    let _ = sess.attach_kiss(&cfg.host, cfg.kiss_port).await;
    Ok(sess)
}

async fn handle_session_event(
    app: &mut App,
    db: &Storage,
    current_qso_guid: &mut Option<String>,
    session: &mut VaracSession,
    evt: SessionEvent,
) {
    match evt {
        SessionEvent::AdvancedBeacon(b) => {
            app.add_log(format!("Beacon: {} {}{}", b.callsign, b.locator, if b.email_gateway() { " (EMAIL GATEWAY)" } else { "" }));
        }
        SessionEvent::TypedCq(cq) => {
            app.add_log(format!("CQ {} de {} {} (slot {})", cq.cq_type, cq.callsign, cq.locator, cq.slot));
        }
        SessionEvent::Connected { source, destination, bandwidth } => {
            let bw_str = bandwidth.as_deref().unwrap_or("?");
            app.session_state = SessionState::Connected;
            app.connected_to = Some(destination.clone());
            app.status_msg = format!("Connected to {destination} ({bw_str} Hz)");
            app.add_system_chat(format!("CONNECTED TO {destination}"));
            app.add_log(format!("Connected: {source} <-> {destination} @ {bw_str}"));

            // Load recent chat history with this station
            if let Ok(history) = db.chat_history_with_callsign(&destination, 20) {
                if !history.is_empty() {
                    app.add_system_chat(format!("--- previous messages with {destination} ---"));
                    for record in &history {
                        let ts = record.creation_time.format("%m-%d %H:%M").to_string();
                        let is_outgoing = record.entry_type == 2;
                        let call = if is_outgoing {
                            app.my_callsign.clone()
                        } else {
                            record.callsign.clone()
                        };
                        app.add_chat(ChatLine {
                            time: ts,
                            callsign: call,
                            text: record.entry.replace("//n", "\n"),
                            is_outgoing,
                            is_system: false,
                        });
                    }
                    app.add_system_chat("--- end of history ---".into());
                }
            }

            let guid = uuid::Uuid::new_v4().to_string();
            let _ = db.insert_qso(&crate::storage::QsoRecord {
                guid: guid.clone(),
                starttime: Utc::now(),
                endtime: None,
                frequency: 0,
                band: String::new(),
                bandwidth: bandwidth.unwrap_or_else(|| "2300".into()),
                callsign: destination,
                my_callsign: source,
                snr_received: None,
                snr_sent: None,
                name: None,
                qth: None,
                comments: None,
            });
            *current_qso_guid = Some(guid);

            // The session runs VarAC's opening handshake and sends our opening report;
            // queued VMail goes out once it completes.
        }
        SessionEvent::Handshake(step) => app.add_log(format!("Handshake: {step}")),
        SessionEvent::Notice(m) => app.add_log(m),
        SessionEvent::Secure(n) => app.add_log(n.text().to_string()),
        // No countdown window here: accept straight away (a frequency we can't place, no)
        SessionEvent::QsyInvitation { target, .. } => {
            use crate::varac::session::QsyTarget;
            let offset = match target {
                QsyTarget::Offset(hz) => Some(hz),
                QsyTarget::Up => Some(750),
                QsyTarget::Down => Some(-750),
                QsyTarget::Absolute(_) => None,
            };
            app.add_log(format!("QSY invitation: {target:?} → {}", if offset.is_some() { "accepted" } else { "declined" }));
            let _ = session.answer_qsy(offset.is_some(), false, offset.unwrap_or(0)).await;
        }
        SessionEvent::QsyResolved => {}
        SessionEvent::FrameId { .. } | SessionEvent::TxStarted { .. } | SessionEvent::RxProgress { .. } => {}
        SessionEvent::QsyInvitationRequested => {
            // No slot picker in the TUI: stay on frequency
            let _ = session.send_qsy_invitation(0).await;
            app.add_log("Partner allows auto-QSY; staying on frequency".into());
        }
        SessionEvent::QsyBy(hz) => app.add_log(format!("QSY {hz:+} Hz agreed with partner")),
        SessionEvent::HandshakeComplete => {
            // Try to send pending outbox VMails
            if let Ok(pending) = db.pending_outbox_vmails() {
                for vm in &pending {
                    let vmail = VMail {
                        guid: vm.guid.clone(),
                        from: vm.vmail_from.clone(),
                        to: vm.vmail_to.clone(),
                        subject: vm.subject.clone(),
                        body: vm.msg.clone(),
                        compose_time: vm.creation_time.format("%Y-%m-%d %H:%M:%S").to_string(),
                        timestamp: vm.creation_time,
                        urgent: vm.urgent,
                        is_parked: false,
                        has_attachment: false,
                        reply_to: None,
                        raw: false,
                    };
                    if session.send_vmail(&vmail).await.is_ok() {
                        let _ = db.mark_vmail_sent(&vm.guid, &Utc::now());
                        app.add_log(format!("Outbox VMail to {} delivered", vm.vmail_to));
                    }
                }
            }
        }
        SessionEvent::Disconnected => {
            let prev = app.connected_to.take();
            app.session_state = SessionState::Idle;
            let call = prev.as_deref().unwrap_or("?");
            app.status_msg = format!("Disconnected from {call}");
            app.add_system_chat(format!("DISCONNECTED FROM {call}"));
            if let Some(guid) = current_qso_guid.take() {
                let _ = db.update_qso_end(&guid, &Utc::now());
            }
        }
        SessionEvent::Ptt(on) => {
            app.ptt = on;
        }
        SessionEvent::BufferUpdate(n) => {
            app.buffer_bytes = n;
        }
        SessionEvent::Busy(on) => {
            app.busy = on;
        }
        SessionEvent::CqFrame { source, bandwidth } => {
            let ts = Utc::now().format("%H:%M:%S").to_string();
            app.cq_entries.push_back(CqEntry {
                time: ts,
                callsign: source.clone(),
                bandwidth,
                snr: None,
                locator: None,
                is_beacon: source.ends_with("-9"),
                cq_type: None,
            });
            if app.cq_entries.len() > 200 {
                app.cq_entries.pop_front();
            }
            app.add_log(format!("CQ from {source} @ {bandwidth} Hz"));
        }
        SessionEvent::Message(msg) => {
            let responses = handle_incoming_message(app, db, current_qso_guid, &msg);
            for resp in responses {
                let result = match resp {
                    SessionResponse::SendVMailAck => session.send_vmail_ack().await,
                    SessionResponse::SendInfoResponse => session.send_info_response().await,
                    SessionResponse::SendVersionResponse => session.send_version_response().await,
                    SessionResponse::SendFileReady => session.send_file_ready().await,
                    SessionResponse::SendFileOk => session.send_file_ok().await,
                    SessionResponse::SendFileAbort => session.send_file_abort().await,
                };
                if let Err(e) = result {
                    app.add_log(format!("Response send error: {e}"));
                }
            }
        }
        SessionEvent::VaraEvent(evt) => {
            app.add_log(format!("{evt:?}"));
        }
    }
}

fn handle_incoming_message(
    app: &mut App,
    db: &Storage,
    qso_guid: &Option<String>,
    msg: &IncomingMessage,
) -> Vec<SessionResponse> {
    let ts = Utc::now().format("%H:%M:%S").to_string();
    let partner = app.connected_to.clone().unwrap_or_else(|| "?".into());
    let mut responses = Vec::new();

    match msg {
        IncomingMessage::Opening(report) => {
            if let Some(w) = &report.welcome {
                app.add_chat(ChatLine { time: ts.clone(), callsign: partner.clone(), text: w.clone(), is_outgoing: false, is_system: false });
            }
            app.add_system_chat(format!(
                "de {} R{:+}dB{}{}{}",
                report.callsign,
                report.signal_report_db,
                report.name.as_ref().map(|n| format!(" [{n}]")).unwrap_or_default(),
                report.qth.as_ref().map(|q| format!(" {q}")).unwrap_or_default(),
                report.locator.as_ref().map(|l| format!(" ({l})")).unwrap_or_default(),
            ));
        }
        IncomingMessage::VMail(vmail) => {
            app.add_log(format!("VMail from {}: [{}]", vmail.from, vmail.subject));
            app.status_msg = format!("VMail received from {}", vmail.from);
            let guid = uuid::Uuid::new_v4().to_string();
            let _ = db.insert_vmail(&crate::storage::VMailRecord {
                guid,
                creation_time: vmail.timestamp,
                sent_time: None,
                received_time: Some(Utc::now()),
                folder_id: crate::storage::VMailFolder::Inbox as i32,
                vmail_to: vmail.to.clone(),
                vmail_from: vmail.from.clone(),
                vmail_via: None,
                subject: vmail.subject.clone(),
                msg: vmail.body.clone(),
                read_status: false,
                is_deleted: false,
                urgent: vmail.urgent,
                delivery_band: None,
                delivery_snr: None,
                frequency: None,
            });
            app.unread_vmails = db.unread_vmail_count().unwrap_or(0);
            refresh_vmail_list(app, db);
            // <SMR> is sent by the session
        }
        IncomingMessage::VMailAck => {
            app.add_system_chat("VMail delivery confirmed".into());
        }
        IncomingMessage::Disconnect => {
            app.add_system_chat("Partner requested disconnect".into());
        }
        IncomingMessage::Away(reason) => {
            let r = reason.as_deref().unwrap_or("(no message)");
            app.add_system_chat(format!("Station AWAY: {r}"));
        }
        IncomingMessage::Chat(chat_msg) => {
            let display_text = chat_msg.text.replace("//n", "\n");
            let prefix = if chat_msg.reply_to.is_some() { "\u{21B3} " } else { "" };
            app.add_chat(ChatLine {
                time: ts,
                callsign: partner.clone(),
                text: format!("{prefix}{display_text}"),
                is_outgoing: false,
                is_system: false,
            });
            if let Some(guid) = qso_guid {
                let reply_id = chat_msg.reply_to.as_ref().and_then(|r| r.parse::<i64>().ok());
                let _ = db.insert_chat(&crate::storage::ChatRecord {
                    guid: uuid::Uuid::new_v4().to_string(),
                    entry_type: 1,
                    qso_guid: guid.clone(),
                    callsign: partner,
                    entry: display_text,
                    creation_time: Utc::now(),
                    chat_id: None,
                    reply_on_chat_id: reply_id,
                });
            }
        }
        IncomingMessage::RawText(text) => {
            let display_text = text.replace("//n", "\n");
            app.add_chat(ChatLine {
                time: ts,
                callsign: partner.clone(),
                text: display_text.clone(),
                is_outgoing: false,
                is_system: false,
            });
            if let Some(guid) = qso_guid {
                let _ = db.insert_chat(&crate::storage::ChatRecord {
                    guid: uuid::Uuid::new_v4().to_string(),
                    entry_type: 1,
                    qso_guid: guid.clone(),
                    callsign: partner,
                    entry: display_text,
                    creation_time: Utc::now(),
                    chat_id: None,
                    reply_on_chat_id: None,
                });
            }
        }
        IncomingMessage::InfoRequest => {
            // answered automatically by the session
            app.add_log("Info request answered".into());
        }
        IncomingMessage::VersionRequest => {
            app.add_log("Version request answered".into());
        }
        IncomingMessage::BbsListRequest => {
            app.add_log("BBS list request received (no files hosted)".into());
        }
        IncomingMessage::BbsFileRequest(name) => {
            app.add_log(format!("BBS file request: {name} (not hosting)"));
        }
        IncomingMessage::BbsFileOffer(offer) => {
            let original = bbs::strip_gzip_suffix(&offer.name);
            app.add_system_chat(format!(
                "File offered: {} ({} bytes compressed) — /accept or /decline",
                original, offer.compressed_size,
            ));
            app.status_msg = format!("File offer: {} — /accept or /decline", original);
            app.pending_file_offer = Some(offer.clone());
        }
        IncomingMessage::BbsFileReady => {
            app.add_log("Partner ready to receive file".into());
        }
        IncomingMessage::BbsFilePayload(data) => {
            let b64_str = String::from_utf8_lossy(data);
            if let Some(offer) = app.pending_file_offer.take() {
                let original = bbs::strip_gzip_suffix(&offer.name);
                match bbs::receive_file(b64_str.trim(), &offer.md5_hex) {
                    Ok(file_data) => {
                        let save_dir = directories::UserDirs::new()
                            .and_then(|d| d.download_dir().map(|p| p.to_path_buf()))
                            .unwrap_or_else(|| std::path::PathBuf::from("."));
                        let save_path = save_dir.join(original);
                        match std::fs::write(&save_path, &file_data) {
                            Ok(()) => {
                                app.add_system_chat(format!(
                                    "File saved: {} ({} bytes)",
                                    save_path.display(), file_data.len(),
                                ));
                                app.status_msg = format!("File saved to {}", save_path.display());
                            }
                            Err(e) => {
                                app.add_system_chat(format!("File save error: {e}"));
                            }
                        }
                        responses.push(SessionResponse::SendFileOk);
                    }
                    Err(e) => {
                        app.add_system_chat(format!("File transfer error: {e}"));
                        responses.push(SessionResponse::SendFileAbort);
                    }
                }
            } else {
                app.add_log("Received file data without pending offer".into());
            }
            app.pending_file_md5 = None;
        }
        IncomingMessage::BbsFileOk => {
            app.add_system_chat("File transfer completed successfully".into());
        }
        IncomingMessage::BbsFileAborted => {
            app.add_system_chat("File transfer aborted by partner".into());
            app.pending_file_offer = None;
            app.pending_file_md5 = None;
        }
        IncomingMessage::TestLink => {
            app.add_log("Test link received".into());
        }
        _ => {
            app.add_log(format!("Message: {msg:?}"));
        }
    }

    responses
}

fn refresh_vmail_list(app: &mut App, db: &Storage) {
    app.vmail_list.clear();
    if let Ok(inbox) = db.vmails_in_folder(crate::storage::VMailFolder::Inbox, 50) {
        for vm in inbox {
            app.vmail_list.push(VMailEntry {
                guid: vm.guid,
                time: vm.creation_time.format("%m-%d %H:%M").to_string(),
                from: vm.vmail_from,
                to: vm.vmail_to,
                subject: vm.subject,
                body: vm.msg,
                read: vm.read_status,
            });
        }
    }
    if app.vmail_selected >= app.vmail_list.len() && !app.vmail_list.is_empty() {
        app.vmail_selected = app.vmail_list.len() - 1;
    }
}
