//! Online-only HUD and the results acknowledgment, over the rendered stage.
use crate::{Action, LobbyAction, SelectorUi};
use sdo_net::{PlayerId, Room, Standing};
use winit::window::Window;

pub struct RoundHud<'a> {
    pub room: &'a Room,
    pub player: Option<PlayerId>,
    pub standings: &'a [Standing],
    pub title: &'a str,
    pub seconds: f64,
    pub duration: f64,
    pub countdown: Option<f64>,
    pub failed: bool,
    pub results: bool,
    pub disconnected: bool,
}
impl RoundHud<'_> {
    fn name(&self, player: PlayerId) -> &str {
        self.room
            .members
            .iter()
            .find(|m| m.id == player)
            .map_or("Player", |m| m.name.as_str())
    }
    fn status(&self, standing: &Standing) -> &'static str {
        if standing.forfeited {
            "Disconnected"
        } else if standing.failed {
            "Failed"
        } else if self.results {
            "Finished"
        } else {
            ""
        }
    }
    fn show(&self, ctx: &egui::Context) -> bool {
        use egui::{Align2, Color32, RichText};
        let panel = || {
            egui::Frame::new()
                .fill(Color32::from_black_alpha(170))
                .corner_radius(6.)
                .inner_margin(12.)
        };
        egui::Area::new(egui::Id::unique("round-scoreboard"))
            .anchor(Align2::RIGHT_TOP, [-18., 18.])
            .show(ctx, |ui| {
                panel().show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 2.;
                    ui.set_min_width(190.);
                    ui.strong("LIVE SCORES");
                    if self.standings.is_empty() {
                        for member in &self.room.members {
                            ui.label(format!("{}   0", member.name));
                        }
                    }
                    for standing in self.standings {
                        let color = if Some(standing.player) == self.player {
                            Color32::from_rgb(105, 224, 218)
                        } else {
                            Color32::WHITE
                        };
                        ui.label(
                            RichText::new(format!(
                                "{}   {}",
                                self.name(standing.player),
                                standing.points
                            ))
                            .color(color),
                        );
                        let status = self.status(standing);
                        if !status.is_empty() {
                            ui.small(status);
                        }
                    }
                });
            });
        egui::Area::new(egui::Id::unique("round-track"))
            .anchor(Align2::RIGHT_BOTTOM, [-18., -18.])
            .show(ctx, |ui| {
                panel().show(ui, |ui| {
                    ui.set_width((ctx.content_rect().width() * 0.56).clamp(180., 540.));
                    ui.strong(self.title);
                    ui.add(
                        egui::ProgressBar::new(
                            (self.seconds / self.duration.max(1.)).clamp(0., 1.) as f32,
                        )
                        .text(format!(
                            "{}:{:02} / {}:{:02}",
                            self.seconds.max(0.) as u64 / 60,
                            self.seconds.max(0.) as u64 % 60,
                            self.duration as u64 / 60,
                            self.duration as u64 % 60
                        )),
                    );
                    if self.failed && !self.results {
                        ui.colored_label(
                            Color32::LIGHT_RED,
                            "Stage failed · Waiting for everyone to finish",
                        );
                    } else if !self.results {
                        ui.small("D F J K / Arrows     − / + Scroll speed");
                    }
                });
            });
        if let Some(seconds) = self.countdown.filter(|s| *s > 0.) {
            egui::Area::new(egui::Id::unique("round-countdown"))
                .anchor(Align2::CENTER_CENTER, [80., -100.])
                .show(ctx, |ui| {
                    panel().show(ui, |ui| {
                        ui.label(
                            RichText::new(format!("{:.0}", seconds.ceil()))
                                .size(72.)
                                .strong(),
                        );
                    });
                });
        }
        let mut confirmed = false;
        if self.results {
            let id = egui::Id::unique("round-results");
            egui::Modal::new(id)
                .area(egui::Modal::default_area(id).fade_in(false))
                .show(ctx, |ui| {
                    ui.set_width((ctx.content_rect().width() - 80.).clamp(220., 560.));
                    ui.heading("Results");
                    ui.label(self.title);
                    if self.disconnected {
                        ui.colored_label(Color32::LIGHT_RED, "Connection lost");
                    }
                    ui.separator();
                    egui::ScrollArea::vertical()
                        .max_height((ctx.content_rect().height() - 240.).max(100.))
                        .show(ui, |ui| {
                            egui::Grid::new("round-results-table")
                                .spacing(egui::vec2(20., 8.))
                                .striped(true)
                                .show(ui, |ui| {
                                    for heading in ["Player", "Score", "Max combo", "Status"] {
                                        ui.strong(heading);
                                    }
                                    ui.end_row();
                                    for standing in self.standings {
                                        ui.label(self.name(standing.player));
                                        ui.label(standing.points.to_string());
                                        ui.label(standing.max_combo.to_string());
                                        ui.label(self.status(standing));
                                        ui.end_row();
                                    }
                                });
                        });
                    ui.separator();
                    confirmed = crate::theme::primary(ui, "OK", [ui.available_width(), 36.], true)
                        .clicked();
                });
        }
        confirmed
    }
}
impl SelectorUi {
    #[allow(clippy::too_many_arguments)]
    pub fn draw_round(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        size: [u32; 2],
        window: Option<&Window>,
        hud: RoundHud<'_>,
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
        let mut confirmed = false;
        let output = self.context.run_ui(raw, |ui| {
            confirmed = hud.show(ui.ctx());
        });
        self.paint_output(device, queue, view, size, window, output, true);
        if confirmed {
            Action::Multiplayer(LobbyAction::ConfirmResults)
        } else {
            Action::None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn results_require_ok_and_survive_escape_and_background_clicks() {
        let mut selector = crate::tests::selector();
        selector.lobby.player = Some(1);
        let room: Room = serde_json::from_value(serde_json::json!({
            "keys":4,"levels":[null,null,null],"password_protected":false,"id":1,
            "name":"Room","host":1,"members":[{"id":1,"name":"Dancer","slot":0,
            "x":0.5,"y":0.5,"walking":[0,0],"facing":[0,1],"ready":false}],
            "chart":null,"round":null,"start_us":null
        }))
        .unwrap();
        let standings = [Standing {
            player: 1,
            points: 1234,
            ..Default::default()
        }];
        let hud = RoundHud {
            room: &room,
            player: Some(1),
            standings: &standings,
            title: "Song",
            seconds: 20.,
            duration: 20.,
            countdown: None,
            failed: false,
            results: true,
            disconnected: false,
        };
        let ctx = egui::Context::default();
        let mut confirmed = false;
        for _ in 0..2 {
            let mut out = ctx.run_ui(Default::default(), |ui| {
                confirmed = hud.show(ui.ctx());
            });
            out.textures_delta.clear();
        }
        assert!(!confirmed);
        let raw = egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
            ..Default::default()
        };
        let mut out = ctx.run_ui(raw, |ui| {
            confirmed = hud.show(ui.ctx());
        });
        out.textures_delta.clear();
        assert!(!confirmed);
        let button = crate::tests::text_center(&out, "OK").expect("Results must have an OK button");
        for pressed in [true, false] {
            let raw = egui::RawInput {
                events: vec![
                    egui::Event::PointerMoved(button),
                    egui::Event::PointerButton {
                        pos: button,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
                ..Default::default()
            };
            let mut out = ctx.run_ui(raw, |ui| {
                confirmed = hud.show(ui.ctx());
            });
            out.textures_delta.clear();
        }
        assert!(confirmed, "Only OK acknowledges the results");
    }
}
