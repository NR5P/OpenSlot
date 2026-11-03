//! Help → User guide: docs/USER_GUIDE.md built into the program, with its contents on the
//! left, a search box, and the chosen section on the right.

use super::state::GuiState;
use super::theme;
use eframe::egui::{self, text::LayoutJob, Color32, FontId, RichText, TextFormat};

/// The user guide (docs/USER_GUIDE.md), the same text as in the repository.
pub const GUIDE: &str = include_str!("../../docs/USER_GUIDE.md");

/// One `## ` section of the guide: its title and its lines.
pub struct Section {
    pub title: String,
    pub lines: Vec<&'static str>,
}

/// The guide split into sections; the part before the first `## ` is the introduction.
pub fn sections() -> Vec<Section> {
    let mut out = vec![Section { title: "Introduction".into(), lines: Vec::new() }];
    for line in GUIDE.lines() {
        if let Some(t) = line.strip_prefix("## ") {
            out.push(Section { title: t.trim().to_string(), lines: Vec::new() });
        } else if !line.starts_with("# ") {
            if let Some(s) = out.last_mut() {
                s.lines.push(line);
            }
        }
    }
    out
}

/// Sections whose title or text contains `query` (case ignored); all for an empty query.
pub fn matching(sections: &[Section], query: &str) -> Vec<usize> {
    let q = query.trim().to_lowercase();
    (0..sections.len())
        .filter(|&i| {
            q.is_empty()
                || sections[i].title.to_lowercase().contains(&q)
                || sections[i].lines.iter().any(|l| l.to_lowercase().contains(&q))
        })
        .collect()
}

/// `**bold**` and `` `code` `` inside a line, as a layout job.
fn inline(text: &str, size: f32, color: Color32, strong: Color32) -> LayoutJob {
    let mut job = LayoutJob::default();
    let normal = TextFormat { font_id: FontId::proportional(size), color, ..Default::default() };
    let bold = TextFormat { font_id: FontId::proportional(size), color: strong, ..Default::default() };
    let code = TextFormat {
        font_id: FontId::monospace(size - 1.0),
        color: theme::ACCENT,
        ..Default::default()
    };
    let mut rest = text;
    while !rest.is_empty() {
        let next_b = rest.find("**");
        let next_c = rest.find('`');
        let (at, marker) = match (next_b, next_c) {
            (Some(b), Some(c)) if c < b => (c, "`"),
            (Some(b), _) => (b, "**"),
            (None, Some(c)) => (c, "`"),
            (None, None) => {
                job.append(rest, 0.0, normal.clone());
                break;
            }
        };
        job.append(&rest[..at], 0.0, normal.clone());
        let after = &rest[at + marker.len()..];
        match after.find(marker) {
            Some(end) => {
                job.append(&after[..end], 0.0, if marker == "`" { code.clone() } else { bold.clone() });
                rest = &after[end + marker.len()..];
            }
            None => {
                job.append(&rest[at..], 0.0, normal.clone());
                break;
            }
        }
    }
    job
}

/// Draw one section's lines: `### ` subheadings, `- ` bullets, `1. ` numbered steps,
/// paragraphs.
fn render(ui: &mut egui::Ui, s: &Section) {
    let text = ui.visuals().text_color();
    let strong = ui.visuals().strong_text_color();
    ui.label(RichText::new(&s.title).size(22.0).strong().color(strong));
    ui.add_space(6.0);
    for line in &s.lines {
        let t = line.trim_end();
        if t.is_empty() {
            ui.add_space(6.0);
        } else if let Some(h) = t.strip_prefix("### ") {
            ui.add_space(4.0);
            ui.label(RichText::new(h).size(17.0).strong().color(theme::ACCENT));
        } else if let Some(b) = t.strip_prefix("- ") {
            ui.horizontal_wrapped(|ui| {
                ui.add_space(8.0);
                ui.label(RichText::new("•").color(theme::MUTED));
                ui.label(inline(b, 14.0, text, strong));
            });
        } else if let Some(b) = t.strip_prefix("  - ") {
            ui.horizontal_wrapped(|ui| {
                ui.add_space(26.0);
                ui.label(RichText::new("◦").color(theme::MUTED));
                ui.label(inline(b, 14.0, text, strong));
            });
        } else if let Some((n, b)) = t.split_once(". ").filter(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())) {
            ui.horizontal_wrapped(|ui| {
                ui.add_space(8.0);
                ui.label(RichText::new(format!("{n}.")).strong().color(theme::ACCENT));
                ui.label(inline(b, 14.0, text, strong));
            });
        } else {
            ui.label(inline(t, 14.0, text, strong));
        }
    }
}

/// The Help → User guide window.
pub fn draw_help(ctx: &egui::Context, state: &mut GuiState) {
    if !state.help_open {
        return;
    }
    let secs = sections();
    let mut open = state.help_open;
    egui::Window::new("Openslot User Guide")
        .open(&mut open)
        .default_size([900.0, 640.0])
        .resizable(true)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Search:");
                ui.add(egui::TextEdit::singleline(&mut state.help_search).desired_width(220.0).hint_text("e.g. VMail, slot, gateway"));
                if !state.help_search.is_empty() && ui.small_button("×").clicked() {
                    state.help_search.clear();
                }
            });
            ui.separator();
            let shown = matching(&secs, &state.help_search);
            if !shown.is_empty() && !shown.contains(&state.help_section) {
                state.help_section = shown[0];
            }
            egui::SidePanel::left("help_contents")
                .resizable(true)
                .default_width(230.0)
                .show_inside(ui, |ui| {
                    egui::ScrollArea::vertical().id_salt("help_toc").show(ui, |ui| {
                        if shown.is_empty() {
                            ui.colored_label(theme::MUTED, "Nothing found");
                        }
                        for &i in &shown {
                            if ui.selectable_label(state.help_section == i, &secs[i].title).clicked() {
                                state.help_section = i;
                            }
                        }
                    });
                });
            egui::CentralPanel::default().show_inside(ui, |ui| {
                egui::ScrollArea::vertical().id_salt(("help_body", state.help_section)).show(ui, |ui| {
                    if let Some(s) = secs.get(state.help_section) {
                        render(ui, s);
                    }
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        let pos = shown.iter().position(|&i| i == state.help_section);
                        if let Some(p) = pos.filter(|&p| p > 0) {
                            if ui.button(format!("← {}", secs[shown[p - 1]].title)).clicked() {
                                state.help_section = shown[p - 1];
                            }
                        }
                        if let Some(p) = pos.filter(|&p| p + 1 < shown.len()) {
                            if ui.button(format!("{} →", secs[shown[p + 1]].title)).clicked() {
                                state.help_section = shown[p + 1];
                            }
                        }
                    });
                });
            });
        });
    state.help_open = open && state.help_open;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guide_sections_and_search() {
        let s = sections();
        assert_eq!(s[0].title, "Introduction");
        assert!(s.iter().any(|x| x.title == "VMail"));
        assert!(s.iter().any(|x| x.title == "Troubleshooting"));
        let hits = matching(&s, "relay notification");
        assert!(!hits.is_empty() && hits.len() < s.len());
        assert_eq!(matching(&s, "").len(), s.len());
    }

    #[test]
    fn inline_markup() {
        let job = inline("Click **SEND** then `/help` now", 14.0, Color32::WHITE, Color32::YELLOW);
        let parts: Vec<&str> = job.sections.iter().map(|s| &job.text[s.byte_range.clone()]).collect();
        assert_eq!(parts, ["Click ", "SEND", " then ", "/help", " now"]);
    }
}
