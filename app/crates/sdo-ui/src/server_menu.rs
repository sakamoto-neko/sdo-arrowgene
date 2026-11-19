//! Server connection choices and the multi-room lobby.
use super::multiplayer::room_lock;
use super::{Action, LobbyAction, LobbyView, ServerMenu};
use crate::theme;
use egui::RichText;

fn field(ui: &mut egui::Ui, label: &str, value: &mut String, hint: &str, password: bool) {
    ui.label(label);
    ui.add_sized(
        [ui.available_width(), 34.],
        egui::TextEdit::singleline(value)
            .password(password)
            .hint_text(hint),
    );
    ui.add_space(6.);
}
fn card(ui: &mut egui::Ui, title: &str, description: &str) -> bool {
    theme::panel(18)
        .show(ui, |ui| {
            let clicked = ui
                .add_sized(
                    [ui.available_width(), 64.],
                    egui::Button::new(RichText::new(title).size(23.).strong())
                        .fill(theme::SELECTED),
                )
                .clicked();
            ui.add_space(6.);
            ui.label(RichText::new(description).color(theme::MUTED));
            clicked
        })
        .inner
}
impl LobbyView {
    pub(super) fn show_server_menu(&mut self, ui: &mut egui::Ui) -> Action {
        let mut action = Action::None;
        let form_width = if self.server_menu == ServerMenu::Choose {
            840.
        } else {
            620.
        };
        let margin = ((ui.available_width() - form_width) * 0.5).max(0.);
        egui::ScrollArea::vertical().id_salt("server-connection-form").show(ui, |ui| {
        ui.horizontal_top(|ui| {
            ui.add_space(margin);
            ui.vertical(|ui| {
                ui.set_width((ui.available_width() - margin).clamp(280., form_width));
                ui.add_space(20.);
                if !self.status.is_empty() { ui.label(&self.status); ui.add_space(12.); }
                if let Some(address) = &self.hosting {
                    ui.horizontal(|ui| {
                        ui.label(format!("Your server is running at {address}"));
                        if ui.button("Stop server").clicked() {
                            action = Action::Multiplayer(LobbyAction::StopHosting);
                        }
                    });
                    ui.add_space(12.);
                }
                if self.server_menu == ServerMenu::Choose {
                    if ui.button("Back").clicked() { action = Action::Multiplayer(LobbyAction::Exit); }
                    ui.add_space(12.);
                    ui.label(RichText::new("Play together").size(30.).strong());
                    if cfg!(target_arch="wasm32") { ui.label("Join a server to play in a multiplayer room."); }
                    else {ui.label("Start a server for friends, or connect to one that is already running.");}
                    ui.add_space(24.);
                    ui.columns(if cfg!(target_arch="wasm32"){1}else{2}, |columns| {
                        if !cfg!(target_arch="wasm32") && card(&mut columns[0], "Host Server",
                            "Open a room on this computer. Friends connect directly to your session.") {
                            self.server_menu = ServerMenu::Host;
                        }
                        if card(columns.last_mut().unwrap(), "Join Server",
                            "Enter a friend's server address. Join a room there, or enter their private room directly.") {
                            self.server_menu = ServerMenu::Join;
                        }
                    });
                    return;
                }
                if ui.button("Back").clicked() {
                    self.server_menu = ServerMenu::Choose;
                    self.room_password.clear();
                    return;
                }
                ui.add_space(12.);
                let hosting = self.server_menu == ServerMenu::Host;
                ui.label(RichText::new(if hosting { "Host a server" } else { "Join a server" }).size(28.).strong());
                ui.label(if hosting {
                    "Start one room for friends on this computer."
                } else {
                    "Use the address shared by the server owner."
                });
                ui.add_space(18.);
                theme::panel(20).show(ui, |ui| {
                        ui.set_max_width(600.);
                        field(ui, "Player name", &mut self.preferences.name, "Your name in the room", false);
                        if hosting {
                            field(ui, "Room name", &mut self.room_name, "e.g. Friday practice", false);
                            field(ui, "Room password (optional)", &mut self.room_password, "Leave blank for an open room", true);
                            ui.add_space(4.);
                            ui.small("Friends connect directly to this room.");
                            ui.add_space(10.);
                            egui::CollapsingHeader::new("Network settings").show(ui, |ui| {
                                field(ui, "Listen address", &mut self.preferences.listen, "0.0.0.0:32100", false);
                                ui.small("Friends use this computer's LAN or public address.");
                            });
                            ui.add_space(14.);
                            if theme::primary(ui, "Start server", [ui.available_width(), 42.], self.valid_room_name() && !self.preferences.name.trim().is_empty() && self.hosting.is_none()).clicked() {
                                action = Action::Multiplayer(LobbyAction::Host);
                            }
                        } else {
                            field(ui, "Server address", &mut self.preferences.address, sdo_net::DEFAULT_ADDRESS, false);
                            ui.small("A friend's hosted session opens its room directly. Dedicated servers show a room list.");
                            ui.add_space(14.);
                            if theme::primary(ui, "Connect", [ui.available_width(), 42.], !self.preferences.name.trim().is_empty() && !self.preferences.address.trim().is_empty()).clicked() {
                                action = Action::Multiplayer(LobbyAction::Connect);
                            }
                        }
                    });
            });
        });
        });
        action
    }
    fn connection_bar(&self, ui: &mut egui::Ui) -> Action {
        let mut action = Action::None;
        ui.horizontal(|ui| {
            ui.label(&self.preferences.address);
            if let Some(rtt) = self.rtt_ms {
                ui.small(format!("Ping {rtt:.0} ms"));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Disconnect").clicked() {
                    action = Action::Multiplayer(LobbyAction::Disconnect);
                }
                if self.hosting.is_some() && ui.button("Stop server").clicked() {
                    action = Action::Multiplayer(LobbyAction::StopHosting);
                }
            });
        });
        if !self.status.is_empty() {
            ui.small(&self.status);
        }
        ui.add_space(12.);
        action
    }
    pub(super) fn show_private_admission(&mut self, ui: &mut egui::Ui) -> Action {
        let mut action = self.connection_bar(ui);
        ui.heading("Joining room");
        if let Some(room) = self.directory.first().cloned() {
            ui.label(&room.name);
            if self.password_room.is_none() {
                if room.password_protected {
                    if ui.button("Enter room password").clicked() {
                        self.request_room_password(room.id, room.name);
                    }
                } else if ui
                    .add_enabled(
                        !room.playing && room.occupants < sdo_net::ROOM_SLOTS,
                        egui::Button::new("Retry joining"),
                    )
                    .clicked()
                {
                    action = Action::Multiplayer(LobbyAction::Join(room.id));
                }
            }
        } else {
            ui.label("Waiting for the host to open the private room…");
        }
        action
    }
    pub(super) fn show_server_lobby(&mut self, ui: &mut egui::Ui) -> Action {
        let mut action = self.connection_bar(ui);
        ui.horizontal(|ui| {
            ui.heading("Rooms on this server");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::primary(ui, "Create Room", [140., 36.], true).clicked() {
                    self.create_room_open = true;
                    self.room_error.clear();
                }
            });
        });
        let list_height = ((ui.available_height() - 76.) * 0.55).max(80.);
        let roster_width = (ui.available_width() * 0.32).clamp(110., 290.);
        let rooms_width =
            (ui.available_width() - roster_width - ui.spacing().item_spacing.x).max(100.);
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(rooms_width, list_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| self.room_list(ui, list_height, &mut action),
            );
            ui.allocate_ui_with_layout(
                egui::vec2(roster_width, list_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.strong(format!("Online players ({})", self.online_players.len()));
                    egui::ScrollArea::vertical()
                        .id_salt("online-players")
                        .max_height(ui.available_height())
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            for player in &self.online_players {
                                let location = player
                                    .room
                                    .and_then(|id| self.directory.iter().find(|room| room.id == id))
                                    .map_or("Lobby", |room| room.name.as_str());
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(&player.name);
                                    ui.weak(location);
                                });
                            }
                        });
                },
            );
        });
        ui.separator();
        ui.strong("Lobby chat");
        self.chat_history(ui, (ui.available_height() - 42.).max(40.));
        self.chat_input(ui, &mut action);
        action
    }

    fn room_list(&mut self, ui: &mut egui::Ui, height: f32, action: &mut Action) {
        let mut request = None;
        egui::ScrollArea::vertical()
            .id_salt("server-room-list")
            .max_height(height)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if self.directory.is_empty() {
                    ui.label("No rooms yet.");
                }
                for room in &self.directory {
                    ui.push_id(room.id, |ui| {
                        theme::panel(16).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                if room.password_protected {
                                    room_lock(ui);
                                }
                                ui.label(RichText::new(&room.name).size(20.).strong());
                                ui.small(format!("{}/8 players", room.occupants));
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if theme::primary(
                                            ui,
                                            "Join",
                                            [90., 34.],
                                            !room.playing && room.occupants < 8,
                                        )
                                        .clicked()
                                        {
                                            if room.password_protected {
                                                request = Some((room.id, room.name.clone()));
                                            } else {
                                                *action =
                                                    Action::Multiplayer(LobbyAction::Join(room.id));
                                            }
                                        }
                                    },
                                );
                            });
                            ui.small(if room.playing {
                                "Round in progress"
                            } else if room.occupants >= 8 {
                                "Room full"
                            } else {
                                "Open for players"
                            });
                        });
                    });
                    ui.add_space(8.);
                }
            });
        if let Some((id, name)) = request {
            self.request_room_password(id, name);
        }
    }
    pub(super) fn show_create_room_prompt(&mut self, ctx: &egui::Context) -> Action {
        if self.private_server || self.room().is_some() {
            self.create_room_open = false;
            return Action::None;
        }
        let mut submit = false;
        let mut cancel = false;
        let id = egui::Id::unique("create-room");
        let response = egui::Modal::new(id)
            .area(egui::Modal::default_area(id).fade_in(false))
            .frame(
                egui::Frame::popup(&ctx.global_style())
                    .inner_margin(20.)
                    .fill(egui::Color32::from_rgb(20, 30, 45)),
            )
            .show(ctx, |ui| {
                ui.set_width(420.);
                ui.heading("Create a room");
                ui.label("As room owner, you choose the song and start each round.");
                ui.add_space(12.);
                field(
                    ui,
                    "Room name",
                    &mut self.room_name,
                    "e.g. Evening practice",
                    false,
                );
                field(
                    ui,
                    "Room password (optional)",
                    &mut self.room_password,
                    "Leave blank for no password",
                    true,
                );
                if !self.room_error.is_empty() {
                    ui.colored_label(egui::Color32::LIGHT_RED, &self.room_error);
                }
                ui.horizontal(|ui| {
                    cancel = ui
                        .add_enabled(!self.creating_room, egui::Button::new("Cancel"))
                        .clicked();
                    submit = ui
                        .add_enabled(
                            !self.creating_room && self.valid_room_name(),
                            egui::Button::new("Create room"),
                        )
                        .clicked();
                    if self.creating_room {
                        ui.spinner();
                    }
                });
            });
        if !self.creating_room && (cancel || response.should_close()) {
            self.create_room_open = false;
            self.room_password.clear();
        }
        if submit {
            return Action::Multiplayer(LobbyAction::Create);
        }
        Action::None
    }
}
