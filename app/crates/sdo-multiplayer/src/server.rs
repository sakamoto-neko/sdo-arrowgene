use crate::{referee::Referee, *};
use anyhow::{Context, Result, ensure};
use sdo_gameplay::Practice;
use sdo_platform::net::{ServerConnection, ServerListener};
use std::{
    collections::{BTreeMap, VecDeque},
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub struct Server {
    pub address: SocketAddr,
    private_key: Option<String>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}
impl Server {
    pub fn bind(address: &str) -> Result<Self> {
        Self::bind_mode(address, None)
    }
    pub fn bind_private(address: &str) -> Result<Self> {
        let mut bytes = [0; 32];
        getrandom::fill(&mut bytes)
            .map_err(|error| anyhow::anyhow!("Cannot create private host credential: {error}"))?;
        let key = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        Self::bind_mode(address, Some(key))
    }
    pub fn private_key(&self) -> Option<&str> {
        self.private_key.as_deref()
    }
    fn bind_mode(address: &str, private_key: Option<String>) -> Result<Self> {
        let listener = ServerListener::bind(address)
            .with_context(|| format!("Cannot host lobby at {address}"))?;
        let address = listener.local_addr()?;
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let hub = Hub {
            private_key: private_key.clone(),
            ..Default::default()
        };
        let thread = thread::Builder::new()
            .name("lobby-server".into())
            .spawn(move || serve(listener, stopping, hub))?;
        Ok(Self {
            address,
            private_key,
            stop,
            thread: Some(thread),
        })
    }
    pub fn run(address: &str) -> Result<()> {
        let server = Self::bind(address)?;
        println!(
            "SDO dedicated server listening on {} (protocol {PROTOCOL}, multiple rooms); Ctrl-C stops it",
            server.address
        );
        loop {
            thread::park_timeout(Duration::from_secs(1));
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
struct Peer {
    wire: ServerConnection,
    last: Instant,
    connected: Instant,
    tokens: u32,
    refill: Instant,
}
struct Player {
    name: Option<String>,
    room: Option<RoomId>,
    ready: Option<i64>,
    slot: Option<u8>,
    x: f32,
    y: f32,
    facing: [i8; 2],
    walking: [i8; 2],
    appearance: Option<Appearance>,
    needs_directory: bool,
    needs_room: bool,
    movement_changed: bool,
    walk_updated_us: u64,
}
impl Player {
    fn advance_walk(&mut self, now: u64) {
        let dt = now.saturating_sub(self.walk_updated_us).min(250_000) as f32 / 1e6;
        self.walk_updated_us = now;
        let old = [self.x, self.y];
        [self.x, self.y] = walk_position(old, self.walking, dt);
        self.movement_changed |= old != [self.x, self.y];
    }
}
struct Match {
    id: u64,
    start: u64,
    players: BTreeMap<PlayerId, (i64, Referee)>,
    last_report: u64,
    settled_since: Option<u64>,
}
impl Match {
    fn standings(&self, final_result: bool) -> Vec<Standing> {
        let mut standings: Vec<_> = self
            .players
            .iter()
            .map(|(&id, (_, judge))| judge.standing(id))
            .collect();
        let finished =
            |s: &Standing| !s.forfeited && self.players[&s.player].1.practice().completed();
        standings.sort_by_key(|s| {
            (
                final_result && !finished(s),
                s.forfeited,
                std::cmp::Reverse(s.points),
                s.player,
            )
        });
        if final_result && standings.len() == 4 {
            let count = standings.iter().take_while(|s| finished(s)).count();
            let placed = &mut standings[..count];
            if placed
                .windows(2)
                .all(|pair| pair[0].points != pair[1].points)
            {
                for (s, percent) in placed.iter_mut().zip([4, 3, 2, 1]) {
                    s.points += s.points * percent / 100;
                }
            }
        }
        standings
    }
}

struct RoomState {
    view: Room,
    password: Option<crate::password::RoomPassword>,
    practice: Option<Practice>,
    raw: Option<Vec<u8>>,
    chat: VecDeque<ChatMessage>,
    game: Option<Match>,
    last_offer: u64,
    published: Option<Room>,
}
#[derive(Default)]
struct Hub {
    private_key: Option<String>,
    private_room: Option<RoomId>,
    players: BTreeMap<PlayerId, Player>,
    rooms: BTreeMap<RoomId, RoomState>,
    next: u64,
    next_chat: u64,
    lobby_chat: VecDeque<ChatMessage>,
    roster: Vec<OnlinePlayer>,
    out: Vec<(PlayerId, Event)>,
    dirty: bool,
    directory: Vec<RoomSummary>,
    walk_tick: Option<u64>,
}
impl Hub {
    fn connect(&mut self, id: PlayerId) {
        self.players.insert(
            id,
            Player {
                name: None,
                room: None,
                ready: None,
                slot: None,
                x: 0.5,
                y: 0.65,
                facing: [0, 1],
                walking: [0, 0],
                appearance: None,
                needs_directory: true,
                needs_room: true,
                movement_changed: false,
                walk_updated_us: 0,
            },
        );
    }
    fn disconnect(&mut self, id: PlayerId) {
        self.leave(id);
        if self.players.remove(&id).is_some_and(|p| p.name.is_some()) {
            self.dirty = true;
        }
    }
    fn emit(&mut self, id: PlayerId, event: Event) {
        self.out.push((id, event));
    }
    fn chat(&mut self, room: Option<RoomId>, player: Option<PlayerId>, text: String, error: bool) {
        self.next_chat += 1;
        let name = player
            .and_then(|id| self.players.get(&id))
            .and_then(|p| p.name.clone())
            .unwrap_or_else(|| "System".into());
        let message = ChatMessage {
            id: self.next_chat,
            player,
            name,
            text,
            error,
        };
        let history = match room {
            Some(id) => &mut self.rooms.get_mut(&id).unwrap().chat,
            None => &mut self.lobby_chat,
        };
        history.push_back(message.clone());
        if history.len() > MAX_CHAT_MESSAGES {
            history.pop_front();
        }
        for (&id, p) in &self.players {
            if p.name.is_some() && p.room == room {
                self.out.push((
                    id,
                    Event::Chat {
                        room,
                        message: message.clone(),
                    },
                ));
            }
        }
    }
    fn chat_history(&mut self, id: PlayerId, room: Option<RoomId>) {
        let history = room.map_or(&self.lobby_chat, |room| &self.rooms[&room].chat);
        let messages = history.iter().cloned().collect();
        self.emit(id, Event::ChatHistory { room, messages });
    }
    fn appearances_on_join(&mut self, id: PlayerId, room: RoomId) {
        let prior = self
            .players
            .iter()
            .filter(|&(&other, p)| other != id && p.room == Some(room))
            .map(|(&player, p)| (player, p.appearance.clone()))
            .collect::<Vec<_>>();
        for (player, appearance) in prior {
            self.emit(
                id,
                Event::Appearance {
                    room,
                    player,
                    appearance,
                },
            );
        }
        self.broadcast_appearance(id, room);
    }
    fn broadcast_appearance(&mut self, player: PlayerId, room: RoomId) {
        let appearance = self.players[&player].appearance.clone();
        for (&id, p) in &self.players {
            if p.room == Some(room) {
                self.out.push((
                    id,
                    Event::Appearance {
                        room,
                        player,
                        appearance: appearance.clone(),
                    },
                ));
            }
        }
    }
    fn room_id(&self, id: PlayerId) -> Result<RoomId> {
        self.players
            .get(&id)
            .and_then(|p| p.room)
            .context("Join a room first")
    }
    fn host(&self, id: PlayerId) -> Result<RoomId> {
        let room = self.room_id(id)?;
        ensure!(
            self.rooms[&room].view.host == id,
            "Only the host can do this"
        );
        Ok(room)
    }
    fn idle(&self, room: RoomId) -> Result<()> {
        ensure!(self.rooms[&room].game.is_none(), "Round is in progress");
        Ok(())
    }
    fn clear_ready(&mut self, room: RoomId) {
        for p in self.players.values_mut().filter(|p| p.room == Some(room)) {
            p.ready = None;
        }
        self.dirty = true;
    }
    fn leave(&mut self, id: PlayerId) {
        let Some(room) = self.players.get_mut(&id).and_then(|p| {
            p.ready = None;
            p.slot = None;
            p.walking = [0, 0];
            p.facing = [0, 1];
            p.y = 0.65;
            p.needs_directory = true;
            p.needs_room = true;
            p.movement_changed = false;
            p.room.take()
        }) else {
            return;
        };
        self.emit(id, Event::RoomState(None));
        if let Some(state) = self.rooms.get_mut(&room) {
            if let Some(game) = &mut state.game
                && let Some((_, judge)) = game.players.get_mut(&id)
            {
                judge.forfeited = true;
            }
            let mut remaining = self
                .players
                .iter()
                .filter(|(_, p)| p.room == Some(room))
                .map(|(&id, _)| id);
            if let Some(next) = remaining.next() {
                if state.view.host == id {
                    state.view.host = next;
                }
            } else if self.private_room == Some(room) {
                state.view.host = 0;
                state.game = None;
                state.view.round = None;
                state.view.start_us = None;
            } else {
                self.rooms.remove(&room);
            }
        }
        self.dirty = true;
    }
    fn abort(&mut self, room: RoomId, reason: &str) {
        let Some(game) = self.rooms.get_mut(&room).and_then(|r| r.game.take()) else {
            return;
        };
        for id in game.players.keys() {
            if self.players.get(id).is_none_or(|p| p.room != Some(room)) {
                continue;
            }
            self.emit(
                *id,
                Event::Aborted {
                    room,
                    round: game.id,
                    reason: reason.into(),
                },
            );
        }
        let r = self.rooms.get_mut(&room).unwrap();
        r.view.round = None;
        r.view.start_us = None;
        self.clear_ready(room);
    }
    fn create_room(&mut self, id: PlayerId, name: &str, password: &str) -> Result<RoomId> {
        ensure!(
            self.players[&id].room.is_none(),
            "Leave the current room first"
        );
        ensure!(self.rooms.len() < 32, "Server room limit reached");
        let name = name.trim();
        ensure!(
            !name.is_empty() && name.len() <= 60 && !name.chars().any(char::is_control),
            "Invalid room name"
        );
        let password = crate::password::RoomPassword::new(password)?;
        self.next += 1;
        let room = self.next;
        self.rooms.insert(
            room,
            RoomState {
                view: Room {
                    keys: 4,
                    levels: [None; 3],
                    password_protected: password.is_some(),
                    id: room,
                    name: name.into(),
                    host: id,
                    members: vec![],
                    chart: None,
                    round: None,
                    start_us: None,
                },
                password,
                practice: None,
                raw: None,
                chat: VecDeque::new(),
                game: None,
                last_offer: 0,
                published: None,
            },
        );
        let player = self.players.get_mut(&id).unwrap();
        player.room = Some(room);
        player.slot = Some(0);
        player.x = 0.1;
        self.chat_history(id, Some(room));
        self.appearances_on_join(id, room);
        self.dirty = true;
        Ok(room)
    }
    fn join_room(&mut self, id: PlayerId, room: RoomId, password: &str) -> Result<()> {
        ensure!(
            self.players[&id].room.is_none(),
            "Leave the current room first"
        );
        let state = self.rooms.get(&room).context("Room no longer exists")?;
        ensure!(
            state
                .password
                .as_ref()
                .is_none_or(|expected| expected.matches(password)),
            "Incorrect room password"
        );
        ensure!(state.game.is_none(), "Wait for this round to finish");
        let slot = (0..ROOM_SLOTS as u8)
            .find(|slot| {
                !self
                    .players
                    .values()
                    .any(|player| player.room == Some(room) && player.slot == Some(*slot))
            })
            .context("Room is full")?;
        let player = self.players.get_mut(&id).unwrap();
        player.room = Some(room);
        player.slot = Some(slot);
        player.x = 0.1 + slot as f32 * (0.8 / 7.);
        if self.rooms[&room].view.host == 0 {
            self.rooms.get_mut(&room).unwrap().view.host = id;
        }
        self.chat_history(id, Some(room));
        self.appearances_on_join(id, room);
        self.dirty = true;
        Ok(())
    }
    fn admit_private(&mut self, id: PlayerId) {
        let Some(room) = self.private_room else {
            return;
        };
        if self.rooms[&room].view.password_protected {
            self.emit(
                id,
                Event::RoomPasswordRequired {
                    room,
                    name: self.rooms[&room].view.name.clone(),
                },
            );
        } else if let Err(error) = self.join_room(id, room, "") {
            self.emit(id, Event::Error(error.to_string()));
        }
    }
    fn apply(&mut self, id: PlayerId, cmd: Command, now: u64) -> Result<()> {
        if let Command::Hello { protocol, name } = cmd {
            ensure!(self.players[&id].name.is_none(), "Already connected");
            ensure!(protocol == PROTOCOL, "Incompatible multiplayer protocol");
            let name = name.trim();
            ensure!(
                !name.is_empty() && name.len() <= 40 && !name.chars().any(char::is_control),
                "Name must contain 1 to 40 visible bytes"
            );
            self.players.get_mut(&id).unwrap().name = Some(name.into());
            self.emit(
                id,
                Event::Welcome {
                    protocol: PROTOCOL,
                    player: id,
                    private_server: self.private_key.is_some(),
                },
            );
            if self.private_key.is_some() {
                self.admit_private(id);
            } else {
                self.chat_history(id, None);
            }
            self.dirty = true;
            return Ok(());
        }
        ensure!(self.players[&id].name.is_some(), "Handshake required");
        match cmd {
            Command::Ping { sent_us } => self.emit(
                id,
                Event::Pong {
                    sent_us,
                    server_us: now,
                },
            ),
            Command::Create { name, password } => {
                ensure!(
                    self.private_key.is_none(),
                    "Private servers only allow their owner's single room"
                );
                self.create_room(id, &name, &password)?;
            }
            Command::OpenPrivateRoom {
                name,
                password,
                key,
            } => {
                ensure!(
                    self.private_key.as_deref() == Some(key.as_str()),
                    "Only the private server owner can open its room"
                );
                ensure!(
                    self.private_room.is_none(),
                    "This private server already has its room"
                );
                let room = self.create_room(id, &name, &password)?;
                self.private_room = Some(room);
                let waiting = self
                    .players
                    .iter()
                    .filter(|(_, player)| player.name.is_some() && player.room.is_none())
                    .map(|(&id, _)| id)
                    .collect::<Vec<_>>();
                for guest in waiting {
                    self.admit_private(guest);
                }
            }
            Command::Join { room, password } => self.join_room(id, room, &password)?,
            Command::Leave => {
                ensure!(
                    self.private_key.is_none(),
                    "Disconnect to leave a private server"
                );
                self.room_id(id)?;
                self.leave(id);
                self.chat_history(id, None);
            }
            Command::Walk { direction } => {
                let room = self.room_id(id)?;
                ensure!(
                    direction.iter().all(|v| (-1..=1).contains(v)),
                    "Invalid walking direction"
                );
                let direction = if self.rooms[&room].game.is_some() {
                    [0, 0]
                } else {
                    direction
                };
                let player = self.players.get_mut(&id).unwrap();
                player.advance_walk(now);
                player.movement_changed |= player.walking != direction;
                player.walking = direction;
                if direction != [0, 0] {
                    player.facing = direction;
                }
            }
            Command::TakeSlot { slot } => {
                let room = self.room_id(id)?;
                self.idle(room)?;
                ensure!(
                    slot < ROOM_SLOTS as u8
                        && !self.players.iter().any(|(&other, p)| other != id
                            && p.room == Some(room)
                            && p.slot == Some(slot)),
                    "That player slot is unavailable"
                );
                self.players.get_mut(&id).unwrap().slot = Some(slot);
                self.dirty = true;
            }
            Command::SetAppearance { appearance } => {
                let room = self.room_id(id)?;
                if let Some(appearance) = &appearance {
                    appearance.validate()?;
                }
                self.idle(room)?;
                // An identity change is durable state. Never discard the last
                // outfit update, and recheck compatibility before the next start.
                if self.players[&id].appearance == appearance {
                    return Ok(());
                }
                self.clear_ready(room);
                self.players.get_mut(&id).unwrap().appearance = appearance;
                self.broadcast_appearance(id, room);
            }
            Command::Offer { spec, raw } => {
                let room = self.host(id)?;
                self.idle(room)?;
                let state = &self.rooms[&room];
                ensure!(
                    state.last_offer == 0 || now.saturating_sub(state.last_offer) >= 1_000_000,
                    "Please wait before changing charts"
                );
                let practice = prepare_chart(&spec, raw.clone())?;
                let header = sdo_song::chart::ChartHeader::parse(&raw)?;
                let levels = std::array::from_fn(|index| header.level(Difficulty::ALL[index]));
                let state = self.rooms.get_mut(&room).unwrap();
                state.view.keys = 4;
                state.view.levels = levels;
                state.view.chart = Some(spec);
                state.practice = Some(practice);
                state.raw = Some(raw);
                state.last_offer = now;
                self.clear_ready(room);
            }
            Command::Ready { spec, offset_us } => {
                let room = self.room_id(id)?;
                self.idle(room)?;
                ensure!(
                    self.rooms[&room].view.chart.as_ref() == Some(&spec),
                    "Local chart/audio or rules do not match the room; load the exact file"
                );
                ensure!(
                    offset_us.unsigned_abs() <= 500_000,
                    "Online calibration must be within +/-500 ms"
                );
                self.players.get_mut(&id).unwrap().ready = Some(offset_us);
                self.dirty = true;
            }
            Command::Unready => {
                let room = self.room_id(id)?;
                if self.rooms[&room]
                    .game
                    .as_ref()
                    .is_some_and(|game| now >= game.start)
                {
                    if let Some((_, judge)) = self
                        .rooms
                        .get_mut(&room)
                        .and_then(|r| r.game.as_mut())
                        .and_then(|g| g.players.get_mut(&id))
                    {
                        judge.forfeited = true;
                    }
                } else {
                    self.abort(room, "A player cancelled loading");
                }
                self.players.get_mut(&id).unwrap().ready = None;
                self.dirty = true;
            }
            Command::TransferHost { player } => {
                ensure!(
                    self.private_key.is_none(),
                    "Room control cannot be transferred on a GUI-hosted server"
                );
                let room = self.host(id)?;
                self.idle(room)?;
                ensure!(
                    self.players
                        .get(&player)
                        .is_some_and(|target| target.room == Some(room)),
                    "The new host must be a member of this room"
                );
                if player != id {
                    self.rooms.get_mut(&room).unwrap().view.host = player;
                    self.players.get_mut(&id).unwrap().ready = None;
                    self.dirty = true;
                }
            }
            Command::SetDifficulty { difficulty } => {
                let room = self.host(id)?;
                self.idle(room)?;
                ensure!(difficulty < 3, "Invalid difficulty");
                let state = &self.rooms[&room];
                let mut spec = state.view.chart.clone().context("Choose a song first")?;
                if spec.difficulty == difficulty {
                    return Ok(());
                }
                spec.difficulty = difficulty;
                let practice = prepare_chart(
                    &spec,
                    state
                        .raw
                        .clone()
                        .context("Room chart bytes are unavailable")?,
                )?;
                let state = self.rooms.get_mut(&room).unwrap();
                state.view.chart = Some(spec);
                state.practice = Some(practice);
                self.clear_ready(room);
            }
            Command::Chat { room, text } => {
                ensure!(
                    self.players[&id].room == room
                        && (room.is_some() || self.private_key.is_none()),
                    "Chat channel changed"
                );
                let text = text.trim();
                ensure!(
                    !text.is_empty()
                        && text.len() <= MAX_CHAT_BYTES
                        && !text.chars().any(char::is_control),
                    "Chat messages must contain 1 to 400 visible bytes"
                );
                self.chat(room, Some(id), text.into(), false);
            }
            Command::Unavailable { spec, reason } => {
                let room = self.room_id(id)?;
                // Ignore a late report from a selection that the host already replaced.
                if self.rooms[&room].view.chart.as_ref() != Some(&spec) {
                    return Ok(());
                }
                ensure!(
                    !reason.is_empty()
                        && reason.len() <= MAX_CHAT_BYTES
                        && !reason.chars().any(char::is_control),
                    "Invalid content error"
                );
                ensure!(
                    self.rooms[&room].game.is_none(),
                    "Round is already in progress"
                );
                self.players.get_mut(&id).unwrap().ready = None;
                let name = self.players[&id].name.as_deref().unwrap();
                let mut text = format!("{name} cannot load {}: {reason}", spec.title);
                while text.len() > MAX_CHAT_BYTES {
                    text.pop();
                }
                self.chat(Some(room), None, text, true);
                self.dirty = true;
            }
            Command::Start => {
                let room = self.host(id)?;
                self.idle(room)?;
                let members = self
                    .players
                    .iter()
                    .filter(|(_, p)| p.room == Some(room))
                    .collect::<Vec<_>>();
                ensure!(members.len() >= 2, "At least two players are required");
                ensure!(
                    members.iter().all(|(_, p)| p.ready.is_some()),
                    "Every player must load and verify the same local chart first"
                );
                let practice = self.rooms[&room]
                    .practice
                    .as_ref()
                    .context("Choose a chart first")?;
                let players = members
                    .iter()
                    .map(|(id, p)| (**id, (p.ready.unwrap(), Referee::new(practice.clone()))))
                    .collect::<BTreeMap<_, _>>();
                self.next += 1;
                let round = self.next;
                let start_us = now + 3_000_000;
                let state = self.rooms.get_mut(&room).unwrap();
                state.view.round = Some(round);
                state.view.start_us = Some(start_us);
                state.game = Some(Match {
                    id: round,
                    start: start_us,
                    players,
                    last_report: 0,
                    settled_since: None,
                });
                let ids = members.iter().map(|(id, _)| **id).collect::<Vec<_>>();
                let offsets = members
                    .iter()
                    .map(|(id, p)| (**id, p.ready.unwrap()))
                    .collect::<BTreeMap<_, _>>();
                for id in ids {
                    let player = self.players.get_mut(&id).unwrap();
                    player.advance_walk(now);
                    player.movement_changed |= player.walking != [0, 0];
                    player.walking = [0, 0];
                    self.emit(
                        id,
                        Event::Started {
                            room,
                            round,
                            start_us,
                            offsets: offsets.clone(),
                        },
                    );
                }
                self.dirty = true;
            }
            Command::Input {
                round,
                sequence,
                song_time,
                lane,
                down,
            } => {
                let room = self.room_id(id)?;
                let game = self
                    .rooms
                    .get_mut(&room)
                    .and_then(|r| r.game.as_mut())
                    .context("No active round")?;
                ensure!(game.id == round, "Stale round input");
                let (offset, judge) = game
                    .players
                    .get_mut(&id)
                    .context("Not playing this round")?;
                let elapsed = (now as i128 - game.start as i128) as f64 / 1e6;
                let at = song_time.seconds();
                ensure!(
                    now as i128 >= game.start as i128 + (*offset).min(0) as i128
                        && at >= *offset as f64 / 1e6
                        && at <= elapsed + 0.15
                        && at >= elapsed - LATE_US as f64 / 1e6,
                    "Input timestamp is outside the allowed window"
                );
                judge.input(sequence, song_time.chart_time(*offset), lane, down)?;
                let recipients = game
                    .players
                    .keys()
                    .copied()
                    .filter(|id| self.players.get(id).is_some_and(|p| p.room == Some(room)))
                    .collect::<Vec<_>>();
                for recipient in recipients {
                    self.emit(
                        recipient,
                        Event::Input {
                            room,
                            round,
                            player: id,
                            sequence,
                            song_time,
                            lane,
                            down,
                        },
                    );
                }
            }
            Command::Forfeit { round } => {
                let room = self.room_id(id)?;
                if let Some(game) = self.rooms.get_mut(&room).and_then(|r| r.game.as_mut())
                    && game.id == round
                    && let Some((_, judge)) = game.players.get_mut(&id)
                {
                    judge.forfeited = true;
                }
            }
            Command::Hello { .. } => unreachable!(),
        }
        Ok(())
    }
    fn tick(&mut self, now: u64) {
        let last = *self.walk_tick.get_or_insert(now);
        if now.saturating_sub(last) >= 50_000 {
            self.walk_tick = Some(now);
            let mut updates: BTreeMap<RoomId, Vec<Walker>> = BTreeMap::new();
            for (&id, player) in &mut self.players {
                let Some(room) = player.room else { continue };
                player.advance_walk(now);
                if player.movement_changed {
                    player.movement_changed = false;
                    updates.entry(room).or_default().push(Walker {
                        id,
                        position: [player.x, player.y],
                        walking: player.walking,
                        facing: player.facing,
                    });
                }
            }
            for (room, walkers) in updates {
                let state = self.rooms.get_mut(&room).unwrap();
                for walker in &walkers {
                    for view in std::iter::once(&mut state.view).chain(&mut state.published) {
                        if let Some(member) = view.members.iter_mut().find(|m| m.id == walker.id) {
                            walker.apply(member, now);
                        }
                    }
                }
                for (&id, p) in &self.players {
                    if p.room == Some(room) {
                        self.out.push((
                            id,
                            Event::Movement {
                                room,
                                state_us: now,
                                walkers: walkers.clone(),
                            },
                        ));
                    }
                }
            }
        }
        let mut broadcasts = Vec::new();
        let mut finished = Vec::new();
        for (&room, state) in &mut self.rooms {
            if let Some(game) = &mut state.game {
                for (offset, judge) in game.players.values_mut() {
                    let elapsed = now as i128 - game.start as i128 - *offset as i128;
                    judge.advance(elapsed.max(0) as u64);
                }
                let all_forfeited = game.players.values().all(|(_, judge)| judge.forfeited);
                if game.players.values().all(|(_, judge)| judge.terminal()) {
                    game.settled_since.get_or_insert(now);
                } else {
                    game.settled_since = None;
                }
                let done = all_forfeited
                    || game
                        .settled_since
                        .is_some_and(|since| now.saturating_sub(since) >= LATE_US + 500_000)
                    || now
                        >= game.start
                            + state.view.chart.as_ref().unwrap().duration_us
                            + 5_000_000
                            + LATE_US
                            + 500_000;
                if done || now.saturating_sub(game.last_report) >= 100_000 {
                    game.last_report = now;
                    for (&id, (_, judge)) in &mut game.players {
                        if self.players.get(&id).is_some_and(|p| p.room == Some(room))
                            && let Some((revision, changes)) = judge.judgment_changes()
                        {
                            broadcasts.push((
                                id,
                                Event::Judgments {
                                    room,
                                    round: game.id,
                                    revision,
                                    changes,
                                },
                            ));
                        }
                    }
                    let standings = game.standings(done);
                    let event = Event::Scores {
                        room,
                        round: game.id,
                        standings,
                        final_result: done,
                        server_us: now,
                    };
                    for id in game
                        .players
                        .keys()
                        .filter(|id| self.players.get(id).is_some_and(|p| p.room == Some(room)))
                    {
                        broadcasts.push((*id, event.clone()));
                    }
                }
                if done {
                    finished.push(room);
                }
            }
        }
        self.out.extend(broadcasts);
        for room in finished {
            let state = self.rooms.get_mut(&room).unwrap();
            state.game = None;
            state.view.round = None;
            state.view.start_us = None;
            self.clear_ready(room);
        }
        if self.dirty {
            self.dirty = false;
            let mut directory = Vec::new();
            for (&id, state) in &mut self.rooms {
                state.view.members = self
                    .players
                    .iter()
                    .filter(|(_, p)| p.room == Some(id))
                    .map(|(&id, p)| Member {
                        state_us: state
                            .view
                            .members
                            .iter()
                            .find(|member| {
                                member.id == id
                                    && member.x == p.x
                                    && member.y == p.y
                                    && member.walking == p.walking
                                    && member.facing == p.facing
                            })
                            .map_or(now, |m| m.state_us),
                        x: p.x,
                        y: p.y,
                        facing: p.facing,
                        walking: p.walking,
                        slot: p.slot.unwrap(),
                        id,
                        name: p.name.clone().unwrap(),
                        ready: p.ready.is_some(),
                    })
                    .collect();
                directory.push(RoomSummary {
                    id,
                    name: state.view.name.clone(),
                    occupants: state.view.members.len(),
                    playing: state.game.is_some(),
                    password_protected: state.view.password_protected,
                });
                let changed = state.published.as_ref() != Some(&state.view);
                for member in &state.view.members {
                    let player = self.players.get_mut(&member.id).unwrap();
                    if changed || player.needs_room {
                        self.out.push((
                            member.id,
                            Event::RoomState(Some(Box::new(state.view.clone()))),
                        ));
                        player.needs_room = false;
                    }
                }
                if changed {
                    state.published = Some(state.view.clone());
                }
            }
            let changed = self.directory != directory;
            self.directory = directory;
            let roster: Vec<_> = self
                .players
                .iter()
                .filter_map(|(&id, p)| {
                    Some(OnlinePlayer {
                        id,
                        name: p.name.clone()?,
                        room: p.room,
                    })
                })
                .collect();
            let roster_changed = roster != self.roster;
            self.roster = roster;
            for (&id, player) in &mut self.players {
                if player.name.is_none() || player.room.is_some() {
                    continue;
                }
                if changed || player.needs_directory {
                    self.out
                        .push((id, Event::Directory(self.directory.clone())));
                }
                if self.private_key.is_none() && (roster_changed || player.needs_directory) {
                    self.out
                        .push((id, Event::OnlinePlayers(self.roster.clone())));
                }
                player.needs_directory = false;
            }
        }
    }
}
fn serve(mut listener: ServerListener, stop: Arc<AtomicBool>, mut hub: Hub) {
    let epoch = Instant::now();
    let mut peers = BTreeMap::<PlayerId, Peer>::new();
    let mut next = 0;
    let interval = Duration::from_millis(5);
    let mut tick = Instant::now();
    while !stop.load(Ordering::Acquire) {
        for _ in 0..8 {
            match listener.accept(next as usize + 1) {
                Ok(Some(wire)) => {
                    if peers.len() >= 32 {
                        continue;
                    }
                    {
                        next += 1;
                        let now = Instant::now();
                        peers.insert(
                            next,
                            Peer {
                                wire,
                                last: now,
                                connected: now,
                                tokens: 512,
                                refill: now,
                            },
                        );
                        hub.connect(next);
                    }
                }
                Ok(None) | Err(_) => break,
            }
        }
        let now = epoch.elapsed().as_micros() as u64;
        let mut dropped = Vec::new();
        for (&id, peer) in &mut peers {
            if peer.last.elapsed() > Duration::from_secs(15)
                || hub.players[&id].name.is_none()
                    && peer.connected.elapsed() > Duration::from_secs(5)
            {
                dropped.push(id);
                continue;
            }
            if peer.refill.elapsed() >= Duration::from_secs(1) {
                peer.tokens = 512;
                peer.refill = Instant::now();
            }
            match peer.wire.poll() {
                Ok(commands) => {
                    for command in commands {
                        if peer.tokens == 0 {
                            dropped.push(id);
                            break;
                        }
                        peer.tokens -= 1;
                        peer.last = Instant::now();
                        let input = matches!(command, Command::Input { .. });
                        if let Err(error) = hub.apply(id, command, now) {
                            if input || hub.players[&id].name.is_none() {
                                dropped.push(id);
                                break;
                            }
                            hub.emit(id, Event::Error(error.to_string()));
                        }
                    }
                }
                Err(_) => dropped.push(id),
            }
        }
        for id in dropped {
            peers.remove(&id);
            hub.disconnect(id);
        }
        if tick <= Instant::now() || hub.dirty {
            hub.tick(now);
            if tick <= Instant::now() {
                tick = Instant::now() + interval;
            }
        }
        let mut failed_writes = Vec::new();
        for (id, event) in hub.out.drain(..) {
            if let Some(peer) = peers.get_mut(&id)
                && peer.wire.send(&event).is_err()
            {
                failed_writes.push(id);
            }
        }
        for (&id, peer) in &mut peers {
            if peer.wire.flush().is_err() {
                failed_writes.push(id);
            }
        }
        for id in failed_writes {
            peers.remove(&id);
            hub.disconnect(id);
        }
        if let Err(e) = listener.wait(tick.saturating_duration_since(Instant::now()))
            && e.kind() != std::io::ErrorKind::Interrupted
        {
            eprintln!("Lobby listener failed: {e}");
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (ChartSpec, Vec<u8>) {
        let mut raw = vec![0; 300];
        raw[4..8].copy_from_slice(b"gn\0\0");
        raw[8..12].copy_from_slice(&2.9f32.to_le_bytes());
        raw[16..20].copy_from_slice(&120f32.to_le_bytes());
        raw.extend(0u32.to_le_bytes());
        raw.extend(2u16.to_le_bytes());
        raw.extend(4u16.to_le_bytes());
        for value in [0u32, 0, 1, 0] {
            raw.extend(value.to_le_bytes());
        }
        let end = raw.len() as u32;
        for (i, offset) in [300, end, end, end].into_iter().enumerate() {
            raw[284 + 4 * i..288 + 4 * i].copy_from_slice(&offset.to_le_bytes());
        }
        let spec = ChartSpec {
            pattern_sha256: sdo_song::chart::pattern_sha256(&raw),
            audio_sha256: "a".repeat(64),
            dance_sha256: None,
            title: "Fixture".into(),
            kind: "k".into(),
            difficulty: 0,
            judgment: "keyboard".into(),
            bpm: 120.,
            duration_us: 2_000_000,
        };
        (spec, raw)
    }
    #[test]
    fn placement_bonus_only_applies_to_settled_four_player_individual_results() {
        let chart = Practice::from_notes(
            (1..=10)
                .map(|i| sdo_gameplay::Note {
                    lane: sdo_gameplay::Lane::Left,
                    start: f64::from(i),
                    end: f64::from(i),
                })
                .collect(),
            11.,
            sdo_gameplay::judgment::find("keyboard").unwrap(),
        );
        let mut game = Match {
            id: 1,
            start: 0,
            players: BTreeMap::new(),
            last_report: 0,
            settled_since: None,
        };
        for cools in 0..4 {
            let mut run = chart.clone();
            for i in 1..=10 {
                let delay = if i <= cools { 0.08 } else { 0. };
                run.advance_to(f64::from(i) + delay, [false; 4], false);
                run.press(sdo_gameplay::Lane::Left);
            }
            game.players
                .insert(cools as PlayerId, (0, Referee::new(run)));
        }
        let points = |game: &Match, settled| {
            game.standings(settled)
                .into_iter()
                .map(|s| s.points)
                .collect::<Vec<_>>()
        };
        assert_eq!(points(&game, false), [680, 670, 660, 650]);
        assert_eq!(points(&game, true), [707, 690, 673, 656]);
        assert_eq!(points(&game, true), [707, 690, 673, 656]);
        game.players.get_mut(&0).unwrap().1.forfeit(11_000_000);
        assert_eq!(points(&game, true), [696, 679, 663, 680]);
        game.players.remove(&0);
        assert_eq!(points(&game, true), [670, 660, 650]);
        let tied = game.players[&1].1.practice().clone();
        game.players.insert(0, (0, Referee::new(tied)));
        assert_eq!(points(&game, true), [670, 670, 660, 650]);
    }

    fn hub() -> Result<Hub> {
        let mut hub = Hub::default();
        for id in 1..=3 {
            hub.connect(id);
            hub.apply(
                id,
                Command::Hello {
                    protocol: PROTOCOL,
                    name: format!("P{id}"),
                },
                0,
            )?;
        }
        hub.apply(
            1,
            Command::Create {
                password: String::new(),
                name: "Test".into(),
            },
            0,
        )?;
        hub.apply(
            2,
            Command::Join {
                room: 1,
                password: String::new(),
            },
            0,
        )?;
        hub.tick(0);
        hub.out.clear();
        Ok(hub)
    }
    #[test]
    fn appearance_changes_are_durable_and_invalidate_readiness() -> Result<()> {
        let mut hub = hub()?;
        for id in [1, 2] {
            hub.players.get_mut(&id).unwrap().ready = Some(0);
        }
        let mut appearance = Appearance {
            preset: "male".into(),
            equipment: BTreeMap::new(),
            sha256: "a".repeat(64),
        };
        hub.apply(
            1,
            Command::SetAppearance {
                appearance: Some(appearance.clone()),
            },
            100,
        )?;
        assert!(hub.players[&1].ready.is_none() && hub.players[&2].ready.is_none());
        appearance.sha256 = "b".repeat(64);
        hub.apply(
            1,
            Command::SetAppearance {
                appearance: Some(appearance.clone()),
            },
            101,
        )?;
        assert_eq!(hub.players[&1].appearance.as_ref(), Some(&appearance));
        Ok(())
    }

    fn hello(hub: &mut Hub, id: PlayerId) -> Result<()> {
        hub.connect(id);
        hub.apply(
            id,
            Command::Hello {
                protocol: PROTOCOL,
                name: format!("P{id}"),
            },
            0,
        )
    }
    #[test]
    fn protected_rooms_check_passwords_without_exposing_credentials() -> Result<()> {
        let mut hub = hub()?;
        let password = " Room 密碼 ";
        hub.apply(
            3,
            Command::Create {
                name: "Locked".into(),
                password: password.into(),
            },
            0,
        )?;
        assert!(hub.rooms[&2].view.password_protected);
        hub.apply(1, Command::Leave, 0)?;
        for wrong in ["", "wrong", "Room 密碼"] {
            assert!(
                hub.apply(
                    1,
                    Command::Join {
                        room: 2,
                        password: wrong.into()
                    },
                    0
                )
                .is_err()
            );
            assert!(hub.players[&1].room.is_none());
            assert!(hub.players[&1].slot.is_none());
        }
        hub.apply(
            1,
            Command::Join {
                room: 2,
                password: password.into(),
            },
            0,
        )?;
        assert_eq!(hub.players[&1].room, Some(2));
        assert_eq!(hub.players[&1].slot, Some(1));
        hub.tick(1);
        let snapshot = serde_json::to_string(
            &hub.rooms
                .values()
                .map(|room| room.view.clone())
                .collect::<Vec<_>>(),
        )?;
        assert!(!snapshot.contains(password));
        assert!(!snapshot.contains("digest") && !snapshot.contains("salt"));
        assert_eq!(
            hub.rooms.len(),
            2,
            "Public servers still allow multiple rooms"
        );
        assert_eq!(hub.rooms[&1].view.host, 2);
        assert_eq!(hub.rooms[&2].view.host, 3);
        Ok(())
    }
    #[test]
    fn private_servers_reserve_one_room_for_owner_and_auto_join_connections() -> Result<()> {
        let mut hub = Hub {
            private_key: Some("owner-key".into()),
            ..Default::default()
        };
        hello(&mut hub, 1)?;
        hello(&mut hub, 2)?;
        assert!(
            hub.apply(
                2,
                Command::Create {
                    name: "Hijack".into(),
                    password: String::new()
                },
                0
            )
            .is_err()
        );
        assert!(
            hub.apply(
                2,
                Command::OpenPrivateRoom {
                    name: "Hijack".into(),
                    password: String::new(),
                    key: "wrong".into(),
                },
                0
            )
            .is_err()
        );
        assert!(hub.rooms.is_empty());
        hub.apply(
            1,
            Command::OpenPrivateRoom {
                name: "Private".into(),
                password: String::new(),
                key: "owner-key".into(),
            },
            0,
        )?;
        assert_eq!(hub.players[&1].room, Some(1));
        assert_eq!(
            hub.players[&2].room,
            Some(1),
            "Already connected guests join automatically"
        );
        assert!(
            hub.apply(
                1,
                Command::OpenPrivateRoom {
                    name: "Second".into(),
                    password: String::new(),
                    key: "owner-key".into(),
                },
                0
            )
            .is_err()
        );
        assert!(hub.apply(2, Command::Leave, 0).is_err());
        hub.leave(1);
        assert_eq!(hub.rooms[&1].view.host, 2);
        hub.leave(2);
        assert_eq!(hub.rooms.len(), 1, "An empty private room is retained");
        assert_eq!(hub.rooms[&1].view.host, 0);
        hello(&mut hub, 3)?;
        assert_eq!(hub.players[&3].room, Some(1));
        assert_eq!(hub.players[&3].slot, Some(0));
        assert_eq!(hub.rooms[&1].view.host, 3);
        Ok(())
    }
    #[test]
    fn private_passwords_require_admission_and_cannot_be_bypassed() -> Result<()> {
        let mut hub = Hub {
            private_key: Some("key".into()),
            ..Default::default()
        };
        hello(&mut hub, 1)?;
        hello(&mut hub, 2)?;
        hub.apply(
            1,
            Command::OpenPrivateRoom {
                name: "Locked private".into(),
                password: "secret".into(),
                key: "key".into(),
            },
            0,
        )?;
        assert!(
            hub.apply(
                2,
                Command::Chat {
                    room: None,
                    text: "waiting".into()
                },
                0
            )
            .is_err()
        );
        hub.tick(0);
        assert!(hub.out.iter().all(|(id, event)| *id != 2
            || !matches!(
                event,
                Event::ChatHistory { .. } | Event::Chat { .. } | Event::OnlinePlayers(_)
            )));
        assert!(hub.players[&2].room.is_none());
        assert!(hub.out.iter().any(|(id, event)| {
            *id == 2 && matches!(event, Event::RoomPasswordRequired { room: 1, .. })
        }));
        assert!(
            hub.apply(
                2,
                Command::Join {
                    room: 1,
                    password: String::new()
                },
                0
            )
            .is_err()
        );
        assert!(
            hub.apply(
                2,
                Command::Join {
                    room: 1,
                    password: "wrong".into()
                },
                0
            )
            .is_err()
        );
        hub.apply(
            2,
            Command::Join {
                room: 1,
                password: "secret".into(),
            },
            0,
        )?;
        assert_eq!(hub.players[&2].room, Some(1));
        hello(&mut hub, 3)?;
        assert!(
            hub.players[&3].room.is_none(),
            "Later connections also require the password"
        );
        Ok(())
    }
    #[test]
    fn dedicated_host_transfer_changes_only_room_control_and_checks_authority() -> Result<()> {
        let mut hub = hub()?;
        assert!(
            hub.apply(2, Command::TransferHost { player: 1 }, 0)
                .is_err()
        );
        assert!(
            hub.apply(1, Command::TransferHost { player: 3 }, 0)
                .is_err()
        );
        hub.players.get_mut(&1).unwrap().ready = Some(0);
        hub.players.get_mut(&2).unwrap().ready = Some(0);
        hub.apply(1, Command::TransferHost { player: 2 }, 0)?;
        assert_eq!(hub.rooms[&1].view.host, 2);
        assert_eq!(hub.rooms.len(), 1);
        assert!(hub.players[&1].ready.is_none());
        assert!(hub.players[&2].ready.is_some());
        assert!(
            hub.apply(1, Command::TransferHost { player: 1 }, 0)
                .is_err()
        );
        hub.apply(2, Command::TransferHost { player: 1 }, 0)?;
        assert_eq!(hub.rooms[&1].view.host, 1);
        hub.private_key = Some("GUI owner".into());
        assert!(
            hub.apply(1, Command::TransferHost { player: 2 }, 0)
                .is_err()
        );
        assert_eq!(hub.rooms[&1].view.host, 1);
        Ok(())
    }
    #[test]
    fn room_levels_are_broadcast_from_verified_gn_bytes() -> Result<()> {
        let mut hub = hub()?;
        let (mut spec, mut raw) = fixture();
        raw[20..22].copy_from_slice(&7u16.to_le_bytes());
        spec.pattern_sha256 = sdo_song::chart::pattern_sha256(&raw);
        hub.apply(1, Command::Offer { spec, raw }, 1)?;
        assert_eq!(hub.rooms[&1].view.keys, 4);
        assert_eq!(hub.rooms[&1].view.levels[0], Some(7));
        hub.tick(2);
        assert!(hub.out.iter().any(|(_, event)| matches!(
            event, Event::RoomState(Some(room)) if room.keys == 4 && room.levels[0] == Some(7)
        )));
        Ok(())
    }
    #[test]
    fn private_auto_join_respects_eight_slots() -> Result<()> {
        let mut hub = Hub {
            private_key: Some("key".into()),
            ..Default::default()
        };
        hello(&mut hub, 1)?;
        hub.apply(
            1,
            Command::OpenPrivateRoom {
                name: "Private".into(),
                password: String::new(),
                key: "key".into(),
            },
            0,
        )?;
        for id in 2..=9 {
            hello(&mut hub, id)?;
        }
        assert_eq!(
            hub.players
                .values()
                .filter(|player| player.room == Some(1))
                .count(),
            8
        );
        assert!(hub.players[&9].room.is_none());
        assert!(hub.out.iter().any(|(id, event)| {
            *id == 9 && matches!(event, Event::Error(message) if message == "Room is full")
        }));
        Ok(())
    }
    #[test]
    fn detailed_state_and_walking_stay_in_the_joined_room() -> Result<()> {
        let mut hub = hub()?;
        for id in [10, 11] {
            hello(&mut hub, id)?;
        }
        let other = hub.create_room(10, "Other", "")?;
        hub.tick(1);
        assert!(hub.out.iter().all(|(id, event)| match event {
            Event::RoomState(Some(r)) => hub.players[id].room == Some(r.id),
            Event::Directory(_) => hub.players[id].room.is_none(),
            _ => true,
        }));
        assert!(
            hub.out
                .iter()
                .any(|(id, e)| *id == 11 && matches!(e,Event::Directory(rooms) if rooms.len()==2))
        );
        hub.out.clear();
        hub.apply(1, Command::Walk { direction: [-1, 0] }, 2)?;
        hub.tick(50_001);
        assert!(!hub.out.is_empty());
        assert!(hub.out.iter().all(|(id,e)|[1,2].contains(id) && matches!(e,Event::Movement{room:1,walkers,..} if walkers.len()==1 && walkers[0].id==1)));
        hub.out.clear();
        hub.leave(2);
        hub.join_room(2, other, "")?;
        hub.tick(60_000);
        hub.out.clear();
        hub.apply(1, Command::Walk { direction: [0, 0] }, 61_000)?;
        hub.tick(100_002);
        assert!(hub.out.iter().all(|(id, _)| *id == 1));
        assert!(
            hub.out
                .iter()
                .any(|(_, e)| matches!(e,Event::Movement{walkers,..} if walkers[0].walking==[0,0]))
        );
        Ok(())
    }
    #[test]
    fn round_events_stop_when_a_participant_changes_rooms() -> Result<()> {
        let (spec, raw) = fixture();
        let mut hub = hub()?;
        let other = hub.create_room(3, "Other", "")?;
        hub.apply(
            1,
            Command::Offer {
                spec: spec.clone(),
                raw,
            },
            1,
        )?;
        for id in [1, 2] {
            hub.apply(
                id,
                Command::Ready {
                    spec: spec.clone(),
                    offset_us: 0,
                },
                2,
            )?;
        }
        hub.apply(1, Command::Start, 3)?;
        let game = hub.rooms[&1].game.as_ref().unwrap();
        let (round, start) = (game.id, game.start);
        hub.leave(2);
        hub.join_room(2, other, "")?;
        hub.tick(start);
        hub.out.clear();
        hub.apply(
            1,
            Command::Input {
                round,
                sequence: 1,
                song_time: SongTime::new(1.).unwrap(),
                lane: 0,
                down: true,
            },
            start + 1_000_000,
        )?;
        hub.tick(start + 1_200_000);
        hub.tick(start + 12_000_000);
        assert!(hub.out.iter().any(|(_, e)| matches!(
            e,
            Event::Scores {
                final_result: true,
                ..
            }
        )));
        assert!(
            hub.out
                .iter()
                .any(|(_, e)| matches!(e, Event::Judgments { .. }))
        );
        assert!(
            hub.out
                .iter()
                .any(|(_, e)| matches!(e, Event::Input { .. }))
        );
        for (id, event) in hub.out {
            if matches!(
                event,
                Event::Input { .. }
                    | Event::Judgments { .. }
                    | Event::Scores { .. }
                    | Event::Aborted { .. }
            ) {
                assert_eq!(id, 1);
            }
        }
        Ok(())
    }
    #[test]
    fn room_walking_is_authoritative_clamped_and_stops() -> Result<()> {
        let mut hub = hub()?;
        hub.tick(0);
        let before = hub.players[&1].x;
        hub.apply(1, Command::Walk { direction: [1, 0] }, 1)?;
        hub.tick(100_000);
        assert!(hub.players[&1].x > before);
        assert!(
            hub.rooms[&1]
                .view
                .members
                .iter()
                .any(|m| m.id == 1 && m.walking == [1, 0])
        );
        hub.apply(1, Command::Walk { direction: [0, 0] }, 100_001)?;
        let stopped = hub.players[&1].x;
        hub.tick(200_000);
        assert_eq!(hub.players[&1].x, stopped);
        hub.apply(1, Command::Walk { direction: [1, 0] }, 200_001)?;
        for step in 3..=300 {
            hub.tick(step * 100_000);
        }
        assert_eq!(hub.players[&1].x, 0.94);
        assert!(
            hub.apply(1, Command::Walk { direction: [2, 0] }, 30_000_001)
                .is_err()
        );
        let before_y = hub.players[&1].y;
        hub.apply(1, Command::Walk { direction: [0, -1] }, 30_000_002)?;
        hub.tick(30_100_000);
        assert!(hub.players[&1].y < before_y);
        hub.apply(1, Command::Walk { direction: [0, 0] }, 30_100_001)?;
        assert_eq!(
            hub.players[&1].facing,
            [0, -1],
            "Stopping keeps the last facing"
        );
        let before = [hub.players[&1].x, hub.players[&1].y];
        hub.apply(1, Command::Walk { direction: [-1, 1] }, 30_100_002)?;
        hub.tick(30_200_000);
        let after = [hub.players[&1].x, hub.players[&1].y];
        let distance = ((after[0] - before[0]).powi(2) + (after[1] - before[1]).powi(2)).sqrt();
        assert!(
            (distance - 0.028).abs() < 0.001,
            "Diagonal movement must have the same speed"
        );
        for step in 303..=700 {
            hub.tick(step * 100_000);
        }
        assert_eq!(hub.players[&1].x, 0.06);
        assert_eq!(hub.players[&1].y, 0.94);
        hub.apply(1, Command::Walk { direction: [0, -1] }, 70_000_001)?;
        for step in 701..=1100 {
            hub.tick(step * 100_000);
        }
        assert_eq!(hub.players[&1].y, 0.12);
        hub.leave(1);
        assert_eq!(hub.players[&1].walking, [0, 0]);
        Ok(())
    }
    #[test]
    fn eight_slots_remain_stable_and_appearances_follow_members() -> Result<()> {
        let mut hub = hub()?;
        for id in 4..=9 {
            hub.connect(id);
            hub.apply(
                id,
                Command::Hello {
                    protocol: PROTOCOL,
                    name: format!("P{id}"),
                },
                0,
            )?;
        }
        for id in 3..=8 {
            hub.apply(
                id,
                Command::Join {
                    room: 1,
                    password: String::new(),
                },
                1,
            )?;
        }
        assert!(
            hub.apply(
                9,
                Command::Join {
                    room: 1,
                    password: String::new()
                },
                2
            )
            .is_err()
        );
        assert!(hub.apply(1, Command::TakeSlot { slot: 1 }, 2).is_err());
        assert!(hub.apply(1, Command::TakeSlot { slot: 8 }, 2).is_err());
        hub.out.clear();
        let appearance = Appearance {
            preset: "male".into(),
            equipment: BTreeMap::from([("hair".into(), "001234".into())]),
            sha256: "a".repeat(64),
        };
        hub.apply(
            1,
            Command::SetAppearance {
                appearance: Some(appearance.clone()),
            },
            3,
        )?;
        assert_eq!(
            hub.out
                .iter()
                .filter(|(_, event)| matches!(
                    event,
                    Event::Appearance {
                        appearance: Some(_),
                        ..
                    }
                ))
                .count(),
            8
        );
        assert!(hub.out.iter().all(|(id, _)| *id != 9));
        hub.leave(3);
        hub.apply(2, Command::TakeSlot { slot: 2 }, 4)?;
        hub.out.clear();
        hub.apply(
            9,
            Command::Join {
                room: 1,
                password: String::new(),
            },
            5,
        )?;
        assert_eq!(hub.players[&9].slot, Some(1));
        assert_eq!(hub.players[&4].slot, Some(3));
        assert_eq!(hub.players[&1].slot, Some(0));
        assert!(hub.out.iter().any(|(id,event)|matches!(event,Event::Appearance{player:1,appearance:Some(data),..} if *id==9 && data==&appearance)));
        hub.tick(6);
        assert_eq!(
            hub.rooms[&1]
                .view
                .members
                .iter()
                .map(|m| m.slot)
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            8
        );
        let mut invalid = appearance;
        invalid.sha256.pop();
        assert!(
            hub.apply(
                1,
                Command::SetAppearance {
                    appearance: Some(invalid)
                },
                7
            )
            .is_err()
        );
        Ok(())
    }
    #[test]
    fn only_host_selects_difficulty_and_changes_invalidate_every_ready() -> Result<()> {
        let (mut spec, mut raw) = fixture();
        let events = raw[300..].to_vec();
        raw.extend(&events);
        raw.extend(&events);
        for i in 0..4 {
            raw[284 + 4 * i..288 + 4 * i]
                .copy_from_slice(&((300 + i * events.len()) as u32).to_le_bytes());
        }
        spec.pattern_sha256 = sdo_song::chart::pattern_sha256(&raw);
        let mut hub = hub()?;
        hub.apply(
            1,
            Command::Offer {
                spec: spec.clone(),
                raw: raw.clone(),
            },
            1,
        )?;
        for id in [1, 2] {
            hub.apply(
                id,
                Command::Ready {
                    spec: spec.clone(),
                    offset_us: 0,
                },
                2,
            )?;
        }
        assert!(
            hub.apply(
                2,
                Command::Offer {
                    spec: spec.clone(),
                    raw
                },
                3
            )
            .is_err()
        );
        assert!(
            hub.apply(2, Command::SetDifficulty { difficulty: 2 }, 3)
                .is_err()
        );
        assert!(hub.apply(2, Command::Start, 3).is_err());
        hub.apply(1, Command::SetDifficulty { difficulty: 2 }, 4)?;
        let selected = hub.rooms[&1].view.chart.as_ref().unwrap();
        assert_eq!(selected.difficulty, 2);
        assert_eq!(selected.pattern_sha256, spec.pattern_sha256);
        assert!(hub.players[&1].ready.is_none() && hub.players[&2].ready.is_none());
        assert!(
            hub.apply(
                2,
                Command::Ready {
                    spec: spec.clone(),
                    offset_us: 0
                },
                5
            )
            .is_err()
        );
        assert!(hub.apply(1, Command::Start, 5).is_err());
        spec.difficulty = 2;
        for id in [1, 2] {
            hub.apply(
                id,
                Command::Ready {
                    spec: spec.clone(),
                    offset_us: 0,
                },
                6,
            )?;
        }
        hub.apply(1, Command::Start, 7)?;
        assert!(
            hub.apply(1, Command::SetDifficulty { difficulty: 0 }, 8)
                .is_err()
        );
        Ok(())
    }
    #[test]
    fn missing_song_errors_are_room_chat_messages_and_block_start() -> Result<()> {
        let (spec, raw) = fixture();
        let mut hub = hub()?;
        hub.apply(
            1,
            Command::Offer {
                spec: spec.clone(),
                raw,
            },
            1,
        )?;
        for id in [1, 2] {
            hub.apply(
                id,
                Command::Ready {
                    spec: spec.clone(),
                    offset_us: 0,
                },
                2,
            )?;
        }
        hub.out.clear();
        hub.apply(
            2,
            Command::Unavailable {
                spec: spec.clone(),
                reason: "Song is missing locally".into(),
            },
            3,
        )?;
        assert!(hub.players[&2].ready.is_none());
        assert!(hub.apply(1, Command::Start, 4).is_err());
        let messages = hub
            .out
            .iter()
            .filter_map(|(id, event)| {
                if let Event::Chat { room, message } = event {
                    Some((*id, *room, message))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        assert_eq!(messages.len(), 2);
        assert!(
            messages
                .iter()
                .all(|(id, room, message)| [1, 2].contains(id)
                    && *room == Some(1)
                    && message.error
                    && message.player.is_none()
                    && message.text.contains("P2")
                    && message.text.contains("Song is missing"))
        );
        let before = hub.rooms[&1].chat.len();
        let mut stale = spec;
        stale.difficulty = 2;
        hub.apply(
            2,
            Command::Unavailable {
                spec: stale,
                reason: "Old selection".into(),
            },
            5,
        )?;
        assert_eq!(hub.rooms[&1].chat.len(), before);
        Ok(())
    }
    #[test]
    fn lobby_roster_and_chat_follow_membership() -> Result<()> {
        let mut hub = hub()?; // P1 and P2 share room 1; P3 is in the lobby.
        hello(&mut hub, 4)?;
        hello(&mut hub, 5)?;
        hub.create_room(5, "Second room", "")?;
        hub.connect(6); // No handshake.
        hub.tick(1);
        let recipients = |hub: &Hub| {
            hub.out
                .iter()
                .filter_map(|(id, event)| matches!(event, Event::OnlinePlayers(_)).then_some(*id))
                .collect::<Vec<_>>()
        };
        assert_eq!(recipients(&hub), [3, 4]);
        assert_eq!(
            hub.roster
                .iter()
                .map(|p| (p.id, p.room))
                .collect::<Vec<_>>(),
            [
                (1, Some(1)),
                (2, Some(1)),
                (3, None),
                (4, None),
                (5, Some(2))
            ]
        );
        hub.out.clear();
        let chat = |room, text: &str| Command::Chat {
            room,
            text: text.into(),
        };
        for (sender, scope, expected) in [
            (3, None, &[3, 4][..]),
            (2, Some(1), &[1, 2]),
            (5, Some(2), &[5]),
        ] {
            hub.apply(sender, chat(scope, "hello"), 2)?;
            let recipients = hub
                .out
                .drain(..)
                .map(|(id, event)| {
                    assert!(matches!(event,Event::Chat{room,..} if room==scope));
                    id
                })
                .collect::<Vec<_>>();
            assert_eq!(recipients, expected);
        }
        assert!(hub.apply(6, chat(None, "before hello"), 2).is_err());
        for i in 0..=MAX_CHAT_MESSAGES {
            hub.apply(3, chat(None, &format!("lobby {i}")), 3)?;
        }
        hub.out.clear();
        hub.join_room(3, 2, "")?;
        assert!(hub.apply(3, chat(None, "old lobby draft"), 4).is_err());
        assert!(hub.apply(3, chat(Some(1), "another room"), 4).is_err());
        hub.tick(4);
        assert_eq!(recipients(&hub), [4]);
        hub.out.clear();
        hub.apply(3, Command::Leave, 5)?;
        assert!(hub.apply(3, chat(Some(2), "old room draft"), 5).is_err());
        assert!(hub.out.iter().any(|(id,event)|*id==3 && matches!(event, Event::ChatHistory{room:None,messages}
            if messages.len()==MAX_CHAT_MESSAGES && messages[0].text=="lobby 1" && messages.last().unwrap().text==format!("lobby {MAX_CHAT_MESSAGES}"))));
        hub.tick(5);
        hub.out.clear();
        hub.disconnect(4);
        hub.tick(6);
        assert_eq!(recipients(&hub), [3]);
        assert!(!hub.roster.iter().any(|p| p.id == 4));
        hub.out.clear();
        hub.tick(7);
        assert!(recipients(&hub).is_empty());
        Ok(())
    }

    #[test]
    fn room_chat_is_scoped_bounded_and_restored_on_join() -> Result<()> {
        let mut hub = hub()?;
        assert!(
            hub.apply(
                3,
                Command::Chat {
                    room: Some(1),
                    text: "Outside room".into()
                },
                1
            )
            .is_err()
        );
        hub.out.clear();
        hub.apply(
            2,
            Command::Chat {
                room: Some(1),
                text: "Hello host".into(),
            },
            2,
        )?;
        assert!(hub.out.iter().all(|(id, _)| [1, 2].contains(id)));
        let message = hub.rooms[&1].chat.back().unwrap();
        assert_eq!(message.name, "P2");
        assert_eq!(message.player, Some(2));
        assert!(!message.error);
        assert!(
            hub.apply(
                2,
                Command::Chat {
                    room: Some(1),
                    text: "x".repeat(MAX_CHAT_BYTES + 1)
                },
                3
            )
            .is_err()
        );
        for i in 0..MAX_CHAT_MESSAGES + 1 {
            hub.apply(
                1,
                Command::Chat {
                    room: Some(1),
                    text: format!("Message {i}"),
                },
                4,
            )?;
        }
        assert_eq!(hub.rooms[&1].chat.len(), MAX_CHAT_MESSAGES);
        hub.out.clear();
        hub.apply(
            3,
            Command::Join {
                room: 1,
                password: String::new(),
            },
            5,
        )?;
        let history = hub
            .out
            .iter()
            .find_map(|(id, event)| {
                if let Event::ChatHistory {
                    room: Some(1),
                    messages,
                } = event
                {
                    assert_eq!(*id, 3);
                    Some(messages)
                } else {
                    None
                }
            })
            .unwrap();
        assert_eq!(history.len(), MAX_CHAT_MESSAGES);
        assert_eq!(
            history.last().unwrap().text,
            format!("Message {}", MAX_CHAT_MESSAGES)
        );
        Ok(())
    }
    #[test]
    fn exact_file_and_rules_are_required_before_every_start() -> Result<()> {
        let (spec, raw) = fixture();
        let mut hub = hub()?;
        assert!(hub.apply(1, Command::Start, 0).is_err());
        let mut corrupt = raw.clone();
        corrupt[100] ^= 1;
        assert!(
            hub.apply(
                1,
                Command::Offer {
                    spec: spec.clone(),
                    raw: corrupt
                },
                1
            )
            .is_err()
        );
        hub.apply(
            1,
            Command::Offer {
                spec: spec.clone(),
                raw,
            },
            1,
        )?;
        hub.apply(
            1,
            Command::Ready {
                spec: spec.clone(),
                offset_us: 0,
            },
            2,
        )?;
        for mismatch in 0..6 {
            let mut wrong = spec.clone();
            match mismatch {
                0 => wrong.pattern_sha256 = "b".repeat(64),
                1 => wrong.audio_sha256 = "b".repeat(64),
                2 => wrong.bpm = 121.,
                3 => wrong.difficulty = 1,
                4 => wrong.judgment = "pad".into(),
                _ => wrong.kind = "t".into(),
            };
            assert!(
                hub.apply(
                    2,
                    Command::Ready {
                        spec: wrong,
                        offset_us: 0
                    },
                    2
                )
                .is_err()
            );
            assert!(hub.apply(1, Command::Start, 2).is_err());
        }
        assert!(
            hub.apply(
                2,
                Command::Ready {
                    spec: spec.clone(),
                    offset_us: i64::MIN
                },
                2
            )
            .is_err()
        );
        hub.apply(2, Command::Ready { spec, offset_us: 0 }, 3)?;
        assert!(hub.apply(2, Command::Start, 3).is_err());
        hub.apply(1, Command::Start, 4)?;
        assert!(
            hub.apply(
                3,
                Command::Join {
                    room: 1,
                    password: String::new()
                },
                5
            )
            .is_err()
        );
        assert!(
            hub.apply(
                1,
                Command::Input {
                    round: 99,
                    sequence: 1,
                    song_time: SongTime::new(1.).unwrap(),
                    lane: 0,
                    down: true
                },
                4_000_004
            )
            .is_err()
        );
        assert!(
            hub.apply(
                1,
                Command::Input {
                    round: 2,
                    sequence: 1,
                    song_time: SongTime::new(1.).unwrap(),
                    lane: 0,
                    down: true
                },
                4
            )
            .is_err()
        );
        hub.tick(4_600_004); // Both players have a speculative miss.
        hub.apply(
            1,
            Command::Input {
                round: 2,
                sequence: 1,
                song_time: SongTime::new(1.).unwrap(),
                lane: 0,
                down: true,
            },
            4_600_004,
        )?;
        hub.tick(13_000_000);
        let final_scores = hub
            .out
            .iter()
            .find_map(|(_, event)| {
                if let Event::Scores {
                    standings,
                    final_result: true,
                    ..
                } = event
                {
                    Some(standings)
                } else {
                    None
                }
            })
            .unwrap();
        assert_eq!(final_scores[0].player, 1);
        assert!(final_scores[0].points > 0);
        assert_eq!(final_scores[1].points, 0);
        assert!(hub.players[&1].ready.is_none());
        assert!(hub.players[&2].ready.is_none());
        assert!(hub.apply(1, Command::Start, 12_000_000).is_err());
        Ok(())
    }
    #[test]
    fn changing_chart_invalidates_ready_and_leaving_transfers_host() -> Result<()> {
        let (spec, raw) = fixture();
        let mut hub = hub()?;
        hub.apply(
            1,
            Command::Offer {
                spec: spec.clone(),
                raw: raw.clone(),
            },
            1,
        )?;
        hub.apply(
            2,
            Command::Ready {
                spec: spec.clone(),
                offset_us: 0,
            },
            2,
        )?;
        let mut changed = spec.clone();
        changed.judgment = "pad".into();
        hub.apply(1, Command::Offer { spec: changed, raw }, 1_000_002)?;
        assert!(hub.players[&2].ready.is_none());
        assert!(
            hub.apply(2, Command::Ready { spec, offset_us: 0 }, 1_000_003)
                .is_err()
        );
        hub.leave(1);
        assert_eq!(hub.rooms[&1].view.host, 2);
        hub.leave(2);
        assert!(hub.rooms.is_empty());
        Ok(())
    }
    #[test]
    fn unready_aborts_countdown_for_everyone() -> Result<()> {
        let (spec, raw) = fixture();
        let mut hub = hub()?;
        hub.apply(
            1,
            Command::Offer {
                spec: spec.clone(),
                raw,
            },
            1,
        )?;
        for id in [1, 2] {
            hub.apply(
                id,
                Command::Ready {
                    spec: spec.clone(),
                    offset_us: 0,
                },
                2,
            )?;
        }
        hub.apply(1, Command::Start, 3)?;
        hub.apply(2, Command::Unready, 4)?;
        assert!(hub.rooms[&1].game.is_none());
        assert_eq!(
            hub.out
                .iter()
                .filter(|(_, e)| matches!(e, Event::Aborted { .. }))
                .count(),
            2
        );
        Ok(())
    }
    #[test]
    fn unready_after_start_only_forfeits_that_player_and_all_forfeits_end_the_round() -> Result<()>
    {
        let (spec, raw) = fixture();
        let mut hub = hub()?;
        hub.apply(
            1,
            Command::Offer {
                spec: spec.clone(),
                raw,
            },
            1,
        )?;
        for id in [1, 2] {
            hub.apply(
                id,
                Command::Ready {
                    spec: spec.clone(),
                    offset_us: 0,
                },
                2,
            )?;
        }
        hub.apply(1, Command::Start, 3)?;
        hub.apply(2, Command::Unready, 3_000_004)?;
        assert!(hub.rooms[&1].game.is_some());
        assert!(hub.rooms[&1].game.as_ref().unwrap().players[&2].1.forfeited);
        assert!(!hub.rooms[&1].game.as_ref().unwrap().players[&1].1.forfeited);
        hub.apply(1, Command::Forfeit { round: 2 }, 3_000_005)?;
        hub.tick(3_000_006);
        assert!(hub.rooms[&1].game.is_none());
        assert!(hub.out.iter().any(|(_, e)| matches!(
            e,
            Event::Scores {
                final_result: true,
                ..
            }
        )));
        Ok(())
    }
    #[test]
    fn calibration_accepts_negative_preroll_and_positive_audio_delay() -> Result<()> {
        let (spec, raw) = fixture();
        let mut hub = hub()?;
        hub.apply(
            1,
            Command::Offer {
                spec: spec.clone(),
                raw,
            },
            1,
        )?;
        hub.apply(
            1,
            Command::Ready {
                spec: spec.clone(),
                offset_us: -100_000,
            },
            2,
        )?;
        hub.apply(
            2,
            Command::Ready {
                spec,
                offset_us: 100_000,
            },
            2,
        )?;
        hub.apply(1, Command::Start, 3)?;
        assert!(
            hub.apply(
                1,
                Command::Input {
                    round: 2,
                    sequence: 1,
                    song_time: SongTime::new(-0.1).unwrap(),
                    lane: 0,
                    down: true
                },
                2_800_003
            )
            .is_err()
        );
        hub.apply(
            1,
            Command::Input {
                round: 2,
                sequence: 1,
                song_time: SongTime::new(-0.1).unwrap(),
                lane: 0,
                down: true,
            },
            2_900_003,
        )?;
        hub.apply(
            2,
            Command::Input {
                round: 2,
                sequence: 1,
                song_time: SongTime::new(0.1).unwrap(),
                lane: 0,
                down: true,
            },
            3_100_003,
        )?;
        Ok(())
    }
    #[test]
    fn two_clients_play_a_complete_round_over_loopback() -> Result<()> {
        use sdo_platform::net::{Client, ClientEvent};
        let server = Server::bind("127.0.0.1:0")?;
        let mut a = Client::connect(server.address.to_string(), "Alice".into())?;
        let mut b = Client::connect(server.address.to_string(), "Bob".into())?;
        fn until(client: &mut Client, mut predicate: impl FnMut(&Event) -> bool) -> Result<Event> {
            let began = Instant::now();
            loop {
                for event in client.poll() {
                    match event {
                        ClientEvent::Message(e) => {
                            if let Event::Error(error) = &e {
                                anyhow::bail!("{error}");
                            }
                            if predicate(&e) {
                                return Ok(e);
                            }
                        }
                        ClientEvent::Disconnected(e) => anyhow::bail!("{e}"),
                    }
                }
                ensure!(
                    began.elapsed() < Duration::from_secs(12),
                    "Timed out waiting for server"
                );
                thread::sleep(Duration::from_millis(5));
            }
        }
        until(&mut a, |e| matches!(e, Event::Welcome { .. }))?;
        until(&mut b, |e| matches!(e, Event::Welcome { .. }))?;
        a.send(Command::Create {
            password: String::new(),
            name: "Loopback".into(),
        })?;
        until(&mut a, |e| matches!(e, Event::RoomState(Some(_))))?;
        b.send(Command::Join {
            room: 1,
            password: String::new(),
        })?;
        until(
            &mut b,
            |e| matches!(e,Event::RoomState(Some(room)) if room.members.len()==2),
        )?;
        a.send(Command::SetAppearance {
            appearance: Some(Appearance {
                preset: "male".into(),
                equipment: BTreeMap::new(),
                sha256: "a".repeat(64),
            }),
        })?;
        until(
            &mut b,
            |event| matches!(event,Event::Appearance{appearance:Some(appearance),..} if appearance.preset=="male" && appearance.sha256=="a".repeat(64)),
        )?;
        thread::sleep(Duration::from_millis(80));
        a.send(Command::Walk { direction: [1, -1] })?;
        a.send(Command::SetAppearance {
            appearance: Some(Appearance {
                preset: "male".into(),
                equipment: BTreeMap::from([("hair".into(), "5678".into())]),
                sha256: "b".repeat(64),
            }),
        })?;
        until(
            &mut b,
            |event| matches!(event, Event::Appearance { appearance: Some(p), .. } if p.equipment.get("hair").is_some_and(|v|v=="5678")),
        )?;
        until(
            &mut b,
            |event| matches!(event, Event::Movement{walkers,..} if walkers.iter().any(|m| m.walking == [1, -1] && m.position[1] < 0.65)),
        )?;
        a.send(Command::Walk { direction: [0, 0] })?;
        let (spec, raw) = fixture();
        a.send(Command::Offer {
            spec: spec.clone(),
            raw,
        })?;
        until(
            &mut b,
            |e| matches!(e,Event::RoomState(Some(room)) if room.chart.is_some()),
        )?;
        a.send(Command::Ready {
            spec: spec.clone(),
            offset_us: 0,
        })?;
        b.send(Command::Ready { spec, offset_us: 0 })?;
        until(
            &mut a,
            |e| matches!(e,Event::RoomState(Some(room)) if room.members.iter().all(|m|m.ready)),
        )?;
        a.send(Command::Start)?;
        let Event::Started {
            round, start_us, ..
        } = until(&mut a, |e| matches!(e, Event::Started { .. }))?
        else {
            unreachable!()
        };
        until(&mut b, |e| matches!(e, Event::Started { .. }))?;
        // Keep polling to synchronize both monotonic clocks. Send Alice's hit 400ms late.
        let began = Instant::now();
        let mut sent = false;
        let mut final_a = false;
        let mut final_b = false;
        while !final_a || !final_b {
            for event in a.poll() {
                match event {
                    ClientEvent::Message(Event::Scores {
                        standings,
                        final_result: true,
                        ..
                    }) => {
                        assert!(standings[0].points > 0);
                        assert_eq!(standings[1].points, 0);
                        final_a = true;
                    }
                    ClientEvent::Disconnected(e) => anyhow::bail!("Alice: {e}"),
                    _ => {}
                }
            }
            for event in b.poll() {
                match event {
                    ClientEvent::Message(Event::Scores {
                        standings,
                        final_result: true,
                        ..
                    }) => {
                        assert!(standings[0].points > 0);
                        final_b = true;
                    }
                    ClientEvent::Disconnected(e) => anyhow::bail!("Bob: {e}"),
                    _ => {}
                }
            }
            if !sent && a.server_us().is_some_and(|now| now >= start_us + 1_400_000) {
                a.send(Command::Input {
                    round,
                    sequence: 1,
                    song_time: SongTime::new(1.).unwrap(),
                    lane: 0,
                    down: true,
                })?;
                sent = true;
            }
            ensure!(
                began.elapsed() < Duration::from_secs(13),
                "Round did not finish"
            );
            thread::sleep(Duration::from_millis(5));
        }
        assert!(a.clock.synchronized() && b.clock.synchronized());
        Ok(())
    }
}
