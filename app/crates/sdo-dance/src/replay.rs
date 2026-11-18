//! Render-cadence-independent phase transitions. Late judgments replace a cue
//! suffix and restore its cached boundary poses; authored frames never rewind audio.
use crate::{DancePlayer, DancePose};
use anyhow::Result;
use sdo_assets::CharacterSkeleton;
use sdo_gameplay::DanceCue;
use std::{collections::VecDeque, sync::Arc};

pub struct ReplayDance {
    base: DancePlayer,
    cues: Vec<(f64, bool)>,
    head: Option<Arc<DanceCue>>,
    boundaries: VecDeque<(usize, DancePlayer)>,
    starts_at: Option<f64>,
    offset: f64,
}
impl ReplayDance {
    pub fn new(player: DancePlayer, starts_at: Option<f64>) -> Self {
        Self {
            base: player,
            cues: vec![],
            head: None,
            boundaries: VecDeque::new(),
            starts_at,
            offset: 0.,
        }
    }
    pub fn sample(
        &mut self,
        seconds: f64,
        head: Option<&Arc<DanceCue>>,
        rig: &CharacterSkeleton,
    ) -> Result<Option<DancePose>> {
        self.sample_timed(seconds, head, 0., rig)
    }
    pub fn sample_timed(
        &mut self,
        seconds: f64,
        head: Option<&Arc<DanceCue>>,
        offset: f64,
        rig: &CharacterSkeleton,
    ) -> Result<Option<DancePose>> {
        if self.offset != offset {
            self.head = None;
            self.boundaries.clear();
            self.cues.clear();
            self.offset = offset;
        }
        let changed = match (&self.head, head) {
            (Some(a), Some(b)) => !Arc::ptr_eq(a, b),
            (None, None) => false,
            _ => true,
        };
        if changed || self.boundaries.is_empty() {
            let mut cues = vec![];
            let mut next = head.cloned();
            while let Some(cue) = next {
                cues.push(((cue.at + offset).max(0.), cue.dancing));
                next = cue.previous.clone();
            }
            cues.reverse();
            if let Some(start) = self.starts_at.filter(|s| *s > 0.) {
                let phase = cues
                    .iter()
                    .take_while(|(at, _)| *at <= start)
                    .last()
                    .is_none_or(|(_, v)| *v);
                if phase && !cues.iter().any(|(at, _)| *at == start) {
                    let index = cues.partition_point(|(at, _)| *at < start);
                    cues.insert(index, (start, true));
                }
            }
            let common = self
                .cues
                .iter()
                .zip(&cues)
                .take_while(|(a, b)| a == b)
                .count();
            self.boundaries.retain(|(index, _)| *index <= common);
            let (mut index, mut player) = self
                .boundaries
                .back()
                .cloned()
                .unwrap_or((0, self.base.clone()));
            let mut phase = self.cues.get(index.wrapping_sub(1)).is_none_or(|(_, v)| *v);
            if index == 0 {
                player.reset();
                let _ = player.sample(0., true, rig)?;
            }
            for &(at, dancing) in &cues[index..] {
                // Compute the source pose at the event boundary, never from the
                // preceding video frame. This also handles interrupted blends.
                let _ = player.sample((at - 1e-7).max(0.), phase, rig)?;
                let _ = player.sample(at, dancing, rig)?;
                phase = dancing;
                index += 1;
                self.boundaries.push_back((index, player.clone()));
            }
            if self.boundaries.is_empty() {
                self.boundaries.push_back((0, player));
            }
            self.cues = cues;
            self.head = head.cloned();
        }
        while self.boundaries.len() > 2
            && self
                .cues
                .get(self.boundaries[1].0.saturating_sub(1))
                .is_some_and(|(at, _)| *at < seconds - 3.)
        {
            self.boundaries.pop_front();
        }
        let (index, mut player) = self
            .boundaries
            .iter()
            .rev()
            .find(|(index, _)| *index == 0 || self.cues[index - 1].0 <= seconds)
            .cloned()
            .unwrap_or((0, self.base.clone()));
        let phase = self.cues.get(index.wrapping_sub(1)).is_none_or(|(_, v)| *v);
        player.sample(seconds.max(0.), phase, rig)
    }
}
