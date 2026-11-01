use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Tabs, Wrap};
use ratatui::Frame;

use super::{App, ComposeStep, Tab};
use crate::varac::session::SessionState;

pub fn draw(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),  // tabs
            Constraint::Min(5),    // main area
            Constraint::Length(3), // input
            Constraint::Length(1), // status bar
        ])
        .split(frame.area());

    draw_tabs(frame, app, chunks[0]);

    match app.tab {
        Tab::Chat => draw_chat(frame, app, chunks[1]),
        Tab::VMail => draw_vmail(frame, app, chunks[1]),
        Tab::CqList => draw_cq_list(frame, app, chunks[1]),
        Tab::Broadcasts => draw_broadcasts(frame, app, chunks[1]),
        Tab::Log => draw_log(frame, app, chunks[1]),
    }

    draw_input(frame, app, chunks[2]);
    draw_status_bar(frame, app, chunks[3]);
}

fn draw_tabs(frame: &mut Frame, app: &App, area: Rect) {
    let titles: Vec<Line> = Tab::ALL.iter().map(|t| {
        let mut label = t.label().to_string();
        if *t == Tab::VMail && app.unread_vmails > 0 {
            label = format!("{label} ({})", app.unread_vmails);
        }
        Line::from(label)
    }).collect();

    let idx = Tab::ALL.iter().position(|t| *t == app.tab).unwrap_or(0);
    let tabs = Tabs::new(titles)
        .select(idx)
        .highlight_style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))
        .divider(" | ");

    frame.render_widget(tabs, area);
}

fn draw_chat(frame: &mut Frame, app: &App, area: Rect) {
    let title = match &app.connected_to {
        Some(call) => format!(" Chat — {call} "),
        None => " Chat — not connected ".into(),
    };

    let visible_height = area.height.saturating_sub(2) as usize;
    let total = app.chat_lines.len();
    let scroll = app.chat_scroll as usize;
    let start = total.saturating_sub(visible_height + scroll);
    let end = total.saturating_sub(scroll);

    let lines: Vec<Line> = app.chat_lines
        .iter()
        .skip(start)
        .take(end.saturating_sub(start))
        .map(|line| {
            if line.is_system {
                Line::from(Span::styled(
                    format!(" {} --- {} ---", line.time, line.text),
                    Style::default().fg(Color::DarkGray),
                ))
            } else if line.is_outgoing {
                Line::from(vec![
                    Span::styled(
                        format!(" {} ", line.time),
                        Style::default().fg(Color::DarkGray),
                    ),
                    Span::styled(
                        format!("{}: ", line.callsign),
                        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(&line.text),
                ])
            } else {
                Line::from(vec![
                    Span::styled(
                        format!(" {} ", line.time),
                        Style::default().fg(Color::DarkGray),
                    ),
                    Span::styled(
                        format!("{}: ", line.callsign),
                        Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(&line.text),
                ])
            }
        })
        .collect();

    let para = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(title))
        .wrap(Wrap { trim: false });

    frame.render_widget(para, area);
}

fn draw_vmail(frame: &mut Frame, app: &App, area: Rect) {
    if app.vmail_viewing {
        if let Some(vm) = app.vmail_list.get(app.vmail_selected) {
            let title = format!(" VMail from {} — [{}] ", vm.from, vm.subject);
            let body_text = vm.body.replace("//n", "\n");
            let mut lines: Vec<Line> = vec![
                Line::from(vec![
                    Span::styled("From: ", Style::default().fg(Color::Yellow)),
                    Span::styled(&vm.from, Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
                ]),
                Line::from(vec![
                    Span::styled("To:   ", Style::default().fg(Color::Yellow)),
                    Span::raw(&vm.to),
                ]),
                Line::from(vec![
                    Span::styled("Date: ", Style::default().fg(Color::Yellow)),
                    Span::raw(&vm.time),
                ]),
                Line::from(vec![
                    Span::styled("Subj: ", Style::default().fg(Color::Yellow)),
                    Span::styled(&vm.subject, Style::default().add_modifier(Modifier::BOLD)),
                ]),
                Line::from(""),
            ];
            for line in body_text.lines() {
                lines.push(Line::from(Span::raw(line.to_string())));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                " [Esc/Enter to go back] ",
                Style::default().fg(Color::DarkGray),
            )));

            let para = Paragraph::new(lines)
                .block(Block::default().borders(Borders::ALL).title(title))
                .wrap(Wrap { trim: false });
            frame.render_widget(para, area);
        }
        return;
    }

    let items: Vec<ListItem> = app.vmail_list.iter().enumerate().map(|(i, vm)| {
        let selected = i == app.vmail_selected;
        let base_style = if vm.read {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
        };
        let bg = if selected {
            Style::default().bg(Color::DarkGray)
        } else {
            Style::default()
        };
        let marker = if vm.read { " " } else { "*" };
        let arrow = if selected { ">" } else { " " };
        ListItem::new(Line::from(vec![
            Span::styled(
                format!("{arrow}{marker} {:<12}", vm.time),
                base_style.patch(bg),
            ),
            Span::styled(
                format!("{:<10} ", vm.from),
                Style::default().fg(Color::Green).patch(bg),
            ),
            Span::styled(vm.subject.clone(), base_style.patch(bg)),
        ]))
    }).collect();

    let title = format!(
        " VMail Inbox ({}{}) — Up/Down, Enter=read, d=delete ",
        app.vmail_list.len(),
        if app.unread_vmails > 0 {
            format!(", {} unread", app.unread_vmails)
        } else {
            String::new()
        }
    );

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(title));

    frame.render_widget(list, area);
}

fn draw_cq_list(frame: &mut Frame, app: &App, area: Rect) {
    let header = Line::from(vec![
        Span::styled(
            format!(" {:<8} {:<12} {:<5} {:<5} {:<8} ",
                "Time", "Callsign", "BW", "SNR", "Locator"),
            Style::default().add_modifier(Modifier::BOLD).fg(Color::Yellow),
        ),
    ]);

    let mut lines: Vec<Line> = vec![header];

    for cq in app.cq_entries.iter().rev().take(50) {
        let type_marker = if cq.is_beacon { "B" } else { "C" };
        let snr = cq.snr.as_deref().unwrap_or("--");
        let loc = cq.locator.as_deref().unwrap_or("");
        lines.push(Line::from(vec![
            Span::styled(
                format!(" {:<8}", cq.time),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                format!("{type_marker} {:<10} ", cq.callsign),
                if cq.is_beacon {
                    Style::default().fg(Color::Magenta)
                } else {
                    Style::default().fg(Color::Green)
                },
            ),
            Span::raw(format!("{:<5} {:<5} {:<8}", cq.bandwidth, snr, loc)),
        ]));
    }

    let para = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL)
            .title(format!(" CQ / Beacons ({}) ", app.cq_entries.len())));

    frame.render_widget(para, area);
}

fn draw_broadcasts(frame: &mut Frame, app: &App, area: Rect) {
    let items: Vec<ListItem> = app.broadcasts.iter().rev().take(50).map(|bc| {
        ListItem::new(Line::from(vec![
            Span::styled(
                format!(" {} ", bc.time),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                format!("{} > {}: ", bc.from, bc.to),
                Style::default().fg(Color::Green),
            ),
            Span::raw(&bc.message),
        ]))
    }).collect();

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL)
            .title(format!(" Broadcasts ({}) ", app.broadcasts.len())));

    frame.render_widget(list, area);
}

fn draw_log(frame: &mut Frame, app: &App, area: Rect) {
    let visible = area.height.saturating_sub(2) as usize;
    let lines: Vec<Line> = app.log_lines.iter().rev().take(visible).rev().map(|l| {
        Line::from(Span::styled(l.as_str(), Style::default().fg(Color::DarkGray)))
    }).collect();

    let para = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(" System Log "))
        .wrap(Wrap { trim: false });

    frame.render_widget(para, area);
}

fn draw_input(frame: &mut Frame, app: &App, area: Rect) {
    let (prompt, title) = if let Some(ref compose) = app.compose {
        let p = match compose.step {
            ComposeStep::To => "To: ".to_string(),
            ComposeStep::Subject => format!("Subject (to {}): ", compose.to),
            ComposeStep::Body => format!("Body (to {} [{}]): ", compose.to, compose.subject),
        };
        (p, " VMail Compose (Esc to cancel) ".to_string())
    } else if app.session_state == SessionState::Connected {
        (
            format!("{}> ", app.connected_to.as_deref().unwrap_or("?")),
            " Input (/help for commands) ".to_string(),
        )
    } else {
        ("> ".into(), " Input (/help for commands) ".into())
    };

    let input_line = format!("{prompt}{}", app.input);
    let border_style = if app.compose.is_some() {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };
    let para = Paragraph::new(input_line)
        .block(Block::default().borders(Borders::ALL).title(title).border_style(border_style));

    frame.render_widget(para, area);

    let cursor_x = area.x + 1 + prompt.len() as u16 + app.cursor_pos as u16;
    let cursor_y = area.y + 1;
    frame.set_cursor_position((cursor_x, cursor_y));
}

fn draw_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    let modem = if app.modem_connected {
        Span::raw("")
    } else {
        Span::styled(" NO MODEM ", Style::default().bg(Color::Red).fg(Color::White))
    };

    let state = if !app.modem_connected {
        Span::styled(" OFFLINE ", Style::default().bg(Color::DarkGray).fg(Color::White))
    } else {
        match app.session_state {
            SessionState::Idle => Span::styled(" IDLE ", Style::default().bg(Color::DarkGray).fg(Color::White)),
            SessionState::Connecting => Span::styled(" CONNECTING ", Style::default().bg(Color::Yellow).fg(Color::Black)),
            SessionState::Connected => Span::styled(" CONNECTED ", Style::default().bg(Color::Green).fg(Color::Black)),
            SessionState::AwaitingDisconnect => Span::styled(" DISCONNECTING ", Style::default().bg(Color::Yellow).fg(Color::Black)),
        }
    };

    let ptt = if app.ptt {
        Span::styled(" TX ", Style::default().bg(Color::Red).fg(Color::White).add_modifier(Modifier::BOLD))
    } else if app.modem_connected {
        Span::styled(" RX ", Style::default().bg(Color::Blue).fg(Color::White))
    } else {
        Span::raw("")
    };

    let busy = if app.busy {
        Span::styled(" BUSY ", Style::default().fg(Color::Red))
    } else {
        Span::raw("")
    };

    let buf = if app.buffer_bytes > 0 {
        Span::styled(
            format!(" BUF:{} ", app.buffer_bytes),
            Style::default().fg(Color::Yellow),
        )
    } else {
        Span::raw("")
    };

    let call = Span::styled(
        format!(" {} ", app.my_callsign),
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
    );

    let status = Span::styled(
        format!(" {} ", app.status_msg),
        Style::default().fg(Color::White),
    );

    let bar = Line::from(vec![call, modem, state, ptt, busy, buf, status]);
    let para = Paragraph::new(bar);
    frame.render_widget(para, area);
}
