//! Runtime state for one attempt: clock, input, scoring and character playback.
use anyhow::Result;
use sdo_audio::AudioPlayer;
use sdo_config::ScoreKey;
use sdo_dance::DancePlayer;
use sdo_gameplay::{Practice, input::InputState};
use sdo_ui::notes::NoteDisplay;
use std::time::Instant;

pub(crate) struct PlaySession {
    pub(crate) tail_clock: TailClock,
    pub(crate) score_key: ScoreKey,
    pub(crate) score_save: ScoreSaveState,
    pub(crate) from_start: bool,
    pub(crate) practice: Practice,
    pub(crate) input: InputState,
    pub(crate) display: NoteDisplay,
    pub(crate) audio: AudioPlayer,
    pub(crate) title: String,
    pub(crate) dance: DancePlayer,
    pub(crate) failed_at: Option<Instant>,
    pub(crate) dance_errors: Vec<String>,
}
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) enum ScoreSaveState {
    #[default]
    Pending,
    Saved,
    Ineligible,
    Failed(String),
}
/// The audio clock stops at EOF. Keep the final release/offset window running,
/// while preserving pause and restart behavior, until gameplay finishes.
#[derive(Default)]
pub(crate) struct TailClock {
    pub(crate) elapsed: f64,
    pub(crate) updated: Option<Instant>,
}
impl TailClock {
    pub(crate) fn position(
        &mut self,
        audio: f64,
        duration: f64,
        paused: bool,
        now: Instant,
    ) -> f64 {
        if audio < duration {
            *self = Self::default();
            return audio;
        }
        if let Some(previous) = self.updated.take() {
            self.elapsed += now.duration_since(previous).as_secs_f64();
        }
        if !paused {
            self.updated = Some(now);
        }
        audio + self.elapsed
    }
}
impl PlaySession {
    pub(crate) fn advance(&mut self, offset: f64, autoplay: bool) -> Result<()> {
        self.advance_at(offset, autoplay, Instant::now())
            .map(|_| ())
    }
    pub(crate) fn advance_at(
        &mut self,
        offset: f64,
        autoplay: bool,
        captured: Instant,
    ) -> Result<f64> {
        self.audio.check()?;
        let position = self.tail_clock.position(
            self.audio.position_at(captured),
            self.audio.duration(),
            self.audio.paused(),
            captured,
        );
        self.practice
            .advance_to((position - offset).max(0.0), self.input.held(), autoplay);
        if self.practice.failed() {
            self.failed_at.get_or_insert_with(Instant::now);
            if !self.practice.paused() {
                self.pause(true)?;
            }
        }
        Ok(position)
    }
    pub(crate) fn dance_time(&self) -> f64 {
        // A failed stage stops music, but the character can finish settling into idle.
        self.audio.position() + self.failed_at.map_or(0., |at| at.elapsed().as_secs_f64())
    }
    pub(crate) fn character_pose(
        &mut self,
        rig: &sdo_assets::CharacterSkeleton,
    ) -> Result<Option<sdo_dance::DancePose>> {
        self.dance
            .sample(self.dance_time(), self.practice.dancing(), rig)
    }
    pub(crate) fn dance_status(&self) -> String {
        if self.practice.failed() {
            "STAGE FAILED - R TO RETRY".into()
        } else if !self.practice.dancing() {
            "HIT A NOTE TO DANCE".into()
        } else if !self.dance_errors.is_empty() {
            format!("DANCE GAPS: {}", self.dance_errors.len())
        } else if self.dance.waiting_for_choreography(self.dance_time()) {
            "GET READY".into()
        } else if self.dance.has_choreography() {
            "DANCING".into()
        } else {
            "DANCE UNAVAILABLE".into()
        }
    }
    pub(crate) fn pause(&mut self, paused: bool) -> Result<()> {
        let paused = paused || self.practice.failed();
        self.tail_clock.position(
            self.audio.position(),
            self.audio.duration(),
            paused,
            Instant::now(),
        );
        self.audio.set_paused(paused)?;
        self.practice.set_paused(paused);
        Ok(())
    }
    pub(crate) fn restart(&mut self) -> Result<()> {
        self.audio.seek(0.0)?;
        self.input.clear();
        self.practice.restart();
        self.tail_clock = TailClock::default();
        self.score_key.judgment = self.practice.preset().id.into();
        self.score_save = ScoreSaveState::Pending;
        self.from_start = true;
        self.dance.reset();
        self.failed_at = None;
        self.pause(false)?;
        Ok(())
    }
}
