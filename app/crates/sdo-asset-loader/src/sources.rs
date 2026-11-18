//! Discover and combine converted collections inside the required asset folder.
use crate::{
    AssetLoader, AssetProvider,
    curated::CuratedProvider,
    normalized::{self, scoped},
};
use anyhow::{Context, Result};
use sdo_assets::*;
use sdo_audio::DecodedSong;
use sdo_dance::SongDance;
use sdo_song::{Catalog, Chart, Difficulty};
use serde::{Deserialize, Serialize};
use std::{path::Path, sync::Arc};

/// The sole runtime library location. SDO installations are import inputs only.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SourceSettings {
    pub path: String,
}
impl<'de> Deserialize<'de> for SourceSettings {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Document {
            path: Option<String>,
            custom_assets: Option<LegacyCustom>,
        }
        #[derive(Deserialize)]
        struct LegacyCustom {
            path: String,
        }
        let value = Document::deserialize(deserializer)?;
        Ok(Self {
            path: value
                .path
                .or_else(|| value.custom_assets.map(|c| c.path))
                .unwrap_or_default(),
        })
    }
}
#[derive(Debug, Clone, Default)]
pub struct SourceReport {
    pub name: String,
    pub songs: usize,
    pub skins: usize,
    pub characters: usize,
    pub issues: Vec<String>,
}
struct Source {
    key: String,
    provider: Arc<dyn AssetProvider>,
    catalog: Catalog,
    ui: Arc<UiAssets>,
    characters: Arc<CharacterAssets>,
}
/// Combines already normalized content; it has no knowledge of either file format.
struct Sources {
    sources: Vec<Source>,
    catalog: Catalog,
    ui: Arc<UiAssets>,
}
pub fn open(settings: &SourceSettings) -> Result<(AssetLoader, Vec<SourceReport>)> {
    type PendingProvider = (String, String, Result<Arc<dyn AssetProvider>>);
    let mut providers: Vec<PendingProvider> = Vec::new();
    if !settings.path.trim().is_empty() {
        let root = Path::new(&settings.path);
        match root.canonicalize() {
            Err(error) => providers.push((String::new(), "Assets".into(), Err(error.into()))),
            Ok(root) if !root.is_dir() => providers.push((
                String::new(),
                "Assets".into(),
                Err(anyhow::anyhow!("Asset path must be a folder")),
            )),
            Ok(root) => {
                if ["music.json", "ui", "character"]
                    .iter()
                    .any(|p| root.join(p).exists())
                {
                    providers.push((
                        String::new(),
                        "Custom Assets".into(),
                        CuratedProvider::from_assets(&root)
                            .map(|p| Arc::new(p) as Arc<dyn AssetProvider>),
                    ));
                }
                let games = root.join("games");
                if games.is_symlink() {
                    anyhow::bail!(
                        "The games directory must be inside the asset folder, not a symlink"
                    );
                }
                if games.is_dir() {
                    let mut entries =
                        std::fs::read_dir(&games)?.collect::<std::io::Result<Vec<_>>>()?;
                    entries.sort_by_key(|e| e.file_name());
                    for entry in entries {
                        let name = entry.file_name().to_string_lossy().into_owned();
                        if name.starts_with('.') || !entry.file_type()?.is_dir() {
                            continue;
                        }
                        let path = entry.path();
                        if !["music.json", "ui", "character"]
                            .iter()
                            .any(|p| path.join(p).exists())
                        {
                            continue;
                        }
                        providers.push((
                            name.clone(),
                            name.replace('_', " "),
                            CuratedProvider::from_assets(path)
                                .map(|p| Arc::new(p) as Arc<dyn AssetProvider>),
                        ));
                    }
                }
            }
        }
    }
    let mut sources = Vec::new();
    let mut reports = Vec::new();
    for (key, name, provider) in providers {
        let mut report = SourceReport {
            name,
            ..Default::default()
        };
        match provider {
            Err(e) => report.issues.push(format!("{e:#}")),
            Ok(provider) => {
                let root = if key.is_empty() {
                    Path::new(&settings.path).to_path_buf()
                } else {
                    Path::new(&settings.path).join("games").join(&key)
                };
                let mut catalog = if root.join("music.json").is_file() {
                    provider.catalog().unwrap_or_else(|e| {
                        report.issues.push(format!("Songs: {e:#}"));
                        empty_catalog()
                    })
                } else {
                    empty_catalog()
                };
                filter_catalog(provider.as_ref(), &mut catalog, &mut report.issues);
                let ui = if root.join("ui").is_dir() {
                    provider.ui().unwrap_or_else(|e| {
                        report.issues.push(format!("UI: {e:#}"));
                        Arc::new(normalized::empty_ui())
                    })
                } else {
                    Arc::new(normalized::empty_ui())
                };
                let characters = if root.join("character").is_dir() {
                    provider
                        .characters(&Default::default())
                        .unwrap_or_else(|e| {
                            report.issues.push(format!("Characters: {e:#}"));
                            Arc::new(normalized::merge_characters(&[]).unwrap())
                        })
                } else {
                    Arc::new(normalized::merge_characters(&[])?)
                };
                report.songs = catalog.songs.len();
                report.skins = ui.skins.len();
                report.characters = characters.presets.len();
                report.issues.extend(
                    ui.issues
                        .iter()
                        .chain(&characters.issues)
                        .map(ToString::to_string),
                );
                report.issues.extend(provider.diagnostics());
                sources.push(Source {
                    key,
                    provider,
                    catalog,
                    ui,
                    characters,
                });
            }
        }
        report.issues.sort();
        report.issues.dedup();
        reports.push(report);
    }
    Ok((AssetLoader::new(Sources::new(sources)), reports))
}
impl Sources {
    fn new(sources: Vec<Source>) -> Self {
        let mut catalog = empty_catalog();
        for s in &sources {
            catalog.source_country_keys.extend(
                s.catalog
                    .source_country_keys
                    .iter()
                    .map(|(k, v)| (scoped(&s.key, k), v.clone())),
            );
            for song in &s.catalog.songs {
                let mut song = song.clone();
                song.id = scoped(&s.key, &song.id);
                song.audio = scoped(&s.key, &song.audio);
                if let Some(p) = &mut song.preview {
                    p.path = scoped(&s.key, &p.path);
                }
                if let Some(d) = &mut song.dance_moves_paths {
                    *d = scoped(&s.key, d);
                }
                for p in &mut song.patterns {
                    p.file = scoped(&s.key, &p.file);
                }
                song.source = scoped(&s.key, &song.source);
                song.sources = song
                    .sources
                    .into_iter()
                    .map(|(k, v)| (scoped(&s.key, &k), v))
                    .collect();
                catalog.songs.push(song);
            }
        }
        let ui = Arc::new(normalized::merge_ui(
            &sources
                .iter()
                .map(|s| (s.key.as_str(), s.ui.clone()))
                .collect::<Vec<_>>(),
        ));
        Self {
            sources,
            catalog,
            ui,
        }
    }
}

fn empty_catalog() -> Catalog {
    Catalog {
        duration_unit: "seconds".into(),
        source_country_keys: Default::default(),
        songs: vec![],
    }
}
fn filter_catalog(provider: &dyn AssetProvider, catalog: &mut Catalog, issues: &mut Vec<String>) {
    catalog.songs.retain_mut(|song| {
        if let Err(e) = provider.file_size(&song.audio) {
            issues.push(format!("Song {}: {e:#}", song.id));
            return false;
        }
        if song
            .preview
            .as_ref()
            .is_some_and(|p| provider.file_size(&p.path).is_err())
        {
            issues.push(format!("Song {}: unavailable preview removed", song.id));
            song.preview = None;
        }
        if song
            .dance_moves_paths
            .as_ref()
            .is_some_and(|d| provider.file_size(d).is_err())
        {
            issues.push(format!("Song {}: unavailable dance removed", song.id));
            song.dance_moves_paths = None;
        }
        song.patterns.retain_mut(|p| {
            if p.mode != 4
                || !["k", "t"]
                    .iter()
                    .any(|kind| p.kind.eq_ignore_ascii_case(kind))
            {
                return false;
            }
            for difficulty in Difficulty::ALL {
                if p.levels[difficulty.index()].is_none() {
                    continue;
                }
                let message = match provider.chart(&p.file, difficulty, p.bpm.unwrap_or(song.bpm)) {
                    Ok(chart) => match chart.note_count_warning {
                        Some(warning) => format!("Warning: {warning}"),
                        None => continue,
                    },
                    Err(error) => {
                        p.levels[difficulty.index()] = None;
                        format!("{error:#}")
                    }
                };
                issues.push(format!(
                    "Song {} {} {difficulty:?}: {message}",
                    song.id, p.kind
                ));
            }
            p.levels.iter().any(Option::is_some)
        });
        !song.patterns.is_empty()
    });
}
impl Sources {
    fn route<'a>(&self, id: &'a str) -> Result<(&dyn AssetProvider, &'a str)> {
        let (key, local) = id.split_once("::").unwrap_or(("", id));
        let source = self
            .sources
            .iter()
            .find(|s| s.key == key)
            .context("Asset source is unavailable")?;
        Ok((source.provider.as_ref(), local))
    }
}
impl AssetProvider for Sources {
    fn name(&self) -> &str {
        "Asset sources"
    }
    fn catalog(&self) -> Result<Catalog> {
        Ok(self.catalog.clone())
    }
    fn ui(&self) -> Result<Arc<UiAssets>> {
        Ok(self.ui.clone())
    }
    fn skin(&self, selection: &UiSelection) -> Result<FourKeySkin> {
        if self.ui.ensure_ready().is_err() {
            Ok(normalized::shell_skin(&self.ui))
        } else {
            self.ui.compose(selection)
        }
    }
    fn characters(&self, outfits: &OutfitOverrides) -> Result<Arc<CharacterAssets>> {
        for (preset, slots) in outfits {
            let prefix = preset.split_once("::").map_or("", |(p, _)| p);
            for item in slots.values() {
                anyhow::ensure!(
                    item.split_once("::").map_or("", |(p, _)| p) == prefix,
                    "Clothing does not belong to this character's source"
                );
            }
        }
        let mut parts = Vec::new();
        for s in &self.sources {
            let local: OutfitOverrides = outfits
                .iter()
                .filter_map(|(id, slots)| {
                    let (key, id) = id.split_once("::").unwrap_or(("", id));
                    (key == s.key).then(|| {
                        (
                            id.to_owned(),
                            slots
                                .iter()
                                .map(|(slot, item)| {
                                    (
                                        slot.clone(),
                                        item.split_once("::")
                                            .map_or(item.as_str(), |(_, v)| v)
                                            .to_owned(),
                                    )
                                })
                                .collect(),
                        )
                    })
                })
                .collect();
            let assets = if local.is_empty() {
                s.characters.clone()
            } else {
                s.provider.characters(&local)?
            };
            parts.push((s.key.as_str(), assets));
        }
        Ok(Arc::new(normalized::merge_characters(&parts)?))
    }
    fn clothing_preview(&self, key: &ClothingPreviewKey) -> Result<Arc<CharacterAssets>> {
        anyhow::ensure!(
            key.preset.split_once("::").map_or("", |(p, _)| p)
                == key.item.split_once("::").map_or("", |(p, _)| p),
            "Clothing preview does not belong to this character's source"
        );
        let (provider, preset) = self.route(&key.preset)?;
        let item = key
            .item
            .split_once("::")
            .map_or(key.item.as_str(), |(_, v)| v);
        let preview = provider.clothing_preview(&ClothingPreviewKey {
            preset: preset.into(),
            item: item.into(),
        })?;
        let prefix = key.preset.split_once("::").map_or("", |(p, _)| p);
        Ok(Arc::new(normalized::merge_characters(&[(
            prefix, preview,
        )])?))
    }
    fn motion(&self, id: &str) -> Result<Arc<CharacterMotion>> {
        let (p, id) = self.route(id)?;
        p.motion(id)
    }
    fn chart(&self, id: &str, d: Difficulty, bpm: f64) -> Result<Chart> {
        let (p, id) = self.route(id)?;
        p.chart(id, d, bpm)
    }
    fn audio(
        &self,
        id: &str,
        seconds: f64,
        progress: &mut dyn FnMut(f32) -> bool,
    ) -> Result<DecodedSong> {
        let (p, id) = self.route(id)?;
        p.audio(id, seconds, progress)
    }
    fn dance(&self, id: &str, chart: &Chart, d: Difficulty, bones: usize) -> Result<SongDance> {
        let (p, id) = self.route(id)?;
        p.dance(id, chart, d, bones)
    }
    fn file(&self, id: &str) -> Result<Vec<u8>> {
        let (p, id) = self.route(id)?;
        p.file(id)
    }
    fn file_size(&self, id: &str) -> Result<u64> {
        let (p, id) = self.route(id)?;
        p.file_size(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cumulative_sources_keep_colliding_ids_and_texture_banks_separate() -> Result<()> {
        let mut sources = Vec::new();
        for key in ["first", "second"] {
            let p = Arc::new(crate::tests::MemoryProvider::new());
            sources.push(Source {
                key: key.into(),
                catalog: p.catalog()?,
                ui: p.ui()?,
                characters: p.characters(&Default::default())?,
                provider: p,
            });
        }
        let combined = Sources::new(sources);
        assert_eq!(combined.catalog.songs.len(), 2);
        assert_ne!(combined.catalog.songs[0].id, combined.catalog.songs[1].id);
        let ui = combined.ui.clone();
        assert_eq!(ui.skins.len(), 2);
        assert_ne!(ui.skins[0].choice.id, ui.skins[1].choice.id);
        assert_ne!(
            ui.skins[0].lanes[0].normal[0],
            ui.skins[1].lanes[0].normal[0]
        );
        let loader = AssetLoader::new(combined);
        for key in ["first", "second"] {
            assert_eq!(
                loader.request(crate::request::File(&format!("{key}::sac:file/1")))?,
                b"from archive"
            );
            let selection = ui.selection(
                &format!("{key}::memory-skin"),
                Some(&format!("{key}::board")),
                Some("none"),
                Some(&format!("{key}::score")),
            )?;
            let skin = loader.request(crate::request::Skin(&selection))?;
            assert_eq!(skin.textures[skin.lanes[0].normal[0].0].width, 16);
        }
        assert!(
            loader
                .request(crate::request::File("removed::sac:file/1"))
                .is_err()
        );
        Ok(())
    }
    #[test]
    fn old_source_settings_migrate_only_the_asset_folder() -> Result<()> {
        let settings: SourceSettings = serde_json::from_str(
            r#"{"custom_assets":{"enabled":false,"path":"saved/assets"},"games":[{"name":"Old native source","enabled":true}]}"#,
        )?;
        assert_eq!(settings.path, "saved/assets");
        assert_eq!(
            serde_json::to_value(settings)?,
            serde_json::json!({"path":"saved/assets"})
        );
        Ok(())
    }
    #[test]
    fn empty_folder_has_no_implicit_sources_or_fallbacks() -> Result<()> {
        let root = tempfile::tempdir()?;
        let settings = SourceSettings {
            path: root.path().display().to_string(),
        };
        let (loader, reports) = open(&settings)?;
        assert!(reports.is_empty());
        assert!(loader.request(crate::request::Catalog)?.songs.is_empty());
        assert_eq!(std::fs::read_dir(root.path())?.count(), 0);
        Ok(())
    }
}
