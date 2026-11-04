//! Receive audio for remote stations: the station captures the radio's audio and sends it in
//! 20 ms frames at [`AUDIO_RATE`]; a desktop remote plays it through a jitter buffer.
//!
//! Capturing needs the platform's audio, which the apps with the built-in modem have (they
//! register it with [`set_capture`], using the modem's own devices). Without it, or with no
//! radio audio input chosen, the station simply offers no audio: nothing else depends on it.
//! Playing needs the `remote-audio` feature (cpal).

use std::io;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::{AUDIO_FRAME, AUDIO_RATE, AUDIO_RX};

/// The rate captures deliver (the modem's).
pub const CAPTURE_RATE: u32 = 48_000;
const DECIMATE: usize = (CAPTURE_RATE / AUDIO_RATE) as usize;

/// Reads 48 kHz mono samples, filling the slice (blocking).
pub type Reader = Box<dyn FnMut(&mut [i16]) -> io::Result<()>>;

/// What the app gives us to capture with.
pub struct CaptureHooks {
    /// Opens the named input. Called on the capture thread, which then owns the reader.
    pub open: Box<dyn Fn(&str) -> io::Result<Reader> + Send + Sync>,
    /// The inputs to choose from, for the Remote window: (the name to open, a description).
    pub inputs: Box<dyn Fn() -> Vec<(String, String)> + Send + Sync>,
    /// The input used when none is chosen: the built-in modem's (the radio).
    pub default_input: Box<dyn Fn() -> String + Send + Sync>,
}

static HOOKS: OnceLock<CaptureHooks> = OnceLock::new();

#[allow(dead_code)] // (the apps with the built-in modem)
pub fn set_capture(hooks: CaptureHooks) {
    let _ = HOOKS.set(hooks);
}

/// This app can capture audio at all.
pub fn can_capture() -> bool {
    HOOKS.get().is_some()
}

pub fn inputs() -> Vec<(String, String)> {
    HOOKS.get().map(|h| (h.inputs)()).unwrap_or_default()
}

/// The modem's input (read from its settings at most every few seconds: the station asks
/// on every tick).
pub fn default_input() -> String {
    static CACHE: Mutex<Option<(Instant, String)>> = Mutex::new(None);
    let Some(h) = HOOKS.get() else { return String::new() };
    let Ok(mut c) = CACHE.lock() else { return (h.default_input)() };
    match c.as_ref() {
        Some((at, name)) if at.elapsed() < Duration::from_secs(5) => name.clone(),
        _ => {
            let name = (h.default_input)();
            *c = Some((Instant::now(), name.clone()));
            name
        }
    }
}

/// The station side: one capture shared by every client listening, running only while
/// someone is (the radio's sound card is left alone otherwise).
pub struct AudioHub {
    tx: tokio::sync::broadcast::Sender<Arc<Vec<u8>>>,
    listeners: Arc<AtomicUsize>,
    running: Arc<AtomicBool>,
    device: Arc<Mutex<String>>,
    note: Arc<Mutex<String>>,
}

impl AudioHub {
    pub fn new(device: &str) -> Self {
        let (tx, _) = tokio::sync::broadcast::channel(64);
        Self {
            tx,
            listeners: Arc::new(AtomicUsize::new(0)),
            running: Arc::new(AtomicBool::new(false)),
            device: Arc::new(Mutex::new(device.to_string())),
            note: Arc::new(Mutex::new(String::new())),
        }
    }

    /// Audio can be offered: the app captures, and there's an input to capture from.
    pub fn available(&self) -> bool {
        can_capture() && !self.input().is_empty()
    }

    fn input(&self) -> String {
        let d = self.device.lock().map(|d| d.clone()).unwrap_or_default();
        if d.is_empty() { default_input() } else { d }
    }

    /// Why there's no audio, or what's wrong with it (empty when it's fine).
    pub fn note(&self) -> String {
        if !can_capture() {
            return "This Openslot doesn't capture audio".into();
        }
        if self.input().is_empty() {
            return "No radio audio input chosen (Remote control window)".into();
        }
        self.note.lock().map(|n| n.clone()).unwrap_or_default()
    }

    pub fn listeners(&self) -> usize {
        self.listeners.load(Ordering::Relaxed)
    }

    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<Arc<Vec<u8>>> {
        let rx = self.tx.subscribe();
        self.listeners.fetch_add(1, Ordering::SeqCst);
        if self.available() && !self.running.swap(true, Ordering::SeqCst) {
            self.spawn_capture();
        }
        rx
    }

    pub fn unsubscribe(&self) {
        let mut n = self.listeners.load(Ordering::SeqCst);
        while n > 0 {
            match self.listeners.compare_exchange(n, n - 1, Ordering::SeqCst, Ordering::SeqCst) {
                Ok(_) => break,
                Err(now) => n = now,
            }
        }
    }

    fn spawn_capture(&self) {
        let (tx, listeners, running, note) = (self.tx.clone(), self.listeners.clone(), self.running.clone(), self.note.clone());
        let name = self.input();
        let set_note = move |t: String| {
            if let Ok(mut n) = note.lock() {
                *n = t;
            }
        };
        let r = std::thread::Builder::new().name("remote-audio".into()).spawn(move || {
            let Some(hooks) = HOOKS.get() else {
                running.store(false, Ordering::SeqCst);
                return;
            };
            let mut reader: Option<Reader> = None;
            let mut decim = Decimator::new();
            let mut block = vec![0i16; 480];
            let mut frame: Vec<i16> = Vec::with_capacity(AUDIO_FRAME);
            let mut seq: u32 = 0;
            let mut idle_since: Option<Instant> = None;
            loop {
                if listeners.load(Ordering::SeqCst) == 0 {
                    // (closed after a little while, so a quick reconnect doesn't reopen it)
                    reader = None;
                    let since = *idle_since.get_or_insert_with(Instant::now);
                    if since.elapsed() > Duration::from_secs(5) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                    continue;
                }
                idle_since = None;
                let Some(r) = reader.as_mut() else {
                    match (hooks.open)(&name) {
                        Ok(r) => {
                            reader = Some(r);
                            set_note(String::new());
                            tracing::info!("remote audio: capturing {name}");
                        }
                        Err(e) => {
                            // Never another input instead: the radio's or none
                            set_note(format!("Audio input \"{name}\": {e}"));
                            std::thread::sleep(Duration::from_secs(2));
                        }
                    }
                    continue;
                };
                if let Err(e) = r(&mut block) {
                    set_note(format!("Audio input \"{name}\": {e}"));
                    reader = None;
                    std::thread::sleep(Duration::from_secs(2));
                    continue;
                }
                for &s in &block {
                    if let Some(v) = decim.push(s) {
                        frame.push(v);
                        if frame.len() == AUDIO_FRAME {
                            seq = seq.wrapping_add(1);
                            let _ = tx.send(Arc::new(encode_frame(seq, &frame)));
                            frame.clear();
                        }
                    }
                }
            }
            running.store(false, Ordering::SeqCst);
        });
        if r.is_err() {
            self.running.store(false, Ordering::SeqCst);
        }
    }
}

pub fn encode_frame(seq: u32, samples: &[i16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(5 + samples.len() * 2);
    out.push(AUDIO_RX);
    out.extend_from_slice(&seq.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

pub fn decode_frame(data: &[u8]) -> Option<(u32, Vec<i16>)> {
    if data.len() < 5 || data[0] != AUDIO_RX || (data.len() - 5) % 2 != 0 {
        return None;
    }
    let seq = u32::from_le_bytes(data[1..5].try_into().ok()?);
    Some((seq, data[5..].chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect()))
}

/// 48 kHz → 12 kHz: a windowed-sinc low-pass (5 kHz) and every 4th sample.
pub struct Decimator {
    taps: Vec<f32>,
    hist: Vec<f32>,
    pos: usize,
    phase: usize,
}

impl Decimator {
    const TAPS: usize = 63;

    pub fn new() -> Self {
        let n = Self::TAPS;
        let fc = 5_000.0 / CAPTURE_RATE as f32;
        let mid = (n - 1) as f32 / 2.0;
        let mut taps: Vec<f32> = (0..n)
            .map(|i| {
                let x = i as f32 - mid;
                let sinc = if x == 0.0 { 2.0 * fc } else { (2.0 * std::f32::consts::PI * fc * x).sin() / (std::f32::consts::PI * x) };
                // Blackman window
                let w = 0.42 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (n - 1) as f32).cos()
                    + 0.08 * (4.0 * std::f32::consts::PI * i as f32 / (n - 1) as f32).cos();
                sinc * w
            })
            .collect();
        let sum: f32 = taps.iter().sum();
        taps.iter_mut().for_each(|t| *t /= sum);
        Self { taps, hist: vec![0.0; n], pos: 0, phase: 0 }
    }

    pub fn push(&mut self, s: i16) -> Option<i16> {
        self.hist[self.pos] = s as f32;
        self.pos = (self.pos + 1) % self.hist.len();
        self.phase = (self.phase + 1) % DECIMATE;
        if self.phase != 0 {
            return None;
        }
        let n = self.hist.len();
        let acc: f32 = self.taps.iter().enumerate().map(|(i, t)| t * self.hist[(self.pos + i) % n]).sum();
        Some(acc.round().clamp(-32768.0, 32767.0) as i16)
    }
}

impl Default for Decimator {
    fn default() -> Self {
        Self::new()
    }
}

/// Linear-interpolating sample-rate converter that keeps state across chunks.
#[cfg_attr(not(feature = "remote-audio"), allow(dead_code))]
pub struct Resampler {
    step: f64,
    pos: f64,
    buf: Vec<f32>,
}

#[cfg_attr(not(feature = "remote-audio"), allow(dead_code))]
impl Resampler {
    pub fn new(from: u32, to: u32) -> Self {
        Resampler { step: from as f64 / to as f64, pos: 0.0, buf: Vec::new() }
    }

    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        self.buf.extend_from_slice(input);
        while self.pos + 1.0 < self.buf.len() as f64 {
            let i = self.pos as usize;
            let f = (self.pos - i as f64) as f32;
            out.push(self.buf[i] * (1.0 - f) + self.buf[i + 1] * f);
            self.pos += self.step;
        }
        let used = (self.pos as usize).min(self.buf.len());
        self.buf.drain(..used);
        self.pos -= used as f64;
    }
}

/// Playout buffer for audio that arrives in clumps (as RigHelm's): Wi-Fi often delivers
/// the 20 ms frames several at a time, 100 ms or more apart. It waits until it holds
/// `target` before playing, and when it runs dry anyway it fades out (no click), waits for
/// more, deepens, and fades back in. The two sound cards' clocks never quite agree, so the
/// playback speed follows the measured difference (samples received against played over
/// the last half minute or so) plus a gentle pull toward `target`, never more than 3 % off.
pub struct Jitter {
    buf: std::collections::VecDeque<f32>,
    playing: bool,
    target: usize,
    min_target: usize,
    max_target: usize,
    cap: usize,
    smoothed: f64,
    received: f64,
    ticks: f64,
    since_fade: usize,
    speed: f64,
    frac: f64,
    since_dry: usize,
    last: f32,
    gain: f32,
    gain_step: f32,
    rate: u32,
    pub dropouts: u32,
}

#[cfg_attr(not(feature = "remote-audio"), allow(dead_code))]
impl Jitter {
    const MAX_SKEW: f64 = 0.03;

    pub fn new(rate: u32, start: Duration, max: Duration) -> Self {
        let n = |d: Duration| (d.as_secs_f64() * rate as f64) as usize;
        let target = n(start).max(1);
        let max_target = n(max).max(target);
        Jitter {
            buf: std::collections::VecDeque::new(),
            playing: false,
            target,
            min_target: target,
            max_target,
            cap: max_target * 2,
            smoothed: target as f64,
            received: rate as f64 * 10.0,
            ticks: rate as f64 * 10.0,
            since_fade: 0,
            speed: 1.0,
            frac: 0.0,
            since_dry: 0,
            last: 0.0,
            gain: 0.0,
            gain_step: 1.0 / (rate as f32 * 0.005),
            rate,
            dropouts: 0,
        }
    }

    pub fn push(&mut self, samples: &[f32]) {
        if self.playing {
            self.received += samples.len() as f64;
        }
        self.buf.extend(samples.iter().copied());
        let excess = self.buf.len().saturating_sub(self.cap);
        self.buf.drain(..excess);
    }

    pub fn next(&mut self) -> f32 {
        if !self.playing && self.buf.len() >= self.target {
            self.playing = true;
            self.frac = 0.0;
        }
        if self.playing {
            if self.buf.len() >= 2 {
                let (a, b) = (self.buf[0], self.buf[1]);
                let v = a + (b - a) * self.frac as f32;
                self.steer();
                self.frac += self.speed;
                while self.frac >= 1.0 && self.buf.len() >= 2 {
                    self.buf.pop_front();
                    self.frac -= 1.0;
                }
                self.gain = (self.gain + self.gain_step).min(1.0);
                self.last = v;
                return v * self.gain;
            }
            self.playing = false;
            self.since_dry = 0;
            self.target = (self.target + self.target / 2).min(self.max_target);
            self.dropouts += 1;
        }
        self.gain = (self.gain - self.gain_step).max(0.0);
        self.last * self.gain
    }

    fn steer(&mut self) {
        let rate = self.rate as f64;
        self.ticks += 1.0;
        self.since_fade += 1;
        if self.since_fade >= self.rate as usize {
            self.since_fade = 0;
            self.received *= 0.97;
            self.ticks *= 0.97;
        }
        self.smoothed += (self.buf.len() as f64 - self.smoothed) / (2.0 * rate);
        let drift = ((self.received - self.ticks) / self.ticks).clamp(-Self::MAX_SKEW, Self::MAX_SKEW);
        let err = (self.smoothed - self.target as f64) / (rate / 10.0);
        let pull = (err * 0.005).clamp(-0.01, 0.01);
        self.speed = 1.0 + (drift + pull).clamp(-Self::MAX_SKEW, Self::MAX_SKEW);
        self.since_dry += 1;
        if self.since_dry >= self.rate as usize * 300 {
            self.since_dry = 0;
            self.target = (self.target - self.target / 10).max(self.min_target);
        }
    }
}

/// The remote side: plays received frames on this computer's default output.
/// Whether frame `seq` plays after `last`: None for a repeat or one late by under 5 s, else
/// how many frames of silence fill the gap before it (a short gap only). A frame further back
/// than that means the station started its count over (it restarted), so it's taken.
fn frame_order(last: Option<u32>, seq: u32) -> Option<u32> {
    let Some(last) = last else { return Some(0) };
    let ahead = seq.wrapping_sub(last);
    if ahead == 0 {
        return None;
    }
    if ahead > u32::MAX / 2 {
        return (last.wrapping_sub(seq) >= 250).then_some(0);
    }
    Some(if (2..=5).contains(&ahead) { ahead - 1 } else { 0 })
}

pub struct Player {
    ring: Arc<Mutex<Option<(Resampler, Jitter)>>>,
    stop: Arc<AtomicBool>,
    last_seq: Option<u32>,
    /// 0…1, set from the remote's volume slider
    pub volume: Arc<Mutex<f32>>,
}

impl Player {
    #[cfg(feature = "remote-audio")]
    pub fn start() -> Result<Self, String> {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        let ring: Arc<Mutex<Option<(Resampler, Jitter)>>> = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let volume = Arc::new(Mutex::new(0.8f32));
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();
        let (r2, s2, v2) = (ring.clone(), stop.clone(), volume.clone());
        // (cpal streams can't move between threads on every platform: this one owns it)
        std::thread::Builder::new()
            .name("remote-play".into())
            .spawn(move || {
                let open = || -> Result<cpal::Stream, String> {
                    let device = cpal::default_host().default_output_device().ok_or("no audio output on this computer")?;
                    let supported = device.default_output_config().map_err(|e| e.to_string())?;
                    let config = supported.config();
                    let channels = (config.channels as usize).max(1);
                    let rate = config.sample_rate.0;
                    if let Ok(mut r) = r2.lock() {
                        *r = Some((Resampler::new(AUDIO_RATE, rate), Jitter::new(rate, Duration::from_millis(120), Duration::from_millis(500))));
                    }
                    let (ring, vol) = (r2.clone(), v2.clone());
                    let fill = move |data: &mut [f32]| {
                        let gain = vol.lock().map(|v| *v).unwrap_or(1.0);
                        let mut r = ring.lock().ok();
                        for frame in data.chunks_mut(channels) {
                            let v = r.as_mut().and_then(|r| r.as_mut()).map_or(0.0, |(_, j)| j.next()) * gain;
                            frame.fill(v);
                        }
                    };
                    let err = |e: cpal::StreamError| tracing::debug!("remote audio output: {e}");
                    let stream = match supported.sample_format() {
                        cpal::SampleFormat::F32 => device.build_output_stream::<f32, _, _>(&config, move |d: &mut [f32], _: &cpal::OutputCallbackInfo| fill(d), err, None),
                        cpal::SampleFormat::I16 => {
                            let mut tmp = Vec::new();
                            device.build_output_stream::<i16, _, _>(&config, move |d: &mut [i16], _: &cpal::OutputCallbackInfo| {
                                tmp.resize(d.len(), 0.0);
                                fill(&mut tmp);
                                for (o, v) in d.iter_mut().zip(&tmp) {
                                    *o = (v * 32767.0).clamp(-32768.0, 32767.0) as i16;
                                }
                            }, err, None)
                        }
                        other => return Err(format!("unsupported output format {other:?}")),
                    }
                    .map_err(|e| e.to_string())?;
                    stream.play().map_err(|e| e.to_string())?;
                    Ok(stream)
                };
                match open() {
                    Ok(stream) => {
                        let _ = ready_tx.send(Ok(()));
                        while !s2.load(Ordering::Relaxed) {
                            std::thread::sleep(Duration::from_millis(100));
                        }
                        drop(stream);
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        ready_rx.recv_timeout(Duration::from_secs(5)).map_err(|_| "the audio output didn't start".to_string())??;
        Ok(Self { ring, stop, last_seq: None, volume })
    }

    #[cfg(not(feature = "remote-audio"))]
    pub fn start() -> Result<Self, String> {
        Err("this build of Openslot doesn't play audio".into())
    }

    /// A frame from the station (gaps of a few frames are filled with silence, so the
    /// buffer fades across them rather than splicing).
    pub fn push(&mut self, seq: u32, samples: &[i16]) {
        let Some(missing) = frame_order(self.last_seq, seq) else { return };
        if missing > 0 {
            let gap = vec![0.0f32; AUDIO_FRAME * missing as usize];
            self.feed(&gap);
        }
        self.last_seq = Some(seq);
        let f: Vec<f32> = samples.iter().map(|&s| s as f32 / 32768.0).collect();
        self.feed(&f);
    }

    /// A new session with the station: its frames may be numbered afresh.
    pub fn new_session(&mut self) {
        self.last_seq = None;
    }

    fn feed(&mut self, samples: &[f32]) {
        if let Ok(mut r) = self.ring.lock() {
            if let Some((rs, j)) = r.as_mut() {
                let mut out = Vec::with_capacity(samples.len() * 4 + 8);
                rs.process(samples, &mut out);
                j.push(&out);
            }
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip() {
        let s: Vec<i16> = (0..AUDIO_FRAME as i16).map(|i| i * 100 - 9000).collect();
        let f = encode_frame(42, &s);
        assert_eq!(f.len(), 5 + AUDIO_FRAME * 2);
        assert_eq!(decode_frame(&f), Some((42, s)));
        assert_eq!(decode_frame(&[2, 0, 0, 0, 0]), None);
    }

    /// A 1 kHz tone passes; 9 kHz (above the new Nyquist) is stopped.
    #[test]
    fn decimator_filters() {
        let rms = |freq: f32| {
            let mut d = Decimator::new();
            let out: Vec<f32> = (0..48_000)
                .filter_map(|i| d.push((10_000.0 * (2.0 * std::f32::consts::PI * freq * i as f32 / 48_000.0).sin()) as i16))
                .skip(100)
                .map(|v| v as f32)
                .collect();
            (out.iter().map(|v| v * v).sum::<f32>() / out.len() as f32).sqrt()
        };
        assert_eq!(rms(1000.0).round() as i32 / 100, 70); // 10000 / √2
        assert!(rms(9000.0) < 50.0);
    }

    #[test]
    fn jitter_waits_then_plays() {
        let mut j = Jitter::new(1000, Duration::from_millis(50), Duration::from_millis(200));
        j.push(&[0.5; 20]);
        assert_eq!(j.next(), 0.0, "still filling");
        j.push(&[0.5; 100]);
        let v: Vec<f32> = (0..400).map(|_| j.next()).collect();
        assert!(v[60] > 0.49, "plays at full level after the fade in");
        assert!(v[399].abs() < 0.01, "fades out when it runs dry");
        assert!(j.dropouts <= 1);
    }

    /// Frames in order play, a short gap is filled, repeats and late ones are dropped, and
    /// a station that restarted (its count from 1 again) is heard at once.
    #[test]
    fn frames_play_in_order_and_a_restart_is_heard() {
        assert_eq!(frame_order(None, 1), Some(0));
        assert_eq!(frame_order(Some(1), 2), Some(0));
        assert_eq!(frame_order(Some(3), 6), Some(2), "two frames of silence");
        assert_eq!(frame_order(Some(7), 7), None, "a repeat");
        assert_eq!(frame_order(Some(7), 5), None, "late");
        assert_eq!(frame_order(Some(3000), 1), Some(0), "the station restarted");
        assert_eq!(frame_order(Some(u32::MAX), 0), Some(0), "the count wraps");
    }

    /// The desktop remote's player on this computer's output (silence, so nothing is heard):
    /// frames in order, a short gap filled, a late one dropped.
    #[cfg(feature = "remote-audio")]
    #[test]
    fn player_takes_frames() {
        let mut p = match Player::start() {
            Ok(p) => p,
            // (a machine with no audio output: nothing to test)
            Err(e) => return eprintln!("no audio output: {e}"),
        };
        let quiet = vec![0i16; AUDIO_FRAME];
        for seq in [1, 2, 3, 6, 7, 5] {
            p.push(seq, &quiet);
        }
        assert_eq!(p.last_seq, Some(7), "the late frame 5 was dropped");
        let depth = p.ring.lock().unwrap().as_ref().map(|(_, j)| j.buf.len()).unwrap();
        assert!(depth > 0, "audio waiting to play");
        std::thread::sleep(Duration::from_millis(300));
        drop(p);
    }
}
