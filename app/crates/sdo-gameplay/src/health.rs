//! Normal four-key HP recovered from the SDO-X 14.8.7.15 client.
use crate::Judgment;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum HealthDifficulty {
    Easy,
    #[default]
    Normal,
    Hard,
}
impl HealthDifficulty {
    pub fn from_index(index: usize) -> Self {
        match index {
            0 => Self::Easy,
            2 => Self::Hard,
            _ => Self::Normal,
        }
    }
    pub fn delta(self, judgment: Judgment) -> i32 {
        let table = match self {
            Self::Easy => [6, 4, -10, -50],
            Self::Normal => [4, 2, -7, -40],
            Self::Hard => [2, 1, -5, -30],
        };
        table[judgment.index()]
    }
}
#[derive(Debug, Clone, Copy)]
pub struct Health {
    value: i32,
    difficulty: HealthDifficulty,
}
impl Health {
    pub const MIN: i32 = -150;
    pub const MAX: i32 = 1000;
    pub fn new(difficulty: HealthDifficulty) -> Self {
        Self {
            value: Self::MAX,
            difficulty,
        }
    }
    pub fn with_value(value: i32) -> Self {
        Self {
            value: value.clamp(Self::MIN, Self::MAX),
            difficulty: HealthDifficulty::Normal,
        }
    }
    pub fn value(self) -> i32 {
        self.value
    }
    pub fn difficulty(self) -> HealthDifficulty {
        self.difficulty
    }
    pub fn fraction(self) -> f32 {
        (self.value - Self::MIN) as f32 / (Self::MAX - Self::MIN) as f32
    }
    pub fn failed(self) -> bool {
        self.value == Self::MIN
    }
    pub fn low(self) -> bool {
        self.value <= 100
    }
    pub(crate) fn apply(&mut self, judgment: Judgment) {
        // Depletion ends this solo attempt; revival items are not part of this app.
        if !self.failed() {
            self.value = (self.value + self.difficulty.delta(judgment)).clamp(Self::MIN, Self::MAX);
        }
    }
    pub(crate) fn reset(&mut self) {
        self.value = Self::MAX;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_difficulty_tables_and_signed_bar_range() {
        for (difficulty, expected) in [
            (HealthDifficulty::Easy, [6, 4, -10, -50]),
            (HealthDifficulty::Normal, [4, 2, -7, -40]),
            (HealthDifficulty::Hard, [2, 1, -5, -30]),
        ] {
            let mut health = Health::new(difficulty);
            assert_eq!(health.value(), 1000);
            assert_eq!(health.fraction(), 1.0);
            health.apply(Judgment::Perfect);
            assert_eq!(health.value(), 1000);
            for (grade, delta) in Judgment::ALL.into_iter().zip(expected) {
                health.value = 400;
                health.apply(grade);
                assert_eq!(health.value(), 400 + delta);
            }
            health.value = 0;
            assert!(!health.failed());
            assert!((health.fraction() - 150.0 / 1150.0).abs() < 1e-6);
            health.value = -149;
            health.apply(Judgment::Miss);
            assert!(health.failed());
            assert_eq!(health.fraction(), 0.0);
            health.apply(Judgment::Perfect);
            assert_eq!(health.value(), -150);
            health.reset();
            assert_eq!(health.value(), 1000);
            assert_eq!(health.difficulty(), difficulty);
        }
    }
}
