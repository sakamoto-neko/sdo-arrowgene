//! Pad and Keyboard judgment policies using recovered BPM-dependent native tap tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub enum Judgment {
    Perfect,
    Cool,
    Bad,
    Miss,
}
impl Judgment {
    pub const ALL: [Self; 4] = [Self::Perfect, Self::Cool, Self::Bad, Self::Miss];
    pub fn label(self) -> &'static str {
        match self {
            Self::Perfect => "PERFECT",
            Self::Cool => "COOL",
            Self::Bad => "BAD",
            Self::Miss => "MISS",
        }
    }
    pub fn index(self) -> usize {
        self as usize
    }
}
#[derive(Debug, Clone, Copy)]
pub struct Preset {
    pub id: &'static str,
    pub name: &'static str,
    /// [perfect, cool, bad, outer candidate/miss] at 48 ticks per beat.
    pub ticks: [u16; 4],
    /// Base tap thresholds and tick quantization are verified; score/holds are separate.
    pub verified_native_taps: bool,
    pub evidence: &'static str,
}
impl Preset {
    pub fn windows(self, bpm: f64) -> [f64; 3] {
        [self.ticks[0], self.ticks[1], self.ticks[2]].map(|x| x as f64 * 60.0 / (48.0 * bpm))
    }
    pub fn capture_window(self, bpm: f64) -> f64 {
        self.ticks[3] as f64 * 60.0 / (48.0 * bpm)
    }
    pub fn judge_at(self, input: f64, note: f64, bpm: f64) -> Option<Judgment> {
        if !input.is_finite() || !note.is_finite() || !bpm.is_finite() || bpm <= 0.0 {
            return None;
        }
        // Native f32 clock: BPM * 48.0f * (1.0f/60.0f). Input truncates to an
        // integer before comparing with the GN position (measure*192 + subdivision).
        let rate = bpm as f32 * 48.0 * (1.0f32 / 60.0);
        let input_tick = (input * rate as f64) as f32 as i64;
        let note_tick = (note * bpm * 48.0 / 60.0).round() as i64;
        let error = (input_tick - note_tick).unsigned_abs();
        self.ticks
            .iter()
            .position(|&bound| error <= bound as u64)
            .map(|i| Judgment::ALL[i])
    }
}
pub const PRESETS: [Preset; 2] = [
    Preset {
        id: "pad",
        name: "Pad",
        ticks: [6, 15, 20, 25],
        verified_native_taps: true,
        evidence: "USA 0x53b9a3; Taiwan 0x534974; India 0x50cac0; Thailand 0x76ef21. Shared pad tap table; also standalone 2006/v1/v5.",
    },
    Preset {
        id: "keyboard",
        name: "Keyboard",
        ticks: [5, 10, 16, 20],
        verified_native_taps: true,
        evidence: "USA 0x53b9c1 (edx=5); Taiwan 0x534992; India 0x50cade; Thailand 0x76efb5. Shared keyboard tap table; also standalone 2006/v1/v5.",
    },
];
pub fn find(id: &str) -> Option<Preset> {
    PRESETS
        .iter()
        .copied()
        .find(|p| p.id.eq_ignore_ascii_case(id))
}
pub fn next(id: &str) -> Preset {
    PRESETS[(PRESETS
        .iter()
        .position(|p| p.id.eq_ignore_ascii_case(id))
        .unwrap_or(0)
        + 1)
        % PRESETS.len()]
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_pad_and_keyboard_are_selectable_and_cycle_between_each_other() {
        assert_eq!(PRESETS.map(|p| p.name), ["Pad", "Keyboard"]);
        assert_eq!(find("Pad").unwrap().ticks, [6, 15, 20, 25]);
        assert_eq!(find("Keyboard").unwrap().ticks, [5, 10, 16, 20]);
        assert_eq!(next("pad").id, "keyboard");
        assert_eq!(next("keyboard").id, "pad");
        for id in [
            "practice",
            "practice-strict",
            "practice-relaxed",
            "thailand-mode4",
            "legacy-standard",
            "legacy-tight",
            "dance-mat",
            "sdo-x-2014",
        ] {
            assert!(find(id).is_none(), "{id}");
        }
    }
    #[test]
    fn native_thresholds_scale_with_bpm_and_retain_integer_quantization() {
        let p = find("pad").unwrap();
        assert!((p.windows(120.0)[0] - 0.0625).abs() < 1e-12);
        assert!((p.windows(240.0)[0] - 0.03125).abs() < 1e-12);
        assert_eq!(
            p.judge_at(2.0 + 6.5 / 96.0, 2.0, 120.0),
            Some(Judgment::Perfect)
        );
        assert_eq!(
            p.judge_at(2.0 - 6.5 / 96.0, 2.0, 120.0),
            Some(Judgment::Cool)
        );
        assert_eq!(
            p.judge_at(2.0 + 21.0 / 96.0, 2.0, 120.0),
            Some(Judgment::Miss)
        );
        assert_eq!(p.judge_at(2.0 + 26.0 / 96.0, 2.0, 120.0), None);
    }
    #[test]
    fn pad_and_keyboard_judge_the_same_tick_differently() {
        let time = 2.0 + 12.5 / 96.0;
        assert_eq!(
            find("pad").unwrap().judge_at(time, 2.0, 120.0),
            Some(Judgment::Cool)
        );
        assert_eq!(
            find("keyboard").unwrap().judge_at(time, 2.0, 120.0),
            Some(Judgment::Bad)
        );
        assert_eq!(
            find("keyboard").unwrap().judge_at(f64::NAN, 0.0, 120.0),
            None
        );
        assert_eq!(find("pad").unwrap().judge_at(1.0, 1.0, 0.0), None);
    }
}
