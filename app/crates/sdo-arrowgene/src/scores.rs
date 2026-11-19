use anyhow::{Context, Result};
use sdo_config::{ScoreBook, ScoreKey, ScoreRecord};
use sdo_gameplay::Practice;
use sdo_platform::{SystemTime, UNIX_EPOCH};
use std::{path::PathBuf, sync::Arc};

#[derive(Default)]
pub struct ScoreStore {
    pub book: Arc<ScoreBook>,
    pub error: Option<String>,
    path: Option<PathBuf>,
}
impl ScoreStore {
    pub fn load(path: PathBuf) -> Self {
        match ScoreBook::load(&path) {
            Ok(book) => Self {
                book: Arc::new(book),
                path: Some(path),
                error: None,
            },
            Err(error) => Self {
                // Preserve unreadable scores instead of overwriting them with an empty history.
                error: Some(format!("Cannot load score history: {error:#}")),
                ..Self::default()
            },
        }
    }
    pub fn record(
        &mut self,
        key: ScoreKey,
        practice: &Practice,
        from_start: bool,
        autoplay: bool,
    ) -> Result<bool> {
        if autoplay || !from_start || !practice.completed() {
            return Ok(false);
        }
        let score = practice.score();
        self.append(key, score.points, score.max_combo, score.counts)?;
        Ok(true)
    }

    pub fn record_multiplayer(
        &mut self,
        key: ScoreKey,
        standing: &sdo_net::Standing,
        note_count: usize,
    ) -> Result<bool> {
        let judged: u64 = standing.counts.iter().copied().map(u64::from).sum();
        if standing.failed || standing.forfeited || note_count == 0 || judged != note_count as u64 {
            return Ok(false);
        }
        self.append(key, standing.points, standing.max_combo, standing.counts)?;
        Ok(true)
    }

    fn append(
        &mut self,
        key: ScoreKey,
        points: u64,
        max_combo: u32,
        judgments: [u32; 4],
    ) -> Result<()> {
        anyhow::ensure!(
            key.is_verified(),
            "Cannot save a score without the played pattern's SHA-256"
        );
        let path = self.path.as_deref().context(
            "Score history is unavailable. Fix the loading error and restart; existing scores have been left unchanged.",
        )?;
        let mut book = self.book.as_ref().clone();
        book.scores.push(ScoreRecord {
            key,
            points,
            max_combo,
            judgments,
            achieved_at_unix: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        });
        // Publish the result only after it is on disk, so a failed write can be retried.
        book.save(path)?;
        self.book = Arc::new(book);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sdo_gameplay::{Lane, Note};
    fn attempt() -> Practice {
        Practice::from_notes(
            vec![Note {
                lane: Lane::Left,
                start: 1.,
                end: 1.,
            }],
            10.,
            sdo_gameplay::judgment::find("keyboard").unwrap(),
        )
    }
    #[test]
    fn only_completed_manual_full_attempts_are_recorded() -> Result<()> {
        let path =
            std::env::temp_dir().join(format!("sdo-score-attempts-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut store = ScoreStore::load(path.clone());
        let key = ScoreKey::new("00001", "k", 0, "keyboard", &"a".repeat(64));
        let mut practice = attempt();
        practice.advance_to(1., [false; 4], false);
        assert!(!store.record(key.clone(), &practice, true, false)?);
        practice.press(Lane::Left);
        assert!(!practice.finished(), "The audio still has an outro");
        assert!(!store.record(key.clone(), &practice, true, true)?);
        assert!(!store.record(key.clone(), &practice, false, false)?);
        assert!(!path.exists());
        assert!(store.record(key.clone(), &practice, true, false)?);
        assert_eq!(
            ScoreStore::load(path.clone())
                .book
                .best(&key)
                .unwrap()
                .points,
            68
        );
        practice.restart();
        assert!(!store.record(key, &practice, true, false)?);
        assert_eq!(store.book.scores.len(), 1);
        std::fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn multiplayer_history_keeps_settled_points_separate_from_solo_scores() -> Result<()> {
        let temp = std::env::temp_dir().join(format!("sdo-score-modes-{}", std::process::id()));
        let solo_path = temp.join("score.json");
        let online_path = temp.join("multiplayer-scores.json");
        let mut solo = ScoreStore::load(solo_path.clone());
        let mut online = ScoreStore::load(online_path.clone());
        let key = ScoreKey::new("1", "k", 0, "keyboard", &"a".repeat(64));
        let mut practice = attempt();
        practice.advance_to(1., [false; 4], false);
        practice.press(Lane::Left);
        assert!(solo.record(key.clone(), &practice, true, false)?);
        let result = sdo_net::Standing {
            points: 70, // 68 base points plus the rounded-down first-place bonus.
            counts: [1, 0, 0, 0],
            max_combo: 1,
            ..Default::default()
        };
        for incomplete in [
            sdo_net::Standing {
                failed: true,
                ..result.clone()
            },
            sdo_net::Standing {
                forfeited: true,
                ..result.clone()
            },
            sdo_net::Standing {
                counts: [0; 4],
                ..result.clone()
            },
        ] {
            assert!(!online.record_multiplayer(key.clone(), &incomplete, 1)?);
        }
        assert!(!online_path.exists());
        assert!(online.record_multiplayer(key.clone(), &result, 1)?);
        assert_eq!(ScoreBook::load(&solo_path)?.best(&key).unwrap().points, 68);
        let book = ScoreBook::load(&online_path)?;
        assert_eq!(book.scores.len(), 1);
        let saved = book.best(&key).unwrap();
        assert_eq!(
            (saved.points, saved.max_combo, saved.judgments),
            (70, 1, result.counts)
        );
        std::fs::remove_dir_all(temp)?;
        Ok(())
    }

    #[test]
    fn failed_writes_can_be_retried_without_publishing_or_duplicating_scores() -> Result<()> {
        let temp = std::env::temp_dir().join(format!("sdo-score-retry-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp)?;
        let path = temp.join("score.json");
        let mut store = ScoreStore::load(path.clone());
        let key = ScoreKey::new("00001", "k", 0, "keyboard", &"a".repeat(64));
        let mut practice = attempt();
        practice.advance_to(1., [false; 4], false);
        practice.press(Lane::Left);
        assert!(store.record(key.clone(), &practice, true, false)?);
        let saved = store.book.clone();
        // A directory at the destination forces the atomic replacement to fail.
        std::fs::remove_file(&path)?;
        std::fs::create_dir(&path)?;
        assert!(store.record(key.clone(), &practice, true, false).is_err());
        assert!(Arc::ptr_eq(&saved, &store.book));
        std::fs::remove_dir(&path)?;
        assert!(store.record(key, &practice, true, false)?);
        assert_eq!(store.book.scores.len(), 2);
        assert_eq!(ScoreBook::load(&path)?, *store.book);
        assert_eq!(saved.scores.len(), 1);
        std::fs::remove_dir_all(temp)?;
        Ok(())
    }

    #[test]
    fn unreadable_history_is_preserved_and_cannot_report_a_successful_save() -> Result<()> {
        let path =
            std::env::temp_dir().join(format!("sdo-score-invalid-{}.json", std::process::id()));
        let original = "{\"scores\":";
        std::fs::write(&path, original)?;
        let mut store = ScoreStore::load(path.clone());
        assert!(store.error.take().is_some()); // App consumes the initial popup.
        let mut practice = attempt();
        practice.advance_to(1., [false; 4], false);
        practice.press(Lane::Left);
        assert!(
            store
                .record(
                    ScoreKey::new("00001", "k", 0, "keyboard", &"a".repeat(64)),
                    &practice,
                    true,
                    false
                )
                .is_err()
        );
        assert!(store.book.scores.is_empty());
        assert_eq!(std::fs::read_to_string(&path)?, original);
        std::fs::remove_file(path)?;
        Ok(())
    }
    #[test]
    fn a_completed_score_without_a_pattern_hash_is_not_written() -> Result<()> {
        let path =
            std::env::temp_dir().join(format!("sdo-score-no-hash-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut store = ScoreStore::load(path.clone());
        let mut practice = attempt();
        practice.advance_to(1., [false; 4], false);
        practice.press(Lane::Left);
        assert!(practice.completed());
        assert!(
            store
                .record(
                    ScoreKey::new("1", "k", 0, "keyboard", ""),
                    &practice,
                    true,
                    false
                )
                .is_err()
        );
        assert!(!path.exists());
        Ok(())
    }
}
