//! The frontend's content boundary: typed requests, replaceable providers and
//! cancellable background preparation. Providers own storage and format translation.
#[cfg(target_arch = "wasm32")]
pub mod browser;
#[cfg(not(target_arch = "wasm32"))]
pub mod curated;
#[cfg(not(target_arch = "wasm32"))]
mod normalized;
mod pipeline;
mod preview;
#[cfg(not(target_arch = "wasm32"))]
mod remote;
pub mod request;
pub mod sources;
#[cfg(not(target_arch = "wasm32"))]
mod sources_native;
#[cfg(test)]
mod tests;

pub use pipeline::{
    LoadProgress, LoadRequest, LoadStage, PreparedSong, SongAssetLoader, load_dance,
};
pub use preview::PreviewPlayer;

use anyhow::Result;
use sdo_assets::{
    CharacterAnimation, CharacterAssets, CharacterMotion, ClothingPreviewKey, FourKeySkin,
    OutfitOverrides, UiAssets, UiSelection,
};
use sdo_audio::DecodedSong;
use sdo_dance::{ReactionMotion, SongDance};
use sdo_song::{Catalog, Chart, Difficulty};
use std::sync::Arc;

/// An identifier returned by a provider is opaque to its consumers. For example,
/// `Song::audio` may identify a curated file, an archive entry, or an in-memory blob.
///
/// Implementations translate their storage formats into these shared models.
/// No method requires a filesystem path, extraction directory, renderer or window.
/// Methods may run concurrently on song/preview workers. Returning `false` from
/// an audio progress callback cancels the request and must produce an error.
pub trait AssetProvider: Send + Sync + 'static {
    fn name(&self) -> &str;
    fn diagnostics(&self) -> Vec<String> {
        Vec::new()
    }
    fn catalog(&self) -> Result<Catalog>;
    fn ui(&self) -> Result<Arc<UiAssets>>;
    fn characters(&self, outfits: &OutfitOverrides) -> Result<Arc<CharacterAssets>>;
    fn clothing_preview(&self, key: &ClothingPreviewKey) -> Result<Arc<CharacterAssets>>;
    fn motion(&self, id: &str) -> Result<Arc<CharacterMotion>>;
    fn chart(&self, id: &str, difficulty: Difficulty, bpm: f64) -> Result<Chart>;
    fn audio(
        &self,
        id: &str,
        expected_seconds: f64,
        progress: &mut dyn FnMut(f32) -> bool,
    ) -> Result<DecodedSong>;
    fn dance(
        &self,
        id: &str,
        chart: &Chart,
        difficulty: Difficulty,
        bones: usize,
    ) -> Result<SongDance>;
    fn file(&self, id: &str) -> Result<Vec<u8>>;
    fn file_size(&self, id: &str) -> Result<u64>;

    fn skin(&self, selection: &UiSelection) -> Result<FourKeySkin> {
        self.ui()?.compose(selection)
    }
    fn reaction(&self, animation: &CharacterAnimation, bones: usize) -> Result<ReactionMotion> {
        let motion = self.motion(&animation.motion)?;
        ReactionMotion::new(
            animation.motion.clone(),
            animation.frames_per_second,
            motion,
            bones,
        )
    }
}

/// A cheap, clonable handle shared by the app and background workers. Selecting
/// a different provider requires changing only the application's composition root.
#[derive(Clone)]
pub struct AssetLoader {
    provider: Arc<dyn AssetProvider>,
}
impl AssetLoader {
    pub fn new(provider: impl AssetProvider) -> Self {
        Self::from_shared(Arc::new(provider))
    }
    pub fn from_shared(provider: Arc<dyn AssetProvider>) -> Self {
        Self { provider }
    }
    pub fn name(&self) -> &str {
        self.provider.name()
    }
    pub fn request<R: request::Request>(&self, request: R) -> Result<R::Output> {
        request.load(self.provider.as_ref())
    }
}

/// Verify references without decoding recordings or requiring an audio device.
pub fn verify_catalog(assets: &AssetLoader, catalog: &Catalog) -> Result<sdo_song::CatalogStats> {
    for song in &catalog.songs {
        assets.request(request::FileSize(&song.audio))?;
        if let Some(preview) = &song.preview {
            assets.request(request::FileSize(&preview.path))?;
        }
        for pattern in &song.patterns {
            assets.request(request::FileSize(&pattern.file))?;
        }
    }
    Ok(catalog.stats())
}

fn apply_manifest(catalog: &mut Catalog, manifest: &sdo_assets::distribution::Manifest) {
    use sdo_assets::distribution;
    let listed = |path: &str| manifest.files.contains_key(&distribution::key(path));
    catalog.songs.retain_mut(|song| {
        if !listed(&song.audio) {
            return false;
        }
        song.preview = song.preview.take().filter(|p| listed(&p.path));
        song.dance_moves_paths = song.dance_moves_paths.take().filter(|p| listed(p));
        song.patterns.retain_mut(|p| {
            let key = distribution::key(&p.file);
            let Some(levels) = manifest.charts.get(&key) else {
                return false;
            };
            let Some(file) = manifest.files.get(&key) else {
                return false;
            };
            p.levels = *levels;
            p.sha256 = file.sha256.clone();
            p.mode == 4
                && ["k", "t"].iter().any(|k| p.kind.eq_ignore_ascii_case(k))
                && levels.iter().any(Option::is_some)
        });
        !song.patterns.is_empty()
    });
}
