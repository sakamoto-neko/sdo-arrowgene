//! Only the affected player's deterministic judgment state rolls back. Audio never does.
use crate::{LATE_US, PlayerId, Standing};
use anyhow::{Result, ensure};
use sdo_gameplay::input::{SongTime, TimedInput, Transition};
use sdo_gameplay::{Lane, Practice};
use std::collections::{BTreeMap, VecDeque};
type Key = (SongTime, u64);
#[derive(Clone)]
struct Snapshot {
    at: SongTime,
    practice: Practice,
    held: [bool; 4],
    applied: Option<Key>,
}
pub struct Referee {
    state: Snapshot,
    snapshots: VecDeque<Snapshot>,
    events: BTreeMap<Key, (u8, bool)>,
    sequence: u64,
    pub forfeited: bool,
    rollback_us: u64,
    published: Vec<Option<u8>>,
    published_counts: [u32; 4],
    revised: bool,
    revision: u64,
}
impl Referee {
    pub fn forfeit(&mut self, at_us: u64) {
        if !self.forfeited {
            self.state.practice.stop_dancing(at_us as f64 / 1e6);
            self.forfeited = true;
        }
    }
    pub fn practice(&self) -> &Practice {
        &self.state.practice
    }
    pub fn sequence(&self) -> u64 {
        self.sequence
    }
    pub fn new(practice: Practice) -> Self {
        let state = Snapshot {
            at: SongTime::default(),
            practice,
            held: [false; 4],
            applied: None,
        };
        Self {
            snapshots: VecDeque::from([state.clone()]),
            state,
            events: BTreeMap::new(),
            sequence: 0,
            forfeited: false,
            rollback_us: LATE_US,
            published: Vec::new(),
            published_counts: [0; 4],
            revised: false,
            revision: 0,
        }
    }
    /// The server already checked these inputs. Retain extra presentation
    /// history for clock-estimate and event-loop scheduling differences.
    pub fn for_presentation(practice: Practice) -> Self {
        let mut replay = Self::new(practice);
        replay.rollback_us = LATE_US + 500_000;
        replay
    }
    pub fn input(&mut self, sequence: u64, at: SongTime, lane: u8, down: bool) -> Result<()> {
        ensure!(!self.forfeited && lane < 4, "Invalid input");
        ensure!(
            sequence == self.sequence + 1,
            "Input sequence is discontinuous"
        );
        ensure!(
            at.seconds() >= 0.
                && at.seconds() + self.rollback_us as f64 / 1e6 >= self.state.at.seconds(),
            "Input is outside rollback window"
        );
        ensure!(
            self.events.len() < 2048,
            "Too many inputs in rollback window"
        );
        let target = self.state.at;
        let restore = if at < target {
            Some(
                self.snapshots
                    .iter()
                    .rposition(|s| s.at < at || s.applied.is_none() && s.at.seconds() == 0.)
                    .ok_or_else(|| anyhow::anyhow!("Input predates rollback history"))?,
            )
        } else {
            None
        };
        self.sequence = sequence;
        self.events.insert((at, sequence), (lane, down));
        if let Some(index) = restore {
            self.revised = true;
            self.state = self.snapshots[index].clone();
            self.snapshots.truncate(index + 1);
            self.advance_to(target);
        } else if at == target {
            self.revised = true;
            self.advance_to(target);
        }
        Ok(())
    }
    pub fn advance(&mut self, target: u64) {
        self.advance_to(SongTime::new(target as f64 / 1e6).unwrap());
    }
    pub fn advance_to(&mut self, target: SongTime) {
        if self.forfeited || target < self.state.at {
            return;
        }
        for (&key, &(lane, down)) in &self.events {
            if self.state.applied.is_some_and(|applied| key <= applied) || key.0 > target {
                continue;
            }
            let index = lane as usize;
            if self.state.held[index] != down {
                let lane = Lane::ALL[index];
                TimedInput {
                    at: key.0,
                    transition: if down {
                        Transition::Press(lane)
                    } else {
                        Transition::Release(lane)
                    },
                }
                .apply(&mut self.state.practice, self.state.held);
                self.state.held[index] = down;
            }
            self.state.applied = Some(key);
        }
        self.state
            .practice
            .advance_to(target.seconds(), self.state.held, false);
        self.state.at = target;
        if target.seconds() - self.snapshots.back().unwrap().at.seconds() >= 0.25 {
            self.snapshots.push_back(self.state.clone());
        }
        while self.snapshots.len() > 1
            && self.snapshots[1].at.seconds() + self.rollback_us as f64 / 1e6 + 0.25
                < target.seconds()
        {
            self.snapshots.pop_front();
        }
        if let Some(key) = self.snapshots.front().and_then(|s| s.applied)
            && self
                .events
                .first_key_value()
                .is_some_and(|(first, _)| *first <= key)
        {
            self.events = self.events.split_off(&(key.0, key.1.saturating_add(1)));
        }
    }
    pub fn terminal(&self) -> bool {
        self.forfeited || self.state.practice.failed() || self.state.practice.completed()
    }
    pub fn judgment_changes(&mut self) -> Option<(u64, sdo_net::NoteGradeChanges)> {
        let practice = &self.state.practice;
        let counts = practice.score().counts;
        if counts == self.published_counts && !self.revised {
            return None;
        }
        self.published_counts = counts;
        self.revised = false;
        self.published.resize(practice.note_count(), None);
        let mut changes = Vec::new();
        for (id, previous) in self.published.iter_mut().enumerate() {
            let grade = practice.note_grade(id).map(|g| g.index() as u8);
            if *previous != grade {
                *previous = grade;
                changes.push((id as u32, grade));
            }
        }
        if changes.is_empty() {
            return None;
        }
        self.revision += 1;
        Some((self.revision, changes))
    }
    pub fn standing(&self, player: PlayerId) -> Standing {
        let score = self.state.practice.score();
        Standing {
            health: self.state.practice.health().value(),
            player,
            points: score.points,
            counts: score.counts,
            combo: score.combo,
            max_combo: score.max_combo,
            failed: self.state.practice.failed(),
            forfeited: self.forfeited,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use sdo_gameplay::Note;
    fn us(micros: u64) -> SongTime {
        SongTime::new(micros as f64 / 1e6).unwrap()
    }
    #[test]
    fn exact_time_replay_revises_a_published_miss() -> Result<()> {
        let preset = sdo_gameplay::judgment::find("keyboard").unwrap();
        for (at, expected) in [
            (1.0624998, sdo_gameplay::Judgment::Perfect),
            (1.0625, sdo_gameplay::Judgment::Cool),
        ] {
            let mut solo = Practice::from_notes(
                vec![Note {
                    lane: Lane::Left,
                    start: 1.,
                    end: 1.,
                }],
                3.,
                preset,
            );
            let mut referee = Referee::new(solo.clone());
            let time = SongTime::new(at).unwrap();
            TimedInput {
                at: time,
                transition: Transition::Press(Lane::Left),
            }
            .apply(&mut solo, [false; 4]);
            referee.advance(1_400_000);
            assert_eq!(referee.judgment_changes(), Some((1, vec![(0, Some(3))])));
            referee.input(1, time, 0, true)?;
            assert_eq!(
                referee.judgment_changes(),
                Some((2, vec![(0, Some(expected.index() as u8))]))
            );
            assert_eq!(solo.note_grade(0), Some(expected));
            assert_eq!(solo.score().counts, referee.practice().score().counts);
            assert_eq!(solo.health().value(), referee.practice().health().value());
            assert_eq!(referee.judgment_changes(), None);
        }
        Ok(())
    }
    #[test]
    fn both_presets_agree_on_chords_at_tick_edges_with_late_delivery() -> Result<()> {
        for preset in sdo_gameplay::judgment::PRESETS {
            for bpm in [97.5, 120., 183.] {
                let rate = bpm as f32 * 48. * (1.0f32 / 60.);
                let start = 60. / bpm * 4.;
                for tick in preset.ticks {
                    let threshold = (192. + f64::from(tick) + 1.) / f64::from(rate);
                    let f = threshold as f32;
                    for seconds in [
                        f64::from(f.next_down()),
                        threshold.next_down(),
                        threshold,
                        threshold.next_up(),
                        f64::from(f.next_up()),
                    ] {
                        let at = SongTime::new(seconds).unwrap().chart_time(0);
                        let mut solo = Practice::from_notes(
                            [Lane::Left, Lane::Down]
                                .map(|lane| Note {
                                    lane,
                                    start,
                                    end: start,
                                })
                                .to_vec(),
                            8.,
                            preset,
                        )
                        .with_bpm(bpm);
                        let mut server = Referee::new(solo.clone());
                        server.advance_to(SongTime::new(seconds + 0.6).unwrap());
                        let mut held = [false; 4];
                        for (i, lane) in [Lane::Left, Lane::Down].into_iter().enumerate() {
                            TimedInput {
                                at,
                                transition: Transition::Press(lane),
                            }
                            .apply(&mut solo, held);
                            held[i] = true;
                            server.input(i as u64 + 1, at, i as u8, true)?;
                        }
                        solo.advance_to(seconds + 0.6, held, false);
                        for i in 0..2 {
                            assert_eq!(solo.note_grade(i), server.practice().note_grade(i));
                        }
                        assert_eq!(solo.score().counts, server.practice().score().counts);
                        assert_eq!(solo.health().value(), server.practice().health().value());
                    }
                }
            }
        }
        Ok(())
    }
    fn referee() -> Referee {
        Referee::new(Practice::from_notes(
            vec![
                Note {
                    lane: Lane::Left,
                    start: 1.,
                    end: 1.5,
                },
                Note {
                    lane: Lane::Down,
                    start: 2.,
                    end: 2.,
                },
            ],
            4.,
            sdo_gameplay::judgment::find("keyboard").unwrap(),
        ))
    }
    #[test]
    fn late_hold_and_tap_repair_speculative_misses() -> Result<()> {
        let mut ordered = referee();
        let mut late = referee();
        for (seq, at, lane, down) in [
            (1, 1_000_000, 0, true),
            (2, 1_500_000, 0, false),
            (3, 2_000_000, 1, true),
            (4, 2_010_000, 1, false),
        ] {
            ordered.input(seq, us(at), lane, down)?;
            ordered.advance(at);
            late.advance(at + 600_000);
            late.input(seq, us(at), lane, down)?;
        }
        ordered.advance(3_000_000);
        late.advance(3_000_000);
        let a = ordered.standing(1);
        let b = late.standing(1);
        assert!(a.points > 0);
        assert_eq!(a.points, b.points);
        assert_eq!(a.counts, b.counts);
        assert_eq!(a.max_combo, b.max_combo);
        let phases = |replay: &Referee| {
            let mut output = vec![];
            let mut cue = replay.practice().dance_cue().cloned();
            while let Some(value) = cue {
                output.push((value.at, value.dancing));
                cue = value.previous.clone();
            }
            output
        };
        assert_eq!(
            phases(&ordered),
            phases(&late),
            "Rollback must restore the same presentation history"
        );
        assert!(late.input(5, us(0), 0, true).is_err());
        assert!(late.input(6, us(3_000_000), 4, true).is_err());
        Ok(())
    }
    #[test]
    fn rollback_can_recover_a_speculative_failure() -> Result<()> {
        let mut late = referee();
        late.advance(2_400_000);
        late.input(1, us(1_000_000), 0, true)?;
        late.input(2, us(1_500_000), 0, false)?;
        late.input(3, us(2_000_000), 1, true)?;
        assert!(late.standing(1).points > 0);
        Ok(())
    }
}
