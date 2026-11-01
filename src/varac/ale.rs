//! VarAC's ALE (candidate frequencies, the "ALE - Connect" window, the
//! mid-link QSY suggestion): with the frequency scanner in ALE mode, connecting to a
//! station first ranks the frequencies we heard its beacons on (best average SNR first —
//! the last hour, then the same hour of the last days, then the last days), then calls it
//! on each usable one in turn, with an approval countdown between attempts.

/// A frequency we heard the station on.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub freq_hz: u64,
    pub avg_snr: f32,
    pub beacons: u32,
    pub minutes_since_heard: f64,
    /// "Last 60 min", "Same hour, last 3 days", "Last 3 days"
    pub basis: String,
    /// Why it can't be used ("Not in scanner list", "Outside allowed QSY range"), or empty
    pub reason: String,
}

/// One pass's rows: (frequency, average SNR, beacons, minutes since last heard).
pub type PassRows = Vec<(u64, f32, u32, f64)>;

/// Merge the three passes (a frequency counts once, from its first pass) and mark the
/// unusable ones.
pub fn candidates(
    passes: &[(String, PassRows)],
    in_scanner_list: impl Fn(u64) -> bool,
    qsy_allowed: impl Fn(u64) -> bool,
) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    for (basis, rows) in passes {
        for &(f, avg, n, mins) in rows {
            if out.iter().any(|c| c.freq_hz == f) {
                continue;
            }
            let reason = if !in_scanner_list(f) {
                "Not in scanner list"
            } else if !qsy_allowed(f) {
                "Outside allowed QSY range"
            } else {
                ""
            };
            out.push(Candidate {
                freq_hz: f,
                avg_snr: (avg * 10.0).round() / 10.0,
                beacons: n,
                minutes_since_heard: mins,
                basis: basis.clone(),
                reason: reason.into(),
            });
        }
    }
    out
}

/// The best usable frequency, if any.
pub fn best(c: &[Candidate]) -> Option<&Candidate> {
    c.iter().find(|c| c.reason.is_empty())
}

/// VarAC's "Last heard" column.
pub fn age_text(minutes: f64) -> String {
    if minutes < 60.0 {
        format!("{} min", minutes.round())
    } else if minutes < 1440.0 {
        format!("{:.1} h", minutes / 60.0)
    } else {
        format!("{:.1} d", minutes / 1440.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowState {
    Pending,
    Calling,
    Failed,
    Connected,
    /// Not usable (shown grey with its reason)
    Blocked,
}

impl RowState {
    pub fn icon(self) -> &'static str {
        match self {
            RowState::Pending => "○",
            RowState::Calling => "►",
            RowState::Failed => "✖",
            RowState::Connected => "✔",
            RowState::Blocked => "—",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Row {
    pub freq_hz: u64,
    /// None for the "Stay on current frequency" row
    pub candidate: Option<Candidate>,
    pub state: RowState,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Phase {
    /// Counting down to the next attempt (tenths of a second left; None = manual: CONNECT)
    Approval(Option<u32>),
    Calling,
    /// Connected: the window closes itself after this many tenths
    Success(u32),
    /// All failed or aborted
    Finished,
}

/// What the app must do after a tick or a report.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Retune if needed, then connect to the station (an attempt)
    Call(u64),
    /// Abort the call under way
    Abort,
    /// Retune to this frequency (RETURN)
    Tune(u64),
    Close,
    /// Play VarAC's ring / alarm
    Ring,
    Alarm,
}

/// The "ALE - Connect" window's state.
#[derive(Debug, Clone)]
pub struct Window {
    pub call: String,
    pub start_freq: u64,
    pub rows: Vec<Row>,
    pub selected: usize,
    pub phase: Phase,
    pub attempts: u32,
    pub ptt_cycles: u32,
    pub jump_to: Option<usize>,
    pub timeline: Vec<String>,
    pub status: String,
    pub skip_approval: bool,
    /// The row that just failed (for "No answer on … Next: …")
    pub last_failed: Option<u64>,
}

pub fn fmt_freq(hz: u64) -> String {
    format!("{:.3} MHz", hz as f64 / 1e6)
}

impl Window {
    pub fn new(call: &str, start_freq: u64, candidates: Vec<Candidate>, skip_approval: bool, now: &str) -> Self {
        let mut rows: Vec<Row> = candidates.into_iter().map(|c| Row {
            freq_hz: c.freq_hz,
            state: if c.reason.is_empty() { RowState::Pending } else { RowState::Blocked },
            candidate: Some(c),
        }).collect();
        let usable = rows.iter().filter(|r| r.state == RowState::Pending).count();
        rows.push(Row { freq_hz: start_freq, candidate: None, state: RowState::Pending });
        let mut w = Window {
            call: call.to_uppercase(),
            start_freq,
            selected: 0,
            phase: Phase::Approval(None),
            attempts: 0,
            ptt_cycles: 0,
            jump_to: None,
            timeline: Vec::new(),
            status: String::new(),
            skip_approval,
            last_failed: None,
            rows,
        };
        w.log(now, &format!("ALE sequence started for {}", w.call));
        if usable == 0 {
            w.log(now, &format!("No usable beacon history for {}", w.call));
        }
        w.selected = w.first_pending().unwrap_or(0);
        w.approval(if skip_approval { 1 } else { 15 });
        w
    }

    pub fn log(&mut self, now: &str, text: &str) {
        self.timeline.push(format!("{now}  {text}"));
    }

    fn first_pending(&self) -> Option<usize> {
        self.rows.iter().position(|r| r.state == RowState::Pending)
    }

    pub fn usable(&self) -> usize {
        self.rows.iter().filter(|r| r.candidate.as_ref().is_none_or(|c| c.reason.is_empty())).count()
    }

    fn approval(&mut self, secs: u32) {
        self.phase = Phase::Approval(Some(secs * 10));
        self.refresh_status();
    }

    pub fn refresh_status(&mut self) {
        let f = self.rows.get(self.selected).map(|r| fmt_freq(r.freq_hz)).unwrap_or_default();
        self.status = match self.phase {
            Phase::Approval(Some(t)) => {
                let s = t.div_ceil(10);
                match self.last_failed {
                    Some(failed) if self.attempts > 0 => format!("No answer on {}. Next: {f} in {s}s", fmt_freq(failed)),
                    _ => format!("ALE recommends {f}. Connecting in {s}s"),
                }
            }
            Phase::Approval(None) => format!("Click CONNECT to call {} on {f}", self.call),
            Phase::Calling => format!("Attempt {} of {}: calling {} on {f}, call {} of {}",
                self.attempts, self.usable(), self.call, self.ptt_cycles.max(1), 15),
            Phase::Success(_) => format!("Connected to {} on {f}", self.call),
            Phase::Finished => self.status.clone(),
        };
    }

    /// Every 100 ms. `busy` = our beacon or PTT is on (then the attempt waits).
    pub fn tick(&mut self, busy: bool, now: &str) -> Vec<Action> {
        match self.phase {
            Phase::Approval(Some(t)) if t <= 1 => {
                if busy {
                    self.phase = Phase::Approval(Some(10));
                    self.status = "Waiting for the beacon to end...".into();
                    return Vec::new();
                }
                self.start_attempt(now)
            }
            Phase::Approval(Some(t)) => {
                self.phase = Phase::Approval(Some(t - 1));
                if t % 10 == 1 {
                    self.refresh_status();
                }
                Vec::new()
            }
            Phase::Success(t) if t <= 1 => vec![Action::Close],
            Phase::Success(t) => {
                self.phase = Phase::Success(t - 1);
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    /// CONNECT (or the countdown ran out): call on the selected row.
    pub fn start_attempt(&mut self, now: &str) -> Vec<Action> {
        let Some(row) = self.rows.get_mut(self.selected).filter(|r| r.state == RowState::Pending) else { return Vec::new() };
        row.state = RowState::Calling;
        let f = row.freq_hz;
        self.attempts += 1;
        self.ptt_cycles = 0;
        self.phase = Phase::Calling;
        self.log(now, &format!("Calling on {}", fmt_freq(f)));
        self.refresh_status();
        vec![Action::Call(f)]
    }

    pub fn ptt_on(&mut self) {
        if self.phase == Phase::Calling {
            self.ptt_cycles += 1;
            self.refresh_status();
        }
    }

    pub fn connected(&mut self, now: &str) -> Vec<Action> {
        if self.phase != Phase::Calling {
            return Vec::new();
        }
        let f = self.rows[self.selected].freq_hz;
        self.rows[self.selected].state = RowState::Connected;
        self.log(now, &format!("Connected on {}", fmt_freq(f)));
        self.phase = Phase::Success(50);
        self.refresh_status();
        vec![Action::Ring]
    }

    /// The link ended without connecting.
    pub fn failed(&mut self, now: &str) -> Vec<Action> {
        if self.phase != Phase::Calling {
            return Vec::new();
        }
        let f = self.rows[self.selected].freq_hz;
        self.rows[self.selected].state = RowState::Failed;
        self.log(now, &format!("No answer on {}", fmt_freq(f)));
        self.last_failed = Some(f);
        match self.jump_to.take().filter(|&j| self.rows.get(j).is_some_and(|r| r.state == RowState::Pending)).or_else(|| self.first_pending()) {
            Some(next) => {
                self.selected = next;
                self.approval(if self.skip_approval { 2 } else { 15 });
                Vec::new()
            }
            None => {
                self.phase = Phase::Finished;
                self.status = format!("All {} options failed", self.attempts);
                vec![Action::Alarm]
            }
        }
    }

    /// We aborted the call (ABORT, or a jump to another row).
    pub fn aborted(&mut self, now: &str) {
        if self.phase != Phase::Calling {
            return;
        }
        self.rows[self.selected].state = RowState::Failed;
        self.log(now, "Call aborted");
        match self.jump_to.take() {
            Some(j) => {
                self.selected = j;
                self.log(now, "Modem busy, retrying");
                self.approval(2);
            }
            None => {
                self.phase = Phase::Finished;
                self.status = "Aborted".into();
            }
        }
    }

    /// A click on a row: in the countdown it picks that row (manual CONNECT); during a call
    /// it switches to it (the call is aborted).
    pub fn click_row(&mut self, i: usize, now: &str) -> Vec<Action> {
        if self.rows.get(i).is_none_or(|r| r.state != RowState::Pending) {
            return Vec::new();
        }
        match self.phase {
            Phase::Approval(_) if i != self.selected => {
                self.selected = i;
                self.phase = Phase::Approval(None);
                self.refresh_status();
                Vec::new()
            }
            Phase::Calling => {
                self.jump_to = Some(i);
                let f = self.rows[i].freq_hz;
                self.log(now, &format!("Switching to {}", fmt_freq(f)));
                vec![Action::Abort]
            }
            _ => Vec::new(),
        }
    }

    /// CANCEL / ABORT / CLOSE
    pub fn cancel(&mut self) -> Vec<Action> {
        match self.phase {
            Phase::Calling => {
                self.jump_to = None;
                vec![Action::Abort]
            }
            _ => vec![Action::Close],
        }
    }

    pub fn try_again(&mut self) {
        for r in self.rows.iter_mut() {
            if r.state != RowState::Blocked {
                r.state = RowState::Pending;
            }
        }
        (self.attempts, self.last_failed) = (0, None);
        self.selected = self.first_pending().unwrap_or(0);
        self.approval(15);
    }

    pub fn return_to_start(&self) -> Vec<Action> {
        vec![Action::Tune(self.start_freq), Action::Close]
    }

    pub fn cancel_label(&self) -> &'static str {
        match self.phase {
            Phase::Calling => "ABORT",
            Phase::Approval(_) => "CANCEL",
            _ => "CLOSE",
        }
    }
}

/// VarAC's free-form QSY tag: `<QSYF>` + the frequency in Hz, 12 digits + `</QSYF>`.
pub fn qsyf(freq_hz: u64) -> String {
    format!("<QSYF>{freq_hz:012}</QSYF>")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cands() -> Vec<Candidate> {
        let passes = vec![
            ("Last 60 min".to_string(), vec![(14_105_000, 3.04, 2, 10.0), (7_105_000, -2.0, 1, 30.0)]),
            ("Same hour, last 3 days".to_string(), vec![(14_105_000, 9.0, 5, 1500.0), (10_133_000, -8.0, 3, 1600.0)]),
            ("Last 3 days".to_string(), vec![(21_105_000, 0.0, 1, 3000.0)]),
        ];
        candidates(&passes, |f| f != 21_105_000, |f| f != 7_105_000)
    }

    #[test]
    fn ranking() {
        let c = cands();
        assert_eq!(c.iter().map(|c| c.freq_hz).collect::<Vec<_>>(), vec![14_105_000, 7_105_000, 10_133_000, 21_105_000]);
        assert_eq!(c[0].avg_snr, 3.0);
        assert_eq!(c[0].basis, "Last 60 min");
        assert_eq!(c[1].reason, "Outside allowed QSY range");
        assert_eq!(c[3].reason, "Not in scanner list");
        assert_eq!(best(&c).map(|c| c.freq_hz), Some(14_105_000));
        assert_eq!(age_text(10.4), "10 min");
        assert_eq!(age_text(90.0), "1.5 h");
        assert_eq!(age_text(2880.0), "2.0 d");
        assert_eq!(qsyf(14_105_000), "<QSYF>000014105000</QSYF>");
    }

    #[test]
    fn window_flow() {
        let mut w = Window::new("nr5p", 14_100_000, cands(), false, "00:00:00");
        // usable: 14.105, 10.133, plus "stay"
        assert_eq!(w.usable(), 3);
        assert_eq!(w.rows.last().unwrap().freq_hz, 14_100_000);
        assert!(w.status.starts_with("ALE recommends 14.105 MHz. Connecting in 15s"), "{}", w.status);
        for _ in 0..149 {
            assert!(w.tick(false, "t").is_empty());
        }
        assert_eq!(w.tick(false, "t"), vec![Action::Call(14_105_000)]);
        w.ptt_on();
        assert!(w.status.contains("Attempt 1 of 3: calling NR5P on 14.105 MHz, call 1 of 15"), "{}", w.status);
        // no answer: next usable row (10.133), countdown again
        assert!(w.failed("t").is_empty());
        assert_eq!(w.rows[0].state, RowState::Failed);
        assert_eq!(w.selected, 2);
        assert!(w.status.starts_with("No answer on 14.105 MHz. Next: 10.133 MHz in 15s"), "{}", w.status);
        // a click picks the "stay" row: manual
        let stay = w.rows.len() - 1;
        w.click_row(stay, "t");
        assert_eq!(w.phase, Phase::Approval(None));
        assert_eq!(w.start_attempt("t"), vec![Action::Call(14_100_000)]);
        // switching rows during the call aborts it, then retries there in 2 s
        assert_eq!(w.click_row(2, "t"), vec![Action::Abort]);
        w.aborted("t");
        assert_eq!(w.selected, 2);
        assert_eq!(w.phase, Phase::Approval(Some(20)));
        for _ in 0..19 {
            w.tick(false, "t");
        }
        assert_eq!(w.tick(true, "t"), vec![], "PTT busy: waits");
        assert_eq!(w.status, "Waiting for the beacon to end...");
        for _ in 0..9 {
            w.tick(false, "t");
        }
        assert_eq!(w.tick(false, "t"), vec![Action::Call(10_133_000)]);
        assert_eq!(w.connected("t"), vec![Action::Ring]);
        assert!(w.status.starts_with("Connected to NR5P on 10.133 MHz"));
        for _ in 0..49 {
            assert!(w.tick(false, "t").is_empty());
        }
        assert_eq!(w.tick(false, "t"), vec![Action::Close]);
        // all failed
        let mut f = Window::new("K1ABC", 7_000_000, Vec::new(), true, "t");
        assert!(f.timeline.iter().any(|l| l.ends_with("No usable beacon history for K1ABC")));
        // (skip approval: 1 s)
        for _ in 0..9 {
            assert_eq!(f.tick(false, "t"), Vec::<Action>::new());
        }
        assert_eq!(f.tick(false, "t"), vec![Action::Call(7_000_000)]);
        assert_eq!(f.failed("t"), vec![Action::Alarm]);
        assert_eq!(f.status, "All 1 options failed");
        assert_eq!(f.cancel_label(), "CLOSE");
    }
}
