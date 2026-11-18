//! Read-only music.json catalog: global song identities, explicit paths and provenance.
use crate::chart::{ChartHeader, Difficulty};
use anyhow::{Context, Result, ensure};
use sdo_assets::curated::resolve_resource;
use sdo_assets::curated::{CollectionPaths, resource_path};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::{Component, PathBuf},
};

#[derive(Debug, Clone, Deserialize)]
pub struct Catalog {
    pub duration_unit: String,
    pub source_country_keys: BTreeMap<String, String>,
    pub songs: Vec<Song>,
}
#[derive(Debug, Clone, Deserialize)]
pub struct Song {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub duration: f64,
    pub audio: String,
    pub preview: Option<Preview>,
    pub dance_moves_paths: Option<String>,
    pub bpm: f64,
    pub patterns: Vec<Pattern>,
    pub sources: BTreeMap<String, Source>,
    /// Provenance of the chosen recording, distinct from all known source mappings.
    pub source: String,
}
#[derive(Debug, Clone, Deserialize)]
pub struct Preview {
    pub path: String,
    pub duration: f64,
}
#[derive(Debug, Clone, Deserialize)]
pub struct Pattern {
    pub file: String,
    /// Computed from file bytes. A catalog's declared digest is never trusted.
    #[serde(skip)]
    pub sha256: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub mode: u8,
    pub bpm: Option<f64>,
    /// Read from the referenced GN header at catalog load, never inferred from BPM.
    #[serde(skip)]
    pub levels: [Option<u16>; 3],
    #[serde(skip)]
    pub level_error: Option<String>,
}
#[derive(Debug, Clone, Deserialize)]
pub struct Source {
    pub id: SourceIds,
}
/// A recording may occur under several original IDs within the same country.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum SourceIds {
    Single(String),
    Multiple(Vec<String>),
}
impl SourceIds {
    pub fn as_slice(&self) -> &[String] {
        match self {
            Self::Single(id) => std::slice::from_ref(id),
            Self::Multiple(ids) => ids,
        }
    }
}
pub struct Selection<'a> {
    pub song: &'a Song,
    pub pattern: &'a Pattern,
}
impl Selection<'_> {
    pub fn bpm(&self) -> f64 {
        self.pattern.bpm.unwrap_or(self.song.bpm)
    }
    pub fn resolve(&self, paths: &CollectionPaths) -> Result<SongFiles> {
        Ok(SongFiles {
            audio: resolve_resource(paths.assets(), &self.song.audio)
                .with_context(|| format!("Missing audio for catalog song {}", self.song.id))?,
            pattern: resolve_resource(paths.assets(), &self.pattern.file).with_context(|| {
                format!(
                    "Missing {} chart for catalog song {}",
                    self.pattern.kind, self.song.id
                )
            })?,
        })
    }
}
pub struct SongFiles {
    pub audio: PathBuf,
    pub pattern: PathBuf,
}
#[derive(Debug)]
pub struct CatalogStats {
    pub songs: usize,
    pub patterns: usize,
    pub source_mappings: usize,
    pub original_ids: usize,
    pub bpm_overrides: usize,
    pub previews: usize,
}
fn normalized_id(id: &str) -> Result<&str> {
    ensure!(
        !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()),
        "Catalog song IDs must contain digits: {id:?}"
    );
    let key = id.trim_start_matches('0');
    Ok(if key.is_empty() { "0" } else { key })
}
fn music_path(path: &str, extension: &str) -> Result<()> {
    let relative = resource_path(path)?;
    ensure!(
        matches!(relative.components().next(), Some(Component::Normal(dir)) if dir.to_string_lossy().eq_ignore_ascii_case("music")),
        "Catalog file must be relative to assets/ and inside its music directory: {path:?}"
    );
    ensure!(
        relative
            .extension()
            .is_some_and(|ext| ext.to_string_lossy().eq_ignore_ascii_case(extension)),
        "Expected a .{extension} catalog file: {path:?}"
    );
    Ok(())
}
fn valid_bpm(bpm: f64) -> bool {
    bpm.is_finite() && (10.0..=1000.0).contains(&bpm)
}
impl Song {
    pub fn resolve_preview(&self, paths: &CollectionPaths) -> Result<Option<PathBuf>> {
        self.preview
            .as_ref()
            .map(|preview| {
                resolve_resource(paths.assets(), &preview.path)
                    .with_context(|| format!("Missing preview for catalog song {}", self.id))
            })
            .transpose()
    }
    pub fn pattern(&self, kind: &str, mode: u8) -> Result<&Pattern> {
        self.patterns
            .iter()
            .find(|p| p.kind.eq_ignore_ascii_case(kind) && p.mode == mode)
            .with_context(|| format!("Catalog song {} has no {mode}-key {kind} pattern", self.id))
    }
}
impl Catalog {
    pub fn load(paths: &CollectionPaths) -> Result<Self> {
        let file = resolve_resource(paths.assets(), "music.json")
            .context("Cannot open the collection's assets/music.json catalog")?;
        let mut catalog = Self::parse(&fs::read(&file)?)
            .with_context(|| format!("Invalid music catalog {}", file.display()))?;
        for pattern in catalog
            .songs
            .iter_mut()
            .flat_map(|song| &mut song.patterns)
            .filter(|pattern| pattern.mode == 4)
        {
            let header = (|| -> Result<(ChartHeader, String)> {
                let path = resolve_resource(paths.assets(), &pattern.file)?;
                let raw = fs::read(path)?;
                let header = ChartHeader::parse(&raw)?;
                header.validate_length(raw.len() as u64)?;
                Ok((header, crate::chart::pattern_sha256(&raw)))
            })();
            match header {
                Ok((header, sha256)) => {
                    pattern.sha256 = sha256;
                    pattern.levels = Difficulty::ALL.map(|difficulty| header.level(difficulty));
                }
                Err(error) => pattern.level_error = Some(format!("{error:#}")),
            }
        }
        Ok(catalog)
    }
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let catalog: Self = serde_json::from_slice(bytes).context("Cannot parse music.json")?;
        catalog.validate()?;
        Ok(catalog)
    }
    fn validate(&self) -> Result<()> {
        ensure!(
            self.duration_unit == "seconds",
            "Unsupported catalog duration_unit {:?}; expected seconds",
            self.duration_unit
        );
        ensure!(!self.songs.is_empty(), "Music catalog contains no songs");
        ensure!(
            !self.source_country_keys.is_empty(),
            "Music catalog has no source country keys"
        );
        let mut ids = HashSet::new();
        for song in &self.songs {
            ensure!(
                ids.insert(normalized_id(&song.id)?),
                "Duplicate catalog song ID {}",
                song.id
            );
            ensure!(
                song.duration.is_finite() && song.duration > 0.0,
                "Invalid duration for catalog song {}",
                song.id
            );
            ensure!(
                valid_bpm(song.bpm),
                "Invalid BPM for catalog song {}",
                song.id
            );
            music_path(&song.audio, "ogg")
                .with_context(|| format!("Song {} audio path", song.id))?;
            if let Some(dance) = &song.dance_moves_paths {
                music_path(dance, "dps").with_context(|| format!("Song {} dance path", song.id))?;
            }
            if let Some(preview) = &song.preview {
                music_path(&preview.path, "ogg")
                    .with_context(|| format!("Song {} preview path", song.id))?;
                ensure!(
                    preview.duration.is_finite() && preview.duration > 0.0,
                    "Invalid preview duration for song {}",
                    song.id
                );
            }
            ensure!(
                self.source_country_keys.contains_key(&song.source)
                    && song.sources.contains_key(&song.source),
                "Song {} chosen source {:?} is not in its source mappings",
                song.id,
                song.source
            );
            for (country, source) in &song.sources {
                ensure!(
                    self.source_country_keys.contains_key(country),
                    "Song {} has unknown source country {country:?}",
                    song.id
                );
                ensure!(
                    !source.id.as_slice().is_empty()
                        && source.id.as_slice().iter().all(|id| !id.is_empty()),
                    "Song {} has empty original IDs for {country}",
                    song.id
                );
            }
            ensure!(
                !song.patterns.is_empty(),
                "Song {} has no patterns",
                song.id
            );
            let mut patterns = HashSet::new();
            for pattern in &song.patterns {
                ensure!(
                    !pattern.kind.is_empty() && pattern.mode > 0,
                    "Invalid pattern type/mode for song {}",
                    song.id
                );
                ensure!(
                    patterns.insert((pattern.kind.to_ascii_uppercase(), pattern.mode)),
                    "Song {} has ambiguous {}-key {} patterns",
                    song.id,
                    pattern.mode,
                    pattern.kind
                );
                music_path(&pattern.file, "gn")
                    .with_context(|| format!("Song {} pattern path", song.id))?;
                ensure!(
                    pattern.bpm.is_none_or(valid_bpm),
                    "Invalid pattern BPM override for song {}",
                    song.id
                );
            }
        }
        Ok(())
    }
    pub fn song(&self, id: &str) -> Result<&Song> {
        // Providers may use opaque IDs. Preserve exact identity before accepting
        // the curated collection's convenient zero-padded numeric aliases.
        self.songs
            .iter()
            .find(|song| song.id == id)
            .or_else(|| {
                let key = normalized_id(id).ok()?;
                self.songs
                    .iter()
                    .find(|song| normalized_id(&song.id).ok() == Some(key))
            })
            .with_context(|| {
                format!("Unknown catalog song ID {id:?}; use --list-songs to find an available ID")
            })
    }
    /// Both country keys and display names work, including sdo-x / SDO-X -> sdox.
    pub fn source_key(&self, name: &str) -> Result<&str> {
        self.source_country_keys
            .iter()
            .find(|(key, label)| key.eq_ignore_ascii_case(name) || label.eq_ignore_ascii_case(name))
            .map(|(key, _)| key.as_str())
            .with_context(|| {
                format!(
                    "Unknown music source {name:?}; expected {}",
                    self.source_country_keys
                        .keys()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
    }
    pub fn songs_for_source<'a>(
        &'a self,
        source: Option<&str>,
    ) -> Result<impl Iterator<Item = &'a Song> + 'a> {
        let key = source
            .map(|name| self.source_key(name).map(str::to_owned))
            .transpose()?;
        Ok(self.songs.iter().filter(move |song| {
            key.as_ref()
                .is_none_or(|key| song.sources.contains_key(key))
        }))
    }
    pub fn select<'a>(
        &'a self,
        id: &str,
        source: Option<&str>,
        kind: &str,
        mode: u8,
    ) -> Result<Selection<'a>> {
        let song = self.song(id)?;
        if let Some(source) = source {
            let key = self.source_key(source)?;
            ensure!(
                song.sources.contains_key(key),
                "Catalog song {} has no {source} source mapping",
                song.id
            );
        }
        Ok(Selection {
            song,
            pattern: song.pattern(kind, mode)?,
        })
    }
    pub fn preferred<'a>(
        &'a self,
        title: &str,
        source: Option<&str>,
        kind: &str,
        mode: u8,
    ) -> Result<Selection<'a>> {
        let mut first = None;
        for song in self.songs_for_source(source)? {
            if song.pattern(kind, mode).is_err() {
                continue;
            }
            first.get_or_insert(song);
            if song.title.eq_ignore_ascii_case(title) {
                return self.select(&song.id, source, kind, mode);
            }
        }
        let song = first.context("No matching songs with a supported pattern in music.json")?;
        self.select(&song.id, source, kind, mode)
    }
    pub fn stats(&self) -> CatalogStats {
        CatalogStats {
            songs: self.songs.len(),
            patterns: self.songs.iter().map(|s| s.patterns.len()).sum(),
            source_mappings: self.songs.iter().map(|s| s.sources.len()).sum(),
            original_ids: self
                .songs
                .iter()
                .flat_map(|s| s.sources.values())
                .map(|s| s.id.as_slice().len())
                .sum(),
            bpm_overrides: self
                .songs
                .iter()
                .flat_map(|s| &s.patterns)
                .filter(|p| p.bpm.is_some())
                .count(),
            previews: self.songs.iter().filter(|s| s.preview.is_some()).count(),
        }
    }
    /// Validate every declared file without decoding audio or requiring a GPU.
    pub fn verify_files(&self, paths: &CollectionPaths) -> Result<CatalogStats> {
        for song in &self.songs {
            resolve_resource(paths.assets(), &song.audio)
                .with_context(|| format!("Song {} audio {}", song.id, song.audio))?;
            song.resolve_preview(paths)?;
            for pattern in &song.patterns {
                resolve_resource(paths.assets(), &pattern.file)
                    .with_context(|| format!("Song {} pattern {}", song.id, pattern.file))?;
            }
        }
        Ok(self.stats())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    fn fixture() -> Value {
        json!({
            "duration_unit":"seconds",
            "source_country_keys":{"sdox":"SDO-X","thailand":"Thailand","usa":"USA"},
            "songs":[
                {"id":"00001","title":"Holy Knight","artist":"音楽","duration":133.1,"audio":"music/sdom00001.ogg","bpm":120.0,
                 "patterns":[{"file":"music/sdom00001K.gn","type":"K","mode":4},{"file":"music/sdom00001T.gn","type":"T","mode":4,"bpm":150.0}],
                 "sources":{"sdox":{"id":"5073"},"thailand":{"id":["0026","1037"]}},"source":"sdox"},
                {"id":"00002","title":"Cynthia No Hikari","artist":"Eir Aoi","duration":109.8,"audio":"music/sdom00002.ogg","bpm":166.0,
                 "patterns":[{"file":"music/sdom00002K.gn","type":"K","mode":4}],
                 "sources":{"sdox":{"id":"5073"}},"source":"sdox"}
            ]
        })
    }
    fn parse(value: &Value) -> Result<Catalog> {
        Catalog::parse(&serde_json::to_vec(value).unwrap())
    }
    #[test]
    fn reads_current_schema_with_multi_id_provenance_and_bpm_override() {
        let catalog = parse(&fixture()).unwrap();
        let selected = catalog.select("1", Some("SDO-X"), "t", 4).unwrap();
        assert_eq!(selected.song.id, "00001");
        assert_eq!(selected.song.artist, "音楽");
        assert_eq!(
            selected.song.sources["thailand"].id.as_slice(),
            ["0026", "1037"]
        );
        assert_eq!(selected.bpm(), 150.0);
        assert_eq!(catalog.select("00001", None, "K", 4).unwrap().bpm(), 120.0);
        assert_eq!(catalog.stats().original_ids, 4);
    }
    #[test]
    fn catalog_caches_original_levels_and_reports_unreadable_headers() -> Result<()> {
        let root = std::env::temp_dir().join(format!("sdo-catalog-levels-{}", std::process::id()));
        fs::create_dir_all(root.join("assets/music"))?;
        fs::write(
            root.join("assets/music.json"),
            serde_json::to_vec(&fixture())?,
        )?;
        let mut raw = vec![0u8; 336];
        raw[4..8].copy_from_slice(b"gn\0\0");
        raw[8..12].copy_from_slice(&2.9f32.to_le_bytes());
        for (i, level) in [3u16, 4, 5].into_iter().enumerate() {
            raw[20 + i * 2..22 + i * 2].copy_from_slice(&level.to_le_bytes());
        }
        for (i, offset) in [300u32, 312, 324, 336].into_iter().enumerate() {
            raw[284 + i * 4..288 + i * 4].copy_from_slice(&offset.to_le_bytes());
        }
        fs::write(root.join("assets/music/sdom00001K.gn"), &raw)?;
        raw[20..22].copy_from_slice(&1u16.to_le_bytes());
        raw[292..296].copy_from_slice(&312u32.to_le_bytes());
        fs::write(root.join("assets/music/sdom00001T.gn"), &raw)?;
        fs::write(root.join("assets/music/sdom00002K.gn"), b"broken")?;
        let catalog = Catalog::load(&CollectionPaths::new(&root)?)?;
        assert_eq!(
            catalog.select("1", None, "k", 4)?.pattern.levels,
            [Some(3), Some(4), Some(5)]
        );
        assert_eq!(
            catalog.select("1", None, "t", 4)?.pattern.levels,
            [Some(1), None, Some(5)]
        );
        let bad = catalog.select("2", None, "k", 4)?.pattern;
        assert_eq!(bad.levels, [None; 3]);
        assert!(
            bad.level_error
                .as_ref()
                .unwrap()
                .contains("Truncated GN header")
        );
        let original_hash = catalog.select("1", None, "k", 4)?.pattern.sha256.clone();
        let file = root.join("assets/music/sdom00001K.gn");
        let mut bytes = fs::read(&file)?;
        assert_eq!(original_hash, crate::chart::pattern_sha256(&bytes));
        let mut declared = fixture();
        declared["songs"][0]["patterns"][0]["sha256"] = json!("f".repeat(64));
        fs::write(
            root.join("assets/music.json"),
            serde_json::to_vec(&declared)?,
        )?;
        assert_eq!(
            Catalog::load(&CollectionPaths::new(&root)?)?
                .select("1", None, "k", 4)?
                .pattern
                .sha256,
            original_hash
        );
        bytes[108] = 1;
        fs::write(file, &bytes)?;
        assert_ne!(
            Catalog::load(&CollectionPaths::new(&root)?)?
                .select("1", None, "k", 4)?
                .pattern
                .sha256,
            original_hash
        );
        fs::remove_dir_all(root)?;
        assert_eq!(
            catalog.select("1", None, "k", 4)?.pattern.levels[2],
            Some(5),
            "Displaying levels must not reopen charts"
        );
        Ok(())
    }
    #[test]
    fn global_ids_keep_overridden_original_song_ids_distinct() {
        let catalog = parse(&fixture()).unwrap();
        assert_eq!(catalog.song("00002").unwrap().title, "Cynthia No Hikari");
        assert_eq!(
            catalog
                .preferred("Holy Knight", None, "k", 4)
                .unwrap()
                .song
                .id,
            "00001"
        );
        assert!(catalog.song("5073").is_err());
    }
    #[test]
    fn source_filter_uses_all_mappings_not_only_chosen_recording() {
        let catalog = parse(&fixture()).unwrap();
        assert_eq!(
            catalog.songs_for_source(Some("Thailand")).unwrap().count(),
            1
        );
        assert!(catalog.select("1", Some("thailand"), "K", 4).is_ok());
        assert!(catalog.select("2", Some("thailand"), "K", 4).is_err());
        assert!(catalog.songs_for_source(Some("unknown")).is_err());
    }
    #[test]
    fn incompatible_modes_do_not_silently_select_another_chart() {
        let mut value = fixture();
        value["songs"][0]["patterns"][0]["mode"] = json!(8);
        let catalog = parse(&value).unwrap();
        assert!(catalog.select("1", None, "K", 4).is_err());
        assert!(catalog.select("1", None, "T", 4).is_ok());
    }
    #[test]
    fn duplicate_global_ids_and_patterns_are_rejected() {
        let mut value = fixture();
        value["songs"][1]["id"] = json!("1");
        assert!(parse(&value).is_err());
        let mut value = fixture();
        let p = value["songs"][0]["patterns"][0].clone();
        value["songs"][0]["patterns"]
            .as_array_mut()
            .unwrap()
            .push(p);
        assert!(parse(&value).is_err());
    }
    #[test]
    fn file_paths_cannot_escape_the_music_collection() {
        for path in [
            "../song.ogg",
            "/music/song.ogg",
            "data/song.ogg",
            r"music\..\song.ogg",
            "C:/music/song.ogg",
        ] {
            let mut value = fixture();
            value["songs"][0]["audio"] = json!(path);
            assert!(parse(&value).is_err(), "{path}");
        }
    }
    #[test]
    fn invalid_units_and_timing_metadata_fail_early() {
        let mut value = fixture();
        value["duration_unit"] = json!("milliseconds");
        assert!(parse(&value).is_err());
        let mut value = fixture();
        value["songs"][0]["patterns"][1]["bpm"] = json!(0.0);
        assert!(parse(&value).is_err());
        let mut value = fixture();
        value["songs"][0]["sources"]["thailand"]["id"] = json!([]);
        assert!(parse(&value).is_err());
    }
    #[test]
    fn previews_keep_each_songs_explicit_path_and_duration() {
        let mut value = fixture();
        value["songs"][0]["preview"] =
            json!({"path":"music/first_preview.ogg","duration":14.30254});
        value["songs"][1]["preview"] = json!({"path":"music/second_preview.ogg","duration":11.5});
        let catalog = parse(&value).unwrap();
        assert_eq!(
            catalog.song("1").unwrap().preview.as_ref().unwrap().path,
            "music/first_preview.ogg"
        );
        assert_eq!(
            catalog.song("2").unwrap().preview.as_ref().unwrap().path,
            "music/second_preview.ogg"
        );
        assert_eq!(
            catalog
                .song("1")
                .unwrap()
                .preview
                .as_ref()
                .unwrap()
                .duration,
            14.30254
        );
        assert_eq!(catalog.stats().previews, 2);
        assert_eq!(parse(&fixture()).unwrap().stats().previews, 0);
    }
    #[test]
    fn invalid_preview_paths_and_durations_are_rejected() {
        for preview in [
            json!({"path":"../preview.ogg","duration":2.0}),
            json!({"path":"music/preview.gn","duration":2.0}),
            json!({"path":"music/preview.ogg","duration":0.0}),
        ] {
            let mut value = fixture();
            value["songs"][0]["preview"] = preview;
            assert!(parse(&value).is_err());
        }
    }
}
