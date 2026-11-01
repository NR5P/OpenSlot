//! Event sounds and the narrator, after VarAC's.
//!
//! Sounds: one short tone pattern per event (our own, generated as WAV files on first
//! use), each switchable, and a "mute all". Narrator: the system's text-to-speech
//! (spd-say on Linux, `say` on macOS, System.Speech on Windows), one sentence at a
//! time, callsigns spelled with the NATO alphabet as VarAC does.

use std::sync::{mpsc, Mutex, OnceLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Sound {
    IncomingChat,
    OutgoingChat,
    Broadcast,
    RelayNotification,
    IncomingConnection,
    Connected,
    IsTyping,
    FrequencyChange,
    CqHeard,
    QsyInvitation,
    Alert,
    Reaction,
    GameMove,
    GameWin,
    GameLose,
    GameDraw,
}

impl Sound {
    pub const ALL: [Sound; 16] = [
        Sound::IncomingChat, Sound::OutgoingChat, Sound::Broadcast, Sound::RelayNotification,
        Sound::IncomingConnection, Sound::Connected, Sound::IsTyping, Sound::FrequencyChange,
        Sound::CqHeard, Sound::QsyInvitation, Sound::Alert, Sound::Reaction,
        Sound::GameMove, Sound::GameWin, Sound::GameLose, Sound::GameDraw,
    ];

    /// The config key (VarAC's ini names without "Play…Sound")
    pub fn key(self) -> &'static str {
        match self {
            Sound::IncomingChat => "incoming_chat",
            Sound::OutgoingChat => "outgoing_chat",
            Sound::Broadcast => "broadcast",
            Sound::RelayNotification => "relay_notification",
            Sound::IncomingConnection => "incoming_connection",
            Sound::Connected => "connected",
            Sound::IsTyping => "is_typing",
            Sound::FrequencyChange => "frequency_change",
            Sound::CqHeard => "cq_call",
            Sound::QsyInvitation => "qsy_invitation",
            Sound::Alert => "alert_tags",
            Sound::Reaction => "reactions",
            Sound::GameMove => "hamplay_move",
            Sound::GameWin => "hamplay_win",
            Sound::GameLose => "hamplay_lose",
            Sound::GameDraw => "hamplay_draw",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Sound::IncomingChat => "Incoming chat message",
            Sound::OutgoingChat => "Outgoing chat message",
            Sound::Broadcast => "Broadcasts",
            Sound::RelayNotification => "VMail relay notification",
            Sound::IncomingConnection => "Incoming connection",
            Sound::Connected => "Connected to a station",
            Sound::IsTyping => "Partner is typing",
            Sound::FrequencyChange => "Frequency change",
            Sound::CqHeard => "CQ call heard",
            Sound::QsyInvitation => "QSY invitation",
            Sound::Alert => "Alert tags",
            Sound::Reaction => "Reactions (likes)",
            Sound::GameMove => "HamPlay move",
            Sound::GameWin => "HamPlay win",
            Sound::GameLose => "HamPlay lose",
            Sound::GameDraw => "HamPlay draw",
        }
    }

    /// (frequency Hz, milliseconds) notes; 0 Hz = a pause
    fn notes(self) -> &'static [(f32, u32)] {
        match self {
            Sound::IncomingChat => &[(880.0, 70), (0.0, 30), (1175.0, 90)],
            Sound::OutgoingChat => &[(1175.0, 50), (0.0, 20), (880.0, 60)],
            Sound::Broadcast => &[(660.0, 90), (880.0, 90), (1100.0, 120)],
            Sound::RelayNotification => &[(990.0, 120), (0.0, 60), (990.0, 120), (0.0, 60), (1320.0, 180)],
            Sound::IncomingConnection => &[(740.0, 150), (0.0, 80), (740.0, 150), (0.0, 80), (740.0, 150)],
            Sound::Connected => &[(523.0, 90), (659.0, 90), (784.0, 160)],
            Sound::IsTyping => &[(1500.0, 25)],
            Sound::FrequencyChange => &[(2000.0, 12)],
            Sound::CqHeard => &[(1320.0, 200), (0.0, 50), (990.0, 260)],
            Sound::QsyInvitation => &[(784.0, 100), (988.0, 100), (784.0, 100), (988.0, 160)],
            Sound::Alert => &[(1400.0, 150), (900.0, 150), (1400.0, 150), (900.0, 150), (1400.0, 150), (900.0, 150)],
            Sound::Reaction => &[(1046.0, 60), (1568.0, 110)],
            Sound::GameMove => &[(600.0, 40)],
            Sound::GameWin => &[(523.0, 110), (659.0, 110), (784.0, 110), (1046.0, 260)],
            Sound::GameLose => &[(392.0, 160), (330.0, 160), (262.0, 300)],
            Sound::GameDraw => &[(523.0, 140), (523.0, 140)],
        }
    }
}

/// A mono 16-bit WAV of these notes (soft attack/decay so it doesn't click).
pub fn wav(notes: &[(f32, u32)]) -> Vec<u8> {
    const RATE: u32 = 22050;
    let mut pcm: Vec<i16> = Vec::new();
    for &(f, ms) in notes {
        let n = (RATE * ms / 1000) as usize;
        let ramp = (RATE as usize / 200).min(n / 2).max(1);
        for i in 0..n {
            let env = (i.min(n - 1 - i) as f32 / ramp as f32).min(1.0);
            let v = if f > 0.0 { (2.0 * std::f32::consts::PI * f * i as f32 / RATE as f32).sin() * env * 0.35 } else { 0.0 };
            pcm.push((v * i16::MAX as f32) as i16);
        }
    }
    let data_len = (pcm.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in pcm {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// Which sounds are on (config), read once and refreshed by `reload`.
static SETTINGS: Mutex<Option<(bool, Vec<String>)>> = Mutex::new(None);

pub fn reload() {
    let (mute, off) = crate::config::sound_settings();
    if let Ok(mut s) = SETTINGS.lock() {
        *s = Some((mute, off));
    }
}

fn enabled(s: Sound) -> bool {
    let mut g = match SETTINGS.lock() {
        Ok(g) => g,
        Err(_) => return false,
    };
    if g.is_none() {
        *g = Some(crate::config::sound_settings());
    }
    g.as_ref().is_some_and(|(mute, off)| !mute && !off.iter().any(|k| k == s.key()))
}

/// Play the event's sound, if on.
pub fn play(s: Sound) {
    if !enabled(s) {
        return;
    }
    play_now(s);
}

/// Play regardless of the settings (the settings page's test buttons).
pub fn play_now(s: Sound) {
    std::thread::spawn(move || {
        let dir = std::env::temp_dir().join("openslot_sounds");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(format!("{}.wav", s.key()));
        if !path.exists() {
            let _ = std::fs::write(&path, wav(s.notes()));
        }
        play_file(&path);
    });
}

fn play_file(path: &std::path::Path) {
    use std::process::{Command, Stdio};
    let quiet = |c: &mut Command| {
        c.stdout(Stdio::null()).stderr(Stdio::null());
        no_window(c);
    };
    #[cfg(target_os = "linux")]
    {
        let mut c = Command::new("paplay");
        c.arg(path);
        quiet(&mut c);
        if c.status().is_err() {
            let mut c = Command::new("aplay");
            c.arg("-q").arg(path);
            quiet(&mut c);
            let _ = c.status();
        }
    }
    #[cfg(target_os = "macos")]
    {
        let mut c = Command::new("afplay");
        c.arg(path);
        quiet(&mut c);
        let _ = c.status();
    }
    #[cfg(target_os = "windows")]
    {
        let mut c = Command::new("powershell");
        c.args(["-NoProfile", "-Command", &format!("(New-Object Media.SoundPlayer '{}').PlaySync()", path.display())]);
        quiet(&mut c);
        let _ = c.status();
    }
}

/// A console program started from the GUI (PowerShell for sounds and speech) opens no
/// console window on Windows.
pub fn no_window(c: &mut std::process::Command) -> &mut std::process::Command {
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(c, 0x0800_0000); // CREATE_NO_WINDOW
    c
}

// === Narrator ===

/// What the narrator speaks about (VarAC's Narrate… switches).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Topic {
    IncomingMessages,
    AlertsAndNotifications,
    Beacons,
    MyBeacons,
    Cqs,
    Broadcasts,
    MyBroadcasts,
}

/// VarAC spells callsigns with the NATO alphabet: "W5TST" → "Whiskey Five Tango Sierra Tango".
pub fn phonetic(call: &str) -> String {
    call.chars().filter_map(|c| {
        Some(match c.to_ascii_uppercase() {
            'A' => "Alpha", 'B' => "Bravo", 'C' => "Charlie", 'D' => "Delta", 'E' => "Echo", 'F' => "Foxtrot",
            'G' => "Golf", 'H' => "Hotel", 'I' => "India", 'J' => "Juliett", 'K' => "Kilo", 'L' => "Lima",
            'M' => "Mike", 'N' => "November", 'O' => "Oscar", 'P' => "Papa", 'Q' => "Quebec", 'R' => "Romeo",
            'S' => "Sierra", 'T' => "Tango", 'U' => "Uniform", 'V' => "Victor", 'W' => "Whiskey", 'X' => "X-ray",
            'Y' => "Yankee", 'Z' => "Zulu", '0' => "Zero", '1' => "One", '2' => "Two", '3' => "Three", '4' => "Four",
            '5' => "Five", '6' => "Six", '7' => "Seven", '8' => "Eight", '9' => "Nine", '-' => "Dash", '/' => "Stroke",
            _ => return None,
        })
    }).collect::<Vec<_>>().join(" ")
}

fn narrator() -> &'static Mutex<mpsc::Sender<String>> {
    static N: OnceLock<Mutex<mpsc::Sender<String>>> = OnceLock::new();
    N.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<String>();
        std::thread::Builder::new().name("narrator".into()).spawn(move || {
            for text in rx {
                speak(&text, crate::config::narrator().female);
            }
        }).ok();
        Mutex::new(tx)
    })
}

/// Speak `text` (in turn) if the narrator and this topic are on.
pub fn narrate(topic: Topic, text: impl Into<String>) {
    let n = crate::config::narrator();
    let wanted = match topic {
        Topic::IncomingMessages => n.incoming,
        Topic::AlertsAndNotifications => n.alerts,
        Topic::Beacons => n.beacons,
        Topic::MyBeacons => n.my_beacons,
        Topic::Cqs => n.cqs,
        Topic::Broadcasts => n.broadcasts,
        Topic::MyBroadcasts => n.my_broadcasts,
    };
    if !n.on || !wanted {
        return;
    }
    if let Ok(tx) = narrator().lock() {
        let _ = tx.send(text.into());
    }
}

/// Speak now regardless of the settings (the settings page's test button).
pub fn say_test(text: &str) {
    if let Ok(tx) = narrator().lock() {
        let _ = tx.send(text.to_string());
    }
}

fn speak(text: &str, female: bool) {
    use std::process::{Command, Stdio};
    #[cfg(not(target_os = "macos"))]
    let mut c;
    #[cfg(target_os = "linux")]
    {
        // ("--": text from the air may start with '-')
        c = Command::new("spd-say");
        c.args(["--wait", "-t", if female { "female1" } else { "male1" }, "--", text]);
    }
    #[cfg(target_os = "macos")]
    {
        // say reads the text from stdin when none is given on the command line
        use std::io::Write;
        let mut c = Command::new("say");
        c.args(["-v", if female { "Samantha" } else { "Alex" }]).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
        if let Ok(mut child) = c.spawn() {
            if let Some(mut i) = child.stdin.take() {
                let _ = i.write_all(text.as_bytes());
            }
            let _ = child.wait();
        }
        return;
    }
    #[cfg(target_os = "windows")]
    {
        // the text goes in an environment variable, never into the script
        let g = if female { "Female" } else { "Male" };
        c = Command::new("powershell");
        c.env("OPENSLOT_TTS", text);
        no_window(&mut c);
        c.args(["-NoProfile", "-Command", &format!(
            "Add-Type -AssemblyName System.Speech; $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; $s.SelectVoiceByHints('{g}'); $s.Speak($env:OPENSLOT_TTS)"
        )]);
    }
    #[cfg(not(target_os = "macos"))]
    {
        c.stdout(Stdio::null()).stderr(Stdio::null());
        let _ = c.status();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tones_and_phonetics() {
        let w = wav(&[(1000.0, 100)]);
        assert_eq!(&w[..4], b"RIFF");
        assert_eq!(w.len(), 44 + 2205 * 2);
        assert_eq!(phonetic("w5tst"), "Whiskey Five Tango Sierra Tango");
        assert_eq!(phonetic("NR5P-9"), "November Romeo Five Papa Dash Nine");
        let keys: std::collections::HashSet<_> = Sound::ALL.iter().map(|s| s.key()).collect();
        assert_eq!(keys.len(), Sound::ALL.len());
    }
}
