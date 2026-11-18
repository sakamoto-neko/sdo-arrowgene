//! Discover existing asset folders and apply replacements in the background.
use crate::{app::App, wardrobe::WardrobeThumbnails};
use anyhow::Result;
use sdo_asset_loader::{
    AssetLoader, PreviewPlayer, SongAssetLoader, request as asset,
    sources::{self, SourceReport},
};
use sdo_assets::UiSelection;
pub(crate) type SourceLoadResult = Result<(AssetLoader, Vec<SourceReport>)>;
pub(crate) enum SourceEvent {
    Complete(Box<SourceLoadResult>),
}
impl App {
    pub(crate) fn browse_assets(&mut self) {
        if self.source_reload.is_some() {
            return;
        }
        let mut dialog = rfd::FileDialog::new()
            .set_title("Select the asset folder")
            .set_can_create_directories(true);
        if let Some(state) = &self.state {
            dialog = dialog.set_parent(state.window.as_ref());
        }
        if let Some(folder) = dialog.pick_folder() {
            self.selector.source_settings.path = folder.display().to_string();
            self.reload_sources();
        }
    }
    pub(crate) fn reload_sources(&mut self) {
        if self.source_reload.is_some() {
            return;
        }
        self.sync_settings(true);
        self.cancel_loading();
        self.preview_audio.stop();
        self.preview_id = None;
        let settings = self.selector.source_settings.clone();
        let (send, receive) = std::sync::mpsc::channel();
        self.source_reload = Some(receive);
        self.selector.sources_loading = true;
        self.selector.source_status = "Discovering assets…".into();
        std::thread::spawn(move || {
            let result = sources::open(&settings);
            let _ = send.send(SourceEvent::Complete(Box::new(result)));
        });
    }
    pub(crate) fn poll_sources(&mut self) {
        let Some(receiver) = &self.source_reload else {
            return;
        };
        let result = match receiver.try_recv() {
            Ok(SourceEvent::Complete(result)) => *result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(_) => Err(anyhow::anyhow!("Asset task stopped before completion")),
        };
        self.source_reload = None;
        self.selector.sources_loading = false;
        self.selector.source_status.clear();
        if let Err(error) =
            result.and_then(|(content, reports)| self.apply_sources(content, reports))
        {
            self.selector.error = Some(format!("Could not load assets: {error:#}"));
        }
    }
    fn apply_sources(&mut self, content: AssetLoader, reports: Vec<SourceReport>) -> Result<()> {
        let catalog = content.request(asset::Catalog)?;
        let assets = content.request(asset::Ui)?;
        let defaults = content.request(asset::Characters(&Default::default()))?;
        let mut outfits = self.settings.values.outfits.clone();
        for preset in &self.characters.presets {
            outfits.remove(&preset.choice.id);
        }
        outfits.extend(self.characters.outfit_overrides());
        outfits.retain(|id, slots| {
            let Ok(preset) = defaults.preset(id) else {
                return false;
            };
            slots.retain(|slot, id| {
                defaults.wardrobe.item(id).is_ok_and(|item| {
                    item.slot.key() == slot
                        && defaults
                            .wardrobe
                            .is_compatible(&preset.choice.id, preset.body, item)
                })
            });
            !slots.is_empty()
        });
        let characters = if outfits.is_empty() {
            defaults
        } else {
            content
                .request(asset::Characters(&outfits))
                .unwrap_or(defaults)
        };
        let mut selection = if assets.ensure_ready().is_ok() {
            assets.selection("default", None, None, None)?
        } else {
            UiSelection::default()
        };
        let choices = assets.choices();
        for (current, saved, available) in [
            (
                &mut selection.skin,
                &self.selector.appearance.skin,
                &choices.skins,
            ),
            (
                &mut selection.board,
                &self.selector.appearance.board,
                &choices.boards,
            ),
            (
                &mut selection.effect,
                &self.selector.appearance.effect,
                &choices.effects,
            ),
            (
                &mut selection.score,
                &self.selector.appearance.score,
                &choices.scores,
            ),
        ] {
            if available.iter().any(|c| c.id == *saved) {
                current.clone_from(saved);
            }
        }
        if self.selector.appearance.effect == "none" {
            selection.effect = "none".into();
        }
        let skin = content.request(asset::Skin(&selection))?;
        self.selector.replace_catalog(catalog);
        self.online.availability_checked = None;
        self.online.scene_loader = None;
        self.online.scene_keys.clear();
        self.online.scene_song = None;
        self.online.scene_pending.clear();
        self.online.actors.clear();
        if let Some(state) = &mut self.state {
            state.multiplayer_stage.retain(&[]);
        }
        self.selector.source_reports = reports;
        self.selector.appearance = selection.clone();
        self.selector.ui_choices = choices;
        self.selector.set_preview_assets(&assets);
        self.selector.asset_issues = assets.issues.iter().map(ToString::to_string).collect();
        self.selector.character_choices = characters.choices();
        if characters.preset(&self.selector.character).is_err() {
            self.selector.character = characters
                .presets
                .first()
                .map(|p| p.choice.id.clone())
                .unwrap_or_default();
        }
        self.selector.character_issues =
            characters.issues.iter().map(ToString::to_string).collect();
        self.selector.wardrobe.sync(&characters);
        self.loader = SongAssetLoader::new(content.clone());
        self.preview_audio = PreviewPlayer::new(content.clone());
        self.prepared_song = None;
        if let Some(state) = &mut self.state {
            state.renderer.sync_textures(&skin);
            state.character_texture = state
                .renderer
                .register_external_texture(&state.character_renderer.view);
            state.stage_texture = state
                .renderer
                .register_external_texture(&state.multiplayer_stage.character.view);
            state.character_renderer.sync_assets(
                &state.renderer.device,
                &state.renderer.queue,
                &characters,
            );
            state.wardrobe_thumbnails = WardrobeThumbnails::new(
                &state.renderer.device,
                &state.renderer.queue,
                content.clone(),
            );
            if self.selector.character.is_empty() {
                self.selector.character_texture = None;
            } else if self.selector.character_texture.is_none() {
                self.selector.character_texture =
                    Some(state.selector_ui.register_character_preview(
                        &state.renderer.device,
                        &state.character_renderer.ui_view,
                    ));
            }
        }
        self.content = content;
        self.skin = skin;
        self.active_ui = selection;
        self.characters = characters;
        self.sync_settings(true);
        Ok(())
    }
}
