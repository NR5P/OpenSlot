//! VarAC's slot system: CQ on a slot, wait there, answer on the slot.
//!
//! One calling frequency (CF) per band; QSOs move to "slots" around it, `SlotStepHz` apart
//! (750 Hz for VARA HF at every bandwidth). Slot IDs 1–5 are below the CF (1 = one step
//! down … 5 = five steps), 11–15 above (11 = one step up …); 0 is the CF itself.
//!
//! A CQ names the slot its caller will wait on as the CQFRAME SSID: `CQFRAME W5TST-12 500`
//! = "after this CQ I'm on CF + 2 steps". A CQ without a slot is the bare callsign. A
//! station answering retunes to that slot and connects there; the caller waits
//! `CqSlotWaitTimeSeconds` (300) on the slot, then returns to the CF.

/// VARA HF default slot size (VarAC `SlotStepHz`).
pub const DEFAULT_SLOT_STEP_HZ: i64 = 750;
/// How long the CQ caller stands by on its slot (VarAC `CqSlotWaitTimeSeconds`).
pub const DEFAULT_CQ_SLOT_WAIT_SECS: u64 = 300;
/// A CQ older than this probably isn't waiting on its slot any more (VarAC warns).
pub const CQ_SLOT_STALE_SECS: i64 = 300;

/// Slot IDs from the lowest frequency to the highest, CF (0) in the middle.
pub const SLOTS_BY_FREQUENCY: [u8; 11] = [5, 4, 3, 2, 1, 0, 11, 12, 13, 14, 15];

/// Offset of a slot from the calling frequency, or None for an unknown ID.
pub fn offset_hz(slot: u8, step_hz: i64) -> Option<i64> {
    match slot {
        0 => Some(0),
        1..=5 => Some(-(slot as i64) * step_hz),
        11..=15 => Some((slot as i64 - 10) * step_hz),
        _ => None,
    }
}

pub fn frequency(cf_hz: u64, slot: u8, step_hz: i64) -> Option<u64> {
    u64::try_from(cf_hz as i64 + offset_hz(slot, step_hz)?).ok()
}

/// The slot a dial frequency is on, if it sits exactly on one.
pub fn slot_at(cf_hz: u64, dial_hz: u64, step_hz: i64) -> Option<u8> {
    SLOTS_BY_FREQUENCY
        .into_iter()
        .find(|&s| frequency(cf_hz, s, step_hz) == Some(dial_hz))
}

/// The neighbouring slot up (+1) or down (-1) from `slot`, if any.
pub fn neighbour(slot: u8, dir: i32) -> Option<u8> {
    let i = SLOTS_BY_FREQUENCY.iter().position(|&s| s == slot)? as i32 + dir;
    SLOTS_BY_FREQUENCY.get(usize::try_from(i).ok()?).copied()
}

/// CQFRAME source for a CQ waiting on `slot` (0 = no slot: the bare callsign).
pub fn cq_source(callsign: &str, slot: u8) -> String {
    let base = callsign.split('-').next().unwrap_or(callsign).to_uppercase();
    if slot == 0 { base } else { format!("{base}-{slot}") }
}

/// The slot a heard CQFRAME names (0 = none). Beacons (-8, -9) aren't CQs.
pub fn slot_of_cq(source: &str) -> u8 {
    match source.rsplit_once('-').and_then(|(_, ssid)| ssid.parse::<u8>().ok()) {
        Some(s @ (1..=5 | 11..=15)) => s,
        _ => 0,
    }
}

/// "slot 12 (+1500 Hz)" style label.
pub fn label(slot: u8, step_hz: i64) -> String {
    match offset_hz(slot, step_hz) {
        Some(0) => "CF".into(),
        Some(o) => format!("slot {slot} ({o:+} Hz)"),
        None => format!("slot {slot}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_layout_matches_varac() {
        // Slot → offset: '1' → -step, '2' → -2·step, …; '11' → +step …
        assert_eq!(offset_hz(1, 750), Some(-750));
        assert_eq!(offset_hz(5, 750), Some(-3750));
        assert_eq!(offset_hz(11, 750), Some(750));
        assert_eq!(offset_hz(15, 750), Some(3750));
        assert_eq!(offset_hz(6, 750), None);
        assert_eq!(frequency(14_105_000, 12, 750), Some(14_106_500));
        assert_eq!(slot_at(14_105_000, 14_103_500, 750), Some(2));
        assert_eq!(slot_at(14_105_000, 14_105_100, 750), None);
        assert_eq!(neighbour(1, -1), Some(2), "down from slot 1 is slot 2 (lower)");
        assert_eq!(neighbour(1, 1), Some(0));
        assert_eq!(neighbour(0, 1), Some(11));
        assert_eq!(neighbour(15, 1), None);
    }

    #[test]
    fn cq_names_its_slot_in_the_ssid() {
        assert_eq!(cq_source("W5TST", 12), "W5TST-12");
        assert_eq!(cq_source("W5TST-3", 0), "W5TST");
        assert_eq!(slot_of_cq("W5TST-12"), 12);
        assert_eq!(slot_of_cq("W5TST-3"), 3);
        assert_eq!(slot_of_cq("W5TST"), 0);
        assert_eq!(slot_of_cq("W5TST-9"), 0, "beacon");
        assert_eq!(slot_of_cq("W5TST-8"), 0, "EmComm beacon");
    }
}
