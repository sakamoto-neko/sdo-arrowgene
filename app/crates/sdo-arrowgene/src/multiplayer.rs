//! Multiplayer orchestration uses only prepared game assets and the common chart format.
use crate::{
    app::{App, PlaySelection},
    session::PlaySession,
};
use anyhow::{Context, Result, ensure};
use sdo_asset_loader::PreparedSong;
use sdo_gameplay::input::Transition;
use sdo_multiplayer::Server;
use sdo_net::{ChartSpec, Client, ClientEvent, Command, Event};
use sdo_ui::LobbyAction;
#[derive(Clone)]
pub(crate) enum LoadPurpose {
    Offer,
    Ready(ChartSpec),
}
#[derive(Default)]
pub(crate) struct Online {
    pub round_room: Option<sdo_net::Room>,
    pub round_elapsed: f64,
    pub deadline: Option<std::time::Instant>,
    pub confirmed: Vec<sdo_ui::notes::ServerGrade>,
    pub judgment_revision: u64,
    pub correction: Option<sdo_gameplay::JudgmentEvent>,
    pub client: Option<Client>,
    pub server: Option<Server>,
    pub prepared: Option<(ChartSpec, PlaySession)>,
    pub round: Option<(u64, u64)>,
    pub sequence: u64,
    pub loading: Option<LoadPurpose>,
    pub availability_checked: Option<(u64, ChartSpec)>,
    pub pending_host_room: Option<Command>,
    pub start_when_ready: bool,
    pub walk_sent: [i8; 2],
    pub local_walk: Option<crate::scene::LocalWalk>,
    pub appearance_sent: Option<sdo_net::Appearance>,
    pub appearances: std::collections::BTreeMap<u64, sdo_net::Appearance>,
    pub actors: std::collections::BTreeMap<u64, crate::scene::Actor>,
    pub scene_loader: Option<crate::scene::SceneLoader>,
    pub scene_keys: std::collections::BTreeMap<u64, crate::scene::LoadKey>,
    pub scene_pending: std::collections::BTreeMap<u64, u64>,
    pub scene_generation: u64,
    pub replays: std::collections::BTreeMap<u64, sdo_multiplayer::Replay>,
    pub offsets: std::collections::BTreeMap<u64, i64>,
    pub scene_song: Option<(ChartSpec, Option<crate::scene::SceneSong>)>,
    pub movement: std::collections::BTreeMap<u64, crate::scene::Movement>,
}
impl App {
    pub(crate) fn net_send(&self, command: Command) -> Result<()> {
        self.online
            .client
            .as_ref()
            .context("Connect to a lobby server first")?
            .send(command)
    }
    pub(crate) fn lobby_action(&mut self, action: &LobbyAction) -> Result<()> {
        if self.state.is_some() && matches!(action, LobbyAction::Ready | LobbyAction::Start) {
            self.sync_room_scene()?;
            ensure!(
                !self.selector.lobby.scene_loading,
                "Wait for the room's character and animation assets to finish loading; every participant needs compatible local assets"
            );
        }
        match action {
            LobbyAction::ConfirmResults => {
                if self.selector.lobby.results {
                    self.record_completed_score(true);
                    if self.online.client.is_none() {
                        self.disconnect_online();
                    } else {
                        self.clear_online_attempt();
                    }
                    self.selector.lobby.standings.clear();
                    self.selector.lobby.results = false;
                    self.preview_id = None;
                }
            }
            LobbyAction::Exit => {
                self.disconnect_online();
                self.online.server = None;
                self.selector.lobby.hosting = None;
                self.selector.lobby.open = false;
                self.selector.lobby.server_menu = sdo_ui::ServerMenu::Choose;
                self.selector.settings_open = false;
                self.selector.character_view = false;
                self.selector.refilter();
            }
            LobbyAction::SelectSong(song) => {
                self.selector.select_room_song(song)?;
                self.start_room_selected()?;
            }
            LobbyAction::Connect => {
                self.disconnect_online();
                let prefs = &self.selector.lobby.preferences;
                let wake = self.network_wake.clone();
                self.online.client = Some(Client::connect_with_wake(
                    prefs.address.clone(),
                    prefs.name.clone(),
                    move || {
                        if let Some(proxy) = &wake {
                            let _ = proxy.send_event(());
                        }
                    },
                )?);
                self.selector.lobby.connecting = true;
                self.selector.lobby.status = "Connecting…".into();
            }
            LobbyAction::Host => {
                ensure!(
                    self.selector.lobby.valid_room_name(),
                    "Enter a room name before hosting"
                );
                ensure!(self.online.server.is_none(), "Already hosting a server");
                let name = self.selector.lobby.room_name.trim().to_owned();
                let password = self.selector.lobby.room_password.clone();
                let server = Server::bind_private(&self.selector.lobby.preferences.listen)?;
                let open_room = if let Some(key) = server.private_key() {
                    Command::OpenPrivateRoom {
                        name,
                        password,
                        key: key.to_owned(),
                    }
                } else {
                    Command::Create { name, password }
                };
                self.selector.lobby.hosting = Some(server.address.to_string());
                self.selector.lobby.preferences.address =
                    format!("127.0.0.1:{}", server.address.port());
                self.online.server = Some(server);
                self.lobby_action(&LobbyAction::Connect)?;
                self.online.pending_host_room = Some(open_room);
            }
            LobbyAction::Disconnect => {
                self.disconnect_online();
                self.selector.lobby.server_menu = sdo_ui::ServerMenu::Choose;
            }
            LobbyAction::StopHosting => {
                self.disconnect_online();
                self.online.server = None;
                self.selector.lobby.hosting = None;
            }
            LobbyAction::Create => {
                self.selector.lobby.standings.clear();
                ensure!(
                    self.selector.lobby.valid_room_name(),
                    "Enter a room name before creating a room"
                );
                self.net_send(Command::Create {
                    password: self.selector.lobby.room_password.clone(),
                    name: self.selector.lobby.room_name.trim().into(),
                })?;
                self.selector.lobby.creating_room = true;
            }
            LobbyAction::Join(room) | LobbyAction::JoinWithPassword(room, _) => {
                self.selector.lobby.standings.clear();
                let password = match action {
                    LobbyAction::JoinWithPassword(_, password) => password.clone(),
                    _ => String::new(),
                };
                self.net_send(Command::Join {
                    room: *room,
                    password,
                })?;
                self.selector.lobby.joining_room = true;
            }
            LobbyAction::Leave if self.selector.lobby.private_server => {
                self.lobby_action(&LobbyAction::Exit)?
            }
            LobbyAction::Leave => {
                self.selector.lobby.stop_walking();
                self.sync_room_walk()?;
                self.net_send(Command::Leave)?;
                self.selector.lobby.song_picker_open = false;
                self.selector.lobby.song_preview = None;
                self.selector.lobby.song_selection = None;
                self.clear_online_attempt();
            }
            LobbyAction::Ready => {
                ensure!(!self.options.autoplay, "Autoplay cannot join online rounds");
                ensure!(
                    self.online
                        .client
                        .as_ref()
                        .is_some_and(|c| c.clock.synchronized()),
                    "Wait for clock synchronization"
                );
                let spec = self
                    .selector
                    .lobby
                    .room()
                    .and_then(|r| r.chart.clone())
                    .context("The host must choose a chart")?;
                if let Err(error) = self.selector.room_chart_song(&spec) {
                    self.report_unavailable(&spec, &error.to_string())?;
                    return Ok(());
                }
                self.online.prepared = None;
                self.begin_song(Some(LoadPurpose::Ready(spec)))?;
            }
            LobbyAction::Unready => {
                self.net_send(Command::Unready)?;
                self.clear_online_attempt();
            }
            LobbyAction::Start => {
                let room = self.selector.lobby.room().context("Join a room first")?;
                ensure!(
                    Some(room.host) == self.selector.lobby.player,
                    "Only the room host can start"
                );
                ensure!(
                    room.members.len() >= 2
                        && room
                            .members
                            .iter()
                            .all(|member| member.id == room.host || member.ready),
                    "Wait for every other player to be ready"
                );
                let spec = room.chart.clone().context("Choose a song first")?;
                if self
                    .online
                    .prepared
                    .as_ref()
                    .is_some_and(|(prepared, _)| prepared == &spec)
                {
                    self.net_send(Command::Start)?;
                } else {
                    self.selector.room_chart_song(&spec)?;
                    self.online.start_when_ready = true;
                    if let Err(error) = self.begin_song(Some(LoadPurpose::Ready(spec))) {
                        self.online.start_when_ready = false;
                        return Err(error);
                    }
                }
            }
            LobbyAction::TransferHost(player) => {
                self.net_send(Command::TransferHost { player: *player })?
            }
            LobbyAction::SetDifficulty(difficulty) => self.net_send(Command::SetDifficulty {
                difficulty: *difficulty,
            })?,
            LobbyAction::TakeSlot(slot) => self.net_send(Command::TakeSlot { slot: *slot })?,
            LobbyAction::SendChat => {
                self.net_send(Command::Chat {
                    text: self.selector.lobby.chat_draft.trim().into(),
                })?;
                self.selector.lobby.chat_draft.clear();
            }
        }
        Ok(())
    }
    fn clear_online_attempt(&mut self) {
        self.online.replays.clear();
        self.online.offsets.clear();
        self.online.movement.clear();
        self.online.local_walk = None;
        self.online.round_room = None;
        self.online.deadline = None;
        self.online.confirmed.clear();
        self.online.correction = None;
        self.online.judgment_revision = 0;
        self.online.round_elapsed = 0.;
        self.online.start_when_ready = false;
        if self.online.round.take().is_some() {
            self.session = None;
        }
        self.online.prepared = None;
        self.cancel_loading();
        self.selector.lobby.open = true;
    }
    pub(crate) fn disconnect_online(&mut self) {
        self.clear_online_attempt();
        if let Some(state) = &mut self.state {
            state.multiplayer_stage.retain(&[]);
        }
        self.online.client = None;
        self.selector.character_view = false;
        let view = &mut self.selector.lobby;
        view.player = None;
        view.connecting = false;
        view.song_picker_open = false;
        view.create_room_open = false;
        view.creating_room = false;
        view.room_error.clear();
        view.song_preview = None;
        view.song_selection = None;
        view.private_server = false;
        view.room_password.clear();
        view.clear_password_prompt();
        view.joined_room = None;
        view.directory.clear();
        view.synchronized = false;
        view.rtt_ms = None;
        view.status = "Disconnected".into();
        view.chat_room = None;
        view.messages.clear();
        view.chat_draft.clear();
        view.portraits.clear();
        view.standings.clear();
        view.results = false;
        view.available_avatars.clear();
        view.avatar_issues.clear();
        view.stage_labels.clear();
        view.scene_loading = false;
        view.stop_walking();
        self.online.walk_sent = [0, 0];
        self.online.pending_host_room = None;
        self.online.appearance_sent = None;
        self.online.appearances.clear();
        self.online.actors.clear();
        self.online.scene_loader = None;
        self.online.scene_keys.clear();
        self.online.scene_song = None;
        self.online.scene_pending.clear();
        self.online.replays.clear();
        self.online.offsets.clear();
        self.online.availability_checked = None;
    }
    fn report_unavailable(&mut self, spec: &ChartSpec, reason: &str) -> Result<()> {
        let mut reason = reason.split_whitespace().collect::<Vec<_>>().join(" ");
        while reason.len() > sdo_net::MAX_CHAT_BYTES {
            reason.pop();
        }
        self.net_send(Command::Unavailable {
            spec: spec.clone(),
            reason: reason.clone(),
        })?;
        self.online.prepared = None;
        self.selector.lobby.status = reason;
        self.selector.lobby.open = true;
        Ok(())
    }
    pub(crate) fn online_loading_failed(&mut self, reason: Option<&str>) -> bool {
        self.online.start_when_ready = false;
        if self.online.client.is_none() || self.selector.lobby.room().is_none() {
            return false;
        }
        let reported = if let (Some(reason), Some(LoadPurpose::Ready(spec))) =
            (reason, self.online.loading.clone())
        {
            if let Err(error) = self.report_unavailable(&spec, reason) {
                self.selector.lobby.status = error.to_string();
            }
            true
        } else {
            let _ = self.net_send(Command::Unready);
            false
        };
        self.online.prepared = None;
        self.online.loading = None;
        self.selector.lobby.open = true;
        reported
    }
    fn check_room_song(&mut self) {
        let Some((room, spec)) = self
            .selector
            .lobby
            .room()
            .and_then(|r| r.chart.clone().map(|spec| (r.id, spec)))
        else {
            self.online.availability_checked = None;
            self.selector.lobby.matching_chart = false;
            return;
        };
        // An active round uses its verified in-memory song, even if files change on disk.
        if self
            .selector
            .lobby
            .room()
            .is_some_and(|r| r.round.is_some())
        {
            return;
        }
        if self.online.availability_checked.as_ref() == Some(&(room, spec.clone())) {
            return;
        }
        self.online.availability_checked = Some((room, spec.clone()));
        let available = (|| -> Result<()> {
            let song=self.selector.catalog.songs.iter().find(|song|song.pattern(&spec.kind,4).is_ok_and(|p|p.sha256==spec.pattern_sha256))
                .context("Song or exact chart is missing locally. Add the matching song to your asset folder before readying.")?;
            let pattern = song.pattern(&spec.kind, 4)?;
            ensure!(
                self.content
                    .request(sdo_asset_loader::request::FileSize(&song.audio))
                    .unwrap_or(0)
                    > 0,
                "The song recording is missing or empty locally"
            );
            ensure!(
                self.content
                    .request(sdo_asset_loader::request::FileSize(&pattern.file))
                    .unwrap_or(0)
                    > 0,
                "The chart file is missing or empty locally"
            );
            Ok(())
        })();
        self.selector.lobby.matching_chart = available.is_ok();
        match available {
            Ok(()) => {
                self.selector.lobby.compatibility =
                    "Matching song found locally. Ready checks the exact chart and recording again."
                        .into()
            }
            Err(error) => {
                self.selector.lobby.compatibility = format!("{error:#}");
                if let Err(error) = self.report_unavailable(&spec, &format!("{error:#}")) {
                    self.selector.lobby.status = error.to_string();
                }
            }
        }
    }
    pub(crate) fn prepare_online(
        &mut self,
        prepared: &PreparedSong<PlaySelection>,
    ) -> Result<Option<ChartSpec>> {
        let Some(purpose) = &prepared.context.network else {
            return Ok(None);
        };
        ensure!(
            !self.options.autoplay && self.options.start == 0.,
            "Online rounds require manual play from the beginning"
        );
        ensure!(
            self.selector.timing_offset_seconds().abs() <= 0.5,
            "Online calibration must be within +/-500 ms"
        );
        let room = self
            .selector
            .lobby
            .room()
            .context("You left the room while loading")?;
        let mut title = match purpose {
            LoadPurpose::Ready(spec) => spec.title.clone(),
            LoadPurpose::Offer => self
                .selector
                .catalog
                .songs
                .iter()
                .find(|song| song.id == prepared.context.score_key.song_id)
                .map_or_else(|| prepared.context.title.clone(), |song| song.title.clone()),
        };
        while title.len() > 200 {
            title.pop();
        }
        let candidate = ChartSpec {
            pattern_sha256: prepared.chart.pattern_sha256(),
            audio_sha256: prepared
                .audio_sha256
                .clone()
                .context("Missing multiplayer audio fingerprint")?,
            dance_sha256: Some(sdo_dance::song_signature(prepared.dance.as_ref())),
            title,
            kind: prepared.context.score_key.chart.clone(),
            difficulty: prepared.difficulty.index() as u8,
            judgment: prepared.context.score_key.judgment.clone(),
            bpm: prepared.chart.bpm,
            duration_us: (prepared.audio.duration() * 1e6).round() as u64,
        };
        candidate.validate()?;
        match purpose {
            LoadPurpose::Offer => {
                ensure!(
                    Some(room.host) == self.selector.lobby.player && room.round.is_none(),
                    "Only the host can choose the chart between rounds"
                );
                // Validate the exact in-memory file that the local session will use.
                sdo_multiplayer::prepare_chart(&candidate, prepared.chart.raw.clone())?;
                self.net_send(Command::Offer {
                    spec: candidate.clone(),
                    raw: prepared.chart.raw.clone(),
                })?;
            }
            LoadPurpose::Ready(spec) => ensure!(
                spec == &candidate && room.chart.as_ref() == Some(spec),
                "Local chart, recording, BPM, difficulty or timing rules differ from the room"
            ),
        }
        Ok(Some(candidate))
    }
    pub(crate) fn store_online_session(
        &mut self,
        spec: ChartSpec,
        mut session: PlaySession,
    ) -> Result<()> {
        ensure!(
            self.online
                .client
                .as_ref()
                .is_some_and(|c| c.clock.synchronized()),
            "Wait for clock synchronization, then load the room chart again"
        );
        session.pause(true)?;
        self.online.prepared = Some((spec.clone(), session));
        self.net_send(Command::Ready {
            spec,
            offset_us: (self.selector.timing_offset_seconds() * 1e6).round() as i64,
        })?;
        self.selector.lobby.open = true;
        self.selector.lobby.status = "Local chart and audio verified. Waiting for the host.".into();
        if self.online.start_when_ready {
            self.online.start_when_ready = false;
            self.net_send(Command::Start)?;
        }
        Ok(())
    }
    pub(crate) fn online_running(&self) -> bool {
        self.online.round.is_some()
    }
    #[cfg(test)]
    pub(crate) fn online_started(&self) -> bool {
        self.online.round.is_some_and(|(_, start)| {
            self.online
                .client
                .as_ref()
                .and_then(Client::server_us)
                .is_some_and(|now| now >= start)
        })
    }
    #[cfg(test)]
    pub(crate) fn online_input_open(&self) -> bool {
        if self.selector.lobby.results {
            return false;
        }
        self.online.round.is_some_and(|(_, start)| {
            self.online
                .client
                .as_ref()
                .and_then(Client::server_us)
                .is_some_and(|now| {
                    (now as f64 - start as f64) / 1e6
                        >= self.selector.timing_offset_seconds().min(0.)
                })
        })
    }
    /// Negative calibration can put the first note in the end of the countdown.
    /// Use the shared clock for that short pre-roll, then the audio output clock.
    pub(crate) fn advance_playback(&mut self) -> Result<()> {
        self.advance_playback_at(std::time::Instant::now())
            .map(|_| ())
    }
    fn locked_offset(&self) -> i64 {
        self.selector
            .lobby
            .player
            .and_then(|id| self.online.offsets.get(&id))
            .copied()
            .unwrap_or_default()
    }
    pub(crate) fn advance_playback_at(
        &mut self,
        captured: std::time::Instant,
    ) -> Result<Option<sdo_net::SongTime>> {
        if self.online_running() && self.selector.lobby.results {
            return Ok(None);
        }
        let elapsed = self.online_round_seconds();
        let local = self.selector.lobby.player;
        for (id, replay) in &mut self.online.replays {
            if Some(*id) == local {
                continue;
            }
            let offset = self.online.offsets.get(id).copied().unwrap_or_default();
            if let Some(at) = sdo_net::SongTime::new(elapsed) {
                replay.advance_to(at.chart_time(offset));
            }
        }
        let offset = self.locked_offset();
        let Some(session) = &mut self.session else {
            return Ok(None);
        };
        let song = if let Some(deadline) = self.online.deadline {
            session.audio.check()?;
            let raw = if captured < deadline {
                -deadline.duration_since(captured).as_secs_f64()
            } else {
                session.tail_clock.position(
                    session.audio.position_at(captured),
                    session.audio.duration(),
                    false,
                    captured,
                )
            };
            let song = sdo_net::SongTime::new(raw).context("Invalid playback clock")?;
            if raw < (offset as f64 / 1e6).min(0.) {
                return Ok(None);
            }
            let chart = song.chart_time(offset);
            if let Some(replay) = local.and_then(|id| self.online.replays.get_mut(&id)) {
                replay.advance_to(chart);
            }
            session
                .practice
                .advance_to(chart.seconds(), session.input.held(), false);
            if raw < offset as f64 / 1e6 {
                return Ok(None);
            }
            song
        } else {
            let raw = session.advance_at(
                self.selector.timing_offset_seconds(),
                self.options.autoplay,
                captured,
            )?;
            sdo_net::SongTime::new(raw).context("Invalid playback clock")?
        };
        Ok(Some(song))
    }
    pub(crate) fn send_online_input(
        &mut self,
        transition: Transition,
        song_time: sdo_net::SongTime,
    ) -> Result<()> {
        let Some((round, _)) = self.online.round else {
            return Ok(());
        };
        let offset = self.locked_offset();
        let (lane, down) = match transition {
            Transition::Press(l) => (l, true),
            Transition::Release(l) => (l, false),
        };
        self.online.sequence += 1;
        self.net_send(Command::Input {
            round,
            sequence: self.online.sequence,
            song_time,
            lane: lane.index() as u8,
            down,
        })?;
        if let Some(player) = self.selector.lobby.player
            && let Some(replay) = self.online.replays.get_mut(&player)
        {
            replay.input(
                self.online.sequence,
                song_time.chart_time(offset),
                lane.index() as u8,
                down,
            )?;
        }
        Ok(())
    }
    pub(crate) fn forfeit_online(&mut self) {
        if let Some((round, _)) = self.online.round {
            let _ = self.net_send(Command::Forfeit { round });
        }
        self.clear_online_attempt();
    }
    pub(crate) fn poll_multiplayer(&mut self) {
        let Some(client) = &mut self.online.client else {
            return;
        };
        let events = client.poll();
        self.selector.lobby.rtt_ms = client.clock.rtt_us().map(|us| us as f64 / 1000.);
        self.selector.lobby.synchronized = client.clock.synchronized();
        for event in events {
            let result = self.online_event(event);
            if let Err(error) = result {
                self.forfeit_online();
                self.selector.lobby.status = format!("{error:#}");
                self.selector.error = Some(format!("Multiplayer: {error:#}"));
            }
        }
        self.check_room_song();
    }
    fn online_event(&mut self, event: ClientEvent) -> Result<()> {
        let event = match event {
            ClientEvent::Disconnected(reason) => {
                if self.online.round_room.is_some() && self.session.is_some() {
                    self.online.round_elapsed = self.online_round_seconds().max(0.);
                    self.online.client = None;
                    self.selector.lobby.status = reason;
                    if self.selector.lobby.results {
                        return Ok(());
                    }
                    // The connection closed before the server settled this round.
                    self.selector.lobby.results = true;
                    if let Some(player) = self.selector.lobby.player {
                        if let Some(score) = self
                            .selector
                            .lobby
                            .standings
                            .iter_mut()
                            .find(|s| s.player == player)
                        {
                            score.forfeited = true;
                        } else {
                            self.selector.lobby.standings.push(sdo_net::Standing {
                                player,
                                forfeited: true,
                                ..Default::default()
                            });
                        }
                    }
                    if let Some(session) = &mut self.session {
                        session.audio.set_paused(true)?;
                    }
                    return Ok(());
                }
                self.disconnect_online();
                self.selector.lobby.status = reason;
                return Ok(());
            }
            ClientEvent::Message(event) => event,
        };
        match event {
            Event::Welcome {
                protocol,
                player,
                private_server,
            } => {
                ensure!(protocol == sdo_net::PROTOCOL, "Server protocol differs");
                let view = &mut self.selector.lobby;
                view.player = Some(player);
                view.connecting = false;
                view.private_server = private_server;
                view.status = if private_server {
                    "Connected to a private server. Joining its room…"
                } else {
                    "Connected. Create or join a room."
                }
                .into();
                if let Some(command) = self.online.pending_host_room.take() {
                    self.net_send(command)?;
                }
            }
            Event::RoomPasswordRequired { room, name } => {
                ensure!(
                    self.selector.lobby.private_server && name.len() <= 60,
                    "Invalid private room password request"
                );
                self.selector.lobby.request_room_password(room, name);
            }
            Event::Directory(directory) => {
                ensure!(
                    directory.len() <= 32
                        && directory
                            .iter()
                            .all(|r| r.occupants <= 8 && r.name.len() <= 60),
                    "Invalid room directory"
                );
                if self
                    .selector
                    .lobby
                    .password_room
                    .as_ref()
                    .is_some_and(|(id, _)| !directory.iter().any(|r| r.id == *id))
                {
                    self.selector.lobby.clear_password_prompt();
                }
                self.selector.lobby.directory = directory;
            }
            Event::Movement {
                room,
                state_us,
                walkers,
            } => {
                ensure!(walkers.len() <= 8, "Invalid movement update");
                if let Some(current) = self
                    .selector
                    .lobby
                    .joined_room
                    .as_mut()
                    .filter(|r| r.id == room)
                {
                    for walker in walkers {
                        ensure!(
                            walker.position.iter().all(|v| (0.0..=1.0).contains(v))
                                && walker.facing != [0, 0]
                                && walker
                                    .facing
                                    .iter()
                                    .chain(&walker.walking)
                                    .all(|v| (-1..=1).contains(v)),
                            "Invalid walker"
                        );
                        if let Some(member) = current.members.iter_mut().find(|m| m.id == walker.id)
                            && state_us >= member.state_us
                        {
                            walker.apply(member, state_us);
                            if Some(member.id) == self.selector.lobby.player
                                && let Some(walk) = &mut self.online.local_walk
                            {
                                walk.observe(
                                    member,
                                    std::time::Instant::now(),
                                    self.online.client.as_ref().and_then(Client::server_us),
                                );
                            }
                            self.online
                                .movement
                                .entry(member.id)
                                .or_default()
                                .push(member);
                        }
                    }
                }
            }
            Event::RoomState(snapshot) => {
                let room = snapshot.map(|room| *room);
                if self.selector.lobby.room().map(|r| r.id) != room.as_ref().map(|r| r.id) {
                    self.online.movement.clear();
                    self.online.local_walk = None;
                    self.online.walk_sent = [0, 0];
                    self.selector.lobby.stop_walking();
                }
                self.selector.lobby.directory.clear();
                if let Some(r) = &room {
                    ensure!(
                        r.members
                            .iter()
                            .any(|m| Some(m.id) == self.selector.lobby.player),
                        "Unexpected room subscription"
                    );
                    ensure!(
                        r.members.len() <= 8 && r.keys == 4 && r.name.len() <= 60,
                        "Invalid room snapshot"
                    );
                    if let Some(spec) = &r.chart {
                        spec.validate()?;
                    }
                    let mut slots = std::collections::BTreeSet::new();
                    let mut ids = std::collections::BTreeSet::new();
                    for member in &r.members {
                        ensure!(
                            member.name.len() <= 40
                                && member.slot < 8
                                && slots.insert(member.slot)
                                && ids.insert(member.id),
                            "Invalid room member"
                        );
                        ensure!(
                            [member.x, member.y].iter().all(|v| (0.0..=1.0).contains(v))
                                && member.facing != [0, 0]
                                && member
                                    .facing
                                    .iter()
                                    .chain(&member.walking)
                                    .all(|v| (-1..=1).contains(v)),
                            "Invalid room movement state"
                        );
                        self.online
                            .movement
                            .entry(member.id)
                            .or_default()
                            .push(member);
                        if Some(member.id) == self.selector.lobby.player {
                            let now = std::time::Instant::now();
                            self.online
                                .local_walk
                                .get_or_insert_with(|| crate::scene::LocalWalk::new(member, now))
                                .observe(
                                    member,
                                    now,
                                    self.online.client.as_ref().and_then(Client::server_us),
                                );
                        }
                    }
                    self.online.movement.retain(|id, _| ids.contains(id));
                }
                self.selector.lobby.joined_room = room;
                if self
                    .selector
                    .lobby
                    .room()
                    .is_some_and(|room| Some(room.host) != self.selector.lobby.player)
                {
                    self.online.start_when_ready = false;
                }
                if self.selector.lobby.room().is_some() {
                    self.selector.lobby.create_room_open = false;
                    self.selector.lobby.creating_room = false;
                    self.selector.lobby.room_password.clear();
                    self.selector.lobby.clear_password_prompt();
                }
                let ids = self
                    .selector
                    .lobby
                    .room()
                    .map(|room| {
                        room.members
                            .iter()
                            .map(|m| m.id)
                            .collect::<std::collections::BTreeSet<_>>()
                    })
                    .unwrap_or_default();
                let round_ids = self
                    .online
                    .round_room
                    .as_ref()
                    .map(|r| {
                        r.members
                            .iter()
                            .map(|m| m.id)
                            .collect::<std::collections::BTreeSet<_>>()
                    })
                    .unwrap_or_default();
                self.selector
                    .lobby
                    .portraits
                    .retain(|id, _| ids.contains(id) || round_ids.contains(id));
                self.online
                    .appearances
                    .retain(|id, _| ids.contains(id) || round_ids.contains(id));
                if self.selector.lobby.joined_room.is_none() {
                    self.online.scene_keys.clear();
                    self.online.scene_pending.clear();
                    self.online.actors.clear();
                    self.online.scene_song = None;
                    self.selector.lobby.available_avatars.clear();
                    self.selector.lobby.avatar_issues.clear();
                    self.selector.lobby.stage_labels.clear();
                    if let Some(state) = &mut self.state {
                        state.multiplayer_stage.retain(&[]);
                    }
                }
                let current = self.selector.lobby.room();
                let current_id = current.map(|r| r.id);
                let stale_load = matches!(&self.online.loading,Some(LoadPurpose::Ready(spec)) if current.and_then(|r|r.chart.as_ref())!=Some(spec));
                if self.selector.lobby.chat_room != current_id {
                    self.selector.lobby.chat_room = current_id;
                    self.selector.lobby.messages.clear();
                }
                if stale_load {
                    self.cancel_loading();
                    self.selector.lobby.open = true;
                }
                let current = self.selector.lobby.room();
                // An active room membership is required throughout a local round.
                if current.is_none() && self.online.round.is_some() {
                    self.clear_online_attempt();
                }
            }
            Event::Started {
                room,
                round,
                start_us,
                offsets,
            } => {
                let current = self
                    .selector
                    .lobby
                    .room()
                    .context("No room for this round")?;
                ensure!(current.id == room, "Unexpected room start");
                ensure!(
                    self.online.round.is_none(),
                    "Another round is already active"
                );
                // Record the round before fallible activation so every failure sends a forfeit.
                self.online.round = Some((round, start_us));
                self.online.round_room = Some(current.clone());
                self.online.local_walk = None;
                let (spec, mut session) = self
                    .online
                    .prepared
                    .take()
                    .context("Local chart has not been loaded and verified")?;
                ensure!(
                    current.chart.as_ref() == Some(&spec),
                    "Room chart changed before starting"
                );
                ensure!(
                    offsets.len() == current.members.len()
                        && current.members.iter().all(|m| offsets.contains_key(&m.id))
                        && offsets.values().all(|v| v.unsigned_abs() <= 500_000),
                    "Invalid round participant offsets"
                );
                let mut shared_practice = session.practice.clone();
                shared_practice.set_paused(false);
                self.online.replays = offsets
                    .keys()
                    .map(|id| {
                        (
                            *id,
                            sdo_multiplayer::Replay::for_presentation(shared_practice.clone()),
                        )
                    })
                    .collect();
                self.online.offsets = offsets;
                self.preview_audio.stop();
                self.preview_id = None;
                self.selector.lobby.stop_walking();
                self.online.walk_sent = [0, 0];
                let deadline = self.online.client.as_ref().unwrap().deadline(start_us)?;
                session.display.speed = self.selector.lobby.scroll_speed;
                session.audio.schedule_start(deadline)?;
                self.online.deadline = Some(deadline);
                self.online.confirmed =
                    vec![sdo_ui::notes::ServerGrade::Unreported; session.practice.note_count()];
                self.online.correction = None;
                self.online.judgment_revision = 0;
                session.practice.set_paused(false);
                self.online.round = Some((round, start_us));
                self.online.sequence = 0;
                self.session = Some(session);
                self.selector.lobby.standings.clear();
                self.selector.lobby.results = false;
                self.selector.lobby.status = "Round starting".into();
            }
            Event::Judgments {
                room,
                round,
                revision,
                changes,
            } => {
                if self.online.round.is_none_or(|(id, _)| id != round)
                    || self.online.round_room.as_ref().is_none_or(|r| r.id != room)
                {
                    return Ok(());
                }
                ensure!(
                    revision == self.online.judgment_revision + 1,
                    "Missing judgment revision"
                );
                let practice = self
                    .selector
                    .lobby
                    .player
                    .and_then(|id| self.online.replays.get(&id))
                    .context("No active player replay")?
                    .practice();
                for (index, grade) in changes {
                    let index = index as usize;
                    let value = grade
                        .map(|i| {
                            sdo_gameplay::Judgment::ALL
                                .get(i as usize)
                                .copied()
                                .context("Invalid judgment")
                        })
                        .transpose()?;
                    *self
                        .online
                        .confirmed
                        .get_mut(index)
                        .context("Unknown judged note")? = value.map_or(
                        sdo_ui::notes::ServerGrade::Pending,
                        sdo_ui::notes::ServerGrade::Judged,
                    );
                    if self
                        .online
                        .correction
                        .is_some_and(|c| c.note == Some(index))
                    {
                        self.online.correction = None;
                    }
                    if practice.note_grade(index) != value {
                        self.online.correction =
                            value.map(|judgment| sdo_gameplay::JudgmentEvent {
                                note: Some(index),
                                judgment,
                                at: practice.position(),
                                error: 0.,
                            });
                    }
                }
                self.online.judgment_revision = revision;
            }
            Event::Scores {
                room,
                round,
                standings,
                final_result,
                server_us,
            } => {
                if self.online.round.is_some_and(|(active, _)| active == round)
                    && self.selector.lobby.room().is_some_and(|r| r.id == room)
                {
                    let elapsed = self
                        .online
                        .round
                        .map_or(0., |(_, start)| (server_us as f64 - start as f64) / 1e6)
                        .max(0.);
                    for standing in &standings {
                        if standing.forfeited
                            && let Some(replay) = self.online.replays.get_mut(&standing.player)
                        {
                            let chart_time = elapsed
                                - self
                                    .online
                                    .offsets
                                    .get(&standing.player)
                                    .copied()
                                    .unwrap_or(0) as f64
                                    / 1e6;
                            replay.forfeit((chart_time.max(0.) * 1e6) as u64);
                        }
                    }
                    self.selector.lobby.standings = standings;
                    self.selector.lobby.results = final_result;
                    if final_result {
                        self.record_completed_score(false);
                        self.online.round_elapsed = elapsed;
                        if let Some(session) = &mut self.session {
                            session.audio.set_paused(true)?;
                            session.input.clear();
                        }
                        self.selector.lobby.status =
                            "Round complete. Results are judged by the server.".into();
                    }
                }
            }
            Event::Aborted { round, reason, .. } => {
                if self.online.round.is_some_and(|(active, _)| active == round) {
                    self.clear_online_attempt();
                }
                self.selector.lobby.status = reason;
            }
            Event::Error(message) => {
                self.selector.lobby.joining_room = false;
                if self.selector.lobby.creating_room {
                    self.selector.lobby.creating_room = false;
                    self.selector.lobby.room_error = message;
                    return Ok(());
                }
                if self.selector.lobby.password_room.is_some() {
                    self.selector.lobby.password_error = message;
                    return Ok(());
                }
                if self.selector.lobby.private_server && self.selector.lobby.room().is_none() {
                    self.selector.lobby.status = message;
                    return Ok(());
                }
                if self.selector.lobby.room().is_some() && !self.online_running() {
                    let _ = self.net_send(Command::Unready);
                }
                self.online.prepared = None;
                self.selector.lobby.status = message.clone();
                self.selector.error = Some(message);
            }
            Event::Chat { room, message } => {
                ensure!(
                    message.text.len() <= sdo_net::MAX_CHAT_BYTES && message.name.len() <= 40,
                    "Invalid chat message"
                );
                if self.selector.lobby.chat_room == Some(room)
                    || self.selector.lobby.room().is_some_and(|r| r.id == room)
                {
                    self.selector.lobby.receive_chat(room, message);
                }
            }
            Event::ChatHistory { room, messages } => {
                ensure!(
                    messages.len() <= sdo_net::MAX_CHAT_MESSAGES
                        && messages
                            .iter()
                            .all(|m| m.text.len() <= sdo_net::MAX_CHAT_BYTES && m.name.len() <= 40),
                    "Invalid room chat history"
                );
                self.selector.lobby.chat_room = Some(room);
                self.selector.lobby.messages.clear();
                for message in messages {
                    self.selector.lobby.receive_chat(room, message);
                }
            }
            Event::Appearance {
                room,
                player,
                appearance,
            } => {
                // Admission history identifies the joined room before the next
                // membership snapshot. Preserve its appearances in either order.
                if self.selector.lobby.chat_room == Some(room)
                    || self.selector.lobby.room().is_some_and(|r| r.id == room)
                {
                    if let Some(appearance) = appearance {
                        appearance.validate()?;
                        self.online.appearances.insert(player, appearance);
                    } else {
                        self.online.appearances.remove(&player);
                        self.online.scene_keys.remove(&player);
                        self.online.actors.remove(&player);
                        if let Some(state) = &mut self.state {
                            state.multiplayer_stage.remove_model(player);
                        }
                    }
                }
            }
            Event::Input {
                room,
                round,
                player,
                sequence,
                song_time,
                lane,
                down,
            } => {
                if self.online.round.is_some_and(|(id, _)| id == round)
                    && self
                        .online
                        .round_room
                        .as_ref()
                        .is_some_and(|r| r.id == room)
                {
                    let replay = self
                        .online
                        .replays
                        .get_mut(&player)
                        .context("Unknown round participant")?;
                    if sequence > replay.sequence() {
                        replay.input(
                            sequence,
                            song_time.chart_time(self.online.offsets[&player]),
                            lane,
                            down,
                        )?;
                    }
                }
            }
            Event::Pong { .. } => {}
        }
        Ok(())
    }
    pub(crate) fn sync_room_walk(&mut self) -> Result<()> {
        let direction = if self.focused
            && self.selector.lobby.open
            && !self.online_running()
            && self.selector.lobby.room().is_some()
        {
            self.selector.lobby.walk_direction
        } else {
            [0, 0]
        };
        if direction != self.online.walk_sent
            && self.online.client.is_some()
            && self.selector.lobby.room().is_some()
        {
            self.net_send(Command::Walk { direction })?;
            self.online.walk_sent = direction;
            if let Some(walk) = &mut self.online.local_walk {
                walk.turn(
                    direction,
                    std::time::Instant::now(),
                    self.online.client.as_ref().and_then(Client::server_us),
                );
            }
        }
        Ok(())
    }
    pub(crate) fn online_status_lines(&self) -> Vec<String> {
        if let Some((_, start)) = self.online.round {
            let now = self
                .online
                .client
                .as_ref()
                .and_then(Client::server_us)
                .unwrap_or(0);
            if now < start {
                return vec![format!(
                    "ONLINE - START IN {:.1}",
                    (start - now) as f64 / 1e6
                )];
            }
        } else {
            return Vec::new();
        }
        let mut lines = vec!["ONLINE - ESC FORFEITS".into()];
        for score in &self.selector.lobby.standings {
            let name = self
                .selector
                .lobby
                .room()
                .and_then(|r| r.members.iter().find(|m| m.id == score.player))
                .map(|m| m.name.as_str())
                .unwrap_or("Left");
            lines.push(format!(
                "{}: {}{}",
                name.chars().take(12).collect::<String>(),
                score.points,
                if score.forfeited {
                    " FORFEIT"
                } else if score.failed {
                    " FAILED"
                } else {
                    ""
                }
            ));
        }
        lines
    }
}

impl App {
    pub(crate) fn online_round_seconds(&self) -> f64 {
        if self.selector.lobby.results {
            return self.online.round_elapsed;
        }
        self.online
            .round
            .and_then(|(_, start)| {
                self.online
                    .client
                    .as_ref()
                    .and_then(Client::server_us)
                    .map(|now| (now as f64 - start as f64) / 1e6)
            })
            .unwrap_or(0.)
    }
    pub(crate) fn window_focus(&mut self, focused: bool) -> Result<()> {
        let captured = std::time::Instant::now();
        self.focused = focused;
        if !focused {
            self.selector.lobby.stop_walking();
            self.sync_room_walk()?;
            let song = self.advance_playback_at(captured)?;
            let online = self.online_running();

            let mut releases = Vec::new();
            if let Some(session) = &mut self.session {
                for transition in session.input.clear() {
                    if let Transition::Release(lane) = transition {
                        session.practice.release(lane);
                    }
                    if online && let Some(at) = song {
                        releases.push((transition, at));
                    }
                }
                if !online {
                    session.pause(true)?;
                }
            }
            for (transition, at) in releases {
                self.send_online_input(transition, at)?;
            }
        }
        if self.session.is_none() {
            self.preview_audio
                .update(focused || self.selector.lobby.room().is_some());
        }
        Ok(())
    }
}

/// Existing 3D geometry supplies both the close-up card and the stage animation.
#[cfg(test)]
mod tests {
    use super::*;
    use sdo_asset_loader::{request as asset, sources};
    use sdo_config::ScoreKey;
    use sdo_net::{Member, Room};
    fn empty_app() -> Result<App> {
        let (content, _) = sources::open(&Default::default())?;
        App::new(
            content.clone(),
            Default::default(),
            content.request(asset::Catalog)?,
            content.request(asset::Ui)?,
            None,
            Default::default(),
            Default::default(),
        )
    }
    #[test]
    fn joining_keeps_appearances_received_before_the_room_snapshot() -> Result<()> {
        let mut app = empty_app()?;
        app.selector.lobby.player = Some(2);
        let appearance = sdo_net::Appearance {
            preset: "male".into(),
            equipment: Default::default(),
            sha256: "a".repeat(64),
        };
        // The server emits admission history and existing appearances before
        // its next tick publishes the new membership in Event::Rooms.
        app.online_event(ClientEvent::Message(Event::ChatHistory {
            room: 7,
            messages: vec![],
        }))?;
        assert!(app.selector.lobby.room().is_none());
        app.online_event(ClientEvent::Message(Event::Appearance {
            room: 7,
            player: 1,
            appearance: Some(appearance.clone()),
        }))?;
        assert_eq!(app.online.appearances.get(&1), Some(&appearance));
        let room: Room = serde_json::from_value(serde_json::json!({
            "id":7,"name":"Joined room","host":1,"keys":4,"levels":[null,null,null],
            "password_protected":false,"chart":null,"round":null,"start_us":null,
            "members":[
                {"id":1,"name":"Host","slot":0,"x":0.2,"y":0.6,"facing":[0,1],"walking":[0,0],"ready":false},
                {"id":2,"name":"Guest","slot":1,"x":0.4,"y":0.6,"facing":[0,1],"walking":[0,0],"ready":false}
            ]
        }))?;
        app.online_event(ClientEvent::Message(Event::RoomState(Some(Box::new(room)))))?;
        assert_eq!(app.online.appearances.get(&1), Some(&appearance));
        app.online_event(ClientEvent::Message(Event::Appearance {
            room: 8,
            player: 3,
            appearance: Some(appearance),
        }))?;
        assert!(
            !app.online.appearances.contains_key(&3),
            "Other rooms remain isolated"
        );
        Ok(())
    }
    #[test]
    fn acknowledging_disconnected_results_clears_stale_room_membership() -> Result<()> {
        let mut app = empty_app()?;
        app.online.round = Some((1, 1));
        app.selector.lobby.player = Some(1);
        app.selector.lobby.results = true;
        app.lobby_action(&LobbyAction::ConfirmResults)?;
        assert!(app.online.round.is_none());
        assert!(app.selector.lobby.player.is_none());
        assert!(!app.selector.lobby.results);
        Ok(())
    }
    #[test]
    fn hosting_requires_a_name_and_creates_the_named_room_after_connecting() -> Result<()> {
        let mut app = empty_app()?;
        assert!(app.lobby_action(&LobbyAction::Host).is_err());
        assert!(app.online.server.is_none());
        app.selector.lobby.room_name = "Portrait room".into();
        app.selector.lobby.preferences.listen = "127.0.0.1:0".into();
        app.lobby_action(&LobbyAction::Host)?;
        let began = std::time::Instant::now();
        while app.selector.lobby.room().is_none() {
            app.poll_multiplayer();
            ensure!(
                began.elapsed() < std::time::Duration::from_secs(5),
                "Named room did not appear"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let room = app.selector.lobby.room().unwrap();
        assert_eq!(room.name, "Portrait room");
        assert_eq!(Some(room.host), app.selector.lobby.player);
        assert_eq!(room.members[0].slot, 0);
        assert!(
            app.selector.lobby.private_server,
            "GUI hosts always run a single-room session"
        );
        app.selector.lobby.song_picker_open = true;
        app.lobby_action(&LobbyAction::Exit)?;
        assert!(!app.selector.lobby.open);
        assert!(!app.selector.lobby.song_picker_open);
        assert!(app.selector.lobby.room().is_none());
        assert!(app.online.client.is_none());
        assert!(app.online.server.is_none());
        assert!(app.selector.lobby.hosting.is_none());
        Ok(())
    }

    #[test]
    fn private_hosting_auto_opens_room_and_password_prompt_handles_retries() -> Result<()> {
        fn until(
            host: &mut App,
            guest: &mut App,
            predicate: impl Fn(&App, &App) -> bool,
        ) -> Result<()> {
            let began = std::time::Instant::now();
            while !predicate(host, guest) {
                host.poll_multiplayer();
                guest.poll_multiplayer();
                ensure!(
                    began.elapsed() < std::time::Duration::from_secs(5),
                    "Private admission timed out"
                );
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            Ok(())
        }
        let mut host = empty_app()?;
        let mut guest = empty_app()?;
        host.selector.lobby.room_name = "Private friends".into();
        host.selector.lobby.room_password = "friends only".into();

        host.selector.lobby.preferences.listen = "127.0.0.1:0".into();
        host.lobby_action(&LobbyAction::Host)?;
        until(&mut host, &mut guest, |host, _| {
            host.selector.lobby.room().is_some()
        })?;
        assert!(host.selector.lobby.private_server);
        assert!(host.selector.lobby.room().unwrap().password_protected);
        assert!(host.selector.lobby.room_password.is_empty());
        guest.selector.lobby.preferences.address =
            host.online.server.as_ref().unwrap().address.to_string();
        guest.lobby_action(&LobbyAction::Connect)?;
        until(&mut host, &mut guest, |_, guest| {
            guest.selector.lobby.password_room.is_some()
        })?;
        assert!(guest.selector.lobby.private_server);
        assert!(guest.selector.lobby.room().is_none());
        guest.lobby_action(&LobbyAction::JoinWithPassword(1, "wrong".into()))?;
        until(&mut host, &mut guest, |_, guest| {
            !guest.selector.lobby.password_error.is_empty()
        })?;
        assert!(
            guest.selector.error.is_none(),
            "Password failures stay in the password prompt"
        );
        assert!(guest.selector.lobby.room().is_none());
        guest.lobby_action(&LobbyAction::JoinWithPassword(1, "friends only".into()))?;
        until(&mut host, &mut guest, |host, guest| {
            host.selector
                .lobby
                .room()
                .is_some_and(|room| room.members.len() == 2)
                && guest.selector.lobby.room().is_some()
        })?;
        assert!(guest.selector.lobby.password_room.is_none());
        assert!(guest.selector.lobby.join_password.is_empty());
        guest.lobby_action(&LobbyAction::Leave)?;
        assert!(guest.online.client.is_none());
        host.lobby_action(&LobbyAction::Exit)?;
        Ok(())
    }
    #[test]
    fn multiplayer_preview_browses_independently_and_cancel_restores_room() -> Result<()> {
        let mut app = empty_app()?;
        let raw = serde_json::json!({"duration_unit":"seconds","source_country_keys":{"game":"Game"},"songs":[
            {"id":"1","title":"One","artist":"Artist","duration":10.,"bpm":120.,"audio":"music/1.ogg","patterns":[{"file":"music/1.gn","type":"k","mode":4}],"sources":{"game":{"id":"1"}},"source":"game"},
            {"id":"2","title":"Two","artist":"Artist","duration":10.,"bpm":140.,"audio":"music/2.ogg","patterns":[{"file":"music/2.gn","type":"k","mode":4}],"sources":{"game":{"id":"1"}},"source":"game"}
        ]});
        let mut catalog = sdo_song::Catalog::parse(&serde_json::to_vec(&raw)?)?;
        catalog.songs[0].patterns[0].sha256 = "a".repeat(64);
        catalog.songs[1].patterns[0].sha256 = "b".repeat(64);
        app.selector.replace_catalog(catalog);
        assert_eq!(app.selector.selected().unwrap().id, "1");
        app.selector.lobby.player = Some(1);
        app.selector.lobby.joined_room = Some(Room {
            keys: 4,
            levels: [None; 3],
            password_protected: false,
            id: 33,
            name: "Room".into(),
            host: 1,
            members: vec![Member {
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
            chart: Some(ChartSpec {
                pattern_sha256: "b".repeat(64),
                audio_sha256: "c".repeat(64),
                dance_sha256: None,
                title: "Two".into(),
                kind: "k".into(),
                difficulty: 2,
                judgment: "keyboard".into(),
                bpm: 140.,
                duration_us: 10_000_000,
            }),
            round: None,
            start_us: None,
        });
        app.selector.lobby.open = true;
        app.focused = false;
        assert!(app.preview_active());
        assert_eq!(app.preview_song().unwrap().id, "2");
        app.sync_preview();
        assert_eq!(app.preview_id, Some(format!("room:33:{}", "b".repeat(64))));
        assert_eq!(app.selector.selected().unwrap().id, "1");
        assert!(
            app.start_selected().is_err(),
            "Single-player playback is unavailable in Multiplayer"
        );
        app.selector.lobby.song_picker_open = true;
        app.selector.lobby.song_preview = Some("1".into());
        assert_eq!(app.preview_song().unwrap().id, "1");
        app.sync_preview();
        assert_eq!(app.preview_id.as_deref(), Some("room-picker:33:1"));
        app.selector.lobby.song_preview = Some("2".into());
        app.sync_preview();
        assert_eq!(app.preview_id.as_deref(), Some("room-picker:33:2"));
        app.selector.lobby.song_preview = None;
        app.sync_preview();
        assert!(app.preview_song().is_none() && app.preview_id.is_none());
        app.selector.lobby.song_picker_open = false;
        app.sync_preview();
        assert_eq!(app.preview_song().unwrap().id, "2");
        assert_eq!(
            app.selector
                .lobby
                .room()
                .unwrap()
                .chart
                .as_ref()
                .unwrap()
                .title,
            "Two"
        );
        assert_eq!(app.selector.selected().unwrap().id, "1");
        app.selector.select_room_song("2")?;
        assert_eq!(app.selector.selected().unwrap().id, "1");
        assert_eq!(app.selector.difficulty, 0);
        assert_eq!(app.selector.room_song_selection().unwrap().difficulty, 2);
        app.selector.lobby.joined_room = None;
        assert!(!app.preview_active());
        assert!(
            app.preview_song().is_none(),
            "Server menus must not play the single-player selection"
        );
        app.lobby_action(&LobbyAction::Exit)?;
        assert_eq!(app.preview_song().unwrap().id, "1");
        Ok(())
    }
    #[test]
    fn missing_song_is_reported_automatically_in_room_chat_once() -> Result<()> {
        use std::time::{Duration, Instant};
        let (content, _) = sources::open(&Default::default())?;
        let mut guest = App::new(
            content.clone(),
            Default::default(),
            content.request(asset::Catalog)?,
            content.request(asset::Ui)?,
            None,
            Default::default(),
            Default::default(),
        )?;
        let server = Server::bind("127.0.0.1:0")?;
        let mut host = Client::connect(server.address.to_string(), "Host".into())?;
        guest.selector.lobby.preferences.address = server.address.to_string();
        guest.selector.lobby.preferences.name = "Missing Player".into();
        guest.lobby_action(&LobbyAction::Connect)?;
        fn drive(
            host: &mut Client,
            guest: &mut App,
            predicate: impl Fn(&[Event], &App) -> bool,
        ) -> Result<Vec<Event>> {
            let began = Instant::now();
            let mut events = Vec::new();
            loop {
                for event in host.poll() {
                    match event {
                        ClientEvent::Message(event) => {
                            if let Event::Error(message) = &event {
                                anyhow::bail!("Host: {message}");
                            }
                            events.push(event);
                        }
                        ClientEvent::Disconnected(message) => anyhow::bail!("Host: {message}"),
                    }
                }
                guest.poll_multiplayer();
                ensure!(
                    guest.selector.error.is_none(),
                    "Guest modal error: {:?}",
                    guest.selector.error
                );
                if predicate(&events, guest) {
                    return Ok(events);
                }
                ensure!(
                    began.elapsed() < Duration::from_secs(5),
                    "Room chat test timed out"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        drive(&mut host, &mut guest, |events, guest| {
            events.iter().any(|e| matches!(e, Event::Welcome { .. }))
                && guest.selector.lobby.player.is_some()
        })?;
        host.send(Command::Create {
            password: String::new(),
            name: "Content check".into(),
        })?;
        drive(&mut host, &mut guest, |_, guest| {
            guest.selector.lobby.directory.len() == 1
        })?;
        guest.lobby_action(&LobbyAction::Join(1))?;
        drive(&mut host, &mut guest, |_, guest| {
            guest.selector.lobby.room().is_some()
        })?;
        let mut raw = vec![0; 300];
        raw[4..8].copy_from_slice(b"gn\0\0");
        raw[8..12].copy_from_slice(&2.9f32.to_le_bytes());
        raw[16..20].copy_from_slice(&120f32.to_le_bytes());
        raw.extend(0u32.to_le_bytes());
        raw.extend(2u16.to_le_bytes());
        raw.extend(1u16.to_le_bytes());
        raw.extend(1u32.to_le_bytes());
        let end = raw.len() as u32;
        for (i, offset) in [300, end, end, end].into_iter().enumerate() {
            raw[284 + 4 * i..288 + 4 * i].copy_from_slice(&offset.to_le_bytes());
        }
        let spec = ChartSpec {
            pattern_sha256: sdo_song::chart::pattern_sha256(&raw),
            audio_sha256: "a".repeat(64),
            dance_sha256: None,
            title: "Unavailable Song".into(),
            kind: "k".into(),
            difficulty: 0,
            judgment: "keyboard".into(),
            bpm: 120.,
            duration_us: 1_000_000,
        };
        host.send(Command::Offer { spec, raw })?;
        let received = drive(&mut host, &mut guest, |events, guest| {
            events
                .iter()
                .any(|e| matches!(e,Event::Chat{message,..} if message.error))
                && guest.selector.lobby.messages.iter().any(|m| m.error)
        })?;
        assert_eq!(
            received
                .iter()
                .filter(|e| matches!(e,Event::Chat{message,..} if message.error))
                .count(),
            1
        );
        let error = &guest.selector.lobby.messages[0];
        assert!(
            error.text.contains("Missing Player")
                && error.text.contains("Unavailable Song")
                && error.text.contains("missing locally")
        );
        assert!(!guest.selector.lobby.matching_chart);
        for _ in 0..20 {
            guest.poll_multiplayer();
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(
            guest
                .selector
                .lobby
                .messages
                .iter()
                .filter(|m| m.error)
                .count(),
            1
        );
        guest.selector.lobby.chat_draft = "I need to import this song".into();
        guest.lobby_action(&LobbyAction::SendChat)?;
        drive(&mut host, &mut guest, |events, guest| {
            events.iter().any(|e|matches!(e,Event::Chat{message,..} if message.player==guest.selector.lobby.player)) && guest.selector.lobby.messages.len()==2
        })?;
        assert!(guest.selector.lobby.chat_draft.is_empty());
        // A failed ready load is also a room error, without a modal hiding the chat.
        let spec = guest.selector.lobby.room().unwrap().chart.clone().unwrap();
        guest.online.loading = Some(LoadPurpose::Ready(spec));
        assert!(guest.online_loading_failed(Some("Recording could not be decoded")));
        drive(&mut host, &mut guest, |_, guest| {
            guest
                .selector
                .lobby
                .messages
                .iter()
                .any(|m| m.text.contains("Recording could not be decoded"))
        })?;
        assert!(guest.selector.error.is_none());
        Ok(())
    }
    #[test]
    fn local_preparation_rehashes_bytes_and_checks_recording_without_native_formats() -> Result<()>
    {
        let (content, _) = sources::open(&Default::default())?;
        let mut app = App::new(
            content.clone(),
            Default::default(),
            content.request(asset::Catalog)?,
            content.request(asset::Ui)?,
            None,
            Default::default(),
            Default::default(),
        )?;
        let mut raw = vec![0; 300];
        raw[4..8].copy_from_slice(b"gn\0\0");
        raw[8..12].copy_from_slice(&2.9f32.to_le_bytes());
        raw[16..20].copy_from_slice(&120f32.to_le_bytes());
        raw.extend(0u32.to_le_bytes());
        raw.extend(2u16.to_le_bytes());
        raw.extend(1u16.to_le_bytes());
        raw.extend(1u32.to_le_bytes());
        let end = raw.len() as u32;
        for (i, offset) in [300, end, end, end].into_iter().enumerate() {
            raw[284 + 4 * i..288 + 4 * i].copy_from_slice(&offset.to_le_bytes());
        }
        let chart = sdo_song::Chart::parse(raw, sdo_song::Difficulty::Easy)?;
        let audio = sdo_audio::DecodedSong {
            samples: vec![0.; 48000],
            channels: 1,
            sample_rate: 48000,
        };
        let spec = ChartSpec {
            pattern_sha256: chart.pattern_sha256(),
            audio_sha256: audio.sha256(),
            dance_sha256: Some(sdo_dance::song_signature(None)),
            title: "Peer title".into(),
            kind: "k".into(),
            difficulty: 0,
            judgment: "keyboard".into(),
            bpm: 120.,
            duration_us: 1_000_000,
        };
        app.selector.lobby.player = Some(2);
        app.selector.lobby.joined_room = Some(Room {
            keys: 4,
            levels: [None; 3],
            password_protected: false,
            id: 1,
            name: "Room".into(),
            host: 1,
            members: vec![Member {
                state_us: 0,
                x: 0.5,
                y: 0.65,
                facing: [1, 0],
                walking: [0, 0],
                slot: 1,
                id: 2,
                name: "Guest".into(),
                ready: false,
            }],
            chart: Some(spec.clone()),
            round: None,
            start_us: None,
        });
        let mut prepared = PreparedSong {
            context: PlaySelection {
                network: Some(LoadPurpose::Ready(spec.clone())),
                score_key: ScoreKey::new(
                    "different-package::17",
                    "k",
                    0,
                    "keyboard",
                    &spec.pattern_sha256,
                ),
                title: "Different local title".into(),
            },
            pattern_sha256: spec.pattern_sha256.clone(),
            audio_sha256: Some(spec.audio_sha256.clone()),
            chart,
            audio,
            dance: None,
            idle: None,
            miss: None,
            dance_errors: vec![],
            difficulty: sdo_song::Difficulty::Easy,
        };
        assert_eq!(app.prepare_online(&prepared)?, Some(spec.clone()));
        // A cached catalog/key digest cannot stand in for the actual bytes.
        prepared.chart.raw[100] ^= 1;
        assert!(app.prepare_online(&prepared).is_err());
        prepared.chart.raw[100] ^= 1;
        prepared.audio_sha256 = Some("f".repeat(64));
        assert!(app.prepare_online(&prepared).is_err());
        prepared.audio_sha256 = Some(spec.audio_sha256.clone());
        prepared.chart.bpm = 121.;
        assert!(app.prepare_online(&prepared).is_err());
        prepared.chart.bpm = 120.;
        app.focused = false;
        assert!(
            app.online_event(ClientEvent::Message(Event::Started {
                room: 1,
                round: 4,
                start_us: 3_000_000,
                offsets: std::collections::BTreeMap::new(),
            }))
            .is_err()
        );
        assert_eq!(
            app.online.round,
            Some((4, 3_000_000)),
            "Activation failures retain the round ID long enough to forfeit it"
        );
        app.forfeit_online();
        assert!(app.online.round.is_none());
        Ok(())
    }
}
