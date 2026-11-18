use crate::cli::Options;
use anyhow::Result;
use sdo_assets::{CharacterAssets, ClothingCategory, OutfitOverrides, UiAssets, UiSelection};
use sdo_config::{ScrollSpeed, load_preferences, save_preferences};
use sdo_song::catalog::Catalog;
use sdo_song::chart::Difficulty;
use sdo_ui::{SongSelector, SongSort};
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub multiplayer: sdo_net::Preferences,
    pub asset_paths: Option<sdo_asset_loader::sources::SourceSettings>,
    pub scroll_speed: ScrollSpeed,
    pub multiplayer_scroll_speed: ScrollSpeed,
    pub judgment: String,
    pub chart: String,
    pub difficulty: String,
    pub offset_ms: f64,
    pub selected_song: Option<String>,
    pub music_region: Option<String>,
    pub song_search: String,
    pub song_sort: String,
    pub song_sort_descending: bool,
    pub appearance: Option<UiSelection>,
    pub character: Option<String>,
    pub outfits: OutfitOverrides,
    pub character_preview: PreviewSettings,
    pub wardrobe: WardrobeSettings,
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            multiplayer: Default::default(),
            asset_paths: None,
            scroll_speed: ScrollSpeed::default(),
            multiplayer_scroll_speed: ScrollSpeed::default(),
            judgment: "keyboard".into(),
            chart: "k".into(),
            difficulty: "easy".into(),
            offset_ms: 0.,
            selected_song: None,
            music_region: None,
            song_search: String::new(),
            song_sort: SongSort::default().id().into(),
            song_sort_descending: false,
            appearance: None,
            character: None,
            outfits: Default::default(),
            character_preview: Default::default(),
            wardrobe: Default::default(),
            extra: Default::default(),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PreviewSettings {
    pub yaw: f32,
    pub elevation: f32,
    pub zoom: f32,
}
impl Default for PreviewSettings {
    fn default() -> Self {
        Self {
            yaw: 0.,
            elevation: 0.04,
            zoom: 1.,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WardrobeSettings {
    pub category: String,
    pub search: String,
}
impl Default for WardrobeSettings {
    fn default() -> Self {
        Self {
            category: "Hair".into(),
            search: String::new(),
        }
    }
}

#[derive(Default)]
pub struct SettingsStore {
    pub values: Settings,
    pub issues: Vec<String>,
    path: Option<PathBuf>,
    last_attempt: Option<Settings>,
    pending: Option<(Settings, Instant)>,
}
impl SettingsStore {
    pub fn load(path: PathBuf) -> Self {
        let mut store = Self {
            path: Some(path.clone()),
            ..Self::default()
        };
        match load_preferences::<Settings>(&path) {
            Ok(values) => {
                if path.exists() {
                    store.last_attempt = Some(values.clone());
                }
                store.values = values;
            }
            Err(error) => store.issues.push(format!("{error:#}")),
        }
        store
    }
    pub fn initialize_sources(&mut self) {
        self.values.asset_paths.get_or_insert_with(Default::default);
    }
    pub fn apply_options(&mut self, options: &mut Options, catalog: &Catalog) {
        options.speed.get_or_insert(self.values.scroll_speed);
        if options.chart == "k" && !options.specified.contains("--chart") {
            if ["k", "t"].contains(&self.values.chart.as_str()) {
                options.chart.clone_from(&self.values.chart);
            } else {
                self.issues.push(format!(
                    "Unknown saved chart family {:?}",
                    self.values.chart
                ));
            }
        }
        if options.difficulty.index() == 0 && !options.specified.contains("--difficulty") {
            match Difficulty::parse(&self.values.difficulty) {
                Ok(value) => options.difficulty = value,
                Err(error) => self
                    .issues
                    .push(format!("Invalid saved difficulty: {error:#}")),
            }
        }
        if options.preset == "keyboard" && !options.specified.contains("--judgment") {
            if let Some(preset) = sdo_gameplay::judgment::find(&self.values.judgment) {
                options.preset = preset.id.into();
            } else {
                self.issues.push(format!(
                    "Unknown saved judgment preset {:?}",
                    self.values.judgment
                ));
            }
        }
        if options.offset == 0. && !options.specified.contains("--offset-ms") {
            if self.values.offset_ms.is_finite() && self.values.offset_ms.abs() <= 5000. {
                options.offset = self.values.offset_ms / 1000.;
            } else {
                self.issues
                    .push("Saved timing offset must be within +/-5000 ms".into());
            }
        }
        if options.music_region.is_none()
            && !catalog.source_country_keys.is_empty()
            && let Some(source) = &self.values.music_region
        {
            match catalog.source_key(source) {
                Ok(key) => options.music_region = Some(key.into()),
                Err(error) => self
                    .issues
                    .push(format!("Cannot restore music filter: {error:#}")),
            }
        }
    }
    pub fn appearance(&mut self, options: &Options, assets: &UiAssets) -> Result<UiSelection> {
        if assets.ensure_ready().is_err() {
            return Ok(UiSelection::default());
        }
        let mut selection = options.ui_selection(assets)?;
        if let Some(saved) = &self.values.appearance {
            let choices = assets.choices();
            for (field, current, previous, supplied, available) in [
                (
                    "noteskin",
                    &mut selection.skin,
                    &saved.skin,
                    options.skin != "default" || options.specified.contains("--skin"),
                    &choices.skins,
                ),
                (
                    "board",
                    &mut selection.board,
                    &saved.board,
                    options.board.is_some(),
                    &choices.boards,
                ),
                (
                    "effect",
                    &mut selection.effect,
                    &saved.effect,
                    options.hit_effect.is_some(),
                    &choices.effects,
                ),
                (
                    "score",
                    &mut selection.score,
                    &saved.score,
                    options.score_ui.is_some(),
                    &choices.scores,
                ),
            ] {
                if supplied {
                    continue;
                }
                if available.iter().any(|choice| choice.id == *previous)
                    || (field == "effect" && previous == "none")
                {
                    current.clone_from(previous);
                } else {
                    self.issues.push(format!(
                        "Saved {field} {previous:?} is unavailable; using the default."
                    ));
                }
            }
        }
        Ok(selection)
    }
    pub fn restore_menu(&mut self, selector: &mut SongSelector, explicit_song: bool) {
        selector.lobby.preferences = self.values.multiplayer.clone();
        if let Some(sort) = SongSort::parse(&self.values.song_sort) {
            selector.sort = sort;
        } else {
            self.issues.push(format!(
                "Unknown saved song sort {:?}",
                self.values.song_sort
            ));
        }
        selector.sort_descending = self.values.song_sort_descending;
        if !explicit_song {
            selector.query.clone_from(&self.values.song_search);
        }
        selector.refilter();
        let preview = &self.values.character_preview;
        if preview.yaw.is_finite()
            && (-0.6..=0.6).contains(&preview.elevation)
            && (0.55..=2.5).contains(&preview.zoom)
        {
            selector.character_yaw = preview.yaw;
            selector.character_elevation = preview.elevation;
            selector.character_zoom = preview.zoom;
        } else {
            self.issues.push(
                "Saved character camera is outside the supported range; using the default.".into(),
            );
        }
        let category = ClothingCategory::ALL.into_iter().find(|category| {
            category
                .label()
                .eq_ignore_ascii_case(&self.values.wardrobe.category)
        });
        if category.is_none() {
            self.issues.push(format!(
                "Unknown saved wardrobe category {:?}",
                self.values.wardrobe.category
            ));
        }
        selector
            .wardrobe
            .browse(category.unwrap_or_default(), &self.values.wardrobe.search);
    }
    pub fn capture(&self, selector: &SongSelector, characters: &CharacterAssets) -> Settings {
        Settings {
            multiplayer: selector.lobby.preferences.clone(),
            asset_paths: Some(selector.source_settings.clone()),
            scroll_speed: selector.scroll_speed,
            multiplayer_scroll_speed: selector.lobby.scroll_speed,
            judgment: selector.judgment.clone(),
            chart: selector.kind.clone(),
            difficulty: ["easy", "normal", "hard"][selector.difficulty].into(),
            offset_ms: selector.timing_offset_ms,
            selected_song: selector.selected().map(|song| song.id.clone()),
            music_region: selector.source.clone(),
            song_search: selector.query.clone(),
            song_sort: selector.sort.id().into(),
            song_sort_descending: selector.sort_descending,
            appearance: Some(selector.appearance.clone()),
            character: Some(selector.character.clone()),
            outfits: {
                let mut saved = self.values.outfits.clone();
                for preset in &characters.presets {
                    saved.remove(&preset.choice.id);
                }
                saved.extend(characters.outfit_overrides());
                saved
            },
            character_preview: PreviewSettings {
                yaw: selector.character_yaw,
                elevation: selector.character_elevation,
                zoom: selector.character_zoom,
            },
            wardrobe: WardrobeSettings {
                category: selector.wardrobe.category().label().into(),
                search: selector.wardrobe.query().into(),
            },
            extra: self.values.extra.clone(),
        }
    }
    pub fn finish_restore(&mut self, current: Settings) -> Option<String> {
        if self.issues.is_empty() {
            return None;
        }
        // Don't replace a damaged/stale file just because startup used defaults.
        // A subsequent user change can save a new valid document.
        self.last_attempt = Some(current);
        Some(format!(
            "Some settings could not be restored:\n\n{}",
            std::mem::take(&mut self.issues).join("\n\n")
        ))
    }
    pub fn update(&mut self, current: Settings, flush: bool) -> Result<()> {
        self.update_at(current, flush, Instant::now())
    }
    fn update_at(&mut self, current: Settings, flush: bool, now: Instant) -> Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if self.last_attempt.as_ref() == Some(&current) {
            self.pending = None;
            return Ok(());
        }
        if self
            .pending
            .as_ref()
            .is_none_or(|(values, _)| values != &current)
        {
            self.pending = Some((current, now));
        }
        if flush
            || self.pending.as_ref().is_some_and(|(_, changed)| {
                now.duration_since(*changed) >= Duration::from_millis(250)
            })
        {
            let (values, _) = self.pending.take().unwrap();
            let result = save_preferences(path, &values);
            // Surface errors once per change instead of retrying every frame.
            self.last_attempt = Some(values.clone());
            result?;
            self.values = values;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn explicit_cli_defaults_override_saved_choices() -> Result<()> {
        let catalog = Catalog {
            duration_unit: "seconds".into(),
            source_country_keys: Default::default(),
            songs: Vec::new(),
        };
        let mut store = SettingsStore::default();
        store.values.scroll_speed = ScrollSpeed::new(8)?;
        store.values.chart = "t".into();
        store.values.difficulty = "hard".into();
        store.values.judgment = "pad".into();
        store.values.offset_ms = 100.;
        let mut defaults = Options::default();
        store.apply_options(&mut defaults, &catalog);
        assert_eq!(defaults.speed.unwrap().value(), 8);
        assert_eq!(defaults.chart, "t");
        assert_eq!(defaults.difficulty.index(), 2);
        assert_eq!(defaults.preset, "pad");
        assert_eq!(defaults.offset, 0.1);
        let mut explicit = Options::parse_args(
            [
                "--speed",
                "1",
                "--chart",
                "k",
                "--difficulty",
                "easy",
                "--judgment",
                "keyboard",
                "--offset-ms",
                "0",
            ]
            .map(str::to_owned),
        )?
        .unwrap();
        store.apply_options(&mut explicit, &catalog);
        assert_eq!(explicit.speed.unwrap().value(), 1);
        assert_eq!(explicit.chart, "k");
        assert_eq!(explicit.difficulty.index(), 0);
        assert_eq!(explicit.preset, "keyboard");
        assert_eq!(explicit.offset, 0.);
        Ok(())
    }

    #[test]
    fn settings_round_trip_debounce_and_errors() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("sdo-settings-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir)?;
        let path = dir.join("bin/settings.json");
        fs::create_dir_all(path.parent().unwrap())?;
        fs::write(&path, br#"{"scroll_speed":3,"future_option":true}"#)?;
        let mut store = SettingsStore::load(path.clone());
        let mut settings = store.values.clone();
        assert_eq!(settings.scroll_speed.value(), 3);
        settings.asset_paths = Some(sdo_asset_loader::sources::SourceSettings {
            path: "saved/assets".into(),
        });
        settings.multiplayer.name = "Alice".into();
        settings.multiplayer.address = "rhythm.example:32100".into();
        settings.character = Some("female".into());
        settings.selected_song = Some("01518".into());
        settings.difficulty = "hard".into();
        settings.song_sort = "level".into();
        settings.song_sort_descending = true;
        settings.appearance = Some(UiSelection {
            skin: "rainbow_1".into(),
            board: "default".into(),
            effect: "none".into(),
            score: "default".into(),
        });
        settings.outfits.insert(
            "male".into(),
            [("shoes".into(), "000130_MAN_SHOES.MSH".into())].into(),
        );
        settings.outfits.insert(
            "female".into(),
            [("hair".into(), "000002_WOMAN_HAIR.MSH".into())].into(),
        );
        settings.character_preview.zoom = 1.5;
        settings.wardrobe.category = "Shoes".into();
        let now = Instant::now();
        store.update_at(settings.clone(), false, now)?;
        assert!(
            load_preferences::<serde_json::Value>(&path)?
                .get("character")
                .is_none()
        );
        store.update_at(settings.clone(), false, now + Duration::from_millis(251))?;
        assert_eq!(SettingsStore::load(path.clone()).values, settings);
        assert_eq!(
            load_preferences::<serde_json::Value>(&path)?["future_option"],
            true
        );
        settings.scroll_speed = ScrollSpeed::new(8)?;
        store.update(settings.clone(), true)?;
        assert_eq!(load_preferences::<Settings>(&path)?, settings);
        fs::write(&path, "invalid json")?;
        let mut broken = SettingsStore::load(path.clone());
        assert!(
            broken
                .finish_restore(Settings::default())
                .unwrap()
                .contains("Invalid settings")
        );
        broken.update(Settings::default(), true)?;
        assert_eq!(fs::read_to_string(&path)?, "invalid json");
        // A subsequent deliberate change repairs the file.
        broken.update(settings.clone(), true)?;
        assert_eq!(load_preferences::<Settings>(&path)?, settings);
        // Save failures are returned once; another render doesn't repeat the error.
        fs::remove_file(&path)?;
        fs::create_dir(&path)?;
        settings.scroll_speed = ScrollSpeed::default();
        assert!(broken.update(settings.clone(), true).is_err());
        broken.update(settings, true)?;
        fs::remove_dir_all(dir)?;
        Ok(())
    }
    #[test]
    fn empty_sources_can_reload_and_keep_disabled_configuration_and_outfits() -> Result<()> {
        use sdo_asset_loader::{
            request as asset,
            sources::{self, SourceSettings},
        };
        let settings = SourceSettings::default();
        let (content, _) = sources::open(&settings)?;
        let mut store = SettingsStore::default();
        store.values.asset_paths = Some(settings.clone());
        store.values.outfits.insert(
            "disabled::male".into(),
            [("hair".into(), "disabled::item".into())].into(),
        );
        let saved = store.values.outfits.clone();
        let mut app = crate::app::App::new(
            content.clone(),
            Options::default(),
            content.request(asset::Catalog)?,
            content.request(asset::Ui)?,
            None,
            store,
            crate::scores::ScoreStore::default(),
        )?;
        assert!(app.selector.settings_open);
        assert!(app.selector.selected().is_none());
        assert_eq!(
            app.settings.capture(&app.selector, &app.characters).outfits,
            saved
        );
        app.reload_sources();
        let started = Instant::now();
        while app.source_reload.is_some() {
            app.poll_sources();
            anyhow::ensure!(
                started.elapsed() < Duration::from_secs(5),
                "Timed out reloading empty sources"
            );
            std::thread::yield_now();
        }
        assert_eq!(app.selector.source_settings, settings);
        assert!(app.selector.error.is_none());
        assert_eq!(
            app.settings.capture(&app.selector, &app.characters).outfits,
            saved
        );
        Ok(())
    }
}
