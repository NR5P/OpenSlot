//! Alert tags, as in VarAC's `VarAC_alert_tags.conf`: one tag per line,
//! `TAG,Description,FontColor,BackColor,B|C|BR|DS,email1|email2` — the flags say where to
//! look (beacons, CQs, broadcasts, the QSO's chat; 1 when missing), colours are given
//! by name. A tag matches when the text contains it (case-sensitive, as in VarAC);
//! every matching tag fires.

pub const MAX_TAGS: usize = 20;
pub const FILE_NAME: &str = "VarAC_alert_tags.conf";

#[derive(Debug, Clone, PartialEq)]
pub struct AlertTag {
    pub tag: String,
    pub description: String,
    pub font_color: String,
    pub back_color: String,
    pub beacons: bool,
    pub cqs: bool,
    pub broadcasts: bool,
    pub datastream: bool,
    pub emails: Vec<String>,
}

impl Default for AlertTag {
    fn default() -> Self {
        AlertTag {
            tag: String::new(),
            description: String::new(),
            font_color: "White".into(),
            back_color: "Red".into(),
            beacons: true,
            cqs: true,
            broadcasts: true,
            datastream: true,
            emails: Vec::new(),
        }
    }
}

/// Where a match came from (VarAC alert.source_id).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Beacon = 1,
    Cq = 2,
    Broadcast = 3,
    Datastream = 4,
}

impl Source {
    pub fn from_id(id: i64) -> Self {
        match id {
            1 => Source::Beacon,
            2 => Source::Cq,
            3 => Source::Broadcast,
            _ => Source::Datastream,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Source::Beacon => "BEACON",
            Source::Cq => "CQ",
            Source::Broadcast => "BROADCAST",
            Source::Datastream => "DATASTREAM",
        }
    }
}

pub fn parse(content: &str) -> Vec<AlertTag> {
    let mut out: Vec<AlertTag> = Vec::new();
    for line in content.lines() {
        let f: Vec<&str> = line.split(',').collect();
        if f.len() < 4 || f[0].trim().is_empty() {
            continue;
        }
        let flags: Vec<bool> = f.get(4).map(|s| s.split('|').map(|v| v.trim() != "0").collect()).unwrap_or_default();
        let flag = |i: usize| flags.get(i).copied().unwrap_or(true);
        let tag = AlertTag {
            tag: f[0].to_string(),
            description: f[1].to_string(),
            font_color: f[2].trim().to_string(),
            back_color: f[3].trim().to_string(),
            beacons: flag(0),
            cqs: flag(1),
            broadcasts: flag(2),
            datastream: flag(3),
            emails: f.get(5).map(|s| s.split('|').map(|e| e.trim().to_string()).filter(|e| !e.is_empty()).collect()).unwrap_or_default(),
        };
        if !out.iter().any(|t| t.tag == tag.tag) && out.len() < MAX_TAGS {
            out.push(tag);
        }
    }
    out
}

/// The file text (commas taken out of tags and descriptions, as VarAC's editor does).
pub fn serialize(tags: &[AlertTag]) -> String {
    let b = |v: bool| if v { "1" } else { "0" };
    tags.iter()
        .filter(|t| !t.tag.trim().is_empty())
        .map(|t| format!(
            "{},{},{},{},{}|{}|{}|{},{}\r\n",
            t.tag.replace(',', ""),
            t.description.replace(',', ""),
            t.font_color,
            t.back_color,
            b(t.beacons), b(t.cqs), b(t.broadcasts), b(t.datastream),
            t.emails.join("|"),
        ))
        .collect()
}

/// The tags `texts` set off for this source.
pub fn matches<'a>(tags: &'a [AlertTag], source: Source, texts: &[&str]) -> Vec<&'a AlertTag> {
    tags.iter()
        .filter(|t| match source {
            Source::Beacon => t.beacons,
            Source::Cq => t.cqs,
            Source::Broadcast => t.broadcasts,
            Source::Datastream => t.datastream,
        })
        .filter(|t| !t.tag.is_empty() && texts.iter().any(|x| x.contains(t.tag.as_str())))
        .collect()
}

/// RGB of a colour name (the common ones VarAC's picker offers); None if unknown.
pub fn color_rgb(name: &str) -> Option<(u8, u8, u8)> {
    Some(match name.trim().to_ascii_lowercase().as_str() {
        "black" => (0, 0, 0),
        "white" => (255, 255, 255),
        "red" => (255, 0, 0),
        "darkred" => (139, 0, 0),
        "green" => (0, 128, 0),
        "darkgreen" => (0, 100, 0),
        "lime" => (0, 255, 0),
        "limegreen" => (50, 205, 50),
        "blue" => (0, 0, 255),
        "navy" => (0, 0, 128),
        "dodgerblue" => (30, 144, 255),
        "lightblue" => (173, 216, 230),
        "yellow" => (255, 255, 0),
        "gold" => (255, 215, 0),
        "orange" => (255, 165, 0),
        "darkorange" => (255, 140, 0),
        "orangered" => (255, 69, 0),
        "magenta" | "fuchsia" => (255, 0, 255),
        "purple" => (128, 0, 128),
        "violet" => (238, 130, 238),
        "pink" => (255, 192, 203),
        "hotpink" => (255, 105, 180),
        "cyan" | "aqua" => (0, 255, 255),
        "teal" => (0, 128, 128),
        "gray" | "grey" => (128, 128, 128),
        "silver" => (192, 192, 192),
        "lightgray" | "lightgrey" => (211, 211, 211),
        "brown" => (165, 42, 42),
        "maroon" => (128, 0, 0),
        "olive" => (128, 128, 0),
        "seashell" => (255, 245, 238),
        "steelblue" => (70, 130, 180),
        _ => return None,
    })
}

pub const COLOR_NAMES: &[&str] = &[
    "White", "Black", "Red", "DarkRed", "Orange", "DarkOrange", "Yellow", "Gold", "Lime", "Green",
    "DarkGreen", "Cyan", "Teal", "DodgerBlue", "Blue", "Navy", "Magenta", "Purple", "Pink", "Gray",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_round_trip_and_matching() {
        let text = "POTA,Parks on the air,White,Green,1|1|1|0,me@example.com\r\nSOTA,Summits,Black,Yellow\r\nPOTA,dup,Red,Red,1|1|1|1,\r\n,empty,Red,Red\r\n";
        let tags = parse(text);
        assert_eq!(tags.len(), 2);
        assert_eq!(tags[0].emails, vec!["me@example.com"]);
        assert!(!tags[0].datastream && tags[0].beacons);
        assert!(tags[1].datastream, "flags default to 1");
        assert_eq!(serialize(&tags), "POTA,Parks on the air,White,Green,1|1|1|0,me@example.com\r\nSOTA,Summits,Black,Yellow,1|1|1|1,\r\n");
        // case-sensitive Contains, per source flags
        assert_eq!(matches(&tags, Source::Broadcast, &["K1ABC", "pota at US-1234 POTA"]).len(), 1);
        assert!(matches(&tags, Source::Broadcast, &["pota"]).is_empty());
        assert!(matches(&tags, Source::Datastream, &["POTA today"]).is_empty());
        assert_eq!(matches(&tags, Source::Datastream, &["SOTA today"]).len(), 1);
        assert_eq!(color_rgb("DodgerBlue"), Some((30, 144, 255)));
    }
}
