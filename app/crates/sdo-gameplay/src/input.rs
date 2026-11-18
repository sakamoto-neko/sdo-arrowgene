//! Logical input state independent of winit. Multiple bindings can hold one lane.
use crate::Lane;
use std::collections::HashSet;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Binding {
    D,
    F,
    J,
    K,
    Left,
    Down,
    Up,
    Right,
}
impl Binding {
    pub fn lane(self) -> Lane {
        match self {
            Self::D | Self::Left => Lane::Left,
            Self::F | Self::Down => Lane::Down,
            Self::J | Self::Up => Lane::Up,
            Self::K | Self::Right => Lane::Right,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    Press(Lane),
    Release(Lane),
}
impl Transition {
    pub fn apply(self, practice: &mut crate::Practice) {
        match self {
            Self::Press(lane) => practice.press(lane),
            Self::Release(lane) => practice.release(lane),
        }
    }
}
/// A finite point on the song timeline. Negative values represent the countdown.
#[derive(Clone, Copy, Debug, Default)]
pub struct SongTime(f64);
impl SongTime {
    pub fn new(seconds: f64) -> Option<Self> {
        seconds.is_finite().then_some(Self(seconds))
    }
    pub fn seconds(self) -> f64 {
        self.0
    }
    pub fn chart_time(self, offset_us: i64) -> Self {
        Self((self.0 - offset_us as f64 / 1e6).max(0.))
    }
}
impl PartialOrd for SongTime {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl PartialEq for SongTime {
    fn eq(&self, other: &Self) -> bool {
        self.0.to_bits() == other.0.to_bits()
    }
}
impl Eq for SongTime {}
impl Ord for SongTime {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.total_cmp(&other.0)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct TimedInput {
    pub at: SongTime,
    pub transition: Transition,
}
impl TimedInput {
    pub fn apply(self, practice: &mut crate::Practice, held_before: [bool; 4]) {
        practice.advance_to(self.at.seconds(), held_before, false);
        self.transition.apply(practice);
    }
}
#[derive(Default)]
pub struct InputState {
    bindings: HashSet<Binding>,
    pressed_at: [Option<f64>; 4],
}
impl InputState {
    pub fn held(&self) -> [bool; 4] {
        let mut lanes = [false; 4];
        for key in &self.bindings {
            lanes[key.lane().index()] = true;
        }
        lanes
    }
    pub fn held_seconds(&self, lane: Lane, now: f64) -> f64 {
        self.last_pressed_at(lane)
            .map(|at| (now - at).max(0.0))
            .unwrap_or(0.0)
    }
    pub fn last_pressed_at(&self, lane: Lane) -> Option<f64> {
        self.pressed_at[lane.index()]
    }
    pub fn update(&mut self, binding: Binding, down: bool, now: f64) -> Option<Transition> {
        let lane = binding.lane();
        let before = self.held()[lane.index()];
        if down {
            self.bindings.insert(binding);
        } else {
            self.bindings.remove(&binding);
        }
        let after = self.held()[lane.index()];
        match (before, after) {
            (false, true) => {
                self.pressed_at[lane.index()] = Some(now);
                Some(Transition::Press(lane))
            }
            (true, false) => Some(Transition::Release(lane)),
            _ => None,
        }
    }
    pub fn clear(&mut self) -> Vec<Transition> {
        let held = self.held();
        self.bindings.clear();
        self.pressed_at = [None; 4];
        Lane::ALL
            .into_iter()
            .filter(|lane| held[lane.index()])
            .map(Transition::Release)
            .collect()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repeat_and_alternate_binding_do_not_double_press_or_release() {
        let mut input = InputState::default();
        assert_eq!(
            input.update(Binding::D, true, 0.0),
            Some(Transition::Press(Lane::Left))
        );
        assert_eq!(input.update(Binding::D, true, 0.1), None);
        assert_eq!(input.update(Binding::Left, true, 0.2), None);
        assert_eq!(input.update(Binding::D, false, 0.3), None);
        assert!(input.held()[0]);
        assert_eq!(
            input.update(Binding::Left, false, 0.4),
            Some(Transition::Release(Lane::Left))
        );
    }
    #[test]
    fn focus_loss_clears_every_lane() {
        let mut input = InputState::default();
        input.update(Binding::D, true, 0.0);
        input.update(Binding::K, true, 0.0);
        assert_eq!(input.clear().len(), 2);
        assert_eq!(input.held(), [false; 4]);
        assert_eq!(input.last_pressed_at(Lane::Left), None);
    }
    #[test]
    fn quick_taps_keep_animation_start_without_repeat_or_alternate_binding_retrigger() {
        let mut input = InputState::default();
        assert_eq!(input.last_pressed_at(Lane::Left), None);
        input.update(Binding::D, true, 1.0);
        input.update(Binding::D, true, 1.01);
        input.update(Binding::Left, true, 1.02);
        assert_eq!(input.last_pressed_at(Lane::Left), Some(1.0));
        input.update(Binding::D, false, 1.03);
        input.update(Binding::Left, false, 1.04);
        assert!(!input.held()[0]);
        assert_eq!(input.last_pressed_at(Lane::Left), Some(1.0));
        input.update(Binding::D, true, 1.05);
        assert_eq!(input.last_pressed_at(Lane::Left), Some(1.05));
    }
}
