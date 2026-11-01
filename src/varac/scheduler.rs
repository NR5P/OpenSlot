//! VarAC's frequency scheduler and scanner.
//!
//! Scheduler (`VarAC_frequency_schedule.conf`, lines `HH:MM,14.105.000`, UTC): checked
//! every 20 s; at a rule's exact minute (or with a QSY still owed) it retunes, unless a
//! QSO, CQ, beacon or connect is going on (then it tries again in 20 s).
//!
//! Scanner (`VarAC_frequency_scanner.conf`, lines `freq,wait,beacon,tune,enabled,start,end`):
//! steps through the enabled entries, skipping those outside their UTC window, stays `wait`
//! seconds on each (longer by the busy hold when the channel was busy), sends a one-time
//! beacon on entries marked so (once per beacon interval per entry), and pauses entirely
//! while anything else is happening.

use chrono::{NaiveTime, Timelike};

pub const SCHEDULE_FILE: &str = "VarAC_frequency_schedule.conf";
pub const SCANNER_FILE: &str = "VarAC_frequency_scanner.conf";
pub const MAX_RULES: usize = 30;
pub const MAX_SCAN: usize = 15;
pub const SCHEDULER_CHECK_SECS: i64 = 20;
/// VarAC's choices for the scanner's wait and busy hold
pub const WAIT_CHOICES: &[u32] = &[5, 6, 7, 8, 9, 10, 15, 20, 25, 30, 35, 40, 45, 50, 55, 60, 70, 80, 90, 100];
pub const HOLD_CHOICES: &[u32] = &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 15, 20, 25, 30, 35, 40, 45, 50, 55, 60];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Inactive,
    Scheduler,
    Scanner,
    /// The scanner's list and loop (no busy hold), and connects go through ALE
    Ale,
}

impl Mode {
    pub fn key(self) -> &'static str {
        match self {
            Mode::Inactive => "INACTIVE",
            Mode::Scheduler => "SCHEDULER",
            Mode::Scanner => "SCANNER",
            Mode::Ale => "ALE",
        }
    }
    pub fn from_key(s: &str) -> Self {
        match s.trim().to_ascii_uppercase().as_str() {
            "SCHEDULER" => Mode::Scheduler,
            "SCANNER" => Mode::Scanner,
            "ALE" => Mode::Ale,
            _ => Mode::Inactive,
        }
    }
    /// The button's text
    pub fn button(self) -> &'static str {
        match self {
            Mode::Inactive => "FREQ SCHEDULE OFF",
            Mode::Scheduler => "FREQ SCHEDULE ON",
            Mode::Scanner => "FREQ SCANNER ON",
            Mode::Ale => "ALE ON",
        }
    }
    /// VarAC's button cycle: OFF → schedule (if any rules) → scanner (if any) → ALE (when
    /// enabled) → OFF
    #[allow(dead_code)]
    pub fn next(self, has_rules: bool, has_scan: bool) -> Self {
        self.next_with_ale(has_rules, has_scan, false)
    }

    pub fn next_with_ale(self, has_rules: bool, has_scan: bool, ale: bool) -> Self {
        match self {
            Mode::Inactive if has_rules => Mode::Scheduler,
            Mode::Inactive | Mode::Scheduler if has_scan => Mode::Scanner,
            Mode::Scanner if ale => Mode::Ale,
            _ => Mode::Inactive,
        }
    }

    /// The scanner loop runs (SCANNER and ALE)
    pub fn scanning(self) -> bool {
        matches!(self, Mode::Scanner | Mode::Ale)
    }
}

/// VarAC's "Hz dotted" frequency: 14.105.000
pub fn dotted(hz: u64) -> String {
    format!("{}.{:03}.{:03}", hz / 1_000_000, hz / 1000 % 1000, hz % 1000)
}

/// 14.105.000, 14.105, 14105000 or 14,105.000 → Hz
pub fn parse_freq(s: &str) -> Option<u64> {
    let t = s.trim().replace(',', ".");
    let parts: Vec<&str> = t.split('.').collect();
    let hz = match parts.as_slice() {
        [mhz, khz, hz] if khz.len() == 3 && hz.len() == 3 => mhz.parse::<u64>().ok()? * 1_000_000 + khz.parse::<u64>().ok()? * 1000 + hz.parse::<u64>().ok()?,
        [whole] => whole.parse::<u64>().ok()?,
        [mhz, frac] => (format!("{mhz}.{frac}").parse::<f64>().ok()? * 1e6).round() as u64,
        _ => return None,
    };
    (hz > 100_000).then_some(hz)
}

fn parse_hm(s: &str) -> Option<NaiveTime> {
    let t = s.trim().replace('.', ":");
    let (h, m) = t.split_once(':')?;
    NaiveTime::from_hms_opt(h.trim().parse().ok()?, m.trim().parse().ok()?, 0)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Rule {
    pub at: NaiveTime,
    pub freq_hz: u64,
}

pub fn parse_schedule(text: &str) -> Vec<Rule> {
    let mut out: Vec<Rule> = Vec::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split(',').collect();
        if f.len() != 2 {
            continue;
        }
        let (Some(at), Some(freq_hz)) = (parse_hm(f[0]), parse_freq(f[1])) else { continue };
        if out.iter().any(|r| r.at == at) {
            continue; // VarAC: "duplicate times definied in the parking schedule list. skipping"
        }
        out.push(Rule { at, freq_hz });
    }
    out.sort_by_key(|r| r.at);
    out
}

pub fn serialize_schedule(rules: &[Rule]) -> String {
    rules.iter().map(|r| format!("{},{}\r\n", r.at.format("%H:%M"), dotted(r.freq_hz))).collect()
}

/// The rule for this exact minute (VarAC only acts on an exact HH:mm).
pub fn rule_now(rules: &[Rule], now: NaiveTime) -> Option<u64> {
    rules.iter().find(|r| r.at.hour() == now.hour() && r.at.minute() == now.minute()).map(|r| r.freq_hz)
}

/// Where the schedule says we should be now: the last rule at or before now, else the
/// day's last rule (VarAC at startup).
pub fn rule_in_force(rules: &[Rule], now: NaiveTime) -> Option<u64> {
    rules.iter().rev().find(|r| r.at <= now).or_else(|| rules.last()).map(|r| r.freq_hz)
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScanEntry {
    pub freq_hz: u64,
    pub wait_secs: u32,
    pub beacon: bool,
    pub tune: bool,
    pub enabled: bool,
    /// UTC window (no wrap past midnight), None = always
    pub window: Option<(NaiveTime, NaiveTime)>,
}

impl ScanEntry {
    /// VarAC's minimum wait: 10 s with a beacon, 8 with a tune, 15 with both
    pub fn min_wait(&self) -> u32 {
        match (self.beacon, self.tune) {
            (true, true) => 15,
            (true, false) => 10,
            (false, true) => 8,
            _ => 5,
        }
    }
    pub fn active_at(&self, now: NaiveTime) -> bool {
        self.window.is_none_or(|(a, b)| now >= a && now <= b)
    }
}

pub fn parse_scanner(text: &str) -> Vec<ScanEntry> {
    let mut out = Vec::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split(',').map(str::trim).collect();
        if f.len() != 7 {
            continue;
        }
        let Some(freq_hz) = parse_freq(f[0]) else { continue };
        let window = match (parse_hm(f[5]), parse_hm(f[6])) {
            (Some(a), Some(b)) => Some((a, b)),
            _ => None,
        };
        out.push(ScanEntry {
            freq_hz,
            wait_secs: f[1].parse().unwrap_or(30),
            beacon: f[2] == "1",
            tune: f[3] == "1",
            enabled: f[4] == "1",
            window,
        });
        if out.len() >= MAX_SCAN {
            break;
        }
    }
    out
}

pub fn serialize_scanner(entries: &[ScanEntry]) -> String {
    let b = |v: bool| if v { "1" } else { "0" };
    entries.iter().map(|e| {
        let (s, t) = match e.window {
            Some((a, z)) => (a.format("%H:%M").to_string(), z.format("%H:%M").to_string()),
            None => (":".into(), ":".into()),
        };
        format!("{},{},{},{},{},{},{}\r\n", dotted(e.freq_hz), e.wait_secs, b(e.beacon), b(e.tune), b(e.enabled), s, t)
    }).collect()
}

/// The answer to a `<FSR>` peek: `<FSO>` when off, else VarAC's list.
pub fn fsr_answer(mode: Mode, rules: &[Rule], scan: &[ScanEntry]) -> String {
    match mode {
        Mode::Inactive => "<FSO>".into(),
        Mode::Scheduler => {
            let mut s = "FREQ. SCHEDULE:\r\n".to_string();
            for r in rules {
                s.push_str(&format!("{}) {}\r\n", r.at.format("%H:%M"), dotted(r.freq_hz)));
            }
            s
        }
        // VarAC answers nothing in ALE mode (its list text is empty)
        Mode::Ale => String::new(),
        Mode::Scanner => {
            let mut s = "FREQ. SCANNER:\r\n".to_string();
            for e in scan.iter().filter(|e| e.enabled) {
                match e.window {
                    Some((a, b)) => s.push_str(&format!("{} {}-{}\r\n", dotted(e.freq_hz), a.format("%H:%M"), b.format("%H:%M"))),
                    None => s.push_str(&format!("{} Always\r\n", dotted(e.freq_hz))),
                }
            }
            s
        }
    }
}

/// What the scanner wants done this second.
#[derive(Debug, Clone, PartialEq)]
pub enum ScanAction {
    Tune { freq_hz: u64, tune: bool },
    Beacon,
    ExtendHold(u32),
}

/// The scanner's place in its list (as in VarAC).
#[derive(Debug, Clone, Default)]
pub struct Scanner {
    pub index: usize,
    /// When we last changed frequency (seconds, the app's clock)
    pub last_change: Option<i64>,
    /// The busy hold was added at this stop
    pub hold_applied: bool,
    pub override_wait: Option<u32>,
    /// Last one-time beacon per entry
    pub last_beacon: Vec<Option<i64>>,
    /// The last retune asked for: (frequency, when) — not asked again while the radio gets there
    pub tuning: Option<(u64, i64)>,
}

/// The radio is close enough to the entry's frequency (CAT steps can round a few Hz)
fn at_freq(current: u64, want: u64) -> bool {
    current.abs_diff(want) <= 50
}

impl Scanner {
    pub fn reset(&mut self, n: usize) {
        *self = Scanner { last_beacon: vec![None; n], ..Default::default() };
    }

    /// One pass (call every second while the scanner runs and nothing pauses it).
    /// `now` in seconds; `busy_since_change` = the channel was busy after we arrived;
    /// `beacon_interval_secs`; `ptt_ok` = PTT not disabled; `beacon_ok` = a beacon may go
    /// out now (modem up, channel quiet, bandwidth allowed) — if not, it's skipped this stop.
    #[allow(clippy::too_many_arguments)]
    pub fn step(&mut self, list: &[ScanEntry], utc: NaiveTime, now: i64, current_hz: u64, busy_since_change: bool,
                hold_secs: u32, beacon_interval_secs: i64, ptt_ok: bool, beacon_ok: bool) -> Vec<ScanAction> {
        let active: Vec<usize> = (0..list.len()).filter(|&i| list[i].enabled).collect();
        if active.is_empty() {
            return Vec::new();
        }
        if self.last_beacon.len() != list.len() {
            self.last_beacon.resize(list.len(), None);
        }
        if !active.contains(&self.index) {
            self.index = active[0];
        }
        let mut out = Vec::new();
        // outside its time window: on to the next one now
        let mut guard = 0;
        while !list[self.index].active_at(utc) {
            self.advance(&active);
            guard += 1;
            if guard > active.len() {
                return out; // nothing is in its window
            }
        }
        let e = &list[self.index];
        if !at_freq(current_hz, e.freq_hz) {
            // asked already: give the radio a few seconds to report the new frequency
            if self.tuning.is_some_and(|(f, t)| f == e.freq_hz && now - t < 8) {
                return out;
            }
            self.tuning = Some((e.freq_hz, now));
            out.push(ScanAction::Tune { freq_hz: e.freq_hz, tune: e.tune && ptt_ok });
            self.last_change = Some(now);
            return out;
        }
        if e.beacon && ptt_ok && beacon_ok && self.last_beacon[self.index].is_none_or(|t| now - t > beacon_interval_secs) {
            self.last_beacon[self.index] = Some(now);
            out.push(ScanAction::Beacon);
            return out;
        }
        // (already there when the scanner started: the wait counts from now)
        let since = now - *self.last_change.get_or_insert(now);
        if hold_secs > 0 && busy_since_change && !self.hold_applied {
            self.hold_applied = true;
            self.override_wait = Some(e.wait_secs + hold_secs);
            out.push(ScanAction::ExtendHold(hold_secs));
        }
        let wait = self.override_wait.unwrap_or(e.wait_secs) as i64;
        if since > wait {
            self.advance(&active);
        }
        out
    }

    fn advance(&mut self, active: &[usize]) {
        let pos = active.iter().position(|&i| i == self.index).unwrap_or(0);
        self.index = active[(pos + 1) % active.len()];
        self.hold_applied = false;
        self.override_wait = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(h: u32, m: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(h, m, 0).unwrap()
    }

    #[test]
    fn schedule_file_and_rules() {
        let rules = parse_schedule("18:00,7.105.000\r\n06:00,14.105.000\r\n06:00,21.105.000\r\nbad\r\n12.30,10.133.000\r\n");
        assert_eq!(rules.len(), 3, "duplicate time skipped, '.' read as ':'");
        assert_eq!(serialize_schedule(&rules), "06:00,14.105.000\r\n12:30,10.133.000\r\n18:00,7.105.000\r\n");
        assert_eq!(rule_now(&rules, t(12, 30)), Some(10_133_000));
        assert_eq!(rule_now(&rules, t(12, 31)), None);
        assert_eq!(rule_in_force(&rules, t(13, 0)), Some(10_133_000));
        assert_eq!(rule_in_force(&rules, t(3, 0)), Some(7_105_000), "before the first rule: the day's last");
        assert_eq!(fsr_answer(Mode::Scheduler, &rules, &[]), "FREQ. SCHEDULE:\r\n06:00) 14.105.000\r\n12:30) 10.133.000\r\n18:00) 7.105.000\r\n");
        assert_eq!(fsr_answer(Mode::Inactive, &rules, &[]), "<FSO>");
    }

    #[test]
    fn scanner_file() {
        let text = "14.105.000,30,1,0,1,08:00,16:00\r\n7.105.000,10,0,0,1,:,:\r\n10.133.000,10,0,0,0,:,:\r\n";
        let s = parse_scanner(text);
        assert_eq!(s.len(), 3);
        assert_eq!(s[0].window, Some((t(8, 0), t(16, 0))));
        assert_eq!(s[1].window, None);
        assert_eq!(serialize_scanner(&s), text);
        assert_eq!(fsr_answer(Mode::Scanner, &[], &s), "FREQ. SCANNER:\r\n14.105.000 08:00-16:00\r\n7.105.000 Always\r\n");
        assert_eq!(parse_freq("14.105"), Some(14_105_000));
        assert_eq!(parse_freq("7105000"), Some(7_105_000));
        assert_eq!(Mode::Inactive.next(true, true), Mode::Scheduler);
        assert_eq!(Mode::Scheduler.next(true, true), Mode::Scanner);
        assert_eq!(Mode::Scheduler.next(true, false), Mode::Inactive);
        assert_eq!(Mode::Inactive.next(false, true), Mode::Scanner);
        assert_eq!(Mode::Scanner.next(true, true), Mode::Inactive);
    }

    #[test]
    fn scanner_steps() {
        let list = parse_scanner("14.105.000,10,1,0,1,:,:\r\n7.105.000,5,0,0,1,:,:\r\n10.133.000,5,0,0,1,20:00,21:00\r\n");
        let mut s = Scanner::default();
        let noon = t(12, 0);
        // tune to the first, then its beacon, then wait 10 s and move on
        assert_eq!(s.step(&list, noon, 0, 3_000_000, false, 0, 900, true, true), vec![ScanAction::Tune { freq_hz: 14_105_000, tune: false }]);
        assert_eq!(s.step(&list, noon, 1, 14_105_000, false, 0, 900, true, true), vec![ScanAction::Beacon]);
        assert!(s.step(&list, noon, 5, 14_105_000, false, 0, 900, true, true).is_empty());
        assert!(s.step(&list, noon, 11, 14_105_000, false, 0, 900, true, true).is_empty());
        assert_eq!(s.index, 1);
        assert_eq!(s.step(&list, noon, 12, 14_105_000, false, 0, 900, true, true), vec![ScanAction::Tune { freq_hz: 7_105_000, tune: false }]);
        // busy: hold extended once
        assert_eq!(s.step(&list, noon, 13, 7_105_000, true, 10, 900, true, true), vec![ScanAction::ExtendHold(10)]);
        assert!(s.step(&list, noon, 18, 7_105_000, true, 10, 900, true, true).is_empty());
        assert_eq!(s.index, 1, "5 s + 10 s hold");
        assert!(s.step(&list, noon, 28, 7_105_000, true, 10, 900, true, true).is_empty());
        // the third is outside its window: straight back to the first, no second beacon yet
        assert_eq!(s.step(&list, noon, 29, 7_105_000, false, 0, 900, true, true), vec![ScanAction::Tune { freq_hz: 14_105_000, tune: false }]);
        assert!(s.step(&list, noon, 30, 14_105_000, false, 0, 900, true, true).is_empty());
        // starting on the first frequency already: it still moves on after the wait
        let mut z = Scanner::default();
        assert!(z.step(&list, noon, 50, 14_105_000, false, 0, 900, false, true).is_empty());
        assert!(z.step(&list, noon, 61, 14_105_000, false, 0, 900, false, true).is_empty());
        assert_eq!(z.step(&list, noon, 62, 14_105_000, false, 0, 900, false, true), vec![ScanAction::Tune { freq_hz: 7_105_000, tune: false }]);
        // PTT disabled: no beacons
        let mut q = Scanner::default();
        q.step(&list, noon, 0, 0, false, 0, 900, false, true);
        assert!(q.step(&list, noon, 1, 14_105_000, false, 0, 900, false, true).is_empty());
    }
}
