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
    remote: Option<Arc<crate::remote::RemoteCache>>,
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
        Self::with_cache(paths, None)
    }
    pub(crate) fn with_cache(
        paths: CollectionPaths,
        remote: Option<Arc<crate::remote::RemoteCache>>,
    ) -> Result<Self> {
        Ok(Self {
            dances: Mutex::new(DanceLibrary::new({
                let root = paths.assets().join("character/motion");
                let remote = remote.clone();
                move |name| {
                    let id = format!("character/motion/{name}");
                    let path = if let Some(remote) = &remote {
                        if remote
                            .manifest
                            .files
                            .contains_key(&sdo_assets::distribution::key(&id))
                        {
                            // Song preparation fetches its DPS dependencies first.
                            // Standalone character animations use verified local data.
                            remote.resolve(&id, false, &mut |_| true)?
                        } else {
                            resolve_resource(
                                remote
                                    .local
                                    .as_deref()
                                    .context("Motion unavailable locally")?,
                                &id,
                            )?
                        }
                    } else {
                        resolve_resource(&root, name)?
                    };
                    Ok(Arc::new(CharacterMotion::parse(&fs::read(path)?)?))
                }
            })),
            paths,
            remote,
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
        let character_paths = self
            .remote
            .as_ref()
            .and_then(|r| r.local.as_ref())
            .map(CollectionPaths::from_assets)
            .transpose()?;
        let assets = Arc::new(sdo_assets::curated::characters(
            character_paths.as_ref().unwrap_or(&self.paths),
        )?);
        *cached = Some(assets.clone());
        Ok(assets)
    }
}
impl AssetProvider for CuratedProvider {
    fn name(&self) -> &str {
        "Custom Assets"
    }
    fn catalog(&self) -> Result<Catalog> {
        if let Some(remote) = &self.remote {
            return remote.catalog(self.paths.assets());
        }
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
        if let Some(remote) = &self.remote {
            let path = format!("character/motion/{id}");
            if remote
                .manifest
                .files
                .contains_key(&sdo_assets::distribution::key(&path))
            {
                // A cosmetic lookup before Play must not cache a missing song motion.
                remote.resolve(&path, false, &mut |_| true)?;
            }
        }
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
        let path = if let Some(remote) = &self.remote {
            remote.resolve(id, true, &mut |p| progress(p * 0.5))?
        } else {
            resolve_resource(self.paths.assets(), id)?
        };
        DecodedSong::decode_with_progress(fs::File::open(&path)?, expected_seconds, &mut |p| {
            progress(if self.remote.is_some() {
                0.5 + p * 0.5
            } else {
                p
            })
        })
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
        if let Some(remote) = &self.remote
            && let Some(motions) = remote
                .manifest
                .dance_motions
                .get(&sdo_assets::distribution::key(id))
        {
            for motion in motions {
                remote.resolve(motion, true, &mut |_| true)?;
            }
        }
        self.dances
            .lock()
            .map_err(|_| anyhow::anyhow!("Motion cache unavailable"))?
            .load(&raw, &chart.raw, difficulty.index(), bones)
    }
    fn file(&self, id: &str) -> Result<Vec<u8>> {
        let path = if let Some(remote) = &self.remote {
            remote.resolve(id, true, &mut |_| true)?
        } else {
            resolve_resource(self.paths.assets(), id)?
        };
        fs::read(&path).with_context(|| format!("Cannot read asset {id}"))
    }
    fn file_size(&self, id: &str) -> Result<u64> {
        if let Some(remote) = &self.remote {
            return Ok(remote
                .manifest
                .files
                .get(&sdo_assets::distribution::key(id))
                .context("Asset is absent from manifest")?
                .bytes);
        }
        let path = resolve_resource(self.paths.assets(), id)?;
        Ok(fs::metadata(path)?.len())
    }
}
