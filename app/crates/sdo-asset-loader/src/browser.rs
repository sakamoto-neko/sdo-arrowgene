//! The HTTP manifest supplies catalog metadata before songs are fetched. Both platforms
//! use the same curated parsers once the browser has mounted the verified bytes.
use super::{AssetLoader, AssetProvider};
use anyhow::{Context, Result};
use sdo_assets::{
    curated::resolve_resource,
    distribution::{Manifest, key},
    *,
};
use sdo_dance::{DanceLibrary, SongDance};
use sdo_song::{Catalog, Chart, Difficulty};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};
pub async fn fetch_dance(path: &str) -> Result<()> {
    fetch(path).await?;
    let raw = sdo_platform::assets::read(resolve_resource(Path::new("/assets"), path)?)?;
    for name in sdo_dance::DanceScript::parse(&raw)?.motion_names() {
        let id = format!("character/motion/{name}");
        // Some authored clips are absent from the collection. DanceLibrary reports those gaps.
        let _ = fetch(&id).await;
    }
    Ok(())
}
pub async fn audio(path: &str) -> Result<sdo_audio::DecodedSong> {
    let (sample_rate, channels, samples) = sdo_platform::audio::decode(path).await?;
    Ok(sdo_audio::DecodedSong {
        sample_rate,
        channels,
        samples,
    })
}
pub async fn fetch(name: &str) -> Result<()> {
    let root = Path::new("/assets");
    let path = resolve_resource(root, name)?;
    let logical = path
        .strip_prefix(root)?
        .to_str()
        .context("Invalid asset filename")?;
    sdo_platform::assets::fetch(logical).await
}
pub struct BrowserAssets {
    catalog: Catalog,
    ui: Arc<UiAssets>,
    characters: Arc<CharacterAssets>,
    manifest: Manifest,
    dances: Mutex<DanceLibrary>,
}
impl BrowserAssets {
    pub async fn open(url: &str) -> Result<AssetLoader> {
        use sdo_platform::asset_cache::{self, AssetCache};
        let endpoint = sdo_platform::remote::endpoint(url)?;
        let manifest_bytes = asset_cache::index(
            &endpoint.join("manifest.json")?,
            distribution::MAX_MANIFEST,
            false,
        )
        .await?
        .context("Missing asset manifest")?;
        let manifest: Manifest = serde_json::from_slice(&manifest_bytes)?;
        manifest.validate()?;
        let mut characters = distribution::CharacterManifest::default();
        if let Some(bytes) = asset_cache::index(
            &endpoint.join("characters.json")?,
            distribution::MAX_MANIFEST,
            true,
        )
        .await?
        {
            characters = serde_json::from_slice(&bytes)?;
            characters.validate()?;
        }
        let mut initial = characters.initial;
        let mut files = characters.files;
        for (id, file) in &manifest.files {
            if distribution::essential(id) {
                initial.insert(id.clone());
            }
            if let Some(previous) = files.insert(id.clone(), file.clone()) {
                anyhow::ensure!(
                    previous.sha256 == file.sha256 && previous.bytes == file.bytes,
                    "Conflicting manifests for {id}"
                );
            }
        }
        AssetCache::open(endpoint, files)
            .await?
            .activate(initial)
            .await?;
        let root = Path::new("/assets");
        let paths = curated::CollectionPaths::from_assets(root)?;
        let mut catalog = Catalog::parse(&sdo_platform::assets::read(root.join("music.json"))?)?;
        super::apply_manifest(&mut catalog, &manifest);
        let ui = Arc::new(UiAssets::distribution_defaults(&root.join("ui"))?);
        let characters = Arc::new(curated::characters(&paths)?);
        Ok(AssetLoader::new(Self {
            catalog,
            ui,
            characters,
            manifest,
            dances: Mutex::new(DanceLibrary::new(|name| {
                let path = resolve_resource(Path::new("/assets/character/motion"), name)?;
                Ok(Arc::new(CharacterMotion::parse(
                    &sdo_platform::assets::read(path)?,
                )?))
            })),
        }))
    }
}
impl AssetProvider for BrowserAssets {
    fn name(&self) -> &str {
        "Browser assets"
    }
    fn catalog(&self) -> Result<Catalog> {
        Ok(self.catalog.clone())
    }
    fn ui(&self) -> Result<Arc<UiAssets>> {
        Ok(self.ui.clone())
    }
    fn characters(&self, outfits: &OutfitOverrides) -> Result<Arc<CharacterAssets>> {
        if outfits.is_empty() {
            Ok(self.characters.clone())
        } else {
            Ok(Arc::new(curated::with_outfits(&self.characters, outfits)?))
        }
    }
    fn clothing_preview(&self, k: &ClothingPreviewKey) -> Result<Arc<CharacterAssets>> {
        Ok(Arc::new(curated::clothing_preview(&self.characters, k)?))
    }
    fn motion(&self, id: &str) -> Result<Arc<CharacterMotion>> {
        self.dances.lock().unwrap().motion(id)
    }
    fn chart(&self, id: &str, d: Difficulty, bpm: f64) -> Result<Chart> {
        Chart::parse_with_bpm(self.file(id)?, d, Some(bpm))
    }
    fn audio(
        &self,
        id: &str,
        _seconds: f64,
        _progress: &mut dyn FnMut(f32) -> bool,
    ) -> Result<sdo_audio::DecodedSong> {
        sdo_audio::DecodedSong::decode(self.file(id)?)
    }
    fn dance(&self, id: &str, chart: &Chart, d: Difficulty, bones: usize) -> Result<SongDance> {
        self.dances
            .lock()
            .unwrap()
            .load(&self.file(id)?, &chart.raw, d.index(), bones)
    }
    fn file(&self, id: &str) -> Result<Vec<u8>> {
        Ok(sdo_platform::assets::read(resolve_resource(
            Path::new("/assets"),
            id,
        )?)?)
    }
    fn file_size(&self, id: &str) -> Result<u64> {
        Ok(self
            .manifest
            .files
            .get(&key(id))
            .context("Unknown asset")?
            .bytes)
    }
}
