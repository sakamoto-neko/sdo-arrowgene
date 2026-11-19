//! Searchable song selection, independent of gameplay and audio output.
pub mod notes;
use anyhow::Result;
use egui::{Color32, RichText};
use sdo_assets::{UiAssets, UiChoice, UiChoices, UiSelection};
mod multiplayer;
mod round;
pub use multiplayer::{LobbyAction, LobbyView, RoomSongSelection, ServerMenu};
pub use round::RoundHud;
mod preview;
mod room_settings;
mod room_song_picker;
mod server_menu;
mod sorting;
mod sources;
mod theme;
mod wardrobe;
use preview::AppearancePreview;
use sdo_config::{ScoreBook, ScoreKey};
use sdo_song::catalog::{Catalog, Song};
pub use sorting::SongSort;
use std::sync::Arc;
pub use wardrobe::{ClothingThumbnail, WardrobeChange, WardrobeView};
use winit::{event::WindowEvent, window::Window};

#[derive(Default, Debug, PartialEq, Eq)]
pub enum Action {
    #[default]
    None,
    Play,
    Multiplayer(LobbyAction),
    Quit,
    CancelLoading,
    ReloadSources,
    BrowseAssets,
    Equip(WardrobeChange),
}

pub struct LoadingView {
    pub title: String,
    pub stage: String,
    pub progress: f32,
}
pub struct SongSelector {
    pub lobby: LobbyView,
    pub source_settings: sdo_asset_loader::sources::SourceSettings,
    pub source_reports: Vec<sdo_asset_loader::sources::SourceReport>,
    pub settings_open: bool,
    pub sources_loading: bool,
    pub source_status: String,
    pub scores: Arc<ScoreBook>,
    pub multiplayer_scores: Arc<ScoreBook>,
    pub catalog: Catalog,
    pub query: String,
    pub source: Option<String>,
    pub kind: String,
    pub difficulty: usize,
    pub sort: SongSort,
    pub sort_descending: bool,
    pub judgment: String,
    pub scroll_speed: sdo_config::ScrollSpeed,
    pub timing_offset_ms: f64,
    pub appearance: UiSelection,
    pub character: String,
    pub character_choices: Vec<UiChoice>,
    pub character_issues: Vec<String>,
    pub character_view: bool,
    pub character_yaw: f32,
    pub character_elevation: f32,
    pub character_zoom: f32,
    pub character_texture: Option<egui::TextureId>,
    pub wardrobe: WardrobeView,
    pub ui_choices: UiChoices,
    pub asset_issues: Vec<String>,
    pub error: Option<String>,
    pub loading: Option<LoadingView>,
    room_song_picker: room_song_picker::RoomSongPicker,
    visible: Vec<usize>,
    selected: Option<usize>,
    scroll_to_selected: bool,
}
impl SongSelector {
    pub fn new(
        catalog: Catalog,
        source: Option<String>,
        id: &str,
        kind: String,
        difficulty: usize,
    ) -> Result<Self> {
        let selected = catalog.songs.iter().position(|s| s.id == id);
        let source = source
            .map(|s| catalog.source_key(&s).map(str::to_owned))
            .transpose()?;
        let mut selector = Self {
            lobby: LobbyView::default(),
            source_settings: Default::default(),
            source_reports: Vec::new(),
            settings_open: false,
            timing_offset_ms: 0.,
            sources_loading: false,
            source_status: String::new(),
            scores: Arc::default(),
            multiplayer_scores: Arc::default(),
            catalog,
            query: String::new(),
            source,
            kind,
            difficulty,
            sort: SongSort::default(),
            sort_descending: false,
            judgment: "keyboard".into(),
            scroll_speed: sdo_config::ScrollSpeed::default(),
            character: String::new(),
            character_choices: Vec::new(),
            character_issues: Vec::new(),
            character_view: false,
            character_yaw: 0.0,
            character_elevation: 0.04,
            character_zoom: 1.0,
            character_texture: None,
            wardrobe: WardrobeView::default(),
            appearance: UiSelection {
                effect: "none".into(),
                ..Default::default()
            },
            ui_choices: UiChoices::default(),
            asset_issues: Vec::new(),
            error: None,
            loading: None,
            room_song_picker: Default::default(),
            visible: Vec::new(),
            selected,
            scroll_to_selected: true,
        };
        selector.refilter();
        Ok(selector)
    }
    pub fn timing_offset_seconds(&self) -> f64 {
        self.timing_offset_ms / 1000.
    }

    pub fn set_preview_assets(&mut self, assets: &UiAssets) {
        self.lobby.appearance_preview = Some(AppearancePreview::new(assets));
    }
    pub fn replace_catalog(&mut self, catalog: Catalog) {
        let selected = self.selected().map(|s| s.id.clone());
        self.catalog = catalog;
        self.selected = selected.and_then(|id| self.catalog.songs.iter().position(|s| s.id == id));
        if self
            .source
            .as_ref()
            .is_some_and(|s| !self.catalog.source_country_keys.contains_key(s))
        {
            self.source = None;
        }
        self.refilter();
    }
    /// Resolve a room chart without modifying the single-player browser.
    pub fn room_chart_song(&self, spec: &sdo_net::ChartSpec) -> Result<&Song> {
        self.catalog
            .songs
            .iter()
            .find(|song| {
                song.pattern(&spec.kind, 4)
                    .is_ok_and(|pattern| pattern.sha256 == spec.pattern_sha256)
            })
            .ok_or_else(|| anyhow::anyhow!("The room's exact chart file is missing locally"))
    }
    pub fn room_song_selection(&self) -> Option<&RoomSongSelection> {
        let room = self.lobby.room()?;
        self.lobby
            .song_selection
            .as_ref()
            .filter(|selection| selection.room == room.id)
    }
    /// Room scores follow the exact chart bytes, difficulty and timing rules across catalog aliases.
    pub fn room_personal_best(&self) -> Option<u64> {
        let chart = self.lobby.room()?.chart.as_ref()?;
        let key = ScoreKey::new(
            "",
            &chart.kind,
            chart.difficulty as usize,
            &chart.judgment,
            &chart.pattern_sha256,
        );
        self.multiplayer_scores
            .scores
            .iter()
            .filter(|record| {
                record.key.pattern_sha256 == key.pattern_sha256
                    && record.key.chart == key.chart
                    && record.key.difficulty == key.difficulty
                    && record.key.judgment == key.judgment
            })
            .map(|record| record.points)
            .max()
    }
    pub fn selected(&self) -> Option<&Song> {
        self.selected.map(|i| &self.catalog.songs[i])
    }
    pub fn refilter(&mut self) {
        let query = self.query.to_lowercase();
        let words: Vec<_> = query.split_whitespace().collect();
        self.visible = self
            .catalog
            .songs
            .iter()
            .enumerate()
            .filter(|(_, song)| {
                if song.pattern(&self.kind, 4).is_err()
                    || self
                        .source
                        .as_ref()
                        .is_some_and(|s| !song.sources.contains_key(s))
                {
                    return false;
                }
                let text = format!(
                    "{} {} {} {}",
                    song.id,
                    song.title,
                    song.artist,
                    song.sources
                        .iter()
                        .map(|(s, ids)| format!("{s}:{}", ids.id.as_slice().join("/")))
                        .collect::<Vec<_>>()
                        .join(" ")
                )
                .to_lowercase();
                words.iter().all(|word| text.contains(word))
            })
            .map(|(index, _)| index)
            .collect();
        self.sort_visible();
        if self
            .selected
            .is_none_or(|index| !self.visible.contains(&index))
        {
            self.selected = self.visible.first().copied();
        }
        self.error = None;
        self.scroll_to_selected = true;
    }
    pub fn move_by(&mut self, amount: isize) {
        if self.visible.is_empty() {
            return;
        }
        let pos = self
            .selected
            .and_then(|i| self.visible.iter().position(|&v| v == i))
            .unwrap_or(0);
        let pos = pos
            .saturating_add_signed(amount)
            .min(self.visible.len() - 1);
        self.selected = Some(self.visible[pos]);
        self.error = None;
        self.scroll_to_selected = true;
    }
    pub fn show(&mut self, ui: &mut egui::Ui, preview: &str, progress: (f64, f64)) -> Action {
        if let Some(error) = self.error.clone() {
            let response = egui::Modal::new(egui::Id::unique("app-error")).show(ui.ctx(), |ui| {
                ui.set_max_width(560.);
                ui.heading("Unable to continue");
                ui.add_space(12.);
                ui.label(error);
                ui.add_space(16.);
                ui.button(if self.lobby.open {
                    "Back to multiplayer"
                } else if self.character_view {
                    "Back to character"
                } else {
                    "Back to Single Player"
                })
                .clicked()
            });
            if response.inner || response.should_close() {
                self.error = None;
            }
            return Action::None;
        }
        if let Some(loading) = &self.loading {
            let mut cancelled =
                ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
            ui.vertical_centered(|ui| {
                ui.add_space((ui.available_height() * 0.32).max(30.));
                ui.heading("Loading song");
                ui.add_space(14.);
                ui.label(RichText::new(&loading.title).size(22.));
                ui.add_space(24.);
                ui.add(
                    egui::ProgressBar::new(loading.progress)
                        .desired_width(480.)
                        .desired_height(22.)
                        .show_percentage(),
                );
                ui.add_space(10.);
                ui.label(&loading.stage);
                ui.add_space(24.);
                cancelled |= ui.button("Cancel").clicked();
            });
            ui.ctx().request_repaint();
            return if cancelled {
                Action::CancelLoading
            } else {
                Action::None
            };
        }
        if self.lobby.open {
            return egui::Frame::new()
                .inner_margin(egui::Margin::symmetric(16, 10))
                .show(ui, |ui| self.show_multiplayer(ui))
                .inner;
        }
        egui::Frame::new()
            .fill(theme::SURFACE)
            .inner_margin(egui::Margin::symmetric(22, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    for (label, active) in [
                        (
                            "Single Player",
                            !self.lobby.open && !self.settings_open && !self.character_view,
                        ),
                        (
                            "Character",
                            self.character_view && !self.lobby.open && !self.settings_open,
                        ),
                        ("Settings", self.settings_open),
                        ("Multiplayer", self.lobby.open),
                    ] {
                        if ui
                            .add_sized(
                                [132., 36.],
                                egui::Button::selectable(
                                    active,
                                    RichText::new(label).size(16.).strong(),
                                ),
                            )
                            .clicked()
                        {
                            self.lobby.open = label == "Multiplayer";
                            if !self.lobby.open {
                                self.lobby.stop_walking();
                            }
                            self.settings_open = label == "Settings";
                            self.character_view = label == "Character";
                        }
                    }
                });
            });
        ui.separator();
        if self.lobby.open {
            ui.ctx().request_repaint();
            return Action::None;
        }
        if self.settings_open {
            return egui::Frame::new()
                .inner_margin(egui::Margin::symmetric(22, 14))
                .show(ui, |ui| self.show_sources(ui))
                .inner;
        }
        let mut action = Action::None;
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(22, 10))
            .show(ui, |ui| {
                if self.character_view {
                    if ui
                        .add_sized([170., 32.], egui::Button::new("← Single Player"))
                        .clicked()
                        || ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
                    {
                        self.character_view = false;
                    }
                    ui.add_space(12.);
                    action = self.show_character(ui);
                    return;
                }
                let mut changed = false;
                ui.horizontal(|ui| {
                    ui.spacing_mut().interact_size.y = 32.;
                    let search_width =
                        ui.available_width() - 132. - 56. - 114. - ui.spacing().item_spacing.x * 3.;
                    changed |= ui
                        .add_sized(
                            [search_width.max(160.), 32.0],
                            egui::TextEdit::singleline(&mut self.query)
                                .id(egui::Id::unique("song-search"))
                                .align(egui::Align2::LEFT_CENTER)
                                .hint_text("Search title, artist or ID…"),
                        )
                        .changed();
                    egui::ComboBox::from_id_salt("country")
                        .width(132.)
                        .selected_text(
                            self.source
                                .as_ref()
                                .and_then(|s| self.catalog.source_country_keys.get(s))
                                .map(String::as_str)
                                .unwrap_or("All regions"),
                        )
                        .show_ui(ui, |ui| {
                            changed |= ui
                                .selectable_value(&mut self.source, None, "All regions")
                                .changed();
                            for (key, label) in &self.catalog.source_country_keys {
                                changed |= ui
                                    .selectable_value(&mut self.source, Some(key.clone()), label)
                                    .changed();
                            }
                        });
                    if ui
                        .add_sized([56., 32.], egui::Button::new("Clear"))
                        .clicked()
                    {
                        self.query.clear();
                        changed = true;
                    }
                    if ui
                        .add_sized([114., 32.], egui::Button::new("Character →"))
                        .clicked()
                    {
                        self.character_view = true;
                    }
                });
                if changed {
                    self.refilter();
                }
                if self.character_view {
                    ui.add_space(12.);
                    action = self.show_character(ui);
                    return;
                }
                // Arrow navigation works while the search field has focus, too.
                for (key, delta) in [
                    (egui::Key::ArrowDown, 1),
                    (egui::Key::ArrowUp, -1),
                    (egui::Key::PageDown, 8),
                    (egui::Key::PageUp, -8),
                ] {
                    if ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, key)) {
                        self.move_by(delta);
                    }
                }
                if ui.input(|i| i.key_pressed(egui::Key::Enter)) && self.selected.is_some() {
                    action = Action::Play;
                }
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    action = Action::Quit;
                }
                ui.add_space(8.0);
                let order_changed = self.library_controls(ui);
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(8.0);
                let available = ui.available_size();
                let spacing = ui.spacing().item_spacing;
                let column_width = available.x - 16.;
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 16.;
                    ui.allocate_ui_with_layout(
                        egui::vec2(column_width * 0.54, available.y),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.spacing_mut().item_spacing = spacing;
                            let row_height = 64.0;
                            song_header(ui);
                            let mut scroll = egui::ScrollArea::vertical()
                                .id_salt("songs")
                                .auto_shrink([false, false]);
                            if order_changed {
                                scroll = scroll.vertical_scroll_offset(0.);
                                self.scroll_to_selected = false;
                            } else if self.scroll_to_selected {
                                if let Some(pos) = self
                                    .selected
                                    .and_then(|index| self.visible.iter().position(|&i| i == index))
                                {
                                    scroll = scroll.vertical_scroll_offset(
                                        pos.saturating_sub(3) as f32
                                            * (row_height + ui.spacing().item_spacing.y),
                                    );
                                }
                                self.scroll_to_selected = false;
                            }
                            scroll.show_rows(ui, row_height, self.visible.len(), |ui, range| {
                                for row in range {
                                    let index = self.visible[row];
                                    let song = &self.catalog.songs[index];
                                    let response = song_row(
                                        ui,
                                        song,
                                        &self.kind,
                                        self.difficulty,
                                        self.selected == Some(index),
                                        row_height,
                                        row,
                                    );
                                    if response.clicked() {
                                        self.selected = Some(index);
                                        self.error = None;
                                    }
                                    if response.double_clicked() {
                                        action = Action::Play;
                                    }
                                }
                            });
                        },
                    );
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing = spacing;
                        ui.set_width(column_width * 0.46);
                        theme::panel(12).show(ui, |ui| {
                            egui::ScrollArea::vertical()
                                .id_salt("song-details")
                                .max_height((ui.available_height() - 52.0 - spacing.y).max(120.0))
                                .show(ui, |ui| {
                                    self.song_details(ui, preview, progress);
                                });
                            ui.add_space(6.0);
                            if theme::primary(
                                ui,
                                RichText::new("PLAY  →").size(17.),
                                [ui.available_width(), 44.],
                                self.selected.is_some(),
                            )
                            .clicked()
                            {
                                action = Action::Play;
                            }
                        });
                    });
                });
            });
        action
    }
    fn song_details(&mut self, ui: &mut egui::Ui, preview: &str, progress: (f64, f64)) {
        ui.spacing_mut().item_spacing.y = 3.;
        if let Some(song) = self.selected() {
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(format!("#{}", song.id)).color(theme::MUTED));
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.add(
                            egui::Label::new(RichText::new(&song.title).size(24.).strong())
                                .truncate(),
                        )
                        .on_hover_text(&song.title);
                    });
                });
            });
            ui.add(
                egui::Label::new(RichText::new(&song.artist).size(15.0).color(theme::MUTED))
                    .truncate(),
            )
            .on_hover_text(&song.artist);
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                let pattern = song.pattern(&self.kind, 4).ok();
                let bpm = pattern.and_then(|p| p.bpm).unwrap_or(song.bpm);
                ui.label(format!(
                    "{bpm:.2} BPM   ·   {}:{:02}   ·   4 KEY",
                    song.duration as u64 / 60,
                    song.duration as u64 % 60
                ));
                let level = pattern.and_then(|p| p.levels[self.difficulty]);
                let response = ui.label(
                    RichText::new(match level {
                        Some(level) => format!(
                            "Level {level} · {}",
                            ["Easy", "Normal", "Hard"][self.difficulty]
                        ),
                        None => "Level unavailable".into(),
                    })
                    .color(theme::ACCENT)
                    .strong(),
                );
                if let Some(error) = pattern.and_then(|p| p.level_error.as_deref()) {
                    response.on_hover_text(error);
                }
            });
            let regions = song
                .sources
                .keys()
                .map(|key| self.catalog.source_country_keys[key].as_str())
                .collect::<Vec<_>>()
                .join(" · ");
            ui.add(
                egui::Label::new(RichText::new(&regions).size(11.0).color(theme::MUTED)).truncate(),
            )
            .on_hover_text(regions);
            ui.add_space(6.0);
            let fraction = if progress.1 > 0.0 {
                (progress.0 / progress.1) as f32
            } else {
                0.0
            };
            let label = if progress.1 > 0.0 {
                format!("Preview  {:.0} / {:.0} s", progress.0, progress.1)
            } else {
                preview.to_owned()
            };
            ui.add(
                egui::ProgressBar::new(fraction)
                    .text(label)
                    .desired_height(16.0),
            )
            .on_hover_text(preview);
        } else {
            ui.heading("No matching songs");
            ui.label("Try another title, artist or ID.");
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| self.timing_controls(ui));
        if let Some(song) = self.selected() {
            let key = ScoreKey::new(
                &song.id,
                &self.kind,
                self.difficulty,
                &self.judgment,
                song.pattern(&self.kind, 4)
                    .map_or("", |p| p.sha256.as_str()),
            );
            ui.add_space(6.);
            ui.horizontal_wrapped(|ui| {
                if let Some(best) = self.scores.best(&key) {
                    ui.label(
                        RichText::new(format!("Best score  {}", best.points))
                            .color(theme::ACCENT)
                            .strong(),
                    );
                    if let Some(last) = self.scores.last(&key) {
                        ui.label(
                            RichText::new(format!(
                                "Last score  {}   ·   Best combo  {}",
                                last.points, best.max_combo
                            ))
                            .size(12.)
                            .color(theme::MUTED),
                        );
                    }
                } else {
                    ui.label(RichText::new("No completed score yet").color(theme::MUTED));
                }
            });
        }
        ui.add_space(6.0);
        ui.separator();
        ui.label(RichText::new("Appearance").strong());
        if self.lobby.appearance_preview.is_some() && ui.available_width() >= 330. {
            ui.columns(2, |columns| {
                self.appearance_choices(&mut columns[0]);
                if let Some(preview) = &mut self.lobby.appearance_preview {
                    preview.show(&mut columns[1], &self.appearance);
                }
            });
        } else {
            self.appearance_choices(ui);
            if let Some(preview) = &mut self.lobby.appearance_preview {
                preview.show(ui, &self.appearance);
            }
        }
        if !self.asset_issues.is_empty() {
            ui.collapsing(
                RichText::new(format!(
                    "Unavailable UI assets ({})",
                    self.asset_issues.len()
                ))
                .color(Color32::LIGHT_RED),
                |ui| {
                    for issue in &self.asset_issues {
                        ui.colored_label(Color32::LIGHT_RED, issue);
                    }
                },
            );
        }
        if let Some(error) = &self.error {
            ui.add_space(10.0);
            ui.colored_label(Color32::LIGHT_RED, error);
        }
    }

    fn appearance_choices(&mut self, ui: &mut egui::Ui) {
        asset_choice(
            ui,
            "Note skin",
            &mut self.appearance.skin,
            &self.ui_choices.skins,
            false,
        );
        asset_choice(
            ui,
            "Note effect",
            &mut self.appearance.effect,
            &self.ui_choices.effects,
            true,
        );
    }

    fn library_controls(&mut self, ui: &mut egui::Ui) -> bool {
        let old_kind = self.kind.clone();
        let old_difficulty = self.difficulty;
        let old_sort = (self.sort, self.sort_descending);
        let levels = self
            .selected()
            .and_then(|song| song.pattern(&self.kind, 4).ok())
            .map(|pattern| pattern.levels)
            .unwrap_or([None; 3]);
        ui.horizontal(|ui| {
            ui.spacing_mut().interact_size.y = 28.;
            ui.label(format!("{} songs", self.visible.len()));
            ui.add_space(12.);
            for (i, name) in ["Easy", "Normal", "Hard"].into_iter().enumerate() {
                let response = ui.add(
                    egui::Button::new(name)
                        .selected(self.difficulty == i)
                        .min_size(egui::vec2(64., 28.)),
                );
                if response.clicked() {
                    self.difficulty = i;
                }
                response.on_hover_text(levels[i].map_or_else(
                    || "Level unavailable for the selected song".into(),
                    |level| format!("Level {level} for the selected song"),
                ));
            }
            ui.add_space(12.);
            for (kind, label) in [("t", "Dancemat"), ("k", "Keyboard")] {
                if ui
                    .add(
                        egui::Button::new(label)
                            .selected(self.kind == kind)
                            .min_size(egui::vec2(92., 28.)),
                    )
                    .clicked()
                {
                    self.kind = kind.into();
                }
            }
            ui.add_space(12.);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_sized(
                        [28., 28.],
                        egui::Button::new(if self.sort_descending { "↓" } else { "↑" }),
                    )
                    .on_hover_text(if self.sort_descending {
                        "Descending; click for ascending"
                    } else {
                        "Ascending; click for descending"
                    })
                    .clicked()
                {
                    self.sort_descending = !self.sort_descending;
                }
                egui::ComboBox::from_id_salt("song-sort")
                    .width(106.)
                    .selected_text(self.sort.label())
                    .show_ui(ui, |ui| {
                        for sort in SongSort::ALL {
                            ui.selectable_value(&mut self.sort, sort, sort.label());
                        }
                    });
                ui.label("Sort");
            });
        });
        let order_changed = old_kind != self.kind
            || old_sort != (self.sort, self.sort_descending)
            || (self.sort == SongSort::Level && old_difficulty != self.difficulty);
        if old_kind != self.kind {
            self.refilter();
        } else if order_changed {
            self.sort_visible();
        }
        order_changed
    }
    fn timing_controls(&mut self, ui: &mut egui::Ui) {
        ui.label("Timing");
        egui::ComboBox::from_id_salt("judgment")
            .width(95.)
            .selected_text(
                sdo_gameplay::judgment::find(&self.judgment)
                    .map(|p| p.name)
                    .unwrap_or("Keyboard"),
            )
            .show_ui(ui, |ui| {
                for preset in sdo_gameplay::judgment::PRESETS {
                    ui.selectable_value(&mut self.judgment, preset.id.to_owned(), preset.name);
                }
            });
        ui.label("Speed");
        egui::ComboBox::from_id_salt("scroll-speed")
            .width(56.)
            .selected_text(format!("{}x", self.scroll_speed.value()))
            .show_ui(ui, |ui| {
                for value in 1..=8 {
                    ui.selectable_value(
                        &mut self.scroll_speed,
                        sdo_config::ScrollSpeed::new(value).unwrap(),
                        format!("{value}x"),
                    );
                }
            });
    }
}

fn song_columns(width: f32) -> [f32; 5] {
    [12., 70., width - 148., width - 95., width - 48.]
}

fn song_header(ui: &mut egui::Ui) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 24.), egui::Sense::hover());
    for (x, label) in
        song_columns(rect.width())
            .into_iter()
            .zip(["ID", "SONG / ARTIST", "LEVEL", "BPM", "TIME"])
    {
        ui.painter().text(
            rect.left_center() + egui::vec2(x, 0.),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(11.),
            theme::MUTED,
        );
    }
}

fn song_row(
    ui: &mut egui::Ui,
    song: &Song,
    kind: &str,
    difficulty: usize,
    selected: bool,
    height: f32,
    index: usize,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::click(),
    );
    let [id_x, name_x, level_x, bpm_x, time_x] = song_columns(rect.width());
    let background = if selected {
        theme::SELECTED
    } else if response.hovered() {
        Color32::from_rgb(31, 43, 60)
    } else if index.is_multiple_of(2) {
        Color32::from_rgb(22, 30, 44)
    } else {
        Color32::from_rgb(18, 26, 39)
    };
    let painter = ui.painter();
    painter.rect_filled(rect, 6., background);
    if selected {
        painter.rect_filled(
            egui::Rect::from_min_size(rect.min + egui::vec2(0., 8.), egui::vec2(3., height - 16.)),
            2.,
            theme::ACCENT,
        );
    }
    let muted = theme::MUTED;
    painter.text(
        rect.min + egui::vec2(id_x, height * 0.5),
        egui::Align2::LEFT_CENTER,
        &song.id,
        egui::FontId::proportional(12.),
        if selected { theme::ACCENT } else { muted },
    );
    let title_width = (level_x - name_x - 10.).max(1.);
    for (label, y, size, color) in [
        (
            song.title.as_str(),
            11.,
            16.,
            Color32::from_rgb(231, 238, 247),
        ),
        (song.artist.as_str(), 36., 12., muted),
    ] {
        let mut job = egui::text::LayoutJob::simple_singleline(
            label.to_owned(),
            egui::FontId::proportional(size),
            color,
        );
        job.wrap.max_width = title_width;
        job.wrap.max_rows = 1;
        job.wrap.break_anywhere = true;
        job.wrap.overflow_character = Some('…');
        let galley = painter.layout_job(job);
        painter.galley(rect.min + egui::vec2(name_x, y), galley, color);
    }
    let pattern = song.pattern(kind, 4).ok();
    let level = pattern.and_then(|pattern| pattern.levels[difficulty]);
    painter.text(
        rect.min + egui::vec2(level_x, height * 0.5),
        egui::Align2::LEFT_CENTER,
        level.map_or_else(|| "—".into(), |level| level.to_string()),
        egui::FontId::proportional(13.),
        theme::ACCENT,
    );
    let bpm = pattern
        .map(|p| p.bpm.unwrap_or(song.bpm))
        .unwrap_or(song.bpm);
    painter.text(
        rect.min + egui::vec2(bpm_x, height * 0.5),
        egui::Align2::LEFT_CENTER,
        format!("{bpm:.0}"),
        egui::FontId::proportional(12.),
        muted,
    );
    painter.text(
        rect.min + egui::vec2(time_x, height * 0.5),
        egui::Align2::LEFT_CENTER,
        format!(
            "{}:{:02}",
            song.duration as u64 / 60,
            song.duration as u64 % 60
        ),
        egui::FontId::proportional(12.),
        muted,
    );
    response.on_hover_text(format!("{}\n{}", song.title, song.artist))
}

fn asset_choice(
    ui: &mut egui::Ui,
    label: &str,
    selected: &mut String,
    choices: &[UiChoice],
    allow_none: bool,
) {
    let text = choices
        .iter()
        .find(|c| c.id == *selected)
        .map(|c| c.name.as_str())
        .unwrap_or(if allow_none && selected == "none" {
            "None"
        } else {
            "Unavailable"
        });
    let mut combo = |ui: &mut egui::Ui| {
        let width = ui.available_width();
        ui.add_enabled_ui(!choices.is_empty() || allow_none, |ui| {
            egui::ComboBox::from_id_salt(("ui-asset", label))
                .width(width)
                .truncate()
                .selected_text(text)
                .show_ui(ui, |ui| {
                    if allow_none {
                        ui.selectable_value(selected, "none".into(), "None");
                    }
                    for choice in choices {
                        ui.selectable_value(selected, choice.id.clone(), &choice.name);
                    }
                });
        });
    };
    if ui.available_width() < 216. {
        ui.label(label);
        combo(ui);
    } else {
        ui.horizontal(|ui| {
            ui.add_sized([80., 20.], egui::Label::new(label));
            combo(ui);
        });
    }
}

/// The selector uses the same wgpu device and window as the note display.
pub struct SelectorUi {
    context: egui::Context,
    input: Option<egui_winit::State>,
    renderer: sdo_render::gui::GuiRenderer,
}
impl SelectorUi {
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        window: Option<&Window>,
    ) -> Self {
        let context = egui::Context::default();
        theme::apply(&context);
        // Read system fonts in place; no new client assets are imported into assets/.
        let mut fonts = egui::FontDefinitions::default();
        for (name, candidates) in [
            (
                "cjk",
                &[
                    "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
                    "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
                    "C:/Windows/Fonts/msyh.ttc",
                ][..],
            ),
            (
                "thai",
                &[
                    "/System/Library/Fonts/Supplemental/Thonburi.ttc",
                    "/usr/share/fonts/truetype/tlwg/garuda.ttf",
                ][..],
            ),
        ] {
            if let Some(bytes) = candidates.iter().find_map(|path| std::fs::read(path).ok()) {
                fonts
                    .font_data
                    .insert(name.into(), egui::FontData::from_owned(bytes).into());
                fonts
                    .families
                    .entry(egui::FontFamily::Proportional)
                    .or_default()
                    .push(name.into());
            }
        }
        context.set_fonts(fonts);
        let input = window.map(|window| {
            egui_winit::State::new(
                context.clone(),
                egui::ViewportId::ROOT,
                window,
                Some(window.scale_factor() as f32),
                window.theme(),
                Some(device.limits().max_texture_dimension_2d as usize),
            )
        });
        let renderer = sdo_render::gui::GuiRenderer::new(device, format);
        Self {
            context,
            input,
            renderer,
        }
    }
    pub fn register_character_preview(
        &mut self,
        device: &wgpu::Device,
        view: &wgpu::TextureView,
    ) -> egui::TextureId {
        self.renderer.register(device, view)
    }
    pub fn unregister_preview(&mut self, id: egui::TextureId) {
        self.renderer.unregister(id);
    }
    pub fn on_event(&mut self, window: &Window, event: &WindowEvent) {
        if let Some(input) = &mut self.input {
            let _ = input.on_window_event(window, event);
        }
    }
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        size: [u32; 2],
        window: Option<&Window>,
        selector: &mut SongSelector,
        status: &str,
        progress: (f64, f64),
    ) -> Action {
        let raw = match (&mut self.input, window) {
            (Some(input), Some(window)) => input.take_egui_input(window),
            _ => egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(size[0] as f32, size[1] as f32),
                )),
                ..Default::default()
            },
        };
        let mut action = Action::None;
        let output = self.context.run_ui(raw, |ui| {
            action = selector.show(ui, status, progress);
        });
        self.paint_output(device, queue, view, size, window, output, false);
        action
    }
    #[allow(clippy::too_many_arguments)]
    fn paint_output(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        size: [u32; 2],
        window: Option<&Window>,
        mut output: egui::FullOutput,
        overlay: bool,
    ) {
        if let (Some(input), Some(window)) = (&mut self.input, window) {
            input.handle_platform_output(window, std::mem::take(&mut output.platform_output));
        }
        self.renderer
            .paint(device, queue, view, size, &self.context, output, overlay);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    pub(super) fn selector() -> SongSelector {
        let song = |id: &str, title: &str, source: &str| {
            serde_json::json!({"id":id,"title":title,"artist":"音楽","duration":100.,"audio":format!("music/{id}.ogg"),
            "preview":{"path":format!("music/{id}_preview.ogg"),"duration":14.},"bpm":120.,"patterns":[{"file":format!("music/{id}.gn"),"type":"K","mode":4}],
            "sources":{source:{"id":"5073"}},"source":source})
        };
        let mut catalog = Catalog::parse(&serde_json::to_vec(&serde_json::json!({"duration_unit":"seconds","source_country_keys":{"sdox":"SDO-X","usa":"USA"},
            "songs":[song("00001","Holy Knight","sdox"),song("00002","Cynthia No Hikari","sdox"),song("00003","Other","usa")]})).unwrap()).unwrap();
        for pattern in catalog.songs.iter_mut().flat_map(|s| &mut s.patterns) {
            pattern.sha256 = "a".repeat(64);
        }
        SongSelector::new(catalog, None, "00001", "k".into(), 0).unwrap()
    }
    #[test]
    fn filtering_and_navigation_preserve_global_identity_and_each_preview() {
        let mut s = selector();
        s.move_by(1);
        assert_eq!(s.selected().unwrap().id, "00002");
        assert_eq!(
            s.selected().unwrap().preview.as_ref().unwrap().path,
            "music/00002_preview.ogg"
        );
        s.query = "holy 音楽".into();
        s.refilter();
        assert_eq!(s.selected().unwrap().id, "00001");
        s.query = "no matches".into();
        s.refilter();
        assert!(s.selected().is_none());
        s.move_by(1);
        assert!(s.selected().is_none());
        s.query.clear();
        s.source = Some("usa".into());
        s.refilter();
        assert_eq!(s.selected().unwrap().id, "00003");
    }
    #[test]
    fn navigation_clamps_at_edges_and_searches_original_ids() {
        let mut s = selector();
        s.move_by(-100);
        assert_eq!(s.selected().unwrap().id, "00001");
        s.move_by(100);
        assert_eq!(s.selected().unwrap().id, "00003");
        s.query = "5073".into();
        s.refilter();
        assert_eq!(s.visible.len(), 3);
    }
    pub(super) fn frame(
        context: &egui::Context,
        selector: &mut SongSelector,
        events: Vec<egui::Event>,
    ) -> Action {
        frame_output(context, selector, events).0
    }
    pub(super) fn frame_output(
        context: &egui::Context,
        selector: &mut SongSelector,
        events: Vec<egui::Event>,
    ) -> (Action, egui::FullOutput) {
        frame_at_size(context, selector, events, egui::vec2(1100., 760.))
    }

    pub(super) fn frame_at_size(
        context: &egui::Context,
        selector: &mut SongSelector,
        events: Vec<egui::Event>,
        size: egui::Vec2,
    ) -> (Action, egui::FullOutput) {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            events,
            ..Default::default()
        };
        let mut action = Action::None;
        let mut output = context.run_ui(raw, |ui| {
            action = selector.show(ui, "Preview", (1.0, 14.0));
        });
        output.textures_delta.clear();
        (action, output)
    }
    fn key(key: egui::Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        }
    }
    pub(super) fn text_center(output: &egui::FullOutput, text: &str) -> Option<egui::Pos2> {
        output.shapes.iter().find_map(|s| match &s.shape {
            egui::epaint::Shape::Text(t) if t.galley.text() == text => {
                Some(t.pos + t.galley.size() * 0.5)
            }
            _ => None,
        })
    }
    #[test]
    fn selected_scores_follow_song_difficulty_and_timing() {
        let mut s = selector();
        let context = egui::Context::default();
        let key = ScoreKey::new("00001", "k", 0, "keyboard", &"a".repeat(64));
        let book = Arc::make_mut(&mut s.scores);
        for points in [12000, 8000] {
            book.scores.push(sdo_config::ScoreRecord {
                key: key.clone(),
                points,
                max_combo: 12,
                judgments: [12, 0, 0, 0],
                achieved_at_unix: 0,
            });
        }
        frame(&context, &mut s, vec![]);
        let output = frame_output(&context, &mut s, vec![]).1;
        assert!(text_center(&output, "Best score  12000").is_some());
        assert!(text_center(&output, "Last score  8000   ·   Best combo  12").is_some());
        s.difficulty = 1;
        let output = frame_output(&context, &mut s, vec![]).1;
        assert!(text_center(&output, "No completed score yet").is_some());
        s.difficulty = 0;
        s.judgment = "pad".into();
        let output = frame_output(&context, &mut s, vec![]).1;
        assert!(text_center(&output, "No completed score yet").is_some());
        s.judgment = "keyboard".into();
        s.move_by(1);
        let output = frame_output(&context, &mut s, vec![]).1;
        assert!(text_center(&output, "No completed score yet").is_some());
    }
    #[test]
    fn displayed_levels_follow_selected_chart_and_difficulty() {
        fn click(context: &egui::Context, selector: &mut SongSelector, label: &str) {
            let output = frame_output(context, selector, vec![]).1;
            let pos = text_center(&output, label).expect("Selector button must be visible");
            for pressed in [true, false] {
                frame(
                    context,
                    selector,
                    vec![
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: Default::default(),
                        },
                    ],
                );
            }
        }
        let mut s = selector();
        s.catalog.songs[0].patterns[0].levels = [Some(3), Some(4), Some(5)];
        let mut other: sdo_song::catalog::Pattern = serde_json::from_value(
            serde_json::json!({"file":"music/00001T.gn","type":"T","mode":4}),
        )
        .unwrap();
        other.levels = [Some(1), Some(3), Some(8)];
        s.catalog.songs[0].patterns.push(other);
        let context = egui::Context::default();
        frame(&context, &mut s, vec![]);
        let output = frame_output(&context, &mut s, vec![]).1;
        assert!(text_center(&output, "LEVEL").is_some());
        assert!(text_center(&output, "Level 3 · Easy").is_some());
        click(&context, &mut s, "Hard");
        assert_eq!(s.difficulty, 2);
        let output = frame_output(&context, &mut s, vec![]).1;
        assert!(text_center(&output, "Level 5 · Hard").is_some());
        click(&context, &mut s, "Dancemat");
        assert_eq!(s.kind, "t");
        let output = frame_output(&context, &mut s, vec![]).1;
        assert!(text_center(&output, "Level 8 · Hard").is_some());
        click(&context, &mut s, "Keyboard");
        assert_eq!(s.kind, "k");
        s.move_by(1);
        let output = frame_output(&context, &mut s, vec![]).1;
        assert!(text_center(&output, "Level unavailable").is_some());
    }
    #[test]
    fn loading_and_error_dialogs_block_play_and_allow_return_to_the_list() {
        let mut s = selector();
        let context = egui::Context::default();
        s.loading = Some(LoadingView {
            title: "Example".into(),
            stage: "Decoding audio".into(),
            progress: 0.42,
        });
        assert_eq!(
            frame(&context, &mut s, vec![key(egui::Key::Enter)]),
            Action::None
        );
        assert_eq!(
            frame(&context, &mut s, vec![key(egui::Key::Escape)]),
            Action::CancelLoading
        );
        s.loading = None;
        s.error = Some("Could not load this song: missing chart.gn".into());
        frame(&context, &mut s, vec![]);
        assert_eq!(
            frame(&context, &mut s, vec![key(egui::Key::Enter)]),
            Action::None
        );
        assert!(s.error.is_some());
        frame(&context, &mut s, vec![key(egui::Key::Escape)]);
        assert!(s.error.is_none());
        assert_eq!(s.selected().unwrap().id, "00001");
    }

    #[test]
    fn keyboard_navigation_with_search_focused_and_enter_to_play() {
        let mut s = selector();
        let context = egui::Context::default();
        frame(&context, &mut s, vec![]);
        context.memory_mut(|m| m.request_focus(egui::Id::unique("song-search")));
        frame(&context, &mut s, vec![key(egui::Key::ArrowDown)]);
        assert_eq!(s.selected().unwrap().id, "00002");
        assert_eq!(
            frame(&context, &mut s, vec![key(egui::Key::Enter)]),
            Action::Play
        );
        assert_eq!(
            frame(&context, &mut s, vec![key(egui::Key::Escape)]),
            Action::Quit
        );
    }
    #[test]
    fn clicking_a_song_changes_selection_and_enter_does_nothing_for_empty_results() {
        let mut s = selector();
        let context = egui::Context::default();
        for _ in 0..2 {
            frame(&context, &mut s, vec![]);
        }
        let output = frame_output(&context, &mut s, vec![]).1;
        let pos = text_center(&output, "00002").expect("Second song must be visible");
        for pressed in [true, false] {
            frame(
                &context,
                &mut s,
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
            );
        }
        assert_eq!(s.selected().unwrap().id, "00002");
        s.query = "nothing matches this".into();
        s.refilter();
        assert_eq!(
            frame(&context, &mut s, vec![key(egui::Key::Enter)]),
            Action::None
        );
    }
    #[test]
    fn character_presets_change_via_the_ui_and_back_preserves_song_selection() {
        let mut s = selector();
        s.character = "male".into();
        s.character_view = true;
        s.character_choices = vec![
            UiChoice {
                id: "male".into(),
                name: "Default male".into(),
            },
            UiChoice {
                id: "female".into(),
                name: "Default female".into(),
            },
        ];
        let ctx = egui::Context::default();
        for _ in 0..2 {
            frame(&ctx, &mut s, vec![]);
        }
        assert_eq!(
            frame(&ctx, &mut s, vec![key(egui::Key::Enter)]),
            Action::None
        );
        assert_eq!(s.selected().unwrap().id, "00001");
        fn click(ctx: &egui::Context, s: &mut SongSelector, pos: egui::Pos2) -> egui::FullOutput {
            let mut output = egui::FullOutput::default();
            for pressed in [true, false] {
                output = frame_output(
                    ctx,
                    s,
                    vec![
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: Default::default(),
                        },
                    ],
                )
                .1;
            }
            output
        }
        let output = frame_output(&ctx, &mut s, vec![]).1;
        let pos = text_center(&output, "Default male").expect("Current preset must be visible");
        click(&ctx, &mut s, pos);
        let output = frame_output(&ctx, &mut s, vec![]).1;
        let pos = text_center(&output, "Default female")
            .expect("Preset dropdown must offer both presets");
        click(&ctx, &mut s, pos);
        assert_eq!(s.character, "female");
        assert_eq!(
            frame(&ctx, &mut s, vec![key(egui::Key::Escape)]),
            Action::None
        );
        assert!(!s.character_view);
        assert_eq!(s.character, "female");
        assert_eq!(s.selected().unwrap().id, "00001");
    }
}
