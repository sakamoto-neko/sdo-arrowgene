//! Curated UI and character assets, decoded once and shared by render components.
mod character;
pub mod distribution;
mod wardrobe;
pub use wardrobe::*;
mod motion;
pub use motion::*;
mod dds;
mod hit_effects;
pub use character::*;
mod score;
mod ui;
pub use hit_effects::{HitAnimation, HitFrame};
use std::{path::PathBuf, sync::Arc};
pub use ui::{
    AssetIssue, HealthBar, NoteBoard, NoteEffect, NoteSkin, ScoreUi, UiAssets, UiChoice, UiChoices,
    UiReport, UiSelection,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TextureId(pub usize);
#[derive(Debug)]
pub struct Texture {
    pub source: PathBuf,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}
#[derive(Debug, Clone)]
pub struct GameplayUi {
    pub judgments: [Vec<TextureId>; 4],
    pub combo: Vec<TextureId>,
    pub combo_digits: Vec<TextureId>,
    pub score_digits: Vec<TextureId>,
    pub font: TextureId,
}
#[derive(Debug, Clone)]
pub struct LaneFrames {
    pub normal: Vec<TextureId>,
    pub eighth: Vec<TextureId>,
    pub sixteenth: Vec<TextureId>,
    pub hold_head: Vec<TextureId>,
    pub hold_body: Vec<TextureId>,
    pub hold_tail: Vec<TextureId>,
    pub receptor: Vec<TextureId>,
    pub pressed: Vec<TextureId>,
}
impl LaneFrames {
    pub fn receptor(&self, frame: usize) -> TextureId {
        self.receptor[frame % self.receptor.len()]
    }
    pub fn pressed_receptor(&self, frame: usize) -> TextureId {
        self.pressed[frame.min(self.pressed.len() - 1)]
    }
    pub fn note_variant(&self, frame: usize, beat_phase: usize) -> TextureId {
        let frames = match beat_phase % 4 {
            2 if !self.eighth.is_empty() => &self.eighth,
            1 | 3 if !self.sixteenth.is_empty() => &self.sixteenth,
            _ => &self.normal,
        };
        frames[frame % frames.len()]
    }
}
#[derive(Debug)]
pub struct FourKeySkin {
    pub definition: PathBuf,
    pub resources: Vec<PathBuf>,
    pub textures: Arc<Vec<Texture>>,
    pub lanes: [LaneFrames; 4],
    pub board: TextureId,
    pub ui: GameplayUi,
    pub hit_effect: Option<HitAnimation>,
    pub health: Option<HealthBar>,
}
impl FourKeySkin {
    pub const WHITE: TextureId = TextureId(0);
    pub fn note_frame(&self, lane: usize, frame: usize) -> TextureId {
        let f = &self.lanes[lane].normal;
        f[frame % f.len()]
    }
    pub fn hold_head(&self, lane: usize, frame: usize) -> TextureId {
        let f = &self.lanes[lane].hold_head;
        f[frame % f.len()]
    }
    pub fn hold_body(&self, lane: usize, frame: usize) -> TextureId {
        let f = &self.lanes[lane].hold_body;
        f[frame % f.len()]
    }
    pub fn hold_tail(&self, lane: usize, frame: usize) -> TextureId {
        let f = &self.lanes[lane].hold_tail;
        f[frame % f.len()]
    }
    pub fn receptor(&self, lane: usize, frame: usize) -> TextureId {
        self.lanes[lane].receptor(frame)
    }
    pub fn pressed_receptor(&self, lane: usize, frame: usize) -> TextureId {
        self.lanes[lane].pressed_receptor(frame)
    }
    pub fn note_variant(&self, lane: usize, frame: usize, beat_phase: usize) -> TextureId {
        self.lanes[lane].note_variant(frame, beat_phase)
    }
    pub fn texture(&self, id: TextureId) -> &Texture {
        &self.textures[id.0]
    }
}

pub mod curated;
use curated::{resolve_resource, textures::TextureStore};
