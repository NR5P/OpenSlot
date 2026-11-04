//! Rig controls beyond frequency and mode, for the Rig controls window and remote stations:
//! power and gains, noise blanker/reduction, preamp, attenuator, AGC and the meters.
//!
//! Every backend offers what its radio answers to: the worker asks once for each control
//! (see [`Item::all`]) and only the ones that answer are shown. Levels are 0…1; meters are
//! in their own units (see [`Meter`]).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    RfPower,
    AfGain,
    RfGain,
    Squelch,
    MicGain,
}

impl Level {
    pub const ALL: [Level; 5] = [Level::RfPower, Level::AfGain, Level::RfGain, Level::Squelch, Level::MicGain];

    pub fn label(self) -> &'static str {
        match self {
            Level::RfPower => "Power",
            Level::AfGain => "AF gain",
            Level::RfGain => "RF gain",
            Level::Squelch => "Squelch",
            Level::MicGain => "Mic gain",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Switch {
    NoiseBlanker,
    NoiseReduction,
    Preamp,
    Attenuator,
}

impl Switch {
    pub const ALL: [Switch; 4] = [Switch::NoiseBlanker, Switch::NoiseReduction, Switch::Preamp, Switch::Attenuator];

    pub fn label(self) -> &'static str {
        match self {
            Switch::NoiseBlanker => "NB",
            Switch::NoiseReduction => "NR",
            Switch::Preamp => "Preamp",
            Switch::Attenuator => "ATT",
        }
    }

    pub fn tip(self) -> &'static str {
        match self {
            Switch::NoiseBlanker => "Noise blanker",
            Switch::NoiseReduction => "Noise reduction",
            Switch::Preamp => "Receive preamplifier",
            Switch::Attenuator => "Receive attenuator",
        }
    }
}

/// Signal: dB over S9 (S9 = 0, S1 = -48). Power and ALC: 0…1 of full scale. SWR: the ratio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Meter {
    Signal,
    Power,
    Swr,
    Alc,
}

impl Meter {
    pub const ALL: [Meter; 4] = [Meter::Signal, Meter::Power, Meter::Swr, Meter::Alc];

    /// Only meaningful while transmitting.
    pub fn transmit(self) -> bool {
        !matches!(self, Meter::Signal)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Agc {
    Off,
    Fast,
    Medium,
    Slow,
    Auto,
}

impl Agc {
    pub const ALL: [Agc; 5] = [Agc::Off, Agc::Fast, Agc::Medium, Agc::Slow, Agc::Auto];

    pub fn label(self) -> &'static str {
        match self {
            Agc::Off => "Off",
            Agc::Fast => "Fast",
            Agc::Medium => "Mid",
            Agc::Slow => "Slow",
            Agc::Auto => "Auto",
        }
    }
}

/// What the radio has told us about its extra controls (absent = not offered).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RigExtras {
    #[serde(default)]
    pub levels: BTreeMap<Level, f32>,
    #[serde(default)]
    pub switches: BTreeMap<Switch, bool>,
    #[serde(default)]
    pub agc: Option<Agc>,
    #[serde(default)]
    pub meters: BTreeMap<Meter, f32>,
    /// Every control has been asked about once (until then the window says "Reading…").
    #[serde(default)]
    pub probed: bool,
    /// Counts ALC readings, so a steady meter still shows as freshly read (Tune → AUTO SET)
    #[serde(default, skip_serializing)]
    pub alc_reads: u32,
}

/// What the Rig controls window asks for (locally, or from a remote station).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum RigAction {
    Mode { mode: super::RadioMode },
    Level { level: Level, value: f32 },
    Switch { switch: Switch, on: bool },
    Agc { agc: Agc },
    /// Start the antenna tuner
    Tune,
}

/// One control the worker reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Item {
    Level(Level),
    Switch(Switch),
    Agc,
    Meter(Meter),
}

impl Item {
    pub fn all() -> Vec<Item> {
        let mut v: Vec<Item> = Level::ALL.iter().map(|&l| Item::Level(l)).collect();
        v.extend(Switch::ALL.iter().map(|&s| Item::Switch(s)));
        v.push(Item::Agc);
        v.extend(Meter::ALL.iter().map(|&m| Item::Meter(m)));
        v
    }
}

impl RigExtras {
    /// Drops what's known of a control (the radio stopped answering for it).
    pub fn forget(&mut self, item: Item) {
        match item {
            Item::Level(l) => {
                self.levels.remove(&l);
            }
            Item::Switch(s) => {
                self.switches.remove(&s);
            }
            Item::Agc => self.agc = None,
            Item::Meter(m) => {
                self.meters.remove(&m);
            }
        }
    }
}

/// Piecewise-linear lookup, for the radios' meter scales.
pub fn interpolate(table: &[(f32, f32)], x: f32) -> f32 {
    let Some(first) = table.first() else { return x };
    if x <= first.0 {
        return first.1;
    }
    for w in table.windows(2) {
        let ((x0, y0), (x1, y1)) = (w[0], w[1]);
        if x <= x1 {
            return y0 + (y1 - y0) * (x - x0) / (x1 - x0).max(f32::EPSILON);
        }
    }
    table.last().map_or(x, |l| l.1)
}

/// An S-meter reading as text: "S7", "S9+10".
pub fn s_units(db_over_s9: f32) -> String {
    if db_over_s9 >= 0.5 {
        format!("S9+{:.0}", db_over_s9)
    } else {
        format!("S{}", (9.0 + db_over_s9 / 6.0).round().clamp(0.0, 9.0) as i32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s_meter_text() {
        assert_eq!(s_units(0.0), "S9");
        assert_eq!(s_units(-12.0), "S7");
        assert_eq!(s_units(20.0), "S9+20");
        assert_eq!(s_units(-80.0), "S0");
    }

    #[test]
    fn table_lookup() {
        let t = [(0.0, 1.0), (100.0, 2.0), (200.0, 4.0)];
        assert_eq!(interpolate(&t, -5.0), 1.0);
        assert_eq!(interpolate(&t, 50.0), 1.5);
        assert_eq!(interpolate(&t, 150.0), 3.0);
        assert_eq!(interpolate(&t, 999.0), 4.0);
    }

    /// The remote protocol carries these as JSON objects keyed by name.
    #[test]
    fn json_round_trip() {
        let mut x = RigExtras::default();
        x.levels.insert(Level::RfPower, 0.5);
        x.switches.insert(Switch::Preamp, true);
        x.agc = Some(Agc::Fast);
        x.meters.insert(Meter::Signal, -6.0);
        x.probed = true;
        let j = serde_json::to_string(&x).unwrap();
        assert!(j.contains("\"rf_power\":0.5"), "{j}");
        assert_eq!(serde_json::from_str::<RigExtras>(&j).unwrap(), x);
    }
}
