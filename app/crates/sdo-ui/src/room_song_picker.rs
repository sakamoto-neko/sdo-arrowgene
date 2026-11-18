//! Multiplayer-only song selection; gameplay preferences stay in the room.
use super::{Action, LobbyAction, SongSelector, song_header, song_row};
use anyhow::{Context, Result, ensure};

#[derive(Default)]
pub(super) struct RoomSongPicker {
    query: String,
    selected: Option<String>,
    room: Option<u64>,
}

impl SongSelector {
    pub(super) fn show_multiplayer(&mut self, ui: &mut egui::Ui) -> Action {
        if self.character_view && self.lobby.room().is_some() {
            self.lobby.stop_walking();
            let mut back = false;
            let mut leave = false;
            ui.horizontal(|ui| {
                back = ui.button("Back to room").clicked();
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    leave = ui.button("Leave room").clicked();
                });
            });
            if leave {
                return Action::Multiplayer(LobbyAction::Leave);
            }
            back |=
                ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
            if back {
                self.character_view = false;
                ui.memory_mut(|memory| {
                    if let Some(focused) = memory.focused() {
                        memory.surrender_focus(focused);
                    }
                });
                return Action::None;
            }
            ui.add_space(8.);
            return egui::Frame::new()
                .inner_margin(16.)
                .show(ui, |ui| self.show_character(ui))
                .inner;
        }
        if self.lobby.room().is_none() {
            self.character_view = false;
        }
        let personal_best = self.room_personal_best();
        let mut action = self.lobby.show(
            ui,
            &mut self.appearance,
            &self.ui_choices,
            &mut self.character_view,
            personal_best,
        );
        if action == Action::Multiplayer(LobbyAction::Exit) {
            self.lobby.open = false;
            self.lobby.stop_walking();
        }
        if self.character_view {
            self.lobby.stop_walking();
        }
        if self.lobby.song_picker_open {
            self.lobby.stop_walking();
            let picked = self.show_room_song_picker(ui.ctx());
            if picked != Action::None {
                action = picked;
            }
        }
        if self.lobby.create_room_open {
            let created = self.lobby.show_create_room_prompt(ui.ctx());
            if created != Action::None {
                action = created;
            }
        }
        if self.lobby.password_room.is_some() {
            let password_action = self.lobby.show_password_prompt(ui.ctx());
            if password_action != Action::None {
                action = password_action;
            }
        }
        action
    }

    pub fn select_room_song(&mut self, id: &str) -> Result<()> {
        let room = self
            .lobby
            .room()
            .context("Join a room before choosing a song")?;
        ensure!(
            Some(room.host) == self.lobby.player && room.round.is_none(),
            "Only the room host can choose a song between rounds"
        );
        let kind = room.chart.as_ref().map_or("k", |chart| chart.kind.as_str());
        ensure!(
            self.catalog
                .songs
                .iter()
                .any(|song| song.id == id && song.pattern(kind, 4).is_ok()),
            "The selected song is no longer available locally"
        );
        self.lobby.song_selection = Some(super::RoomSongSelection {
            room: room.id,
            song: id.to_owned(),
            kind: kind.to_owned(),
            difficulty: room
                .chart
                .as_ref()
                .map_or(0, |chart| chart.difficulty as usize),
            judgment: room
                .chart
                .as_ref()
                .map_or("keyboard", |chart| chart.judgment.as_str())
                .to_owned(),
        });
        self.lobby.song_picker_open = false;
        self.lobby.song_preview = None;
        Ok(())
    }

    fn show_room_song_picker(&mut self, ctx: &egui::Context) -> Action {
        let Some(room) = self
            .lobby
            .room()
            .filter(|room| Some(room.host) == self.lobby.player && room.round.is_none())
        else {
            self.lobby.song_picker_open = false;
            self.lobby.song_preview = None;
            return Action::None;
        };
        if self.room_song_picker.room != Some(room.id) {
            self.room_song_picker = RoomSongPicker {
                room: Some(room.id),
                ..Default::default()
            };
        }
        let kind = room.chart.as_ref().map_or("k", |chart| chart.kind.as_str());
        let difficulty = room
            .chart
            .as_ref()
            .map_or(0, |chart| chart.difficulty as usize);
        if self.room_song_picker.selected.is_none() {
            self.room_song_picker.selected = room
                .chart
                .as_ref()
                .and_then(|chart| {
                    self.catalog.songs.iter().find(|song| {
                        song.pattern(kind, 4)
                            .is_ok_and(|pattern| pattern.sha256 == chart.pattern_sha256)
                    })
                })
                .or_else(|| {
                    self.catalog
                        .songs
                        .iter()
                        .find(|song| song.pattern(kind, 4).is_ok())
                })
                .map(|song| song.id.clone());
        }
        let mut confirmed = false;
        let mut cancelled = false;
        let picker_id = egui::Id::unique("room-song-picker");
        let response = egui::Modal::new(picker_id)
            .area(egui::Modal::default_area(picker_id).fade_in(false))
            .backdrop_color(egui::Color32::from_black_alpha(160))
            .frame(
                egui::Frame::popup(&ctx.global_style())
                    .fill(egui::Color32::from_rgb(18, 24, 36))
                    .inner_margin(16.),
            )
            .show(ctx, |ui| {
                ui.set_width((ctx.content_rect().width() - 64.).clamp(300., 900.));
                ui.heading("Choose a room song");
                ui.add_space(8.);
                ui.add_sized(
                    [ui.available_width(), 32.],
                    egui::TextEdit::singleline(&mut self.room_song_picker.query)
                        .id(egui::Id::unique("room-song-search"))
                        .hint_text("Search title, artist or ID…"),
                );
                let query = self.room_song_picker.query.to_lowercase();
                let words = query.split_whitespace().collect::<Vec<_>>();
                let visible = self
                    .catalog
                    .songs
                    .iter()
                    .enumerate()
                    .filter(|(_, song)| {
                        song.pattern(kind, 4).is_ok() && {
                            let text = format!("{} {} {}", song.id, song.title, song.artist)
                                .to_lowercase();
                            words.iter().all(|word| text.contains(word))
                        }
                    })
                    .map(|(index, _)| index)
                    .collect::<Vec<_>>();
                if !visible.iter().any(|&index| {
                    self.room_song_picker.selected.as_deref()
                        == Some(self.catalog.songs[index].id.as_str())
                }) {
                    self.room_song_picker.selected = visible
                        .first()
                        .map(|&index| self.catalog.songs[index].id.clone());
                }
                ui.small(format!("{} songs", visible.len()));
                if !self.lobby.preview_status.is_empty() {
                    ui.small(&self.lobby.preview_status);
                }
                ui.separator();
                song_header(ui);
                let list_height = (ctx.content_rect().height() - 290.).clamp(120., 400.);
                egui::ScrollArea::vertical()
                    .id_salt("room-song-list")
                    .max_height(list_height)
                    .min_scrolled_height(list_height)
                    .auto_shrink([false, false])
                    .show_rows(ui, 64., visible.len(), |ui, range| {
                        for row in range {
                            let song = &self.catalog.songs[visible[row]];
                            let response = song_row(
                                ui,
                                song,
                                kind,
                                difficulty,
                                self.room_song_picker.selected.as_deref() == Some(song.id.as_str()),
                                64.,
                                row,
                            );
                            if response.clicked() {
                                self.room_song_picker.selected = Some(song.id.clone());
                            }
                            if response.double_clicked() {
                                self.room_song_picker.selected = Some(song.id.clone());
                                confirmed = true;
                            }
                        }
                    });
                if visible.is_empty() {
                    ui.label("No matching songs");
                }
                ui.separator();
                ui.horizontal(|ui| {
                    cancelled = ui.button("Cancel").clicked();
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        confirmed |= ui
                            .add_enabled(
                                !visible.is_empty(),
                                egui::Button::new("Select song").min_size(egui::vec2(140., 34.)),
                            )
                            .clicked();
                    });
                });
                confirmed |= !visible.is_empty()
                    && ui.input_mut(|input| {
                        input.consume_key(egui::Modifiers::NONE, egui::Key::Enter)
                    });
            });
        if cancelled || response.should_close() {
            self.lobby.song_picker_open = false;
            self.lobby.song_preview = None;
            return Action::None;
        }
        if confirmed && let Some(id) = self.room_song_picker.selected.clone() {
            self.lobby.song_picker_open = false;
            self.lobby.song_preview = None;
            return Action::Multiplayer(LobbyAction::SelectSong(id));
        }
        self.lobby.song_preview = self.room_song_picker.selected.clone();
        Action::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{frame, frame_output, selector, text_center};

    fn room_selector() -> SongSelector {
        let mut s = selector();
        s.lobby.open = true;
        s.lobby.player = Some(1);
        s.lobby.joined_room = Some(sdo_net::Room {
            keys: 4,
            levels: [None; 3],
            password_protected: false,
            id: 1,
            name: "Song picker room".into(),
            host: 1,
            members: vec![sdo_net::Member {
                state_us: 0,
                id: 1,
                name: "Host".into(),
                slot: 0,
                x: 0.5,
                y: 0.65,
                facing: [0, 1],
                walking: [0, 0],
                ready: false,
            }],
            chart: Some(sdo_net::ChartSpec {
                pattern_sha256: "a".repeat(64),
                audio_sha256: "b".repeat(64),
                dance_sha256: None,
                title: "Holy Knight".into(),
                kind: "k".into(),
                difficulty: 2,
                judgment: "keyboard".into(),
                bpm: 120.,
                duration_us: 100_000_000,
            }),
            round: None,
            start_us: None,
        });
        s
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
    fn click(ctx: &egui::Context, s: &mut SongSelector, label: &str) -> Action {
        frame(ctx, s, vec![]);
        let output = frame_output(ctx, s, vec![]).1;
        let pos = text_center(&output, label).unwrap_or_else(|| panic!("Missing {label}"));
        frame(
            ctx,
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

    #[test]
    fn multiplayer_has_no_top_bar_and_uses_back_only_before_connecting() {
        let ctx = egui::Context::default();
        let mut s = selector();
        let output = frame_output(&ctx, &mut s, vec![]).1;
        assert!(text_center(&output, "Single Player").is_some());
        click(&ctx, &mut s, "Multiplayer");
        let output = frame_output(&ctx, &mut s, vec![]).1;
        assert!(text_center(&output, "Exit").is_none());
        assert!(text_center(&output, "Single Player").is_none());
        assert!(text_center(&output, "Songs").is_none());
        assert_eq!(
            click(&ctx, &mut s, "Back"),
            Action::Multiplayer(LobbyAction::Exit)
        );
        let output = frame_output(&ctx, &mut s, vec![]).1;
        assert!(!s.lobby.open);
        assert!(text_center(&output, "Single Player").is_some());
    }

    #[test]
    fn room_picker_selects_a_song_without_changing_player_preferences() {
        let ctx = egui::Context::default();
        let mut s = room_selector();
        s.query = "Holy".into();
        s.refilter();
        s.scroll_speed = sdo_config::ScrollSpeed::new(7).unwrap();
        let appearance = s.appearance.clone();
        click(&ctx, &mut s, "Choose song");
        assert!(s.lobby.open && s.lobby.song_picker_open);
        s.room_song_picker.query = "Cynthia".into();
        frame(&ctx, &mut s, vec![key(egui::Key::ArrowRight)]);
        assert_eq!(s.lobby.walk_direction, [0, 0]);
        assert_eq!(s.selected().unwrap().id, "00001");
        assert_eq!(s.lobby.song_preview.as_deref(), Some("00002"));
        assert_eq!(
            frame(&ctx, &mut s, vec![key(egui::Key::Enter)]),
            Action::Multiplayer(LobbyAction::SelectSong("00002".into()))
        );
        assert!(s.lobby.open && !s.lobby.song_picker_open);
        s.select_room_song("00002").unwrap();
        assert_eq!(s.selected().unwrap().id, "00001");
        assert_eq!(s.difficulty, 0);
        let selection = s.room_song_selection().unwrap();
        assert_eq!(selection.song, "00002");
        assert_eq!(
            selection.difficulty, 2,
            "Use the room's existing difficulty"
        );
        assert_eq!(s.scroll_speed.value(), 7);
        assert_eq!(s.appearance, appearance);
        assert_eq!(s.query, "Holy", "The popup search is separate");
    }

    #[test]
    fn empty_picker_cannot_play_and_escape_cancels_without_changing_selection() {
        let ctx = egui::Context::default();
        let mut s = room_selector();
        click(&ctx, &mut s, "Choose song");
        s.room_song_picker.query = "no matching room songs".into();
        assert_eq!(
            frame(&ctx, &mut s, vec![key(egui::Key::Enter)]),
            Action::None
        );
        assert!(s.lobby.song_picker_open);
        assert_eq!(
            frame(&ctx, &mut s, vec![key(egui::Key::Escape)]),
            Action::None
        );
        assert!(!s.lobby.song_picker_open);
        assert!(s.lobby.open);
        assert_eq!(s.selected().unwrap().id, "00001");
    }

    #[test]
    fn popup_contains_only_song_selection_and_guests_cannot_choose() {
        let ctx = egui::Context::default();
        let mut s = room_selector();
        s.lobby.song_picker_open = true;
        let mut render_popup = || {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1100., 760.),
                    )),
                    ..Default::default()
                },
                |ui| {
                    s.show_room_song_picker(ui.ctx());
                },
            );
            output.textures_delta.clear();
            output
        };
        render_popup();
        let output = render_popup();
        assert!(text_center(&output, "Choose a room song").is_some());
        assert!(text_center(&output, "Select song").is_some());
        for label in [
            "Single Player",
            "Your note scroll speed",
            "Timing",
            "Appearance",
            "Host difficulty",
            "Keyboard",
            "Dancemat",
        ] {
            assert!(
                text_center(&output, label).is_none(),
                "{label} does not belong in this popup"
            );
        }
        let mut guest = s.lobby.joined_room.as_mut().unwrap().members[0].clone();
        guest.id = 2;
        guest.slot = 1;
        s.lobby.joined_room.as_mut().unwrap().members.push(guest);
        s.lobby.player = Some(2);
        assert!(s.select_room_song("00002").is_err());
        let output = frame_output(&ctx, &mut s, vec![]).1;
        assert!(!s.lobby.song_picker_open);
        assert!(text_center(&output, "Choose song").is_none());
        assert_eq!(s.selected().unwrap().id, "00001");
    }
}
