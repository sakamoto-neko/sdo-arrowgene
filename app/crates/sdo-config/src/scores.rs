use crate::{load_preferences, save_preferences};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Results require the same song, exact pattern bytes, chart, difficulty and timing preset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScoreKey {
    pub song_id: String,
    pub pattern_sha256: String,
    pub chart: String,
    pub difficulty: String,
    pub judgment: String,
}
impl ScoreKey {
    pub fn new(
        song_id: &str,
        chart: &str,
        difficulty: usize,
        judgment: &str,
        pattern_sha256: &str,
    ) -> Self {
        Self {
            song_id: song_id.into(),
            pattern_sha256: pattern_sha256.to_ascii_lowercase(),
            chart: chart.into(),
            difficulty: ["easy", "normal", "hard"][difficulty].into(),
            judgment: judgment.into(),
        }
    }
    pub fn is_verified(&self) -> bool {
        self.pattern_sha256.len() == 64
            && self
                .pattern_sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScoreRecord {
    #[serde(flatten)]
    pub key: ScoreKey,
    pub points: u64,
    pub max_combo: u32,
    /// Perfect, Cool, Bad, Miss counts, in that order.
    pub judgments: [u32; 4],
    pub achieved_at_unix: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScoreBook {
    pub schema_version: u32,
    pub scores: Vec<ScoreRecord>,
}
impl Default for ScoreBook {
    fn default() -> Self {
        Self {
            schema_version: 2,
            scores: Vec::new(),
        }
    }
}
impl ScoreBook {
    pub fn load(path: &Path) -> Result<Self> {
        let book: Self = load_preferences(path)?;
        ensure!(
            book.schema_version == 2,
            "Unsupported score schema {} in {}",
            book.schema_version,
            path.display()
        );
        ensure!(
            book.scores.iter().all(|s| s.key.is_verified()),
            "Score history contains an invalid pattern SHA-256"
        );
        Ok(book)
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        ensure!(
            self.schema_version == 2 && self.scores.iter().all(|s| s.key.is_verified()),
            "Scores require schema 2 and a valid pattern SHA-256"
        );
        save_preferences(path, self)
    }
    pub fn best(&self, key: &ScoreKey) -> Option<&ScoreRecord> {
        if !key.is_verified() {
            return None;
        }
        self.scores
            .iter()
            .filter(|record| record.key == *key)
            .max_by_key(|record| (record.points, record.max_combo))
    }
    pub fn last(&self, key: &ScoreKey) -> Option<&ScoreRecord> {
        if !key.is_verified() {
            return None;
        }
        self.scores.iter().rev().find(|record| record.key == *key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scores_keep_history_and_compare_only_matching_charts() -> Result<()> {
        let path = std::env::temp_dir().join(format!("sdo-scores-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut book = ScoreBook::load(&path)?;
        let key = ScoreKey::new("00001", "k", 0, "keyboard", &"a".repeat(64));
        assert!(book.best(&key).is_none());
        for (key, points) in [
            (key.clone(), 1000),
            (key.clone(), 500),
            (
                ScoreKey::new("00001", "k", 0, "keyboard", &"b".repeat(64)),
                999999,
            ),
            (
                ScoreKey::new("00001", "k", 1, "keyboard", &"a".repeat(64)),
                2000,
            ),
            (
                ScoreKey::new("00001", "t", 0, "keyboard", &"a".repeat(64)),
                3000,
            ),
            (ScoreKey::new("00001", "k", 0, "pad", &"a".repeat(64)), 4000),
            (
                ScoreKey::new("00002", "k", 0, "keyboard", &"a".repeat(64)),
                5000,
            ),
        ] {
            book.scores.push(ScoreRecord {
                key,
                points,
                max_combo: 1,
                judgments: [1, 0, 0, 0],
                achieved_at_unix: 1,
            });
        }
        book.save(&path)?;
        let reloaded = ScoreBook::load(&path)?;
        assert_eq!(book, reloaded);
        assert_eq!(reloaded.best(&key).unwrap().points, 1000);
        assert_eq!(reloaded.last(&key).unwrap().points, 500);
        std::fs::remove_file(path)?;
        Ok(())
    }
    #[test]
    fn only_the_new_hash_required_score_format_is_accepted() -> Result<()> {
        let path =
            std::env::temp_dir().join(format!("sdo-score-schema-{}.json", std::process::id()));
        std::fs::write(&path, br#"{"schema_version":1,"scores":[]}"#)?;
        assert!(ScoreBook::load(&path).is_err());
        let key = ScoreKey::new("1", "k", 0, "keyboard", &"a".repeat(64));
        let record = ScoreRecord {
            key,
            points: 1,
            max_combo: 1,
            judgments: [1, 0, 0, 0],
            achieved_at_unix: 1,
        };
        let mut document = serde_json::to_value(ScoreBook {
            schema_version: 2,
            scores: vec![record],
        })?;
        document["scores"][0]
            .as_object_mut()
            .unwrap()
            .remove("pattern_sha256");
        std::fs::write(&path, serde_json::to_vec(&document)?)?;
        assert!(ScoreBook::load(&path).is_err());
        document["scores"][0]["pattern_sha256"] = serde_json::json!("not-a-digest");
        std::fs::write(&path, serde_json::to_vec(&document)?)?;
        assert!(ScoreBook::load(&path).is_err());
        let mut invalid = ScoreBook::default();
        invalid.scores.push(ScoreRecord {
            key: ScoreKey::new("1", "k", 0, "keyboard", ""),
            points: 0,
            max_combo: 0,
            judgments: [0; 4],
            achieved_at_unix: 1,
        });
        assert!(invalid.save(&path).is_err());
        std::fs::remove_file(path)?;
        Ok(())
    }
}
