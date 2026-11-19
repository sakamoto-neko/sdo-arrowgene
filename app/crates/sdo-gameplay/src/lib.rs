//! Deterministic four-key session driven by an external audio clock.
pub mod input;
pub mod judgment;
pub use crate::judgment::{Judgment, Preset};
mod health;
pub use health::{Health, HealthDifficulty};
use std::sync::Arc;

/// Persistent phase history: rollback snapshots share prior cues instead of
/// copying them. Cue times are judgment times, independent of render cadence.
#[derive(Debug)]
pub struct DanceCue {
    pub at: f64,
    pub dancing: bool,
    pub previous: Option<Arc<DanceCue>>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(usize)]
pub enum Lane {
    Left,
    Down,
    Up,
    Right,
}
impl Lane {
    pub const ALL: [Self; 4] = [Self::Left, Self::Down, Self::Up, Self::Right];
    pub const fn index(self) -> usize {
        self as usize
    }
}
#[derive(Debug, Clone, Copy)]
pub struct Note {
    pub lane: Lane,
    pub start: f64,
    pub end: f64,
}
impl Note {
    pub fn is_hold(&self) -> bool {
        self.end > self.start
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteState {
    Pending,
    Holding,
    Skipped,
    Hit,
    Miss,
}
#[derive(Debug, Default, Clone, Copy)]
pub struct Score {
    pub points: u64,
    pub counts: [u32; 4],
    pub hit: u32,
    pub missed: u32,
    pub combo: u32,
    pub max_combo: u32,
}
pub fn base_points(counts: [u32; 4], max_combo: u32) -> u64 {
    let [perfect, cool, bad, miss] = counts.map(u64::from);
    (50 * perfect + 40 * cool + 20 * bad + 18 * u64::from(max_combo)).saturating_sub(10 * miss)
}

#[derive(Debug, Clone, Copy)]
pub struct JudgmentEvent {
    pub note: Option<usize>,
    pub judgment: Judgment,
    pub at: f64,
    pub error: f64,
}
#[derive(Debug, Clone, Copy)]
pub struct TempoChange {
    pub beat: f64,
    pub at: f64,
    pub bpm: f64,
}
#[derive(Clone)]
pub struct Practice {
    notes: Arc<[Note]>,
    states: Vec<NoteState>,
    first_unresolved: usize,
    hold_limit: usize,
    skipped: usize,
    resolved: usize,
    lane_notes: Arc<[Vec<usize>; 4]>,
    latest_tail: Arc<[f64]>,
    min_bpm: f64,
    hold_grades: Vec<Judgment>,
    position: f64,
    paused: bool,
    duration: f64,
    score: Score,
    health: Health,
    dance_active: bool,
    dance_event: Option<(f64, Judgment)>,
    dance_cue: Option<Arc<DanceCue>>,
    preset: Preset,
    bpm: f64,
    tempo: Arc<[TempoChange]>,
    last_judgment: Option<JudgmentEvent>,
    last_hit_at: [Option<f64>; 4],
}
impl Default for Practice {
    fn default() -> Self {
        Self::new()
    }
}
impl Practice {
    pub fn new() -> Self {
        let beat = 60.0 / 126.0;
        let mut notes = Vec::new();
        for i in 0..64 {
            let start = 2.0 + i as f64 * beat;
            notes.push(Note {
                lane: Lane::ALL[i % 4],
                start,
                end: if i % 8 == 3 {
                    start + beat * 2.0
                } else {
                    start
                },
            });
            if i % 8 == 6 {
                notes.push(Note {
                    lane: Lane::ALL[(i + 2) % 4],
                    start,
                    end: start,
                });
            }
        }
        Self::from_notes(notes, 36.0, crate::judgment::find("keyboard").unwrap()).with_bpm(126.0)
    }
    pub fn from_notes(mut notes: Vec<Note>, duration: f64, preset: Preset) -> Self {
        notes.sort_by(|a, b| a.start.total_cmp(&b.start));
        let mut lanes: [Vec<usize>; 4] = Default::default();
        let mut max_end = f64::NEG_INFINITY;
        let latest_tail = notes
            .iter()
            .enumerate()
            .map(|(index, n)| {
                lanes[n.lane.index()].push(index);
                max_end = max_end.max(n.end.max(n.start));
                max_end
            })
            .collect::<Vec<_>>()
            .into();
        let states = vec![NoteState::Pending; notes.len()];
        let hold_grades = vec![Judgment::Perfect; notes.len()];
        Self {
            first_unresolved: 0,
            hold_limit: 0,
            skipped: 0,
            resolved: 0,
            min_bpm: 120.,
            lane_notes: Arc::new(lanes),
            latest_tail,
            notes: notes.into(),
            states,
            hold_grades,
            position: 0.0,
            paused: false,
            duration,
            score: Score::default(),
            health: Health::new(HealthDifficulty::Normal),
            dance_active: true,
            dance_event: None,
            dance_cue: None,
            preset,
            bpm: 120.0,
            tempo: Arc::default(),
            last_judgment: None,
            last_hit_at: [None; 4],
        }
    }
    pub fn with_bpm(mut self, bpm: f64) -> Self {
        self.bpm = bpm;
        self.min_bpm = self.tempo.iter().fold(bpm, |m, t| m.min(t.bpm));
        self
    }
    pub fn with_health_difficulty(mut self, difficulty: HealthDifficulty) -> Self {
        self.health = Health::new(difficulty);
        self
    }
    pub fn health(&self) -> Health {
        self.health
    }
    pub fn failed(&self) -> bool {
        self.health.failed()
    }
    pub fn dancing(&self) -> bool {
        self.dance_active && !self.failed()
    }
    pub fn dance_cue(&self) -> Option<&Arc<DanceCue>> {
        self.dance_cue.as_ref()
    }
    pub fn stop_dancing(&mut self, at: f64) {
        let before = self.dancing();
        self.dance_active = false;
        self.record_dance_cue(at, before);
    }
    fn record_dance_cue(&mut self, at: f64, before: bool) {
        let dancing = self.dancing();
        if before != dancing
            && !self
                .dance_cue
                .as_ref()
                .is_some_and(|cue| cue.at == at && cue.dancing == dancing)
        {
            self.dance_cue = Some(Arc::new(DanceCue {
                at,
                dancing,
                previous: self.dance_cue.clone(),
            }));
        }
    }
    fn dance_feedback(&mut self, judgment: Judgment, at: f64) {
        if self.dance_event.is_some_and(|(prior_at, prior)| {
            at < prior_at || (at == prior_at && prior == Judgment::Miss)
        }) {
            return;
        }
        let before = self.dancing();
        self.dance_event = Some((at, judgment));
        match judgment {
            Judgment::Miss => {
                self.dance_active = false;
            }
            Judgment::Perfect | Judgment::Cool if !self.failed() => self.dance_active = true,
            _ => {}
        }
        self.record_dance_cue(at, before);
    }
    pub fn with_tempo(mut self, tempo: Vec<TempoChange>) -> Self {
        self.min_bpm = tempo.iter().fold(self.bpm, |m, t| m.min(t.bpm));
        self.tempo = tempo.into();
        self
    }
    pub fn bpm_at(&self, time: f64) -> f64 {
        self.tempo
            .get(
                self.tempo
                    .partition_point(|t| t.at <= time)
                    .saturating_sub(1),
            )
            .map_or(self.bpm, |t| t.bpm)
    }
    pub fn beat_at(&self, time: f64) -> f64 {
        self.tempo
            .get(
                self.tempo
                    .partition_point(|t| t.at <= time)
                    .saturating_sub(1),
            )
            .map_or(time * self.bpm / 60., |t| {
                t.beat + (time - t.at) * t.bpm / 60.
            })
    }
    pub fn bpm(&self) -> f64 {
        self.bpm_at(self.position)
    }
    pub fn windows(&self) -> [f64; 3] {
        self.preset.windows(self.bpm())
    }
    pub fn position(&self) -> f64 {
        self.position
    }
    pub fn duration(&self) -> f64 {
        self.duration
    }
    pub fn completed(&self) -> bool {
        // The result is final after the last judgment, even if the audio has an outro.
        !self.failed() && !self.notes.is_empty() && self.resolved == self.notes.len()
    }
    pub fn finished(&self) -> bool {
        self.position >= self.duration
            && !self.failed()
            && self.resolved + self.skipped == self.notes.len()
    }
    pub fn paused(&self) -> bool {
        self.paused
    }
    pub fn score(&self) -> Score {
        self.score
    }
    pub fn preset(&self) -> Preset {
        self.preset
    }
    pub fn last_judgment(&self) -> Option<JudgmentEvent> {
        self.last_judgment
    }
    pub fn note_grade(&self, index: usize) -> Option<Judgment> {
        matches!(
            self.states.get(index),
            Some(NoteState::Hit | NoteState::Miss)
        )
        .then(|| self.hold_grades[index])
    }
    pub fn note_count(&self) -> usize {
        self.notes.len()
    }
    pub fn notes(&self) -> impl Iterator<Item = (&Note, NoteState)> {
        self.notes.iter().zip(self.states.iter().copied())
    }
    /// The prefix maximum tail includes long holds crossing the visible beat range.
    pub fn visible_notes(
        &self,
        first_beat: f64,
        last_beat: f64,
    ) -> impl Iterator<Item = (usize, (&Note, NoteState))> {
        let start = self
            .latest_tail
            .partition_point(|t| self.beat_at(*t) < first_beat)
            .min(self.first_unresolved);
        let end = self
            .notes
            .partition_point(|n| self.beat_at(n.start) <= last_beat)
            .max(start)
            .min(self.notes.len());
        (start..end).map(|i| (i, (&self.notes[i], self.states[i])))
    }
    pub fn last_hit_at(&self, lane: Lane) -> Option<f64> {
        self.last_hit_at[lane.index()]
    }
    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
    }
    pub fn toggle_pause(&mut self) {
        self.paused = !self.paused;
    }
    pub fn set_preset(&mut self, preset: Preset) {
        self.preset = preset;
        self.restart();
    }
    pub fn restart(&mut self) {
        self.position = 0.0;
        self.first_unresolved = 0;
        self.hold_limit = 0;
        self.resolved = 0;
        self.skipped = 0;
        self.states.fill(NoteState::Pending);
        self.score = Score::default();
        self.health.reset();
        self.dance_active = true;
        self.dance_event = None;
        self.dance_cue = None;
        self.hold_grades.fill(Judgment::Perfect);
        self.last_judgment = None;
        self.last_hit_at = [None; 4];
    }
    pub fn seek_preview(&mut self, seconds: f64) {
        self.restart();
        self.position = seconds.clamp(0.0, self.duration);
        // A practice seek starts a fresh attempt at this point, without grading past notes.
        self.skipped = self.notes.partition_point(|n| n.start < self.position);
        self.first_unresolved = self.skipped;
        self.states[..self.skipped].fill(NoteState::Skipped);
    }
    pub fn show_preview_judgment(&mut self, judgment: Judgment) {
        self.last_judgment = Some(JudgmentEvent {
            note: None,
            judgment,
            at: self.position,
            error: 0.0,
        });
    }
    fn resolve(&mut self, i: usize, judgment: Judgment, at: f64, error: f64) {
        if self.failed() {
            return;
        }
        let was_dancing = self.dancing();
        self.health.apply(judgment);
        self.dance_feedback(judgment, at);
        if self.failed() {
            self.dance_active = false;
        }
        self.record_dance_cue(at, was_dancing);
        self.states[i] = if judgment == Judgment::Miss {
            NoteState::Miss
        } else {
            NoteState::Hit
        };
        self.hold_grades[i] = judgment;
        self.resolved += 1;
        while self
            .states
            .get(self.first_unresolved)
            .is_some_and(|s| matches!(s, NoteState::Hit | NoteState::Miss | NoteState::Skipped))
        {
            self.first_unresolved += 1;
        }
        self.score.counts[judgment.index()] += 1;
        if matches!(judgment, Judgment::Perfect | Judgment::Cool) {
            self.score.hit += 1;
            self.score.combo += 1;
            self.score.max_combo = self.score.max_combo.max(self.score.combo);
        } else {
            self.score.combo = 0;
            if judgment == Judgment::Miss {
                self.score.missed += 1;
            }
        }
        self.score.points = base_points(self.score.counts, self.score.max_combo);
        self.last_judgment = Some(JudgmentEvent {
            note: Some(i),
            judgment,
            at,
            error,
        });
    }
    pub fn advance_to(&mut self, seconds: f64, held: [bool; 4], autoplay: bool) {
        if self.paused
            || self.failed()
            || self.finished()
            || !seconds.is_finite()
            || seconds < self.position
        {
            return;
        }
        // Releases still need a live timing window when a hold ends with the
        // audio. The caller may advance through that final window after music ends.
        self.position = seconds;
        let mut resolved = Vec::new();
        let due = self.notes.partition_point(|n| n.start <= self.position);
        for i in self.first_unresolved..due.max(self.hold_limit) {
            let note = self.notes[i];
            if autoplay && self.states[i] == NoteState::Pending && self.position >= note.start {
                self.last_hit_at[note.lane.index()] = Some(note.start);
                if note.is_hold() {
                    self.states[i] = NoteState::Holding;
                    self.hold_limit = self.hold_limit.max(i + 1);
                    self.hold_grades[i] = Judgment::Perfect;
                    self.dance_feedback(Judgment::Perfect, note.start);
                    self.last_judgment = Some(JudgmentEvent {
                        note: Some(i),
                        judgment: Judgment::Perfect,
                        at: note.start,
                        error: 0.0,
                    });
                } else {
                    resolved.push((note.start, i, Judgment::Perfect, 0.0));
                }
            }
            if !autoplay
                && self.states[i] == NoteState::Pending
                && self.position > note.start + self.preset.capture_window(self.bpm_at(note.start))
            {
                resolved.push((
                    note.start + self.preset.capture_window(self.bpm_at(note.start)),
                    i,
                    Judgment::Miss,
                    self.position - note.start,
                ));
            } else if self.states[i] == NoteState::Holding {
                if autoplay && self.position >= note.end {
                    resolved.push((note.end, i, self.hold_grades[i], 0.0));
                } else if !autoplay && !held[note.lane.index()] {
                    resolved.push((
                        self.position,
                        i,
                        self.hold_release_grade(i, self.position),
                        self.position - note.end,
                    ));
                } else if !autoplay
                    && self.position > note.end + self.preset.capture_window(self.bpm_at(note.end))
                {
                    resolved.push((
                        note.end + self.preset.capture_window(self.bpm_at(note.end)),
                        i,
                        Judgment::Miss,
                        self.position - note.end,
                    ));
                }
            }
        }
        // Long frames and hold completions must apply HP in judgment-time order.
        resolved.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        for (at, i, grade, error) in resolved {
            self.resolve(i, grade, at, error);
            if self.failed() {
                break;
            }
        }
    }
    pub fn advance(&mut self, seconds: f64, held: [bool; 4]) {
        if seconds.is_finite() && seconds > 0.0 {
            self.advance_to(self.position + seconds, held, false);
        }
    }
    pub fn press(&mut self, lane: Lane) {
        if self.paused || self.failed() {
            return;
        }
        let indices = &self.lane_notes[lane.index()];
        // Keep an extra two native ticks around both sides of the truncated f32 clock.
        let radius = (self.preset.ticks[3] as f64 + 2.) * 60. / (48. * self.min_bpm)
            + self.position.abs() * f32::EPSILON as f64 * 4.;
        let begin = indices.partition_point(|i| self.notes[*i].start < self.position - radius);
        let end = indices.partition_point(|i| self.notes[*i].start <= self.position + radius);
        let nearest = indices[begin..end]
            .iter()
            .map(|&i| (i, &self.notes[i]))
            .filter(|(i, n)| {
                self.states[*i] == NoteState::Pending
                    && self
                        .preset
                        .judge_at(self.position, n.start, self.bpm_at(n.start))
                        .is_some()
            })
            .min_by(|(_, a), (_, b)| {
                (a.start - self.position)
                    .abs()
                    .total_cmp(&(b.start - self.position).abs())
            })
            .map(|(i, _)| i);
        if let Some(i) = nearest {
            let error = self.position - self.notes[i].start;
            let judgment = self
                .preset
                .judge_at(
                    self.position,
                    self.notes[i].start,
                    self.bpm_at(self.notes[i].start),
                )
                .unwrap();
            if judgment != Judgment::Miss {
                self.last_hit_at[lane.index()] = Some(self.position);
            }
            if self.notes[i].is_hold() && judgment != Judgment::Miss {
                self.states[i] = NoteState::Holding;
                self.hold_limit = self.hold_limit.max(i + 1);
                self.hold_grades[i] = judgment;
                self.dance_feedback(judgment, self.position);
                self.last_judgment = Some(JudgmentEvent {
                    note: Some(i),
                    judgment,
                    at: self.position,
                    error,
                });
            } else {
                self.resolve(i, judgment, self.position, error);
            }
        }
    }
    pub fn release(&mut self, lane: Lane) {
        if self.paused || self.failed() {
            return;
        }
        for i in self.first_unresolved..self.hold_limit {
            if self.notes[i].lane == lane && self.states[i] == NoteState::Holding {
                let judgment = self.hold_release_grade(i, self.position);
                self.resolve(
                    i,
                    judgment,
                    self.position,
                    self.position - self.notes[i].end,
                );
            }
        }
    }
    fn hold_release_grade(&self, i: usize, at: f64) -> Judgment {
        let end = self.notes[i].end;
        let release = self
            .preset
            .judge_at(at, end, self.bpm_at(end))
            .unwrap_or(Judgment::Miss);
        // One judgment per hold, finalized on release. Neither end can improve
        // the other end's grade. Tail timing uses the tempo at the tail.
        Judgment::ALL[self.hold_grades[i].index().max(release.index())]
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn one(hold: bool) -> Practice {
        Practice::from_notes(
            vec![Note {
                lane: Lane::Left,
                start: 1.0,
                end: if hold { 2.0 } else { 1.0 },
            }],
            5.0,
            crate::judgment::find("keyboard").unwrap(),
        )
    }
    #[test]
    fn the_final_judgment_completes_the_result_before_the_audio_outro() {
        let mut p = Practice::from_notes(
            vec![
                Note {
                    lane: Lane::Left,
                    start: 1.,
                    end: 1.,
                },
                Note {
                    lane: Lane::Up,
                    start: 2.,
                    end: 3.,
                },
            ],
            10.,
            crate::judgment::find("keyboard").unwrap(),
        );
        p.advance_to(1., [false; 4], false);
        p.press(Lane::Left);
        assert!(!p.completed(), "There is still an upcoming hold");
        p.advance_to(2., [false; 4], false);
        p.press(Lane::Up);
        p.advance_to(3., [false, false, true, false], false);
        assert!(!p.completed(), "The last hold must be judged at release");
        p.release(Lane::Up);
        assert!(p.completed());
        assert!(!p.finished());
        assert_eq!(p.score().counts, [2, 0, 0, 0]);
        p.advance_to(4., [false; 4], false);
        assert_eq!(p.position(), 4., "Playback continues through the outro");
        assert!(p.completed());
        p.advance_to(10., [false; 4], false);
        assert!(p.finished());
        p.restart();
        assert!(!p.completed());
        p.seek_preview(10.);
        assert!(
            !p.completed(),
            "Skipped notes do not count as a completed run"
        );

        let mut missed = one(false);
        missed.advance_to(1.4, [false; 4], false);
        assert!(missed.completed(), "A final miss also settles the result");
        assert!(!missed.finished());
        let empty = Practice::from_notes(Vec::new(), 0., p.preset());
        assert!(!empty.completed());
    }
    #[test]
    fn points_follow_judgments_and_only_reward_increases_in_max_combo() {
        let notes = (1..=7)
            .map(|i| Note {
                lane: Lane::Left,
                start: f64::from(i),
                end: f64::from(i),
            })
            .collect();
        let mut p = Practice::from_notes(notes, 8., judgment::find("keyboard").unwrap());
        // Miss, Perfect, Cool, Bad, Cool, Perfect, Perfect.
        for (i, (delay, points)) in [
            (0.3, 0),
            (0., 58),
            (0.08, 116),
            (0.14, 136),
            (0.08, 176),
            (0., 226),
            (0., 294),
        ]
        .into_iter()
        .enumerate()
        {
            p.advance_to(i as f64 + 1. + delay, [false; 4], false);
            p.press(Lane::Left);
            assert_eq!(p.score().points, points);
        }
        assert_eq!(p.score().counts, [3, 2, 1, 1]);
        assert_eq!(p.score().max_combo, 3);
    }

    #[test]
    fn pause_preserves_audio_timing() {
        let mut p = one(false);
        p.advance_to(0.5, [false; 4], false);
        p.set_paused(true);
        p.advance_to(3.0, [false; 4], false);
        assert_eq!(p.position(), 0.5);
    }
    #[test]
    fn perfect_cool_bad_and_miss_are_distinct() {
        for (error, grade) in [
            (0.0, Judgment::Perfect),
            (0.08, Judgment::Cool),
            (0.14, Judgment::Bad),
        ] {
            let mut p = one(false);
            p.advance_to(1.0 + error, [false; 4], false);
            p.press(Lane::Left);
            p.press(Lane::Left);
            assert_eq!(p.score().counts[grade.index()], 1);
            assert_eq!(p.score().counts.iter().sum::<u32>(), 1);
        }
        let mut p = one(false);
        p.advance_to(1.24, [false; 4], false);
        p.press(Lane::Left);
        assert_eq!(p.score().missed, 1);
    }
    #[test]
    fn hold_counts_once_and_early_release_misses() {
        let mut p = one(true);
        p.advance_to(1.0, [false; 4], false);
        p.press(Lane::Left);
        p.release(Lane::Left);
        assert_eq!(p.score().missed, 1);
        assert_eq!(p.score().counts.iter().sum::<u32>(), 1);
        let mut p = one(true);
        p.advance_to(2.1, [false; 4], true);
        assert_eq!(p.score().counts, [1, 0, 0, 0]);
    }
    #[test]
    fn holds_wait_for_release_and_grade_both_ends_once() {
        for (head, release, expected) in [
            (1., 2., Judgment::Perfect),
            (1., 1.92, Judgment::Cool),
            (1., 2.08, Judgment::Cool),
            (1.08, 2., Judgment::Cool),
            (1., 2.14, Judgment::Bad),
            (1., 1.7, Judgment::Miss),
            (1., 2.3, Judgment::Miss),
        ] {
            let mut p = one(true);
            p.advance_to(head, [false; 4], false);
            p.press(Lane::Left);
            assert_eq!(p.score().points, 0);
            assert_eq!(p.score().counts, [0; 4]);
            p.advance_to(release, [true, false, false, false], false);
            p.release(Lane::Left);
            p.release(Lane::Left);
            p.advance_to(3., [false; 4], false);
            assert_eq!(p.score().counts[expected.index()], 1, "{head} -> {release}");
            assert_eq!(p.score().counts.iter().sum::<u32>(), 1);
        }
        let mut p = one(true);
        p.advance_to(1., [false; 4], false);
        p.press(Lane::Left);
        p.advance_to(2., [true, false, false, false], false);
        assert_eq!(p.notes().next().unwrap().1, NoteState::Holding);
        assert_eq!(
            p.score().hit,
            0,
            "Passing the tail without release must not score"
        );
        p.advance_to(2.3, [true, false, false, false], false);
        assert_eq!(p.score().missed, 1);
        p.release(Lane::Left);
        assert_eq!(p.score().missed, 1);
    }
    #[test]
    fn release_timing_uses_tail_tempo_and_survives_pause() {
        let mut p = one(true).with_tempo(vec![
            TempoChange {
                beat: 0.,
                at: 0.,
                bpm: 120.,
            },
            TempoChange {
                beat: 3.,
                at: 1.5,
                bpm: 240.,
            },
        ]);
        p.advance_to(1., [false; 4], false);
        p.press(Lane::Left);
        p.set_paused(true);
        p.release(Lane::Left);
        p.advance_to(3., [false; 4], false);
        assert_eq!(p.notes().next().unwrap().1, NoteState::Holding);
        p.set_paused(false);
        p.advance_to(2.08, [true, false, false, false], false);
        p.release(Lane::Left);
        assert_eq!(p.score().counts[Judgment::Bad.index()], 1);
    }
    #[test]
    fn a_hold_at_audio_end_still_needs_release_and_can_expire() {
        for release in [true, false] {
            let mut p = Practice::from_notes(
                vec![Note {
                    lane: Lane::Left,
                    start: 1.,
                    end: 2.,
                }],
                2.,
                crate::judgment::find("keyboard").unwrap(),
            );
            p.advance_to(1., [false; 4], false);
            p.press(Lane::Left);
            p.advance_to(2., [true, false, false, false], false);
            assert!(!p.completed());
            p.advance_to(
                if release { 2.08 } else { 2.3 },
                [true, false, false, false],
                false,
            );
            if release {
                p.release(Lane::Left);
            }
            assert!(p.completed());
            assert_eq!(
                p.score().counts[if release {
                    Judgment::Cool
                } else {
                    Judgment::Miss
                }
                .index()],
                1
            );
            let finished_at = p.position();
            p.advance_to(10., [false; 4], false);
            assert_eq!(p.position(), finished_at);
        }
    }
    #[test]
    fn changing_policy_resets_score() {
        let mut p = one(false);
        p.advance_to(1.0, [false; 4], true);
        p.set_preset(crate::judgment::PRESETS[0]);
        assert_eq!(p.position(), 0.0);
        assert_eq!(p.score().points, 0);
    }
    #[test]
    fn manual_hit_feedback_requires_an_accepted_note_and_resets_with_the_session() {
        let mut p = one(false);
        p.advance_to(0.4, [false; 4], false);
        p.press(Lane::Left);
        assert_eq!(p.last_hit_at(Lane::Left), None);
        p.advance_to(1.0, [false; 4], false);
        p.press(Lane::Left);
        assert_eq!(p.last_hit_at(Lane::Left), Some(1.0));
        assert_eq!(p.last_hit_at(Lane::Right), None);
        p.restart();
        assert_eq!(p.last_hit_at(Lane::Left), None);
        p.advance_to(1.2, [false; 4], false);
        p.press(Lane::Left);
        assert_eq!(
            p.last_hit_at(Lane::Left),
            None,
            "Miss must not trigger a successful hit effect"
        );
    }
    #[test]
    fn autoplay_chords_and_holds_keep_independent_lane_animation_clocks() {
        let mut p = Practice::from_notes(
            vec![
                Note {
                    lane: Lane::Left,
                    start: 1.0,
                    end: 1.0,
                },
                Note {
                    lane: Lane::Up,
                    start: 1.0,
                    end: 2.0,
                },
                Note {
                    lane: Lane::Left,
                    start: 1.2,
                    end: 1.2,
                },
            ],
            3.0,
            crate::judgment::find("keyboard").unwrap(),
        );
        p.advance_to(1.1, [false; 4], true);
        assert_eq!(p.last_hit_at(Lane::Left), Some(1.0));
        assert_eq!(p.last_hit_at(Lane::Up), Some(1.0));
        p.advance_to(1.3, [false; 4], true);
        assert_eq!(p.last_hit_at(Lane::Left), Some(1.2));
        assert_eq!(p.last_hit_at(Lane::Up), Some(1.0));
        p.advance_to(2.1, [false; 4], true);
        assert_eq!(
            p.last_hit_at(Lane::Up),
            Some(1.0),
            "Hold completion must not invent another key press"
        );
    }
    fn sequence(count: usize, difficulty: HealthDifficulty) -> Practice {
        Practice::from_notes(
            (1..=count)
                .map(|i| Note {
                    lane: Lane::Left,
                    start: i as f64,
                    end: i as f64,
                })
                .collect(),
            count as f64 + 2.,
            crate::judgment::find("keyboard").unwrap(),
        )
        .with_health_difficulty(difficulty)
    }
    #[test]
    fn depletion_latches_at_the_native_boundary_and_restart_restores_the_attempt() {
        for (difficulty, misses) in [
            (HealthDifficulty::Easy, 23),
            (HealthDifficulty::Normal, 29),
            (HealthDifficulty::Hard, 39),
        ] {
            let mut p = sequence(50, difficulty);
            p.advance_to((misses - 1) as f64 + 0.4, [false; 4], false);
            assert!(!p.failed());
            p.advance_to(misses as f64 + 0.4, [false; 4], false);
            assert!(p.failed());
            assert!(!p.completed());
            assert_eq!(p.health().value(), -150);
            assert_eq!(p.score().missed, misses);
            assert!(!p.dancing());
            p.advance_to(50., [false; 4], true);
            p.press(Lane::Left);
            assert_eq!(p.score().missed, misses);
            assert_eq!(p.health().value(), -150);
            assert!(!p.dancing());
            p.restart();
            assert!(p.dancing());
            assert_eq!(p.health().value(), 1000);
            assert_eq!(p.health().difficulty(), difficulty);
            assert_eq!(p.score().missed, 0);
            p.advance_to(1., [false; 4], true);
            assert_eq!(p.score().hit, 1);
        }
    }
    #[test]
    fn long_frames_stop_scoring_at_depletion_and_empty_input_never_changes_health() {
        let mut p = sequence(50, HealthDifficulty::Easy);
        p.advance_to(0.5, [false; 4], false);
        p.press(Lane::Right);
        p.release(Lane::Right);
        assert_eq!(p.health().value(), 1000);
        p.advance_to(50.5, [false; 4], false);
        assert_eq!(p.score().missed, 23);
        assert_eq!(
            p.notes()
                .filter(|(_, state)| *state == NoteState::Miss)
                .count(),
            23
        );
        assert!(p.failed());
    }
    #[test]
    fn misses_select_idle_until_a_good_hit() {
        let mut p = sequence(6, HealthDifficulty::Normal);
        p.advance_to(1.4, [false; 4], false);
        assert!(!p.dancing());
        p.advance_to(2.4, [false; 4], false);
        assert!(!p.dancing());
        p.press(Lane::Right); // Empty press.
        assert!(!p.dancing());
        p.advance_to(3.0 + 0.14, [false; 4], false);
        p.press(Lane::Left); // Bad does not resume.
        assert_eq!(p.last_judgment().unwrap().judgment, Judgment::Bad);
        assert!(!p.dancing());
        p.advance_to(4.08, [false; 4], false);
        p.press(Lane::Left);
        assert_eq!(p.last_judgment().unwrap().judgment, Judgment::Cool);
        assert!(p.dancing());
        assert_eq!(p.health().value(), 915); // Two misses, one Bad, one Cool.
    }
    #[test]
    fn hold_head_resumes_dance_and_release_penalizes_health_exactly_once() {
        let mut p = Practice::from_notes(
            vec![
                Note {
                    lane: Lane::Left,
                    start: 1.,
                    end: 1.,
                },
                Note {
                    lane: Lane::Up,
                    start: 2.,
                    end: 4.,
                },
            ],
            6.,
            crate::judgment::find("keyboard").unwrap(),
        );
        p.advance_to(1.4, [false; 4], false);
        assert!(!p.dancing());
        p.advance_to(2., [false; 4], false);
        p.press(Lane::Up);
        assert!(p.dancing());
        assert_eq!(p.health().value(), 960);
        p.advance_to(2.2, [false, false, true, false], false);
        p.release(Lane::Up);
        p.release(Lane::Up);
        assert_eq!(p.health().value(), 920);
        assert!(!p.dancing());
        p.advance_to(5., [false; 4], false);
        assert_eq!(p.health().value(), 920);
        p.restart();
        p.advance_to(5., [false; 4], true);
        assert_eq!(p.score().hit, 2);
        assert_eq!(p.health().value(), 1000);
    }
    #[test]
    fn same_time_miss_wins_and_older_events_cannot_restart_dance() {
        let mut p = one(false);
        p.dance_feedback(Judgment::Miss, 1.);
        p.dance_feedback(Judgment::Perfect, 1.);
        p.dance_feedback(Judgment::Cool, 0.9);
        assert!(!p.dancing());
        p.dance_feedback(Judgment::Perfect, 1.1);
        p.dance_feedback(Judgment::Miss, 1.0);
        assert!(p.dancing());
    }
    #[test]
    fn seeking_into_a_song_skips_past_notes_without_draining_health() {
        let mut p = sequence(50, HealthDifficulty::Easy);
        p.seek_preview(30.5);
        p.advance_to(31., [false; 4], false);
        assert!(!p.failed());
        assert_eq!(p.health().value(), 1000);
        assert_eq!(p.score().missed, 0);
        assert_eq!(
            p.notes().filter(|(_, s)| *s == NoteState::Skipped).count(),
            30
        );
        p.press(Lane::Left);
        assert_eq!(p.score().hit, 1);
        p.restart();
        assert!(p.notes().all(|(_, s)| s == NoteState::Pending));
    }
}
