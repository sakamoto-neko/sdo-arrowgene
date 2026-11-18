//! Requests have statically known result types; callers never downcast an asset.
use crate::AssetProvider;
use anyhow::Result;
use sdo_assets::{CharacterAnimation, ClothingPreviewKey, OutfitOverrides, UiSelection};
use sdo_song::Difficulty;
use std::sync::Arc;

pub trait Request {
    type Output;
    fn load(self, provider: &dyn AssetProvider) -> Result<Self::Output>;
}

pub struct Catalog;
impl Request for Catalog {
    type Output = sdo_song::Catalog;
    fn load(self, provider: &dyn AssetProvider) -> Result<Self::Output> {
        provider.catalog()
    }
}
pub struct Ui;
impl Request for Ui {
    type Output = Arc<sdo_assets::UiAssets>;
    fn load(self, provider: &dyn AssetProvider) -> Result<Self::Output> {
        provider.ui()
    }
}
pub struct Characters<'a>(pub &'a OutfitOverrides);
impl Request for Characters<'_> {
    type Output = Arc<sdo_assets::CharacterAssets>;
    fn load(self, provider: &dyn AssetProvider) -> Result<Self::Output> {
        provider.characters(self.0)
    }
}
pub struct ClothingPreview<'a>(pub &'a ClothingPreviewKey);
impl Request for ClothingPreview<'_> {
    type Output = Arc<sdo_assets::CharacterAssets>;
    fn load(self, provider: &dyn AssetProvider) -> Result<Self::Output> {
        provider.clothing_preview(self.0)
    }
}
pub struct Skin<'a>(pub &'a UiSelection);
impl Request for Skin<'_> {
    type Output = sdo_assets::FourKeySkin;
    fn load(self, provider: &dyn AssetProvider) -> Result<Self::Output> {
        provider.skin(self.0)
    }
}
pub struct Motion<'a>(pub &'a str);
impl Request for Motion<'_> {
    type Output = Arc<sdo_assets::CharacterMotion>;
    fn load(self, provider: &dyn AssetProvider) -> Result<Self::Output> {
        provider.motion(self.0)
    }
}
pub struct File<'a>(pub &'a str);
impl Request for File<'_> {
    type Output = Vec<u8>;
    fn load(self, provider: &dyn AssetProvider) -> Result<Self::Output> {
        provider.file(self.0)
    }
}
pub struct FileSize<'a>(pub &'a str);
impl Request for FileSize<'_> {
    type Output = u64;
    fn load(self, provider: &dyn AssetProvider) -> Result<Self::Output> {
        provider.file_size(self.0)
    }
}
pub struct Chart<'a> {
    pub id: &'a str,
    pub difficulty: Difficulty,
    pub bpm: f64,
}
impl Request for Chart<'_> {
    type Output = sdo_song::Chart;
    fn load(self, provider: &dyn AssetProvider) -> Result<Self::Output> {
        provider.chart(self.id, self.difficulty, self.bpm)
    }
}
pub struct Audio<'a> {
    pub id: &'a str,
    pub expected_seconds: f64,
    pub progress: &'a mut dyn FnMut(f32) -> bool,
}
impl Request for Audio<'_> {
    type Output = sdo_audio::DecodedSong;
    fn load(self, provider: &dyn AssetProvider) -> Result<Self::Output> {
        provider.audio(self.id, self.expected_seconds, self.progress)
    }
}
pub struct Dance<'a> {
    pub id: &'a str,
    pub chart: &'a sdo_song::Chart,
    pub difficulty: Difficulty,
    pub bones: usize,
}
impl Request for Dance<'_> {
    type Output = sdo_dance::SongDance;
    fn load(self, provider: &dyn AssetProvider) -> Result<Self::Output> {
        provider.dance(self.id, self.chart, self.difficulty, self.bones)
    }
}
pub struct Reaction<'a> {
    pub animation: &'a CharacterAnimation,
    pub bones: usize,
}
impl Request for Reaction<'_> {
    type Output = sdo_dance::ReactionMotion;
    fn load(self, provider: &dyn AssetProvider) -> Result<Self::Output> {
        provider.reaction(self.animation, self.bones)
    }
}
