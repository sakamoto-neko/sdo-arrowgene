//! Window/event orchestration. Assets arrive only through AssetLoader.
use crate::{
    cli::Options,
    scores::ScoreStore,
    session::{PlaySession, ScoreSaveState, TailClock},
    settings::SettingsStore,
    wardrobe::WardrobeThumbnails,
};
use anyhow::{Context, Result};
use sdo_asset_loader::{
    AssetLoader, LoadRequest, PreparedSong, PreviewPlayer, SongAssetLoader, request as asset,
};
use sdo_assets::{CharacterAssets, FourKeySkin, UiAssets, UiSelection};
use sdo_audio::AudioPlayer;
use sdo_config::ScoreKey;
use sdo_dance::DancePlayer;
use sdo_gameplay::input::{Binding, InputState};
use sdo_render::{
    Renderer, Sprite,
    character::{CharacterCamera, CharacterRenderer},
};
use sdo_song::{Catalog, Difficulty};
use sdo_ui::notes::NoteDisplay;
use sdo_ui::{Action, SelectorUi, SongSelector};
use std::sync::Arc;
use winit::{
    application::ApplicationHandler,
    event::{ElementState, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow},
    keyboard::{KeyCode, PhysicalKey},
    window::{Window, WindowId},
};
#[cfg(not(target_arch = "wasm32"))]
use winit::{dpi::LogicalSize, event_loop::EventLoop};

pub(crate) struct WindowState {
    pub(crate) surface: sdo_render::surface::DisplaySurface,
    pub(crate) window: Arc<Window>,
    frame_intervals: Vec<f64>,
    last_present: Option<sdo_platform::Instant>,
    pub(crate) renderer: Renderer,
    pub(crate) selector_ui: SelectorUi,
    pub(crate) character_renderer: CharacterRenderer,
    pub(crate) multiplayer_stage: sdo_render::character::stage::StageRenderer,
    pub(crate) stage_texture: sdo_assets::TextureId,
    pub(crate) wardrobe_thumbnails: WardrobeThumbnails,
    pub(crate) character_texture: sdo_assets::TextureId,
    pub(crate) occluded: bool,
    pub(crate) rendered: u64,
}
pub(crate) struct PlaySelection {
    pub(crate) network: Option<crate::multiplayer::LoadPurpose>,
    pub(crate) score_key: ScoreKey,
    pub(crate) title: String,
}
pub(crate) struct App {
    pub(crate) network_wake: Option<winit::event_loop::EventLoopProxy<()>>,
    pub(crate) online: crate::multiplayer::Online,
    #[cfg(target_arch = "wasm32")]
    pending_outfit: Option<std::sync::mpsc::Receiver<Result<Arc<CharacterAssets>>>>,
    pub(crate) source_reload: Option<std::sync::mpsc::Receiver<crate::sources::SourceEvent>>,
    pub(crate) content: AssetLoader,
    pub(crate) options: Options,
    pub(crate) skin: FourKeySkin,
    pub(crate) selector: SongSelector,
    pub(crate) active_ui: UiSelection,
    pub(crate) settings: SettingsStore,
    pub(crate) scores: ScoreStore,
    pub(crate) multiplayer_scores: ScoreStore,
    pub(crate) characters: Arc<CharacterAssets>,
    pub(crate) loader: SongAssetLoader<PlaySelection>,
    pub(crate) prepared_song: Option<PreparedSong<PlaySelection>>,
    pub(crate) loading_frame_presented: bool,
    pub(crate) preview_audio: PreviewPlayer,
    pub(crate) preview_id: Option<String>,
    pub(crate) session: Option<PlaySession>,
    pub(crate) state: Option<WindowState>,
    pub(crate) focused: bool,
    pub(crate) fatal: Option<anyhow::Error>,
}
impl App {
    pub(crate) fn new(
        content: AssetLoader,
        mut options: Options,
        catalog: Catalog,
        assets: Arc<UiAssets>,
        session: Option<PlaySession>,
        mut settings: SettingsStore,
        (mut scores, mut multiplayer_scores): (ScoreStore, ScoreStore),
    ) -> Result<Self> {
        settings.apply_options(&mut options, &catalog);
        let id = if let Some(id) = options.song.as_deref() {
            catalog
                .select(id, options.music_region.as_deref(), &options.chart, 4)?
                .song
                .id
                .clone()
        } else if let Some(id) = settings.values.selected_song.as_deref()
            && catalog
                .select(id, options.music_region.as_deref(), &options.chart, 4)
                .is_ok()
        {
            id.to_owned()
        } else {
            catalog
                .songs_for_source(options.music_region.as_deref())?
                .find(|song| song.pattern(&options.chart, 4).is_ok())
                .map(|song| song.id.clone())
                .unwrap_or_default()
        };
        let mut selector = SongSelector::new(
            catalog,
            options.music_region.clone(),
            &id,
            options.chart.clone(),
            options.difficulty.index(),
        )?;
        selector.source_settings = settings.values.asset_paths.clone().unwrap_or_default();
        selector.remote_override = options.remote_asset_url.clone();
        selector.settings_open = catalog_is_empty_or_incomplete(&selector.catalog, &assets);
        selector.timing_offset_ms = options.offset * 1000.;
        selector.scroll_speed = options.speed.unwrap_or_default();
        selector.lobby.scroll_speed = settings.values.multiplayer_scroll_speed;
        let mut characters = content.request(asset::Characters(&Default::default()))?;
        if !settings.values.outfits.is_empty() {
            match content.request(asset::Characters(&settings.values.outfits)) {
                Ok(restored) => characters = restored,
                Err(error) => settings
                    .issues
                    .push(format!("Cannot restore outfits: {error:#}")),
            }
        }
        let loader = SongAssetLoader::new(content.clone());
        for issue in &characters.issues {
            eprintln!("Unavailable character: {issue}");
        }
        selector.character_choices = characters.choices();
        selector.wardrobe.sync(&characters);
        selector.character_issues = characters.issues.iter().map(ToString::to_string).collect();
        selector.character = if let Some(id) = options.character.as_deref() {
            characters.preset(id)?.choice.id.clone()
        } else {
            settings
                .values
                .character
                .as_deref()
                .filter(|_| !characters.presets.is_empty())
                .and_then(|id| match characters.preset(id) {
                    Ok(preset) => Some(preset.choice.id.clone()),
                    Err(error) => {
                        settings
                            .issues
                            .push(format!("Cannot restore character: {error:#}"));
                        None
                    }
                })
                .or_else(|| {
                    characters
                        .presets
                        .iter()
                        .find(|p| p.choice.id == "male")
                        .or(characters.presets.first())
                        .map(|p| p.choice.id.clone())
                })
                .unwrap_or_default()
        };
        selector.character_view = options.character_view;
        selector.judgment = sdo_gameplay::judgment::find(&options.preset)
            .unwrap()
            .id
            .into();
        let active_ui = settings.appearance(&options, &assets)?;
        let skin = content.request(asset::Skin(&active_ui))?;
        selector.appearance = active_ui.clone();
        selector.ui_choices = assets.choices();
        selector.set_preview_assets(&assets);
        selector.asset_issues = assets.issues.iter().map(ToString::to_string).collect();
        selector.scores = scores.book.clone();
        selector.multiplayer_scores = multiplayer_scores.book.clone();
        if let Some(error) = multiplayer_scores.error.take() {
            settings.issues.push(error);
        }
        if let Some(error) = scores.error.take() {
            settings.issues.push(error);
        }
        settings.restore_menu(&mut selector, options.song.is_some());
        let snapshot = settings.capture(&selector, &characters);
        selector.error = settings.finish_restore(snapshot);
        Ok(Self {
            network_wake: None,
            online: Default::default(),
            source_reload: None,
            #[cfg(target_arch = "wasm32")]
            pending_outfit: None,
            content: content.clone(),
            options,
            skin,
            selector,
            active_ui,
            settings,
            scores,
            multiplayer_scores,
            characters,
            loader,
            prepared_song: None,
            loading_frame_presented: false,
            preview_audio: PreviewPlayer::new(content.clone()),
            preview_id: None,
            session,
            state: None,
            focused: true,
            fatal: None,
        })
    }
    pub(crate) fn sync_settings(&mut self, flush: bool) {
        let speed = if self.online_running() {
            self.selector.lobby.scroll_speed
        } else {
            self.selector.scroll_speed
        };
        if let Some(session) = &mut self.session {
            session.display.speed = speed;
        }
        let snapshot = self.settings.capture(&self.selector, &self.characters);
        if let Err(error) = self.settings.update(snapshot, flush) {
            eprintln!("{error:#}");
            self.selector.error = Some(format!("{error:#}"));
        }
    }
    pub(crate) fn record_completed_score(&mut self, retry: bool) {
        let Some(session) = &mut self.session else {
            return;
        };
        let needs_save = matches!(session.score_save, ScoreSaveState::Pending)
            || retry && matches!(session.score_save, ScoreSaveState::Failed(_));
        if !needs_save {
            return;
        }
        let result = if self.online.round.is_some() {
            if !self.selector.lobby.results {
                return;
            }
            let Some(standing) = self
                .selector
                .lobby
                .standings
                .iter()
                .find(|standing| Some(standing.player) == self.selector.lobby.player)
            else {
                return;
            };
            self.multiplayer_scores.record_multiplayer(
                session.score_key.clone(),
                standing,
                session.practice.note_count(),
            )
        } else {
            if !session.practice.completed() {
                return;
            }
            self.scores.record(
                session.score_key.clone(),
                &session.practice,
                session.from_start,
                self.options.autoplay,
            )
        };
        if let ScoreSaveState::Failed(previous) = &session.score_save
            && self.selector.error.as_ref() == Some(previous)
        {
            self.selector.error = None;
        }
        session.score_save = match result {
            Ok(true) => ScoreSaveState::Saved,
            Ok(false) => ScoreSaveState::Ineligible,
            Err(error) => {
                eprintln!("{error:#}");
                let message = format!("Could not save this score.\n\n{error:#}");
                self.selector.error = Some(message.clone());
                ScoreSaveState::Failed(message)
            }
        };
        self.selector.scores = self.scores.book.clone();
        self.selector.multiplayer_scores = self.multiplayer_scores.book.clone();
    }
    pub(crate) fn preview_song(&self) -> Option<&sdo_song::catalog::Song> {
        if !self.selector.lobby.open {
            return self.selector.selected();
        }
        if self.selector.lobby.song_picker_open {
            let room = self.selector.lobby.room()?;
            if Some(room.host) != self.selector.lobby.player || room.round.is_some() {
                return None;
            }
            let id = self.selector.lobby.song_preview.as_deref()?;
            self.selector
                .catalog
                .songs
                .iter()
                .find(|song| song.id == id)
        } else if let Some(room) = self.selector.lobby.room() {
            let chart = room.chart.as_ref()?;
            self.selector.catalog.songs.iter().find(|song| {
                song.pattern(&chart.kind, 4)
                    .is_ok_and(|pattern| pattern.sha256 == chart.pattern_sha256)
            })
        } else if self.selector.lobby.open {
            None
        } else {
            self.selector.selected()
        }
    }
    pub(crate) fn preview_active(&self) -> bool {
        self.focused || self.selector.lobby.open && self.selector.lobby.room().is_some()
    }
    pub(crate) fn sync_preview(&mut self) {
        let in_room = self.selector.lobby.open && self.selector.lobby.room().is_some();
        if self.selector.sources_loading
            || self.online_running()
            || self.selector.loading.is_some() && self.online.loading.is_none()
            || self.selector.settings_open && !in_room
        {
            self.preview_audio.stop();
            self.preview_id = None;
            return;
        }
        let song = self.preview_song();
        let id = song.map(|song| {
            if self.selector.lobby.song_picker_open && self.selector.lobby.open {
                format!(
                    "room-picker:{}:{}",
                    self.selector.lobby.room().unwrap().id,
                    song.id
                )
            } else if let Some(room) = self
                .selector
                .lobby
                .room()
                .filter(|_| self.selector.lobby.open)
            {
                format!(
                    "room:{}:{}",
                    room.id,
                    room.chart.as_ref().unwrap().pattern_sha256
                )
            } else {
                song.id.clone()
            }
        });
        let preview = song.and_then(|song| {
            song.preview
                .as_ref()
                .map(|p| p.path.clone())
                .or_else(|| in_room.then(|| song.audio.clone()))
        });
        if self.preview_id == id {
            return;
        }
        self.preview_id = id;
        self.preview_audio.select(preview);
    }

    pub(crate) fn sync_appearance(&mut self) -> Result<()> {
        if self.active_ui != self.selector.appearance {
            let replacement = self
                .content
                .request(asset::Skin(&self.selector.appearance))?;
            self.skin = replacement;
            self.active_ui = self.selector.appearance.clone();
            if let Some(state) = &mut self.state {
                state.renderer.sync_textures(&self.skin);
                state.character_texture = state
                    .renderer
                    .register_external_texture(&state.character_renderer.view);
                state.stage_texture = state
                    .renderer
                    .register_external_texture(&state.multiplayer_stage.character.view);
            }
        }
        Ok(())
    }
    pub(crate) fn start_selected(&mut self) -> Result<()> {
        anyhow::ensure!(
            !self.selector.lobby.open && self.online.client.is_none(),
            "Exit Multiplayer before starting a single-player song"
        );
        self.begin_song(None)
    }
    pub(crate) fn start_room_selected(&mut self) -> Result<()> {
        anyhow::ensure!(
            self.selector.lobby.open,
            "Enter Multiplayer before choosing a room song"
        );
        let room = self.selector.lobby.room().context("Join a room first")?;
        anyhow::ensure!(
            Some(room.host) == self.selector.lobby.player && room.round.is_none(),
            "Only the room host can choose a chart between rounds"
        );
        let selected = self
            .selector
            .room_song_selection()
            .context("Choose a song in the multiplayer picker first")?;
        if let Some((spec, session)) = &self.online.prepared
            && room.chart.as_ref() == Some(spec)
            && selected.song == session.score_key.song_id
            && spec.kind == selected.kind
            && spec.difficulty as usize == selected.difficulty
            && spec.judgment == selected.judgment
        {
            return Ok(());
        }
        self.begin_song(Some(crate::multiplayer::LoadPurpose::Offer))
    }
    pub(crate) fn begin_song(
        &mut self,
        network: Option<crate::multiplayer::LoadPurpose>,
    ) -> Result<()> {
        if self.selector.loading.is_some() {
            return Ok(());
        }
        if self.selector.sources_loading {
            return Ok(());
        }
        self.content.request(asset::Ui)?.ensure_ready()?;
        self.sync_appearance()?;
        let (song, kind, difficulty, judgment) = match &network {
            Some(crate::multiplayer::LoadPurpose::Ready(spec)) => (
                self.selector.room_chart_song(spec)?,
                spec.kind.clone(),
                spec.difficulty as usize,
                spec.judgment.clone(),
            ),
            Some(crate::multiplayer::LoadPurpose::Offer) => {
                let selected = self
                    .selector
                    .room_song_selection()
                    .context("Choose a song in the multiplayer picker first")?;
                let song = self
                    .selector
                    .catalog
                    .songs
                    .iter()
                    .find(|song| song.id == selected.song)
                    .context("The selected multiplayer song is no longer available")?;
                (
                    song,
                    selected.kind.clone(),
                    selected.difficulty,
                    selected.judgment.clone(),
                )
            }
            None => {
                let Some(song) = self.selector.selected() else {
                    return Ok(());
                };
                (
                    song,
                    self.selector.kind.clone(),
                    self.selector.difficulty,
                    self.selector.judgment.clone(),
                )
            }
        };
        let selection = self.selector.catalog.select(&song.id, None, &kind, 4)?;
        let title = format!("{} / {} - {}", song.id, song.title, song.artist);
        let character = self.characters.preset(&self.selector.character).ok();
        let bpm = match &network {
            Some(crate::multiplayer::LoadPurpose::Ready(spec)) => spec.bpm,
            _ => selection.bpm(),
        };
        self.online.loading = network.clone();
        let request = LoadRequest {
            verify_audio: network.is_some(),
            context: PlaySelection {
                network,
                score_key: ScoreKey::new(
                    &song.id,
                    &kind,
                    difficulty,
                    &judgment,
                    &selection.pattern.sha256,
                ),
                title: title.clone(),
            },
            audio: song.audio.clone(),
            pattern: selection.pattern.file.clone(),
            expected_pattern_sha256: Some(selection.pattern.sha256.clone()),
            dance: song.dance_moves_paths.clone(),
            idle: character.and_then(|c| c.idle.clone()),
            miss: character.and_then(|c| c.miss.clone()),
            character_bones: character.map_or(0, |c| c.skeleton.bones.len()),
            bpm,
            duration: song.duration,
            difficulty: [Difficulty::Easy, Difficulty::Normal, Difficulty::Hard][difficulty],
        };
        if self.online.loading.is_none() {
            self.preview_audio.stop();
            self.preview_id = None;
        }
        self.prepared_song = None;
        self.loading_frame_presented = false;
        self.selector.error = None;
        self.selector.loading = Some(sdo_ui::LoadingView {
            title,
            stage: "Reading chart".into(),
            progress: 0.,
        });
        if let Err(error) = self.loader.request(request) {
            self.selector.loading = None;
            self.online.loading = None;
            return Err(error);
        }
        Ok(())
    }
    pub(crate) fn poll_loading(&mut self) {
        if self.selector.loading.is_none() {
            return;
        }
        if let Some(result) = self.loader.poll() {
            match result {
                Ok(prepared) => self.prepared_song = Some(prepared),
                Err(error) => {
                    if !self.online_loading_failed(Some(&error.to_string())) {
                        self.selector.error =
                            Some(format!("Could not load this song.\n\n{error:#}"));
                    }
                    self.selector.loading = None;
                    self.preview_id = None;
                    return;
                }
            }
        }
        if let Some(view) = &mut self.selector.loading {
            view.progress = self.loader.progress.fraction;
            view.stage = self.loader.progress.stage.label().into();
        }
        if self.loading_frame_presented
            && let Some(prepared) = self.prepared_song.take()
        {
            let was_online = self.online.loading.is_some();
            if let Err(error) = self.activate_song(prepared)
                && !self.online_loading_failed(Some(&error.to_string()))
            {
                self.selector.error = Some(format!("Could not start playback.\n\n{error:#}"));
            }
            self.selector.loading = None;
            self.online.loading = None;
            if !was_online {
                self.preview_id = None;
            }
        }
    }
    pub(crate) fn activate_song(&mut self, prepared: PreparedSong<PlaySelection>) -> Result<()> {
        anyhow::ensure!(
            prepared.context.score_key.pattern_sha256 == prepared.pattern_sha256,
            "Prepared chart identity does not match the score key"
        );
        let online_spec = self.prepare_online(&prepared)?;
        let chart = prepared.chart;
        let duration = prepared.audio.duration();
        let preset = sdo_gameplay::judgment::find(&prepared.context.score_key.judgment)
            .context("Unavailable timing preset")?;
        let holds = chart.notes.iter().filter(|n| n.is_hold()).count();
        println!(
            "Playing {} | {:.3} BPM | {} notes ({holds} holds) | {} tempo segments",
            prepared.context.title,
            chart.tempo.first().map_or(chart.bpm, |t| t.bpm),
            chart.notes.len(),
            chart.tempo.len()
        );
        let mut session = PlaySession {
            tail_clock: TailClock::default(),
            score_key: prepared.context.score_key,
            score_save: ScoreSaveState::Pending,
            from_start: true,
            practice: chart.into_practice(
                online_spec
                    .as_ref()
                    .map_or(duration, |spec| spec.duration_us as f64 / 1e6),
                preset,
                prepared.difficulty,
            ),
            input: InputState::default(),
            display: NoteDisplay {
                speed: if online_spec.is_some() {
                    self.selector.lobby.scroll_speed
                } else {
                    self.selector.scroll_speed
                },
            },
            audio: AudioPlayer::new(prepared.audio, 0.)?,
            title: prepared.context.title,
            dance: DancePlayer::new(prepared.dance, prepared.idle, prepared.miss),
            failed_at: None,
            dance_errors: prepared.dance_errors,
        };
        if let Some(spec) = online_spec {
            return self.store_online_session(spec, session);
        }
        session.pause(!self.focused)?;
        self.session = Some(session);
        Ok(())
    }
    pub(crate) fn cancel_loading(&mut self) {
        self.online.loading = None;
        self.loader.cancel();
        self.prepared_song = None;
        self.selector.loading = None;
        self.preview_id = None;
    }
    pub(crate) fn return_to_selector(&mut self) -> Result<()> {
        if self.online_running() {
            self.forfeit_online();
            return Ok(());
        }
        if let Some(session) = &mut self.session {
            session.advance(self.selector.timing_offset_seconds(), self.options.autoplay)?;
        }
        self.record_completed_score(true);
        self.cancel_loading();
        if let Some(session) = &mut self.session {
            session.pause(true)?;
        }
        self.session = None;
        self.preview_id = None;
        self.sync_preview();
        Ok(())
    }
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn initialize(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        let window = Arc::new(
            event_loop.create_window(
                Window::default_attributes()
                    .with_title("SDO Arrowgene | Song Select")
                    .with_inner_size(LogicalSize::new(1100.0, 760.0))
                    .with_min_inner_size(LogicalSize::new(900.0, 680.0)),
            )?,
        );
        let size = window.inner_size();
        let (surface, device, queue, name) =
            pollster::block_on(sdo_render::surface::DisplaySurface::new(
                window.clone(),
                [size.width, size.height],
                self.options.frame_latency,
            ))?;
        println!("GPU: {name}");
        self.attach_window(window, surface, device, queue)
    }
    pub(crate) fn attach_window(
        &mut self,
        window: Arc<Window>,
        surface: sdo_render::surface::DisplaySurface,
        device: wgpu::Device,
        queue: wgpu::Queue,
    ) -> Result<()> {
        let format = surface.format();
        let mut selector_ui = SelectorUi::new(&device, format, Some(&window));
        let character_renderer = CharacterRenderer::new(&device, &queue, &self.characters);
        let multiplayer_stage =
            sdo_render::character::stage::StageRenderer::new(&device, &queue, &self.characters);
        self.selector.lobby.stage_texture = Some(
            selector_ui.register_character_preview(&device, &multiplayer_stage.character.ui_view),
        );
        self.selector.lobby.head_texture =
            Some(selector_ui.register_character_preview(&device, &multiplayer_stage.head_ui_view));
        let wardrobe_thumbnails = WardrobeThumbnails::new(&device, &queue, self.content.clone());
        if !self.selector.character.is_empty() {
            self.selector.character_texture =
                Some(selector_ui.register_character_preview(&device, &character_renderer.ui_view));
        }
        let mut renderer = Renderer::new(device, queue, format, &self.skin);
        let character_texture = renderer.register_external_texture(&character_renderer.view);
        let stage_texture = renderer.register_external_texture(&multiplayer_stage.character.view);
        self.state = Some(WindowState {
            surface,
            window,
            frame_intervals: Vec::new(),
            last_present: None,
            renderer,
            selector_ui,
            character_renderer,
            multiplayer_stage,
            stage_texture,
            wardrobe_thumbnails,
            character_texture,
            occluded: false,
            rendered: 0,
        });
        if let Some(session) = &mut self.session {
            println!(
                "Audio output: {} | {:.3}s",
                session.audio.device_name,
                session.audio.duration()
            );
            session.pause(false)?;
        } else {
            self.sync_preview();
        }
        self.state.as_ref().unwrap().window.request_redraw();
        Ok(())
    }
    pub(crate) fn redraw(&mut self) -> Result<bool> {
        self.poll_multiplayer();
        self.poll_sources();
        #[cfg(target_arch = "wasm32")]
        if let Some(result) = self
            .pending_outfit
            .as_ref()
            .and_then(|rx| rx.try_recv().ok())
        {
            self.pending_outfit = None;
            match result {
                Ok(next) => {
                    self.apply_character(next);
                    self.sync_settings(true);
                }
                Err(error) => self.selector.error = Some(format!("Cannot equip outfit: {error:#}")),
            }
        }
        if let Err(error) = self.sync_room_scene() {
            self.selector.lobby.status = format!("Character assets unavailable: {error:#}");
        }
        self.poll_loading();
        if self.session.is_some() {
            self.advance_playback()?;
        } else {
            self.sync_preview();
            self.preview_audio.update(self.preview_active());
            self.selector.lobby.preview_status = format!(
                "{} · {}",
                if self.selector.lobby.song_picker_open {
                    "Song preview"
                } else {
                    "Room preview"
                },
                self.preview_audio.status()
            );
        }
        self.record_completed_score(false);
        let online_lines = self.online_status_lines();
        let round_seconds = self.online_round_seconds();
        let multiplayer_round = self.online.round_room.is_some() && self.online_running();
        self.render_room_scene()?;
        let state = self.state.as_mut().context("Window is not initialized")?;
        let size = state.window.inner_size();
        if state.occluded || size.width == 0 || size.height == 0 {
            return Ok(false);
        }
        let Some(frame) = state
            .surface
            .acquire(&state.renderer.device, state.window.clone())?
        else {
            return Ok(false);
        };
        let view = frame.view();
        let mut action = Action::None;
        if let Some(session) = &mut self.session {
            let standing = self
                .selector
                .lobby
                .standings
                .iter()
                .find(|s| multiplayer_round && Some(s.player) == self.selector.lobby.player);
            let mut scene = if multiplayer_round {
                let prediction = self
                    .selector
                    .lobby
                    .player
                    .and_then(|id| self.online.replays.get(&id))
                    .map_or(&session.practice, sdo_multiplayer::Replay::practice);
                session.display.round_scene(
                    &self.skin,
                    prediction,
                    &session.input,
                    state.stage_texture,
                    sdo_ui::notes::RoundFeedback {
                        grades: &self.online.confirmed,
                        correction: self.online.correction,
                        totals: standing.map(|s| (s.points, s.combo, s.health)),
                    },
                )
            } else {
                session.display.scene_with_status(
                    &self.skin,
                    &session.practice,
                    &session.input,
                    &online_lines,
                )
            };
            if !multiplayer_round {
                sdo_ui::notes::personal_best(
                    &mut scene,
                    &self.skin,
                    self.scores
                        .book
                        .best(&session.score_key)
                        .map(|score| score.points),
                );
                match session.score_save {
                    ScoreSaveState::Saved => {
                        sdo_ui::notes::score_save_status(&mut scene, &self.skin, true);
                    }
                    ScoreSaveState::Failed(_) => {
                        sdo_ui::notes::score_save_status(&mut scene, &self.skin, false);
                    }
                    _ => {}
                }
                if let Ok(character) = self.characters.preset(&self.selector.character) {
                    let pose = session.character_pose(&character.skeleton)?;
                    state.character_renderer.render_pose(
                        &state.renderer.device,
                        &state.renderer.queue,
                        &character.choice.id,
                        CharacterCamera::default(),
                        pose.as_ref().map(|p| p.globals.as_slice()),
                    )?;
                    scene.push(Sprite::new(state.character_texture, [7., 180., 320., 360.]));
                    let status = if !online_lines.is_empty() && session.practice.failed() {
                        "STAGE FAILED - WAIT FOR RESULTS".into()
                    } else {
                        session.dance_status()
                    };
                    sdo_ui::notes::dance_status(
                        &mut scene,
                        &self.skin,
                        &status,
                        !session.dance_errors.is_empty(),
                    );
                }
            }
            state.renderer.draw(&view, state.surface.size(), &scene)?;
            if multiplayer_round {
                action = state.selector_ui.draw_round(
                    &state.renderer.device,
                    &state.renderer.queue,
                    &view,
                    state.surface.size(),
                    Some(&state.window),
                    sdo_ui::RoundHud {
                        room: self.online.round_room.as_ref().unwrap(),
                        player: self.selector.lobby.player,
                        standings: &self.selector.lobby.standings,
                        title: &session.title,
                        seconds: round_seconds.max(0.),
                        duration: session.audio.duration(),
                        countdown: (round_seconds < 0.).then_some(-round_seconds),
                        failed: self
                            .selector
                            .lobby
                            .standings
                            .iter()
                            .any(|s| Some(s.player) == self.selector.lobby.player && s.failed),
                        results: self.selector.lobby.results,
                        disconnected: self.online.client.is_none(),
                    },
                );
            }
            state.window.set_title(&format!(
                "SDO Arrowgene | {} | {} | Score {} | Combo {} | {}",
                session.title,
                if session.practice.failed() {
                    "Stage failed"
                } else if session.practice.paused() {
                    "Paused"
                } else if self.options.autoplay {
                    "Autoplay"
                } else {
                    "Playing"
                },
                standing.map_or(session.practice.score().points, |s| s.points),
                standing.map_or(session.practice.score().combo, |s| s.combo),
                if online_lines.is_empty() {
                    "Esc songs | Space pause | F1 preset"
                } else {
                    "Online"
                }
            ));
        } else {
            if self.selector.character_view && !self.selector.character.is_empty() {
                state.character_renderer.render(
                    &state.renderer.device,
                    &state.renderer.queue,
                    &self.selector.character,
                    CharacterCamera {
                        yaw: self.selector.character_yaw,
                        elevation: self.selector.character_elevation,
                        zoom: self.selector.character_zoom,
                    },
                )?;
            }
            action = state.selector_ui.draw(
                &state.renderer.device,
                &state.renderer.queue,
                &view,
                state.surface.size(),
                Some(&state.window),
                &mut self.selector,
                self.preview_audio.status(),
                self.preview_audio.progress(),
            );
            state.window.set_title(&format!(
                "SDO Arrowgene | {} | {}",
                if self.selector.lobby.open {
                    "Multiplayer"
                } else if self.selector.character_view {
                    "Character"
                } else if self.selector.settings_open {
                    "Settings"
                } else {
                    "Single Player"
                },
                self.selector
                    .selected()
                    .map(|s| format!("{} - {}", s.title, s.artist))
                    .unwrap_or_else(|| "No matching songs".into())
            ));
        }
        state.wardrobe_thumbnails.update(
            &state.renderer.device,
            &state.renderer.queue,
            &mut state.selector_ui,
            &mut self.selector.wardrobe,
            self.session.is_none()
                && self.selector.character_view
                && self.selector.loading.is_none(),
        );
        state.window.pre_present_notify();
        state
            .surface
            .present(frame, &state.renderer.device, &state.renderer.queue);
        state.rendered += 1;
        if let Some(limit) = self.options.frames {
            let now = sdo_platform::Instant::now();
            if let Some(prev) = state.last_present.replace(now)
                && state.rendered > 20
            {
                state
                    .frame_intervals
                    .push((now - prev).as_secs_f64() * 1000.);
            }
            if state.rendered == limit && !state.frame_intervals.is_empty() {
                state.frame_intervals.sort_by(f64::total_cmp);
                let percentile = |q: f64| {
                    state.frame_intervals
                        [(q * state.frame_intervals.len() as f64).ceil() as usize - 1]
                };
                println!(
                    "Frame submission intervals ({} samples, queue depth hint {}): p50 {:.3} ms, p95 {:.3} ms, p99 {:.3} ms",
                    state.frame_intervals.len(),
                    self.options.frame_latency,
                    percentile(0.50),
                    percentile(0.95),
                    percentile(0.99)
                );
            }
        }
        if self.selector.loading.is_some() {
            self.loading_frame_presented = true;
        }
        let finished = self
            .options
            .frames
            .is_some_and(|limit| state.rendered >= limit);
        if self.session.is_none()
            && let Err(error) = self.sync_appearance()
        {
            self.selector.error = Some(format!("Cannot build selected UI: {error:#}"));
            self.selector.appearance = self.active_ui.clone();
        }
        if let Action::Multiplayer(action) = &action {
            if let Err(error) = self.lobby_action(action) {
                self.selector.error = Some(format!("Multiplayer: {error:#}"));
            }
        } else if action == Action::BrowseAssets {
            self.browse_assets();
        } else if action == Action::ReloadSources {
            self.reload_sources();
        } else if action == Action::Play {
            if let Err(error) = self.start_selected() {
                self.selector.error = Some(format!("Cannot play this chart: {error:#}"));
                self.sync_preview();
            }
        } else if let Action::Equip(change) = &action {
            if let Err(error) = self.equip_character(change) {
                self.selector.error = Some(format!("Cannot equip this item: {error:#}"));
            }
        } else if action == Action::CancelLoading {
            self.online_loading_failed(None);
            self.cancel_loading();
            self.sync_preview();
        } else if self.session.is_none() {
            self.sync_preview();
        }
        if let Err(error) = self.sync_room_walk() {
            self.selector.lobby.status = error.to_string();
        }
        if self.session.is_none() || finished || action != Action::None {
            self.sync_settings(finished || action != Action::None);
        }
        Ok(finished || action == Action::Quit)
    }
    pub(crate) fn equip_character(&mut self, change: &sdo_ui::WardrobeChange) -> Result<()> {
        use sdo_ui::WardrobeChange;
        let outfits = match change {
            WardrobeChange::Set { slot, item } => self.characters.equipment_overrides(
                &self.selector.character,
                *slot,
                item.as_deref(),
            )?,
            WardrobeChange::Reset => {
                let mut outfits = self.characters.outfit_overrides();
                outfits.remove(&self.characters.preset(&self.selector.character)?.choice.id);
                outfits
            }
        };
        #[cfg(not(target_arch = "wasm32"))]
        self.apply_character(self.content.request(asset::Characters(&outfits))?);
        #[cfg(target_arch = "wasm32")]
        {
            let content = self.content.clone();
            let (send, receive) = std::sync::mpsc::channel();
            self.pending_outfit = Some(receive);
            wasm_bindgen_futures::spawn_local(async move {
                let result =
                    sdo_platform::assets::hydrate(|| content.request(asset::Characters(&outfits)))
                        .await;
                let _ = send.send(result);
            });
        }
        Ok(())
    }
    fn apply_character(&mut self, next: Arc<CharacterAssets>) {
        if let Some(state) = &mut self.state {
            state.character_renderer.sync_assets(
                &state.renderer.device,
                &state.renderer.queue,
                &next,
            );
            state.window.request_redraw();
        }
        self.selector.wardrobe.sync(&next);
        self.characters = next;
    }
    #[cfg(test)]
    pub(crate) fn keyboard(&mut self, code: KeyCode, down: bool, repeat: bool) -> Result<()> {
        self.keyboard_at(code, down, repeat, sdo_platform::Instant::now())
    }
    fn keyboard_at(
        &mut self,
        code: KeyCode,
        down: bool,
        repeat: bool,
        captured: sdo_platform::Instant,
    ) -> Result<()> {
        if self.online_running() && (self.selector.lobby.results || code == KeyCode::Escape) {
            return Ok(());
        }
        if down && !repeat && code == KeyCode::Escape {
            return self.return_to_selector();
        }
        let online = self.online_running();
        if self.session.is_none() {
            return Ok(());
        }
        let song = self.advance_playback_at(captured)?;
        self.record_completed_score(down && !repeat && matches!(code, KeyCode::KeyR | KeyCode::F1));
        let session = self.session.as_mut().unwrap();
        if online && song.is_none() {
            return Ok(());
        }
        let binding = match code {
            KeyCode::KeyD => Some(Binding::D),
            KeyCode::KeyF => Some(Binding::F),
            KeyCode::KeyJ => Some(Binding::J),
            KeyCode::KeyK => Some(Binding::K),
            KeyCode::ArrowLeft => Some(Binding::Left),
            KeyCode::ArrowDown => Some(Binding::Down),
            KeyCode::ArrowUp => Some(Binding::Up),
            KeyCode::ArrowRight => Some(Binding::Right),
            _ => None,
        };
        let mut network_input = None;
        if let Some(binding) = binding {
            if let Some(action) = session
                .input
                .update(binding, down, session.practice.position())
                && !self.options.autoplay
            {
                if online && let Some(at) = song {
                    network_input = Some((action, at));
                }
                action.apply(&mut session.practice);
            }
        } else if down && !repeat {
            match code {
                KeyCode::Space | KeyCode::KeyR | KeyCode::F1 if online => {}
                KeyCode::Space => session.pause(!session.practice.paused())?,
                KeyCode::KeyR => session.restart()?,
                KeyCode::F1 => {
                    let next = sdo_gameplay::judgment::next(session.practice.preset().id);
                    session.practice.set_preset(next);
                    self.selector.judgment = next.id.into();
                    session.restart()?;
                }
                KeyCode::Minus | KeyCode::NumpadSubtract => {
                    if online {
                        self.selector.lobby.scroll_speed =
                            self.selector.lobby.scroll_speed.adjusted(-1);
                    } else {
                        self.selector.scroll_speed = self.selector.scroll_speed.adjusted(-1);
                    }
                }
                KeyCode::Equal | KeyCode::NumpadAdd => {
                    if online {
                        self.selector.lobby.scroll_speed =
                            self.selector.lobby.scroll_speed.adjusted(1);
                    } else {
                        self.selector.scroll_speed = self.selector.scroll_speed.adjusted(1);
                    }
                }
                _ => {}
            }
        }
        if let Some((action, at)) = network_input
            && let Err(error) = self.send_online_input(action, at)
        {
            self.forfeit_online();
            self.selector.error = Some(format!("Multiplayer connection failed: {error:#}"));
        }
        self.record_completed_score(false);
        if down
            && !repeat
            && matches!(
                code,
                KeyCode::F1
                    | KeyCode::Minus
                    | KeyCode::NumpadSubtract
                    | KeyCode::Equal
                    | KeyCode::NumpadAdd
            )
        {
            self.sync_settings(true);
        }
        Ok(())
    }
    pub(crate) fn fail(&mut self, event_loop: &ActiveEventLoop, error: anyhow::Error) {
        self.preview_audio.stop();
        #[cfg(target_arch = "wasm32")]
        web_sys::console::error_1(&format!("{error:#}").into());
        self.fatal = Some(error);
        event_loop.exit();
    }
}
impl ApplicationHandler for App {
    fn user_event(&mut self, _: &ActiveEventLoop, _: ()) {
        self.poll_multiplayer();
    }
    #[cfg(not(target_arch = "wasm32"))]
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_none()
            && let Err(error) = self.initialize(event_loop)
        {
            self.fail(event_loop, error);
        }
    }
    #[cfg(target_arch = "wasm32")]
    fn resumed(&mut self, _: &ActiveEventLoop) {}
    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let captured = sdo_platform::Instant::now();
        if self.state.as_ref().is_none_or(|s| s.window.id() != id) {
            return;
        }
        if self.session.is_none() || self.online_running() {
            let state = self.state.as_mut().unwrap();
            state.selector_ui.on_event(&state.window, &event);
        }
        match event {
            WindowEvent::CloseRequested => {
                if let Some(session) = &mut self.session {
                    let _ = session
                        .advance(self.selector.timing_offset_seconds(), self.options.autoplay);
                }
                self.record_completed_score(true);
                self.sync_settings(true);
                self.preview_audio.stop();
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                let state = self.state.as_mut().unwrap();
                if size.width > 0 && size.height > 0 {
                    state
                        .surface
                        .resize(&state.renderer.device, [size.width, size.height]);
                    state.window.request_redraw();
                }
            }
            WindowEvent::Occluded(value) => {
                let state = self.state.as_mut().unwrap();
                state.occluded = value;
                if !value {
                    state.window.request_redraw();
                }
                if self.session.is_none() {
                    self.preview_audio
                        .update(self.selector.lobby.room().is_some() || !value && self.focused);
                }
            }
            WindowEvent::Focused(focused) => {
                if let Err(error) = self.window_focus(focused) {
                    self.fail(event_loop, error);
                }
            }
            WindowEvent::KeyboardInput { event, .. } if self.session.is_some() => {
                if let PhysicalKey::Code(code) = event.physical_key
                    && let Err(error) = self.keyboard_at(
                        code,
                        event.state == ElementState::Pressed,
                        event.repeat,
                        captured,
                    )
                {
                    self.fail(event_loop, error);
                }
            }
            WindowEvent::RedrawRequested => match self.redraw() {
                Ok(true) => {
                    self.preview_audio.stop();
                    event_loop.exit();
                }
                Ok(false) => {}
                Err(error) => self.fail(event_loop, error),
            },
            _ => {}
        }
    }
    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        // Minimized windows may receive no redraws. Keep the shared round clock,
        // health, connection heartbeat and results progressing in the background.
        if self.state.as_ref().is_some_and(|s| s.occluded) {
            self.poll_multiplayer();
            if let Err(error) = self.advance_playback() {
                self.fail(_event_loop, error);
            }
            if let Err(error) = self.sync_room_scene() {
                self.selector.lobby.status = error.to_string();
            }
            _event_loop.set_control_flow(ControlFlow::WaitUntil(
                sdo_platform::Instant::now() + std::time::Duration::from_millis(16),
            ));
        } else {
            _event_loop.set_control_flow(ControlFlow::Wait);
        }
        if let Some(state) = &self.state
            && !state.occluded
        {
            state.window.request_redraw();
        }
    }
}
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn run_app(mut app: App) -> Result<()> {
    let event_loop = EventLoop::new()?;
    app.network_wake = Some(event_loop.create_proxy());
    event_loop.set_control_flow(ControlFlow::Wait);
    event_loop.run_app(&mut app)?;
    if let Some(error) = app.fatal {
        return Err(error);
    }
    Ok(())
}

fn catalog_is_empty_or_incomplete(catalog: &Catalog, assets: &UiAssets) -> bool {
    catalog.songs.is_empty() || assets.ensure_ready().is_err()
}
