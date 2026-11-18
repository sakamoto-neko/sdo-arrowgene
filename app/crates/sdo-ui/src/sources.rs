//! Settings selects the existing-format asset library.
use crate::{Action, SongSelector};
impl SongSelector {
    pub(super) fn show_sources(&mut self, ui: &mut egui::Ui) -> Action {
        let mut action = Action::None;
        ui.heading("Settings");
        ui.add_space(6.);
        egui::ScrollArea::vertical()
            .id_salt("asset-settings")
            .show(ui, |ui| {
                crate::theme::panel(18).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label("Timing offset");
                        ui.add(
                            egui::DragValue::new(&mut self.timing_offset_ms)
                                .range(-5000.0..=5000.0)
                                .speed(1.0)
                                .suffix(" ms"),
                        );
                        if ui.button("Reset to 0").clicked() {
                            self.timing_offset_ms = 0.;
                        }
                    });
                    ui.weak("If you hit early, decrease the offset. If you hit late, increase it.");
                    if self.timing_offset_ms.abs() > 500. {
                        ui.label("Multiplayer accepts offsets from −500 to +500 ms.");
                    }
                });
                ui.add_space(6.);
                if ui.available_width() >= 760. {
                    ui.columns(2, |columns| {
                        crate::theme::panel(18)
                            .show(&mut columns[0], |ui| action = self.asset_folder(ui));
                        crate::theme::panel(18).show(&mut columns[1], |ui| self.content_report(ui));
                    });
                } else {
                    crate::theme::panel(18).show(ui, |ui| action = self.asset_folder(ui));
                    crate::theme::panel(18).show(ui, |ui| self.content_report(ui));
                }
            });
        action
    }

    fn asset_folder(&mut self, ui: &mut egui::Ui) -> Action {
        let mut action = Action::None;
        ui.set_width(ui.available_width());
        ui.label(egui::RichText::new("Asset Folder").size(19.).strong());
        ui.label(
            egui::RichText::new(
                "Songs and assets are discovered in this folder and its game packages.",
            )
            .color(crate::theme::MUTED),
        );
        ui.add_space(6.);
        ui.add_enabled_ui(!self.sources_loading, |ui| {
            ui.horizontal(|ui| {
                ui.add_sized(
                    [(ui.available_width() - 94.).max(100.), 32.],
                    egui::TextEdit::singleline(&mut self.source_settings.path)
                        .hint_text("Select an asset folder"),
                );
                if ui
                    .add_sized([86., 32.], egui::Button::new("Browse…"))
                    .clicked()
                {
                    action = Action::BrowseAssets;
                }
            });
            ui.weak("The path is saved automatically. Reload after changing the path or files.");
            ui.add_space(6.);
            if ui
                .add_enabled(
                    !self.source_settings.path.trim().is_empty(),
                    egui::Button::new("Reload assets"),
                )
                .clicked()
            {
                action = Action::ReloadSources;
            }
        });
        if self.sources_loading {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(&self.source_status);
            });
        }
        action
    }

    fn content_report(&self, ui: &mut egui::Ui) {
        ui.set_width(ui.available_width());
        ui.label(egui::RichText::new("Discovered Content").size(19.).strong());
        if self.source_reports.is_empty() {
            ui.weak("No content in the selected asset folder.");
        }
        for (index, report) in self.source_reports.iter().enumerate() {
            ui.push_id(index, |ui| {
                ui.label(format!(
                    "{}: {} songs · {} noteskins · {} characters",
                    report.name, report.songs, report.skins, report.characters
                ));
                if !report.issues.is_empty() {
                    egui::CollapsingHeader::new(format!(
                        "Content issues ({})",
                        report.issues.len()
                    ))
                    .show(ui, |ui| {
                        for issue in &report.issues {
                            ui.label(issue);
                        }
                    });
                }
            });
        }
    }
}
#[cfg(test)]
mod tests {
    use crate::tests::{frame, frame_output, selector, text_center};
    #[test]
    fn settings_has_asset_controls_without_an_importer() {
        let ctx = egui::Context::default();
        let mut s = selector();
        s.settings_open = true;
        frame(&ctx, &mut s, vec![]);
        let out = frame_output(&ctx, &mut s, vec![]).1;
        assert!(text_center(&out, "Timing offset").is_some());
        assert!(text_center(&out, "Reset to 0").is_some());
        assert!(text_center(&out, "Browse…").is_some());
        assert!(text_center(&out, "Reload assets").is_some());
        assert!(out.shapes.iter().all(|shape|!matches!(&shape.shape,egui::epaint::Shape::Text(text) if text.galley.text().contains("Import"))));
    }
}
