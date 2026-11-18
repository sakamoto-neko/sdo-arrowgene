//! Inline player options in the multiplayer room sidebar.
use super::{LobbyView, asset_choice};
use sdo_assets::{UiChoices, UiSelection};

impl LobbyView {
    pub(super) fn room_player_controls(
        &mut self,
        ui: &mut egui::Ui,
        appearance: &mut UiSelection,
        choices: &UiChoices,
    ) {
        ui.label("Scroll speed");
        let mut speed = self.scroll_speed.value();
        if ui
            .add(egui::Slider::new(&mut speed, 1..=8).suffix("x"))
            .changed()
        {
            self.scroll_speed = sdo_config::ScrollSpeed::new(speed).unwrap();
        }
        asset_choice(ui, "Note skin", &mut appearance.skin, &choices.skins, false);
        asset_choice(
            ui,
            "Note effect",
            &mut appearance.effect,
            &choices.effects,
            true,
        );
    }
}
