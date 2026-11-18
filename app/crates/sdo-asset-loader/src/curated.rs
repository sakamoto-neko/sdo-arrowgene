//! Adapter for the approved assets/ collection. This is the only provider that
//! knows its directory layout. It never falls back to the extraction library.
use crate::AssetProvider;
use anyhow::{Context, Result};
use sdo_assets::curated::CollectionPaths;
use sdo_assets::curated::resolve_resource;
use sdo_assets::{CharacterAssets, CharacterMotion, ClothingPreviewKey, OutfitOverrides, UiAssets};
use sdo_audio::DecodedSong;
use sdo_dance::{DanceLibrary, SongDance};
use sdo_song::{Catalog, Chart, Difficulty};
use std::{
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
};

pub struct CuratedProvider {
    paths: CollectionPaths,
    ui: Mutex<Option<Arc<UiAssets>>>,
    characters: Mutex<Option<Arc<CharacterAssets>>>,
    dances: Mutex<DanceLibrary>,
}
impl CuratedProvider {
    pub fn from_assets(path: impl Into<PathBuf>) -> Result<Self> {
        Self::from_paths(CollectionPaths::from_assets(path)?)
    }
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        Self::from_paths(CollectionPaths::new(root)?)
    }
    pub fn from_env() -> Result<Self> {
        Self::from_paths(CollectionPaths::from_env()?)
    }
    pub fn from_paths(paths: CollectionPaths) -> Result<Self> {
        Ok(Self {
            dances: Mutex::new(DanceLibrary::new({
                let root = paths.assets().join("character/motion");
                move |name| {
                    let path = resolve_resource(&root, name)?;
                    Ok(Arc::new(CharacterMotion::parse(&fs::read(path)?)?))
                }
            })),
            paths,
            ui: Mutex::new(None),
            characters: Mutex::new(None),
        })
    }
    fn defaults(&self) -> Result<Arc<CharacterAssets>> {
        let mut cached = self
            .characters
            .lock()
            .map_err(|_| anyhow::anyhow!("Character cache unavailable"))?;
        if let Some(assets) = cached.as_ref() {
            return Ok(assets.clone());
        }
        let assets = Arc::new(sdo_assets::curated::characters(&self.paths)?);
        *cached = Some(assets.clone());
        Ok(assets)
    }
}
impl AssetProvider for CuratedProvider {
    fn name(&self) -> &str {
        "Custom Assets"
    }
    fn catalog(&self) -> Result<Catalog> {
        Catalog::load(&self.paths)
    }
    fn ui(&self) -> Result<Arc<UiAssets>> {
        let mut cached = self
            .ui
            .lock()
            .map_err(|_| anyhow::anyhow!("UI asset cache unavailable"))?;
        if let Some(assets) = cached.as_ref() {
            return Ok(assets.clone());
        }
        let assets = Arc::new(sdo_assets::curated::ui(&self.paths)?);
        *cached = Some(assets.clone());
        Ok(assets)
    }
    fn characters(&self, outfits: &OutfitOverrides) -> Result<Arc<CharacterAssets>> {
        let defaults = self.defaults()?;
        if outfits.is_empty() {
            return Ok(defaults);
        }
        Ok(Arc::new(sdo_assets::curated::with_outfits(
            &defaults, outfits,
        )?))
    }
    fn clothing_preview(&self, key: &ClothingPreviewKey) -> Result<Arc<CharacterAssets>> {
        Ok(Arc::new(sdo_assets::curated::clothing_preview(
            self.defaults()?.as_ref(),
            key,
        )?))
    }
    fn motion(&self, id: &str) -> Result<Arc<CharacterMotion>> {
        self.dances
            .lock()
            .map_err(|_| anyhow::anyhow!("Motion cache unavailable"))?
            .motion(id)
    }
    fn chart(&self, id: &str, difficulty: Difficulty, bpm: f64) -> Result<Chart> {
        Chart::parse_with_bpm(self.file(id)?, difficulty, Some(bpm))
            .with_context(|| format!("Cannot decode chart {id}"))
    }
    fn audio(
        &self,
        id: &str,
        expected_seconds: f64,
        progress: &mut dyn FnMut(f32) -> bool,
    ) -> Result<DecodedSong> {
        let path = resolve_resource(self.paths.assets(), id)?;
        DecodedSong::decode_with_progress(fs::File::open(&path)?, expected_seconds, progress)
            .with_context(|| format!("Cannot decode audio {id}"))
    }
    fn dance(
        &self,
        id: &str,
        chart: &Chart,
        difficulty: Difficulty,
        bones: usize,
    ) -> Result<SongDance> {
        let raw = self.file(id)?;
        self.dances
            .lock()
            .map_err(|_| anyhow::anyhow!("Motion cache unavailable"))?
            .load(&raw, &chart.raw, difficulty.index(), bones)
    }
    fn file(&self, id: &str) -> Result<Vec<u8>> {
        let path = resolve_resource(self.paths.assets(), id)?;
        fs::read(&path).with_context(|| format!("Cannot read asset {id}"))
    }
    fn file_size(&self, id: &str) -> Result<u64> {
        let path = resolve_resource(self.paths.assets(), id)?;
        Ok(fs::metadata(path)?.len())
    }
}
