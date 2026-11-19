use crate::Action;
use sdo_net::{ChatMessage, PlayerId, Preferences, Room, Standing};
use sdo_render::character::Portrait;
use std::collections::BTreeMap;
#[derive(Debug, PartialEq, Eq)]
pub enum LobbyAction {
    ConfirmResults,
    Exit,
    SelectSong(String),
    Connect,
    Disconnect,
    Host,
    StopHosting,
    Create,
    Join(u64),
    JoinWithPassword(u64, String),
    Leave,
    Ready,
    Unready,
    Start,
    TransferHost(u64),
    SetDifficulty(u8),
    SendChat,
    TakeSlot(u8),
}
pub struct PlayerPortrait {
    pub data: Option<Portrait>,
    texture: Option<egui::TextureHandle>,
    body_texture: Option<egui::TextureHandle>,
}
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServerMenu {
    #[default]
    Choose,
    Host,
    Join,
}
#[derive(Clone, Debug)]
pub struct RoomSongSelection {
    pub room: u64,
    pub song: String,
    pub kind: String,
    pub difficulty: usize,
    pub judgment: String,
}
#[derive(Default)]
pub struct LobbyView {
    pub(super) appearance_preview: Option<crate::AppearancePreview>,
    pub stage_texture: Option<egui::TextureId>,
    pub head_texture: Option<egui::TextureId>,
    pub stage_aspect: f32,
    pub scene_loading: bool,
    pub available_avatars: std::collections::BTreeSet<PlayerId>,
    pub avatar_issues: BTreeMap<PlayerId, String>,
    pub stage_labels: BTreeMap<PlayerId, [f32; 2]>,
    pub open: bool,
    pub song_picker_open: bool,
    pub song_preview: Option<String>,
    pub song_selection: Option<RoomSongSelection>,
    pub preferences: Preferences,
    pub server_menu: ServerMenu,
    pub create_room_open: bool,
    pub creating_room: bool,
    pub room_error: String,
    pub scroll_speed: sdo_config::ScrollSpeed,
    pub player: Option<PlayerId>,
    pub connecting: bool,
    pub directory: Vec<sdo_net::RoomSummary>,
    pub joined_room: Option<Room>,
    pub room_name: String,
    pub room_password: String,
    pub private_server: bool,
    pub password_room: Option<(u64, String)>,
    pub join_password: String,
    pub password_error: String,
    pub joining_room: bool,
    password_prompt_focus: bool,
    pub status: String,
    pub compatibility: String,
    pub matching_chart: bool,
    pub synchronized: bool,
    pub rtt_ms: Option<f64>,
    pub hosting: Option<String>,
    pub standings: Vec<Standing>,
    pub results: bool,
    pub online_players: Vec<sdo_net::OnlinePlayer>,
    pub chat_room: Option<u64>,
    pub messages: Vec<ChatMessage>,
    pub chat_draft: String,
    pub portraits: BTreeMap<PlayerId, PlayerPortrait>,
    pub preview_status: String,
    pub walk_direction: [i8; 2],

    stage_positions: BTreeMap<PlayerId, [f32; 2]>,
    stage_rect: Option<egui::Rect>,
    input_rect: Option<egui::Rect>,
    send_rect: Option<egui::Rect>,
}
impl LobbyView {
    pub fn room(&self) -> Option<&Room> {
        self.joined_room.as_ref()
    }
    pub fn stop_walking(&mut self) {
        self.walk_direction = [0, 0];
    }
    pub fn valid_room_name(&self) -> bool {
        let name = self.room_name.trim();
        !name.is_empty()
            && name.len() <= 60
            && !name.chars().any(char::is_control)
            && self.room_password.len() <= sdo_net::MAX_ROOM_PASSWORD_BYTES
            && !self.room_password.chars().any(char::is_control)
    }
    pub fn receive_portrait(&mut self, player: PlayerId, data: Option<Portrait>) {
        let portrait = self.portraits.entry(player).or_insert(PlayerPortrait {
            data: None,
            texture: None,
            body_texture: None,
        });
        if let Some(data) = &data {
            if let Some(texture) = &mut portrait.texture {
                texture.set(
                    egui::ColorImage::from_rgba_unmultiplied(
                        sdo_render::character::PORTRAIT_SIZE.map(|n| n as usize),
                        &data.rgba,
                    ),
                    egui::TextureOptions::LINEAR,
                );
            }
            if let Some(texture) = &mut portrait.body_texture {
                if data.avatar.len() == sdo_render::character::AVATAR_BYTES {
                    texture.set(
                        egui::ColorImage::from_rgba_unmultiplied(
                            sdo_render::character::AVATAR_SIZE.map(|n| n as usize),
                            &data.avatar,
                        ),
                        egui::TextureOptions::LINEAR,
                    );
                } else {
                    portrait.body_texture = None;
                }
            }
        } else {
            portrait.texture = None;
            portrait.body_texture = None;
        }
        portrait.data = data;
    }

    pub fn receive_chat(&mut self, room: Option<u64>, message: ChatMessage) {
        if self.chat_room != room {
            self.chat_room = room;
            self.messages.clear();
        }
        if self.messages.last().is_none_or(|last| message.id > last.id) {
            self.messages.push(message);
        }
        if self.messages.len() > sdo_net::MAX_CHAT_MESSAGES {
            self.messages.remove(0);
        }
    }
    pub fn request_room_password(&mut self, room: u64, name: String) {
        self.clear_password_prompt();
        self.password_room = Some((room, name));
        self.password_prompt_focus = true;
    }
    pub fn clear_password_prompt(&mut self) {
        self.password_room = None;
        self.join_password.clear();
        self.password_error.clear();
        self.joining_room = false;
        self.password_prompt_focus = false;
    }
    pub(super) fn show_password_prompt(&mut self, ctx: &egui::Context) -> Action {
        let Some((room, name)) = self.password_room.clone() else {
            return Action::None;
        };
        let mut submit = false;
        let mut cancel = false;
        let id = egui::Id::unique("room-password");
        let response = egui::Modal::new(id)
            .area(egui::Modal::default_area(id).fade_in(false))
            .show(ctx, |ui| {
                ui.set_width(360.);
                ui.heading("Room password");
                ui.label(name);
                let input = ui.add_sized(
                    [ui.available_width(), 32.],
                    egui::TextEdit::singleline(&mut self.join_password)
                        .id(egui::Id::unique("room-password-input"))
                        .password(true)
                        .char_limit(sdo_net::MAX_ROOM_PASSWORD_BYTES)
                        .hint_text("Enter the room password"),
                );
                if self.password_prompt_focus {
                    input.request_focus();
                    self.password_prompt_focus = false;
                }
                if !self.password_error.is_empty() {
                    ui.colored_label(egui::Color32::LIGHT_RED, &self.password_error);
                }
                ui.horizontal(|ui| {
                    cancel = ui
                        .add_enabled(!self.joining_room, egui::Button::new("Cancel"))
                        .clicked();
                    submit = ui
                        .add_enabled(
                            !self.joining_room && !self.join_password.is_empty(),
                            egui::Button::new("Join room"),
                        )
                        .clicked();
                    if self.joining_room {
                        ui.spinner();
                    }
                });
                submit |= !self.joining_room
                    && !self.join_password.is_empty()
                    && ui.input_mut(|input| {
                        input.consume_key(egui::Modifiers::NONE, egui::Key::Enter)
                    });
            });
        if (cancel || response.should_close()) && !self.joining_room {
            self.clear_password_prompt();
            return if self.private_server {
                Action::Multiplayer(LobbyAction::Disconnect)
            } else {
                Action::None
            };
        }
        if submit {
            self.password_error.clear();
            return Action::Multiplayer(LobbyAction::JoinWithPassword(
                room,
                self.join_password.clone(),
            ));
        }
        Action::None
    }
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        appearance: &mut sdo_assets::UiSelection,
        choices: &sdo_assets::UiChoices,
        character_view: &mut bool,
        personal_best: Option<u64>,
    ) -> Action {
        if let Some(room) = self.room().cloned() {
            return self.show_room(
                ui,
                &room,
                appearance,
                choices,
                character_view,
                personal_best,
            );
        }
        self.walk_direction = [0, 0];
        if self.connecting {
            ui.add_space(28.);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.heading("Connecting to server");
            });
            ui.label(&self.preferences.address);
            if ui.button("Cancel connection").clicked() {
                return Action::Multiplayer(LobbyAction::Disconnect);
            }
            return Action::None;
        }
        if self.player.is_none() {
            self.show_server_menu(ui)
        } else if self.private_server {
            self.show_private_admission(ui)
        } else {
            self.show_server_lobby(ui)
        }
    }
    fn show_room(
        &mut self,
        ui: &mut egui::Ui,
        room: &Room,
        appearance: &mut sdo_assets::UiSelection,
        choices: &sdo_assets::UiChoices,
        character_view: &mut bool,
        personal_best: Option<u64>,
    ) -> Action {
        let mut action = Action::None;
        ui.horizontal(|ui| {
            if room.password_protected {
                room_lock(ui);
            }
            ui.label(egui::RichText::new(&room.name).strong().size(20.));
            ui.small(format!("{}/8 players", room.members.len()));
            if let Some(rtt) = self.rtt_ms {
                ui.small(format!("{rtt:.0} ms"));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Leave room").clicked() {
                    action = Action::Multiplayer(LobbyAction::Leave);
                    self.walk_direction = [0, 0];
                }
                if Some(room.host) == self.player
                    && room.round.is_none()
                    && ui.button("Choose song").clicked()
                {
                    self.song_picker_open = true;
                    self.walk_direction = [0, 0];
                }
                if ui.button("Character").clicked() {
                    *character_view = true;
                    self.walk_direction = [0, 0];
                }
            });
        });
        self.portrait_strip(ui, room, &mut action);
        ui.add_space(4.);
        let chat_height = 34. + ui.spacing().item_spacing.y * 2.;
        let stage_height = (ui.available_height() - chat_height).max(140.);
        let side = (ui.available_width() * 0.235).clamp(220., 270.);
        let gap = ui.spacing().item_spacing.x;
        let width = (ui.available_width() - side - gap).max(280.);
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(width, stage_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    let history_height = (stage_height * 0.24).clamp(50., 108.);
                    let walk_height = stage_height - history_height - ui.spacing().item_spacing.y;
                    self.walking_area(ui, room, egui::vec2(width, walk_height));
                    self.chat_history(ui, history_height);
                },
            );
            ui.allocate_ui_with_layout(
                egui::vec2(side, stage_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    crate::theme::panel(10).show(ui, |ui| {
                        let side = ui.available_width();
                        egui::ScrollArea::vertical()
                            .id_salt("room-controls")
                            .max_height((stage_height - 22.).max(1.))
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                if let Some(chart) = &room.chart {
                                    ui.label(egui::RichText::new(&chart.title).size(18.).strong());
                                    let level = room.levels[chart.difficulty as usize]
                                        .map_or_else(|| "—".to_owned(), |level| level.to_string());
                                    ui.label(format!(
                                        "{} · Level {level}",
                                        ["Easy", "Normal", "Hard"][chart.difficulty as usize]
                                    ));
                                    let seconds = chart.duration_us / 1_000_000;
                                    ui.label(format!(
                                        "{} keys · {}:{:02}",
                                        room.keys,
                                        seconds / 60,
                                        seconds % 60
                                    ));
                                    ui.small(format!(
                                        "{:.0} BPM · Scroll BPM {:.0}",
                                        chart.bpm,
                                        chart.bpm * self.scroll_speed.multiplier() as f64
                                    ));
                                    ui.label(personal_best.map_or_else(
                                        || "Personal best —".to_owned(),
                                        |score| format!("Personal best {score}"),
                                    ));
                                    ui.separator();
                                    let host = Some(room.host) == self.player;
                                    if host && room.round.is_none() {
                                        ui.horizontal(|ui| {
                                            for (i, label) in
                                                ["Easy", "Normal", "Hard"].iter().enumerate()
                                            {
                                                if ui
                                                    .selectable_label(
                                                        chart.difficulty as usize == i,
                                                        *label,
                                                    )
                                                    .clicked()
                                                    && chart.difficulty as usize != i
                                                {
                                                    action = Action::Multiplayer(
                                                        LobbyAction::SetDifficulty(i as u8),
                                                    );
                                                }
                                            }
                                        });
                                        ui.separator();
                                    }
                                    self.room_player_controls(ui, appearance, choices);
                                    ui.add_space(8.);
                                    if room.round.is_none() {
                                        if host {
                                            let others_ready = room.members.len() >= 2
                                                && room.members.iter().all(|member| {
                                                    member.id == room.host || member.ready
                                                });
                                            if crate::theme::primary(
                                                ui,
                                                "Start",
                                                [side, 38.],
                                                others_ready
                                                    && self.matching_chart
                                                    && !self.scene_loading
                                                    && self.synchronized,
                                            )
                                            .clicked()
                                            {
                                                action = Action::Multiplayer(LobbyAction::Start);
                                            }
                                        } else {
                                            let ready = room
                                                .members
                                                .iter()
                                                .find(|member| Some(member.id) == self.player)
                                                .is_some_and(|member| member.ready);
                                            if ready {
                                                if ui
                                                    .add_sized(
                                                        [side, 36.],
                                                        egui::Button::new("Not Ready"),
                                                    )
                                                    .clicked()
                                                {
                                                    action =
                                                        Action::Multiplayer(LobbyAction::Unready);
                                                }
                                            } else if crate::theme::primary(
                                                ui,
                                                "Ready",
                                                [side, 36.],
                                                self.matching_chart
                                                    && self.synchronized
                                                    && !self.scene_loading,
                                            )
                                            .clicked()
                                            {
                                                action = Action::Multiplayer(LobbyAction::Ready);
                                            }
                                        }
                                    } else {
                                        ui.label("Round in progress");
                                    }
                                    if !self.matching_chart && !self.compatibility.is_empty() {
                                        ui.small(&self.compatibility);
                                    }
                                } else {
                                    ui.label("Waiting for the host to select a song");
                                    ui.separator();
                                    self.room_player_controls(ui, appearance, choices);
                                }
                                if let Some(preview) = &mut self.appearance_preview {
                                    preview.show(ui, appearance);
                                }
                                if self.scene_loading {
                                    let failures = room
                                        .members
                                        .iter()
                                        .filter_map(|member| {
                                            self.avatar_issues
                                                .get(&member.id)
                                                .map(|issue| (member, issue))
                                        })
                                        .collect::<Vec<_>>();
                                    if failures.is_empty() {
                                        ui.small(
                                            "Loading compatible character and animation assets…",
                                        );
                                    } else {
                                        for (member, issue) in failures {
                                            ui.colored_label(
                                                egui::Color32::LIGHT_RED,
                                                format!(
                                                    "{}: character assets unavailable",
                                                    member.name
                                                ),
                                            )
                                            .on_hover_text(issue);
                                        }
                                    }
                                }
                            });
                    });
                },
            );
        });
        self.chat_input(ui, &mut action);
        action
    }
    fn portrait_strip(&mut self, ui: &mut egui::Ui, room: &Room, action: &mut Action) {
        let width = (ui.available_width() - 7. * 5.) / 8.;
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 5.;
            for slot in 0..8 {
                crate::theme::panel(4)
                    .show(ui, |ui| {
                        ui.set_width((width - 10.).max(60.));
                        ui.set_min_height(130.);
                        let portrait_size = egui::Vec2::splat(
                            ui.available_width().min(sdo_render::character::PORTRAIT_SIZE[0] as f32),
                        );
                        ui.vertical_centered(|ui| {
                            if let Some(member) =
                                room.members.iter().find(|m| m.slot as usize == slot)
                            {
                                if let Some(atlas)=self.head_texture.filter(|_|self.available_avatars.contains(&member.id)) {
                                    let left=f32::from(member.slot)/8.;
                                    let uv=egui::Rect::from_min_max(egui::pos2(left,0.),egui::pos2(left+1./8.,1.));
                                    ui.add(egui::Image::new((atlas,portrait_size)).uv(uv));
                                } else if let Some(portrait) = self.portraits.get_mut(&member.id)
                                    && let Some(data) = &portrait.data
                                {
                                    let texture = portrait.texture.get_or_insert_with(|| {
                                        ui.ctx().load_texture(
                                            format!("head-{}", member.id),
                                            egui::ColorImage::from_rgba_unmultiplied(
                                                sdo_render::character::PORTRAIT_SIZE.map(|n| n as usize),
                                                &data.rgba,
                                            ),
                                            egui::TextureOptions::LINEAR,
                                        )
                                    });
                                    ui.add(egui::Image::new((texture.id(), portrait_size)));
                                } else {
                                    small_placeholder(ui, portrait_size);
                                }
                                if let Some(issue)=self.avatar_issues.get(&member.id) {ui.small("Character unavailable").on_hover_text(issue);}
                                ui.label(
                                    egui::RichText::new(
                                        member.name.chars().take(14).collect::<String>(),
                                    )
                                    .strong()
                                    .color(if Some(member.id) == self.player {
                                        crate::theme::ACCENT
                                    } else { egui::Color32::WHITE }),
                                )
                                .on_hover_text(&member.name);
                                ui.small(if member.id == room.host { "Host" }
                                    else if member.ready { "Ready" } else { "Not Ready" });
                                if !self.private_server && Some(room.host) == self.player
                                    && member.id != room.host && room.round.is_none()
                                    && ui.small_button("Make host")
                                        .on_hover_text("Give this player control of the song, difficulty and round start").clicked() {
                                    *action = Action::Multiplayer(LobbyAction::TransferHost(member.id));
                                }
                            } else {
                                small_placeholder(ui, portrait_size);
                                ui.small("Open slot");
                                if room.round.is_none() && ui.small_button("Move here").clicked() {
                                    *action =
                                        Action::Multiplayer(LobbyAction::TakeSlot(slot as u8));
                                }
                            }
                        });
                    });
            }
        });
    }
    fn walking_area(&mut self, ui: &mut egui::Ui, room: &Room, size: egui::Vec2) {
        let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
        self.stage_rect = Some(rect);
        let painter = ui.painter_at(rect);
        let stage = WalkingStage::new(rect);
        self.stage_aspect = rect.width() / rect.height().max(1.);
        if let Some(texture) = self.stage_texture {
            painter.image(
                texture,
                rect,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.)),
                egui::Color32::WHITE,
            );
        } else {
            stage.paint(&painter, rect);
        }
        // Clicking only focuses the room; it never chooses a movement target.
        let editing_controls = ui.ctx().egui_wants_keyboard_input() && !response.has_focus();
        if !self.song_picker_open
            && !editing_controls
            && (response.clicked() || !ui.ctx().text_edit_focused())
        {
            response.request_focus();
        }
        if response.has_focus() {
            ui.memory_mut(|memory| {
                memory.set_focus_lock_filter(
                    response.id,
                    egui::EventFilter {
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        ..Default::default()
                    },
                )
            });
        }
        let keyboard = !self.song_picker_open
            && !editing_controls
            && !egui::Popup::is_any_open(ui.ctx())
            && (!ui.ctx().text_edit_focused() || response.has_focus())
            && room.round.is_none();
        self.walk_direction = if keyboard {
            ui.input(|i| {
                [
                    i.key_down(egui::Key::ArrowRight) as i8
                        - i.key_down(egui::Key::ArrowLeft) as i8,
                    i.key_down(egui::Key::ArrowDown) as i8 - i.key_down(egui::Key::ArrowUp) as i8,
                ]
            })
        } else {
            [0, 0]
        };
        let blend = (ui.input(|i| i.stable_dt).min(0.1) * 14.).min(1.);
        let mut members = room.members.iter().collect::<Vec<_>>();
        members.sort_by(|a, b| a.y.total_cmp(&b.y));
        for member in members.into_iter().filter(|_| self.stage_texture.is_none()) {
            let position = self
                .stage_positions
                .entry(member.id)
                .or_insert([member.x, member.y]);
            position[0] += (member.x - position[0]) * blend;
            position[1] += (member.y - position[1]) * blend;
            let body = stage.body(*position);
            painter.add(egui::Shape::ellipse_filled(
                stage.feet(*position),
                egui::vec2(body.height() * 0.16, body.height() * 0.035),
                egui::Color32::from_black_alpha(80),
            ));
            let mut drawn = false;
            if let Some(portrait) = self.portraits.get_mut(&member.id)
                && let Some(data) = &portrait.data
                && data.avatar.len() == sdo_render::character::AVATAR_BYTES
            {
                let texture = portrait.body_texture.get_or_insert_with(|| {
                    ui.ctx().load_texture(
                        format!("body-{}", member.id),
                        egui::ColorImage::from_rgba_unmultiplied(
                            sdo_render::character::AVATAR_SIZE.map(|n| n as usize),
                            &data.avatar,
                        ),
                        egui::TextureOptions::LINEAR,
                    )
                });
                painter.image(
                    texture.id(),
                    body,
                    egui::Rect::from_min_max(egui::pos2(0., 0.), egui::pos2(1., 1.)),
                    egui::Color32::WHITE,
                );
                drawn = true;
            }
            if !drawn {
                painter.circle_filled(body.center(), 15., egui::Color32::from_rgb(65, 86, 112));
            }
            painter.text(
                egui::pos2(body.center().x, body.top() - 4.),
                egui::Align2::CENTER_BOTTOM,
                &member.name,
                egui::FontId::proportional(14.),
                if Some(member.id) == self.player {
                    crate::theme::ACCENT
                } else {
                    egui::Color32::WHITE
                },
            );
        }
        if self.stage_texture.is_some() {
            for member in &room.members {
                if let Some([x, y]) = self.stage_labels.get(&member.id) {
                    painter.text(
                        egui::pos2(
                            rect.left() + x * rect.width(),
                            rect.top() + y * rect.height(),
                        ),
                        egui::Align2::CENTER_BOTTOM,
                        &member.name,
                        egui::FontId::proportional(14.),
                        if Some(member.id) == self.player {
                            crate::theme::ACCENT
                        } else {
                            egui::Color32::WHITE
                        },
                    );
                }
            }
        }
        if room.members.iter().any(|member| member.walking != [0, 0])
            || self.walk_direction != [0, 0]
        {
            ui.ctx().request_repaint();
        }
    }
    pub(super) fn chat_history(&self, ui: &mut egui::Ui, height: f32) {
        egui::ScrollArea::vertical()
            .id_salt(("chat-history", self.chat_room))
            .max_height(height)
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for message in &self.messages {
                    let mut text =
                        egui::RichText::new(format!("{}: {}", message.name, message.text));
                    if message.error {
                        text = text.color(egui::Color32::LIGHT_RED);
                    }
                    ui.add(egui::Label::new(text).wrap());
                }
            });
    }

    pub(super) fn chat_input(&mut self, ui: &mut egui::Ui, action: &mut Action) {
        ui.add_space(4.);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let valid = !self.chat_draft.trim().is_empty()
                && self.chat_draft.len() <= sdo_net::MAX_CHAT_BYTES;
            let send = ui.add_enabled(
                valid,
                egui::Button::new("Send").min_size(egui::vec2(60., 30.)),
            );
            self.send_rect = Some(send.rect);
            let hint = if self.room().is_some() {
                "Message the room · Enter to send"
            } else {
                "Message the lobby · Enter to send"
            };
            let response = ui.add_sized(
                [(ui.available_width() - 46.).max(100.), 30.],
                egui::TextEdit::singleline(&mut self.chat_draft).hint_text(hint),
            );
            self.input_rect = Some(response.rect);
            if response.has_focus() {
                self.walk_direction = [0, 0];
            }
            ui.label("Chat");
            let enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (enter || send.clicked()) && valid {
                *action = Action::Multiplayer(LobbyAction::SendChat);
                response.request_focus();
                self.walk_direction = [0, 0];
            }
        });
    }
}
/// The floor boundary and player positions share the server's movement limits.
struct WalkingStage {
    floor: egui::Rect,
    character_height: f32,
}
impl WalkingStage {
    fn new(rect: egui::Rect) -> Self {
        let character_height = (rect.height() * 0.5).min(190.);
        let inset = (character_height * 0.6).min(rect.width() * 0.24);
        Self {
            floor: egui::Rect::from_min_max(
                rect.min + egui::vec2(inset, character_height + 20.),
                rect.max - egui::vec2(inset, 26.),
            ),
            character_height,
        }
    }
    fn floor_point(&self, x: f32, depth: f32) -> egui::Pos2 {
        let inset = (1. - depth) * self.floor.width() * 0.06;
        egui::pos2(
            self.floor.left() + inset + x * (self.floor.width() - inset * 2.),
            egui::lerp(self.floor.y_range(), depth),
        )
    }
    fn normalized(position: [f32; 2]) -> [f32; 2] {
        std::array::from_fn(|axis| {
            ((position[axis] - sdo_net::ROOM_WALK_MIN[axis])
                / (sdo_net::ROOM_WALK_MAX[axis] - sdo_net::ROOM_WALK_MIN[axis]))
                .clamp(0., 1.)
        })
    }
    fn feet(&self, position: [f32; 2]) -> egui::Pos2 {
        let [x, depth] = Self::normalized(position);
        self.floor_point(x, depth)
    }
    fn body(&self, position: [f32; 2]) -> egui::Rect {
        let depth = Self::normalized(position)[1];
        let height = self.character_height * (0.88 + depth * 0.12);
        let aspect = sdo_render::character::AVATAR_SIZE[0] as f32
            / sdo_render::character::AVATAR_SIZE[1] as f32;
        // Keep the transparent camera margin below the feet off the floor anchor.
        egui::Rect::from_center_size(
            self.feet(position) - egui::vec2(0., height * 0.37),
            egui::vec2(height * aspect, height),
        )
    }
    fn paint(&self, painter: &egui::Painter, rect: egui::Rect) {
        use egui::{Color32, Shape, Stroke, pos2, vec2};
        painter.rect_filled(rect, 6., Color32::from_rgb(14, 22, 35));
        let horizon = self.floor.top() - 28.;
        painter.rect_filled(
            egui::Rect::from_min_max(rect.min, pos2(rect.right(), horizon)),
            6.,
            Color32::from_rgb(22, 33, 50),
        );
        for fraction in [0.2, 0.5, 0.8] {
            let x = egui::lerp(rect.x_range(), fraction);
            painter.line_segment(
                [pos2(x, rect.top() + 24.), pos2(x, horizon - 18.)],
                Stroke::new(2., Color32::from_rgb(32, 47, 65)),
            );
        }
        painter.line_segment(
            [
                pos2(rect.left() + 18., horizon),
                pos2(rect.right() - 18., horizon),
            ],
            Stroke::new(2., Color32::from_rgb(48, 77, 94)),
        );
        let corners = [
            self.floor_point(0., 0.),
            self.floor_point(1., 0.),
            self.floor_point(1., 1.),
            self.floor_point(0., 1.),
        ];
        painter.add(Shape::convex_polygon(
            vec![
                corners[3],
                corners[2],
                corners[2] + vec2(0., 10.),
                corners[3] + vec2(0., 10.),
            ],
            Color32::from_rgb(22, 35, 48),
            Stroke::NONE,
        ));
        let mut floor = egui::Mesh::default();
        for (index, corner) in corners.iter().enumerate() {
            floor.colored_vertex(
                *corner,
                if index < 2 {
                    Color32::from_rgb(36, 52, 69)
                } else {
                    Color32::from_rgb(54, 75, 89)
                },
            );
        }
        floor.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
        painter.add(Shape::mesh(floor));
        let grid = Stroke::new(1., Color32::from_rgb(65, 86, 100));
        for column in 1..8 {
            let x = column as f32 / 8.;
            painter.line_segment([self.floor_point(x, 0.), self.floor_point(x, 1.)], grid);
        }
        for row in 1..6 {
            let depth = row as f32 / 6.;
            painter.line_segment(
                [self.floor_point(0., depth), self.floor_point(1., depth)],
                grid,
            );
        }
        painter.add(Shape::closed_line(
            corners.to_vec(),
            Stroke::new(1.5, Color32::from_rgb(90, 163, 174)),
        ));
    }
}
pub(super) fn room_lock(ui: &mut egui::Ui) {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(16., 20.), egui::Sense::hover());
    let color = egui::Color32::from_rgb(229, 190, 102);
    ui.painter().add(egui::Shape::line(
        vec![
            rect.min + egui::vec2(4., 9.),
            rect.min + egui::vec2(4., 5.),
            rect.min + egui::vec2(6., 3.),
            rect.min + egui::vec2(10., 3.),
            rect.min + egui::vec2(12., 5.),
            rect.min + egui::vec2(12., 9.),
        ],
        egui::Stroke::new(2., color),
    ));
    ui.painter().rect_filled(
        egui::Rect::from_min_size(rect.min + egui::vec2(2., 8.), egui::vec2(12., 10.)),
        2.,
        color,
    );
    ui.painter().circle_filled(
        rect.min + egui::vec2(8., 12.),
        1.5,
        egui::Color32::from_rgb(32, 37, 46),
    );
    response.on_hover_text("Password protected");
}
fn small_placeholder(ui: &mut egui::Ui, size: egui::Vec2) {
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 5., egui::Color32::from_rgb(29, 41, 57));
    painter.circle_filled(
        rect.center() - egui::vec2(0., 10.),
        10.,
        egui::Color32::from_rgb(65, 84, 106),
    );
    painter.rect_filled(
        egui::Rect::from_center_size(rect.center() + egui::vec2(0., 17.), egui::vec2(30., 26.)),
        8.,
        egui::Color32::from_rgb(65, 84, 106),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{frame, frame_output, selector, text_center};
    fn click(context: &egui::Context, s: &mut crate::SongSelector, text: &str) -> Action {
        frame(context, s, vec![]);
        let output = frame_output(context, s, vec![]).1;
        let pos = text_center(&output, text).unwrap_or_else(|| panic!("Missing {text}"));
        frame(
            context,
            s,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                },
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default(),
                },
            ],
        )
    }
    fn spec() -> sdo_net::ChartSpec {
        sdo_net::ChartSpec {
            pattern_sha256: "a".repeat(64),
            audio_sha256: "b".repeat(64),
            dance_sha256: None,
            title: "Fixture".into(),
            kind: "k".into(),
            difficulty: 0,
            judgment: "keyboard".into(),
            bpm: 120.,
            duration_us: 100_000_000,
        }
    }
    #[test]
    fn walking_floor_matches_server_limits_and_keeps_characters_on_screen() {
        for size in [egui::vec2(854., 440.), egui::vec2(280., 140.)] {
            let rect = egui::Rect::from_min_size(egui::pos2(10., 20.), size);
            let stage = WalkingStage::new(rect);
            for (x, depth) in [(0., 0.), (1., 0.), (1., 1.), (0., 1.), (0.5, 0.5)] {
                let position = std::array::from_fn(|axis| {
                    egui::lerp(
                        sdo_net::ROOM_WALK_MIN[axis]..=sdo_net::ROOM_WALK_MAX[axis],
                        [x, depth][axis],
                    )
                });
                assert!(stage.feet(position).distance(stage.floor_point(x, depth)) < 0.01);
                let body = stage.body(position);
                assert!(
                    rect.contains_rect(body),
                    "Character clipped at {position:?}: {body:?}"
                );
            }
        }
    }
    #[test]
    fn lobby_controls_require_local_chart_clock_and_every_player_ready() {
        let context = egui::Context::default();
        let mut s = selector();
        click(&context, &mut s, "Multiplayer");
        assert!(s.lobby.open);
        click(&context, &mut s, "Join Server");
        assert_eq!(
            click(&context, &mut s, "Connect"),
            Action::Multiplayer(LobbyAction::Connect)
        );
        click(&context, &mut s, "Back");
        click(&context, &mut s, "Host Server");
        assert_eq!(click(&context, &mut s, "Start server"), Action::None);
        s.lobby.player = Some(1);
        assert_eq!(click(&context, &mut s, "Create Room"), Action::None);
        assert!(s.lobby.create_room_open);
        assert_eq!(click(&context, &mut s, "Create room"), Action::None);
        s.lobby.room_name = "Test Room".into();
        assert_eq!(
            click(&context, &mut s, "Create room"),
            Action::Multiplayer(LobbyAction::Create)
        );
        s.lobby.create_room_open = false;
        s.lobby.joined_room = Some(Room {
            keys: 4,
            levels: [None; 3],
            password_protected: false,
            id: 4,
            name: "Room".into(),
            host: 1,
            members: vec![
                sdo_net::Member {
                    state_us: 0,
                    x: 0.5,
                    y: 0.65,
                    facing: [1, 0],
                    walking: [0, 0],
                    slot: 0,
                    id: 1,
                    name: "Host".into(),
                    ready: false,
                },
                sdo_net::Member {
                    state_us: 0,
                    x: 0.5,
                    y: 0.65,
                    facing: [1, 0],
                    walking: [0, 0],
                    slot: 1,
                    id: 2,
                    name: "Guest".into(),
                    ready: false,
                },
            ],
            chart: Some(spec()),
            round: None,
            start_us: None,
        });
        s.lobby.receive_portrait(
            1,
            Some(Portrait {
                avatar: Vec::new(),
                character: "Male".into(),
                rgba: vec![180; sdo_render::character::PORTRAIT_BYTES],
            }),
        );
        let output = frame_output(&context, &mut s, vec![]).1;
        for slot in 1..=8 {
            assert!(text_center(&output, &format!("Slot {slot}")).is_none());
        }
        assert!(s.lobby.portraits[&1].texture.is_some());
        let texture = s.lobby.portraits[&1].texture.as_ref().unwrap().id();
        s.lobby.receive_portrait(
            1,
            Some(Portrait {
                character: "Male".into(),
                rgba: vec![90; sdo_render::character::PORTRAIT_BYTES],
                avatar: vec![],
            }),
        );
        frame(&context, &mut s, vec![]);
        assert_eq!(
            texture,
            s.lobby.portraits[&1].texture.as_ref().unwrap().id(),
            "Live frames must update the existing portrait texture"
        );

        assert!(text_center(&output, "Fixture").is_some());
        assert!(text_center(&output, "Selected song").is_none());
        assert!(text_center(&output, "Chart verification").is_none());
        assert!(text_center(&output, "Exit").is_none());
        assert!(text_center(&output, "Host controls").is_none());
        assert!(text_center(&output, "Your settings").is_none());
        assert_eq!(
            click(&context, &mut s, "Make host"),
            Action::Multiplayer(LobbyAction::TransferHost(2))
        );
        s.lobby.private_server = true;
        let gui_output = frame_output(&context, &mut s, vec![]).1;
        assert!(text_center(&gui_output, "Make host").is_none());
        s.lobby.private_server = false;
        s.lobby.player = Some(2);
        assert_eq!(click(&context, &mut s, "Ready"), Action::None);
        s.lobby.matching_chart = true;
        assert_eq!(click(&context, &mut s, "Ready"), Action::None);
        s.lobby.synchronized = true;
        assert_eq!(
            click(&context, &mut s, "Ready"),
            Action::Multiplayer(LobbyAction::Ready)
        );
        s.lobby.joined_room.as_mut().unwrap().members[1].ready = true;
        assert_eq!(
            click(&context, &mut s, "Not Ready"),
            Action::Multiplayer(LobbyAction::Unready)
        );
        s.lobby.joined_room.as_mut().unwrap().members[1].ready = false;
        s.lobby.player = Some(1);
        assert_eq!(click(&context, &mut s, "Start"), Action::None);
        s.lobby.joined_room.as_mut().unwrap().members[1].ready = true;
        assert!(!s.lobby.joined_room.as_mut().unwrap().members[0].ready);
        assert_eq!(
            click(&context, &mut s, "Start"),
            Action::Multiplayer(LobbyAction::Start)
        );
        assert_eq!(
            click(&context, &mut s, "Normal"),
            Action::Multiplayer(LobbyAction::SetDifficulty(1))
        );
        s.lobby.scroll_speed = sdo_config::ScrollSpeed::new(7).unwrap();
        s.lobby.receive_chat(
            Some(4),
            ChatMessage {
                id: 1,
                player: None,
                name: "System".into(),
                text: "Missing song".into(),
                error: true,
            },
        );
        s.lobby.chat_draft = "Hello room".into();
        let output = frame_output(&context, &mut s, vec![]).1;
        assert!(text_center(&output, "120 BPM · Scroll BPM 840").is_some());
        assert!(text_center(&output, "System: Missing song").is_some());
        assert_eq!(
            click(&context, &mut s, "Send"),
            Action::Multiplayer(LobbyAction::SendChat)
        );
        s.lobby.player = Some(2);
        let output = frame_output(&context, &mut s, vec![]).1;
        for host_control in ["Host difficulty", "Choose song", "Start"] {
            assert!(text_center(&output, host_control).is_none());
        }
    }

    #[test]
    fn protected_room_join_prompts_and_private_servers_hide_room_selection() {
        let ctx = egui::Context::default();
        let mut s = selector();
        s.lobby.open = true;
        s.lobby.player = Some(2);
        s.lobby.directory.push(sdo_net::RoomSummary {
            id: 7,
            name: "Locked room".into(),
            occupants: 1,
            playing: false,
            password_protected: true,
        });
        assert_eq!(click(&ctx, &mut s, "Join"), Action::None);
        assert_eq!(s.lobby.password_room.as_ref().map(|(id, _)| *id), Some(7));
        s.lobby.join_password = "secret".into();
        assert_eq!(
            click(&ctx, &mut s, "Join room"),
            Action::Multiplayer(LobbyAction::JoinWithPassword(7, "secret".into()))
        );
        s.lobby.joining_room = true;
        assert_eq!(click(&ctx, &mut s, "Cancel"), Action::None);
        assert!(
            s.lobby.password_room.is_some(),
            "An in-flight join must await its result"
        );
        s.lobby.clear_password_prompt();
        s.lobby.private_server = true;
        let out = frame_output(&ctx, &mut s, vec![]).1;
        assert!(text_center(&out, "Rooms on this server").is_none());
        assert!(text_center(&out, "Create room").is_none());
        assert!(text_center(&out, "Enter room password").is_some());
        click(&ctx, &mut s, "Enter room password");
        assert_eq!(
            click(&ctx, &mut s, "Cancel"),
            Action::Multiplayer(LobbyAction::Disconnect)
        );
        assert!(s.lobby.join_password.is_empty());
    }
    #[test]
    fn server_menu_shows_one_connection_path_at_a_time() {
        let ctx = egui::Context::default();
        let mut s = selector();
        s.lobby.open = true;
        let output = frame_output(&ctx, &mut s, vec![]).1;
        for label in ["Host Server", "Join Server"] {
            assert!(text_center(&output, label).is_some());
        }
        for label in ["Player name", "Listen address", "Room name"] {
            assert!(text_center(&output, label).is_none());
        }
        click(&ctx, &mut s, "Join Server");
        let output = frame_output(&ctx, &mut s, vec![]).1;
        assert!(text_center(&output, "Server address").is_some());
        assert!(text_center(&output, "Room name").is_none());
        click(&ctx, &mut s, "Back");
        click(&ctx, &mut s, "Host Server");
        let output = frame_output(&ctx, &mut s, vec![]).1;
        assert!(text_center(&output, "Room name").is_some());
        assert!(text_center(&output, "Private server").is_none());
        assert!(text_center(&output, "Connect").is_none());
    }
    #[test]
    fn guests_configure_their_view_without_single_player_or_host_controls() {
        let ctx = egui::Context::default();
        let mut s = selector();
        s.lobby.open = true;
        s.lobby.player = Some(2);
        s.lobby.joined_room = Some(Room {
            keys: 4,
            levels: [None; 3],
            password_protected: false,
            id: 1,
            name: "Room".into(),
            host: 1,
            members: vec![sdo_net::Member {
                state_us: 0,
                id: 2,
                name: "Guest".into(),
                slot: 1,
                x: 0.5,
                y: 0.65,
                facing: [0, 1],
                walking: [0, 0],
                ready: false,
            }],
            chart: Some(spec()),
            round: None,
            start_us: None,
        });
        s.scroll_speed = sdo_config::ScrollSpeed::new(7).unwrap();
        s.lobby.scroll_speed = sdo_config::ScrollSpeed::new(3).unwrap();
        s.appearance.skin = "first".into();
        s.appearance.effect = "glow".into();
        s.lobby.appearance_preview = Some(crate::preview::tests::preview());
        let output = frame_output(&ctx, &mut s, vec![]).1;
        assert!(text_center(&output, "Your settings").is_none());
        assert!(text_center(&output, "Scroll speed").is_some());
        assert!(text_center(&output, "Note skin").is_some());
        assert!(text_center(&output, "Note effect").is_some());
        assert!(text_center(&output, "LIVE PREVIEW").is_some());
        assert!(text_center(&output, "Character").is_some());
        assert!(text_center(&output, "Done").is_none());
        for label in [
            "Single Player",
            "Host controls",
            "Choose song",
            "Start round",
        ] {
            assert!(text_center(&output, label).is_none());
        }
        assert_eq!(s.scroll_speed.value(), 7);
        assert_eq!(s.lobby.scroll_speed.value(), 3);
        assert_eq!(s.lobby.walk_direction, [0, 0]);
        click(&ctx, &mut s, "Character");
        let output = frame_output(&ctx, &mut s, vec![]).1;
        assert!(s.character_view && s.lobby.open);
        assert!(text_center(&output, "Wardrobe").is_some());
        assert!(text_center(&output, "Back to room").is_some());
        assert!(text_center(&output, "Single Player").is_none());
        assert_eq!(s.lobby.walk_direction, [0, 0]);
        click(&ctx, &mut s, "Back to room");
        assert!(!s.character_view && s.lobby.open);
        assert_eq!(s.selected().unwrap().id, "00001");
    }
    #[test]
    fn every_player_sees_shared_song_info_and_their_own_exact_chart_best() {
        let ctx = egui::Context::default();
        let mut s = selector();
        s.lobby.open = true;
        s.lobby.player = Some(2);
        let chart = spec();
        let exact =
            sdo_config::ScoreKey::new("local-alias", "k", 0, "keyboard", &chart.pattern_sha256);
        let wrong =
            sdo_config::ScoreKey::new("local-alias", "k", 2, "keyboard", &chart.pattern_sha256);
        std::sync::Arc::make_mut(&mut s.scores)
            .scores
            .push(sdo_config::ScoreRecord {
                key: exact.clone(),
                points: 20000,
                max_combo: 21,
                judgments: [0; 4],
                achieved_at_unix: 0,
            });
        s.multiplayer_scores = std::sync::Arc::new(sdo_config::ScoreBook {
            scores: vec![
                sdo_config::ScoreRecord {
                    key: exact,
                    points: 12345,
                    max_combo: 20,
                    judgments: [0; 4],
                    achieved_at_unix: 0,
                },
                sdo_config::ScoreRecord {
                    key: wrong,
                    points: 99999,
                    max_combo: 30,
                    judgments: [0; 4],
                    achieved_at_unix: 0,
                },
            ],
            ..sdo_config::ScoreBook::default()
        });
        s.lobby.joined_room = Some(Room {
            keys: 4,
            levels: [Some(3), Some(6), Some(9)],
            password_protected: false,
            id: 1,
            name: "Room".into(),
            host: 1,
            members: vec![sdo_net::Member {
                state_us: 0,
                id: 2,
                name: "Guest".into(),
                slot: 1,
                x: 0.5,
                y: 0.65,
                facing: [0, 1],
                walking: [0, 0],
                ready: false,
            }],
            chart: Some(chart),
            round: None,
            start_us: None,
        });
        s.catalog.songs.clear();
        let output = frame_output(&ctx, &mut s, vec![]).1;
        for label in [
            "Easy · Level 3",
            "4 keys · 1:40",
            "Personal best 12345",
            "120 BPM · Scroll BPM 120",
        ] {
            assert!(text_center(&output, label).is_some(), "Missing {label}");
        }
        for label in [
            "Selected song",
            "Host controls",
            "Your settings",
            "Chart verification",
            "Exit",
            "Slot 2",
        ] {
            assert!(text_center(&output, label).is_none(), "Unwanted {label}");
        }
        assert_eq!(s.room_personal_best(), Some(12345));
    }
    #[test]
    fn stage_chat_and_portraits_fit_and_typing_stops_walk() {
        let context = egui::Context::default();
        crate::theme::apply(&context);
        let mut s = selector();
        s.lobby.open = true;
        s.lobby.player = Some(1);
        s.lobby.joined_room = Some(Room {
            keys: 4,
            levels: [None; 3],
            password_protected: false,
            id: 1,
            name: "Walking room".into(),
            host: 1,
            members: vec![sdo_net::Member {
                state_us: 0,
                x: 0.5,
                y: 0.65,
                facing: [1, 0],
                walking: [0, 0],
                slot: 0,
                id: 1,
                name: "Alice".into(),
                ready: false,
            }],
            chart: Some(spec()),
            round: None,
            start_us: None,
        });
        for size in [egui::vec2(900., 680.), egui::vec2(1100., 760.)] {
            crate::tests::frame_at_size(&context, &mut s, vec![], size);
            let stage = s.lobby.stage_rect.unwrap();
            let input = s.lobby.input_rect.unwrap();
            let send = s.lobby.send_rect.unwrap();
            assert!(send.right() <= size.x, "Send must stay on screen: {send:?}");
            assert!(stage.bottom() <= input.top());
            assert!(
                input.bottom() <= size.y,
                "Chat must stay on screen: {input:?}"
            );
        }
        let out = frame_output(&context, &mut s, vec![]).1;
        let stage = s.lobby.stage_rect.unwrap();
        let input = s.lobby.input_rect.unwrap();
        for slot in 1..=8 {
            assert!(text_center(&out, &format!("Slot {slot}")).is_none());
        }
        let pointer = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        frame(
            &context,
            &mut s,
            vec![
                egui::Event::PointerMoved(stage.center()),
                pointer(stage.center(), true),
                pointer(stage.center(), false),
            ],
        );
        assert_eq!(
            s.lobby.walk_direction,
            [0, 0],
            "Mouse clicks must not move the character"
        );
        frame(
            &context,
            &mut s,
            vec![egui::Event::Key {
                key: egui::Key::ArrowRight,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
        );
        assert_eq!(s.lobby.walk_direction, [1, 0]);
        let arrow = |key, pressed| egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: Default::default(),
        };
        frame(&context, &mut s, vec![arrow(egui::Key::ArrowUp, true)]);
        assert_eq!(s.lobby.walk_direction, [1, -1]);
        frame(&context, &mut s, vec![arrow(egui::Key::ArrowRight, false)]);
        assert_eq!(s.lobby.walk_direction, [0, -1]);
        frame(
            &context,
            &mut s,
            vec![
                arrow(egui::Key::ArrowUp, false),
                arrow(egui::Key::ArrowDown, true),
            ],
        );
        assert_eq!(s.lobby.walk_direction, [0, 1]);
        frame(
            &context,
            &mut s,
            vec![
                egui::Event::PointerMoved(input.center()),
                pointer(input.center(), true),
                pointer(input.center(), false),
            ],
        );
        frame(&context, &mut s, vec![egui::Event::Text("hello".into())]);
        assert_eq!(s.lobby.walk_direction, [0, 0]);
        assert_eq!(s.lobby.chat_draft, "hello");
    }
    #[test]
    fn local_chart_resolution_uses_hash_instead_of_names_or_catalog_ids() {
        let mut s = selector();
        let mut selected = spec();
        selected.pattern_sha256 = "f".repeat(64);
        assert!(s.room_chart_song(&selected).is_err());
        s.catalog.songs[2].patterns[0].sha256 = selected.pattern_sha256.clone();
        selected.difficulty = 2;
        selected.judgment = "pad".into();
        let song = s.room_chart_song(&selected).unwrap();
        assert_eq!(song.id, "00003");
        assert_eq!(song.patterns[0].sha256, selected.pattern_sha256);
        assert_eq!(s.selected().unwrap().id, "00001");
        assert_eq!(s.difficulty, 0);
        assert_eq!(s.judgment, "keyboard");
    }
}
