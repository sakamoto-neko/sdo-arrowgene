//! Resolve remote appearances locally, once per outfit/song change. No disk
//! reads, model uploads, network images or GPU readbacks occur per video frame.
use anyhow::{Context, Result, ensure};
use sdo_asset_loader::{AssetLoader, request as asset};
use sdo_assets::{CharacterAssets, CharacterMotion, CharacterPreset};
use sdo_dance::{DancePlayer, ReplayDance};
use sdo_net::{Appearance, ChartSpec, PlayerId};
use sdo_render::character::stage::StageModel;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    sync::{Arc, mpsc},
};

fn unscoped(id: &str) -> &str {
    id.rsplit_once("::").map_or(id, |(_, v)| v)
}
fn appearance(
    content: &AssetLoader,
    assets: &CharacterAssets,
    preset: &CharacterPreset,
) -> Result<Appearance> {
    let mut hash = Sha256::new();
    hash.update(assets.preset_sha256(&preset.choice.id)?.as_bytes());
    for animation in [&preset.idle, &preset.walk, &preset.miss] {
        if let Some(animation) = animation {
            hash.update([1]);
            hash.update(animation.frames_per_second.to_le_bytes());
            hash.update(
                content
                    .request(asset::Motion(&animation.motion))?
                    .sha256()
                    .as_bytes(),
            );
        } else {
            hash.update([0]);
        }
    }
    let value = Appearance {
        preset: unscoped(&preset.choice.id).into(),
        equipment: preset
            .equipment
            .iter()
            .map(|(slot, item)| (slot.key().into(), unscoped(item).into()))
            .collect(),
        sha256: format!("{:x}", hash.finalize()),
    };
    value.validate()?;
    Ok(value)
}
#[derive(Clone, PartialEq)]
pub(crate) struct SceneSong {
    pub spec: ChartSpec,
    pub chart: String,
    pub dance: Option<String>,
}
pub(crate) struct Actor {
    pub appearance: Appearance,
    pub preset: CharacterPreset,
    idle: Option<(Arc<CharacterMotion>, f32)>,
    walk: Option<(Arc<CharacterMotion>, f32)>,
    pub playback: Option<ReplayDance>,
}
impl Actor {
    pub fn room_pose(&self, seconds: f64, walking: bool) -> Result<Option<Vec<glam::Mat4>>> {
        (if walking {
            self.walk.as_ref().or(self.idle.as_ref())
        } else {
            self.idle.as_ref()
        })
        .map(|(motion, fps)| {
            motion.pose(
                &self.preset.skeleton,
                (seconds.max(0.) * f64::from(*fps)).rem_euclid(f64::from(motion.last_frame)) as f32,
            )
        })
        .transpose()
    }
}
pub(crate) struct Load {
    pub player: PlayerId,
    pub generation: u64,
    pub desired: Option<Appearance>,
    pub assets: Arc<CharacterAssets>,
    pub selected: String,
    pub song: Option<SceneSong>,
    pub upload: bool,
}
pub(crate) struct Loaded {
    pub player: PlayerId,
    pub generation: u64,
    pub result: Result<(Actor, Option<StageModel>)>,
}
pub(crate) struct SceneLoader {
    jobs: mpsc::SyncSender<Load>,
    completed: mpsc::Receiver<Loaded>,
}
impl SceneLoader {
    pub fn new(gpu: sdo_render::Readback, content: AssetLoader) -> Self {
        let (jobs, requests) = mpsc::sync_channel::<Load>(16);
        let (finished, completed) = mpsc::sync_channel(16);
        std::thread::Builder::new()
            .name("room-assets".into())
            .spawn(move || {
                while let Ok(load) = requests.recv() {
                    let result = prepare(&gpu, &content, &load);
                    if finished
                        .send(Loaded {
                            player: load.player,
                            generation: load.generation,
                            result,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .expect("Cannot create room asset worker");
        Self { jobs, completed }
    }
    pub fn request(&self, load: Load) -> Result<()> {
        self.jobs
            .try_send(load)
            .map_err(|_| anyhow::anyhow!("Room asset loading queue is full"))
    }
    pub fn poll(&self) -> Option<Loaded> {
        self.completed.try_recv().ok()
    }
}
fn prepare(
    gpu: &sdo_render::Readback,
    content: &AssetLoader,
    load: &Load,
) -> Result<(Actor, Option<StageModel>)> {
    let (assets, preset) = if let Some(desired) = &load.desired {
        desired.validate()?;
        let mut resolved = None;
        for candidate in &load.assets.presets {
            if !unscoped(&candidate.choice.id).eq_ignore_ascii_case(&desired.preset) {
                continue;
            }
            let prefix = candidate
                .choice
                .id
                .rsplit_once("::")
                .map(|(prefix, _)| prefix);
            let mut slots = BTreeMap::new();
            for (slot, item) in &desired.equipment {
                let id = prefix.map_or_else(|| item.clone(), |prefix| format!("{prefix}::{item}"));
                slots.insert(slot.clone(), id);
            }
            let outfits = BTreeMap::from([(candidate.choice.id.clone(), slots)]);
            let Ok(next) = content.request(asset::Characters(&outfits)) else {
                continue;
            };
            let Ok(preset) = next.preset(&candidate.choice.id).cloned() else {
                continue;
            };
            if appearance(content, &next, &preset).is_ok_and(|identity| identity == *desired) {
                resolved = Some((next, preset));
                break;
            }
        }
        resolved.context(
            "This player's character/outfit/animations are missing or differ from the local assets",
        )?
    } else {
        (
            load.assets.clone(),
            load.assets.preset(&load.selected)?.clone(),
        )
    };
    let identity = appearance(content, &assets, &preset)?;
    let animation=|definition:&Option<sdo_assets::CharacterAnimation>|->Result<Option<(Arc<CharacterMotion>,f32)>> {
        definition.as_ref().map(|definition| {
            let motion=content.request(asset::Motion(&definition.motion))?;
            ensure!(motion.supports_bones(preset.skeleton.bones.len())&&motion.last_frame>0.&&definition.frames_per_second>0.,"Incompatible room animation");
            Ok((motion,definition.frames_per_second))
        }).transpose()
    };
    let idle = animation(&preset.idle)?;
    let walk = animation(&preset.walk)?;
    let playback = if let Some(song) = &load.song {
        let chart = content.request(asset::Chart {
            id: &song.chart,
            difficulty: sdo_song::Difficulty::ALL[song.spec.difficulty as usize],
            bpm: song.spec.bpm,
        })?;
        let dance = song
            .dance
            .as_ref()
            .map(|id| {
                content.request(asset::Dance {
                    id,
                    chart: &chart,
                    difficulty: sdo_song::Difficulty::ALL[song.spec.difficulty as usize],
                    bones: preset.skeleton.bones.len(),
                })
            })
            .transpose()?;
        let start = dance.as_ref().and_then(sdo_dance::SongDance::starts_at);
        ensure!(
            song.spec
                .dance_sha256
                .as_ref()
                .is_none_or(|expected| *expected == sdo_dance::song_signature(dance.as_ref())),
            "Room choreography or a referenced motion differs from the host"
        );
        let reaction = |definition: &Option<sdo_assets::CharacterAnimation>| {
            definition
                .as_ref()
                .map(|animation| {
                    content.request(asset::Reaction {
                        animation,
                        bones: preset.skeleton.bones.len(),
                    })
                })
                .transpose()
        };
        Some(ReplayDance::new(
            DancePlayer::new(dance, reaction(&preset.idle)?, reaction(&preset.miss)?),
            start,
        ))
    } else {
        None
    };
    let model = load
        .upload
        .then(|| StageModel::new(&gpu.device, &gpu.queue, &assets, &preset.choice.id))
        .transpose()?;
    Ok((
        Actor {
            appearance: identity,
            preset,
            idle,
            walk,
            playback,
        },
        model,
    ))
}

#[derive(Clone, PartialEq)]
pub(crate) struct LoadKey {
    pub desired: Option<Appearance>,
    pub local: Option<(String, usize)>,
    pub song: Option<SceneSong>,
}

/// Start local movement immediately, then ease out position errors over one server tick.
pub(crate) struct LocalWalk {
    position: [f32; 2],
    direction: [i8; 2],
    facing: [i8; 2],
    at: std::time::Instant,
    error: [f32; 2],
    observed_us: u64,
    command_us: u64,
}
impl LocalWalk {
    pub fn new(m: &sdo_net::Member, now: std::time::Instant) -> Self {
        Self {
            position: [m.x, m.y],
            direction: m.walking,
            facing: m.facing,
            at: now,
            error: [0.; 2],
            observed_us: m.state_us,
            command_us: 0,
        }
    }
    pub fn sample(&self, now: std::time::Instant) -> ([f32; 2], f32, bool) {
        let dt = now.saturating_duration_since(self.at).as_secs_f32();
        let pos = sdo_net::walk_position(self.position, self.direction, dt);
        let weight = (1. - dt / 0.05).clamp(0., 1.);
        let position = std::array::from_fn(|i| {
            (pos[i] + self.error[i] * weight)
                .clamp(sdo_net::ROOM_WALK_MIN[i], sdo_net::ROOM_WALK_MAX[i])
        });
        (
            position,
            f32::from(self.facing[0]).atan2(f32::from(self.facing[1])),
            self.direction != [0, 0],
        )
    }
    pub fn turn(&mut self, direction: [i8; 2], now: std::time::Instant, server_us: Option<u64>) {
        self.position = self.sample(now).0;
        self.at = now;
        self.error = [0.; 2];
        self.direction = direction;
        self.command_us = server_us.unwrap_or(self.observed_us);
        if direction != [0, 0] {
            self.facing = direction;
        }
    }
    pub fn observe(
        &mut self,
        m: &sdo_net::Member,
        now: std::time::Instant,
        server_us: Option<u64>,
    ) {
        if m.state_us <= self.observed_us
            || m.state_us < self.command_us
            || m.walking != self.direction
        {
            return;
        }
        self.observed_us = m.state_us;
        let old = self.sample(now).0;
        let age = server_us
            .unwrap_or(m.state_us)
            .saturating_sub(m.state_us)
            .min(100_000) as f32
            / 1e6;
        self.position = sdo_net::walk_position([m.x, m.y], m.walking, age);
        self.error = std::array::from_fn(|i| old[i] - self.position[i]);
        self.at = now;
        self.facing = m.facing;
    }
}

/// Interpolate timestamped server snapshots at a shared presentation time.
/// This smooths the 20 Hz room movement independently of the display frame rate.
#[derive(Default)]
pub(crate) struct Movement {
    samples: std::collections::VecDeque<sdo_net::Member>,
}
impl Movement {
    pub fn push(&mut self, member: &sdo_net::Member) {
        if self
            .samples
            .back()
            .is_some_and(|s| s.state_us >= member.state_us)
        {
            return;
        }
        self.samples.push_back(member.clone());
        while self.samples.len() > 8 {
            self.samples.pop_front();
        }
    }
    pub fn sample(&self, at: u64, fallback: &sdo_net::Member) -> ([f32; 2], f32, bool) {
        let end = self
            .samples
            .iter()
            .position(|s| s.state_us >= at)
            .unwrap_or(self.samples.len().saturating_sub(1));
        let a = self.samples.get(end.saturating_sub(1)).unwrap_or(fallback);
        let b = self.samples.get(end).unwrap_or(fallback);
        let t = if b.state_us > a.state_us {
            ((at as f64 - a.state_us as f64) / (b.state_us - a.state_us) as f64).clamp(0., 1.)
                as f32
        } else {
            1.
        };
        let yaw = |m: &sdo_net::Member| f32::from(m.facing[0]).atan2(f32::from(m.facing[1]));
        let delta = (yaw(b) - yaw(a) + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)
            - std::f32::consts::PI;
        (
            [a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t],
            yaw(a) + delta * t,
            (if t < 1. { a.walking } else { b.walking }) != [0, 0],
        )
    }
}

impl crate::app::App {
    pub(crate) fn sync_room_scene(&mut self) -> Result<()> {
        if self.online.client.is_none() || self.selector.sources_loading {
            return Ok(());
        }
        let room = if self.online_running() {
            self.online.round_room.as_ref()
        } else {
            self.selector.lobby.joined_room.as_ref()
        };
        let Some(room) = room else {
            return Ok(());
        };
        let Some(state) = &mut self.state else {
            return Ok(());
        };
        if self.online.scene_loader.is_none() {
            self.online.scene_loader = Some(SceneLoader::new(
                state.renderer.readback(),
                self.content.clone(),
            ));
        }
        let ids = room.members.iter().map(|m| m.id).collect::<Vec<_>>();
        self.online.actors.retain(|id, _| ids.contains(id));
        self.online.scene_keys.retain(|id, _| ids.contains(id));
        self.online.scene_pending.retain(|id, _| ids.contains(id));
        state.multiplayer_stage.retain(&ids);
        while let Some(loaded) = self.online.scene_loader.as_ref().unwrap().poll() {
            if self.online.scene_pending.get(&loaded.player) != Some(&loaded.generation) {
                continue;
            }
            self.online.scene_pending.remove(&loaded.player);
            match loaded.result {
                Ok((actor, model)) => {
                    if let Some(model) = model {
                        state.multiplayer_stage.set_model(loaded.player, model);
                    }
                    self.selector.lobby.avatar_issues.remove(&loaded.player);
                    if Some(loaded.player) == self.selector.lobby.player
                        && self.online.appearance_sent.as_ref() != Some(&actor.appearance)
                    {
                        // Only a changed identity is announced. Animation playback
                        // never sends frames or poses over the connection.
                        self.online.client.as_ref().unwrap().send(
                            sdo_net::Command::SetAppearance {
                                appearance: Some(actor.appearance.clone()),
                            },
                        )?;
                        self.online.appearance_sent = Some(actor.appearance.clone());
                    }
                    self.online.actors.insert(loaded.player, actor);
                }
                Err(error) => {
                    state.multiplayer_stage.remove_model(loaded.player);
                    self.online.actors.remove(&loaded.player);
                    self.selector
                        .lobby
                        .avatar_issues
                        .insert(loaded.player, format!("Character unavailable: {error:#}"));
                }
            }
        }
        if self.online.scene_song.as_ref().map(|(spec, _)| spec) != room.chart.as_ref() {
            self.online.scene_song = room.chart.as_ref().map(|spec| {
                let song = self.selector.catalog.songs.iter().find_map(|song| {
                    song.patterns
                        .iter()
                        .find(|p| p.sha256 == spec.pattern_sha256)
                        .map(|p| SceneSong {
                            spec: spec.clone(),
                            chart: p.file.clone(),
                            dance: song.dance_moves_paths.clone(),
                        })
                });
                (spec.clone(), song)
            });
        }
        let song = self
            .online
            .scene_song
            .as_ref()
            .and_then(|(_, song)| song.as_ref());
        for member in &room.members {
            let local = Some(member.id) == self.selector.lobby.player;
            if local && self.selector.character.is_empty() {
                continue;
            }
            let desired = if local {
                None
            } else {
                self.online.appearances.get(&member.id).cloned()
            };
            if !local && desired.is_none() {
                continue;
            }
            let key = LoadKey {
                desired: desired.clone(),
                local: local.then(|| {
                    (
                        self.selector.character.clone(),
                        Arc::as_ptr(&self.characters) as usize,
                    )
                }),
                song: song.cloned(),
            };
            if self.online.scene_keys.get(&member.id) == Some(&key) {
                continue;
            }
            self.online.scene_generation += 1;
            let generation = self.online.scene_generation;
            let upload = local
                && self
                    .online
                    .scene_keys
                    .get(&member.id)
                    .is_none_or(|old| old.local != key.local)
                || !local
                    && self
                        .online
                        .scene_keys
                        .get(&member.id)
                        .is_none_or(|old| old.desired != key.desired)
                || !state.multiplayer_stage.has_model(member.id);
            self.online.scene_loader.as_ref().unwrap().request(Load {
                player: member.id,
                generation,
                desired,
                assets: self.characters.clone(),
                selected: self.selector.character.clone(),
                song: song.cloned(),
                upload,
            })?;
            self.online.scene_pending.insert(member.id, generation);
            self.online.scene_keys.insert(member.id, key);
        }
        self.selector.lobby.available_avatars = self.online.actors.keys().copied().collect();
        self.selector.lobby.scene_loading = room.members.iter().any(|m| {
            !self.online.actors.contains_key(&m.id) || self.online.scene_pending.contains_key(&m.id)
        });
        Ok(())
    }
    pub(crate) fn render_room_scene(&mut self) -> Result<()> {
        let playing = self.online_running();
        if !playing && self.selector.character_view {
            return Ok(());
        }
        let room = if playing {
            self.online.round_room.as_ref()
        } else {
            self.selector.lobby.joined_room.as_ref()
        };
        let Some(room) = room else {
            return Ok(());
        };
        let sample_us = self
            .online
            .client
            .as_ref()
            .and_then(sdo_net::Client::server_us)
            .unwrap_or(0)
            .saturating_sub(75_000);
        let now = std::time::Instant::now();
        let movement = room
            .members
            .iter()
            .map(|m| {
                if !playing
                    && Some(m.id) == self.selector.lobby.player
                    && let Some(local) = &self.online.local_walk
                {
                    return (m.id, local.sample(now));
                }
                (
                    m.id,
                    self.online.movement.get(&m.id).map_or_else(
                        || {
                            (
                                [m.x, m.y],
                                f32::from(m.facing[0]).atan2(f32::from(m.facing[1])),
                                m.walking != [0, 0],
                            )
                        },
                        |track| track.sample(sample_us, m),
                    ),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let seconds = if playing {
            self.online_round_seconds().max(0.)
        } else {
            sample_us as f64 / 1e6
        };
        let Some(state) = &mut self.state else {
            return Ok(());
        };
        if state.occluded {
            return Ok(());
        }
        let mut poses = BTreeMap::new();
        for member in &room.members {
            let Some(actor) = self.online.actors.get_mut(&member.id) else {
                continue;
            };
            let pose = if playing {
                let cue = self
                    .online
                    .replays
                    .get(&member.id)
                    .and_then(|replay| replay.practice().dance_cue());
                // Inputs are calibrated chart time. Presentation stays at the
                // shared song time so all clients display the same camera/frame.
                let offset = self.online.offsets.get(&member.id).copied().unwrap_or(0) as f64 / 1e6;
                actor
                    .playback
                    .as_mut()
                    .map(|p| p.sample_timed(seconds, cue, offset, &actor.preset.skeleton))
                    .transpose()?
                    .flatten()
                    .map(|p| p.globals)
            } else {
                actor.room_pose(seconds, movement[&member.id].2)?
            };
            poses.insert(member.id, pose);
        }
        let dancers = room
            .members
            .iter()
            .map(|member| sdo_render::character::stage::Dancer {
                id: member.id,
                slot: member.slot,
                position: (!playing).then_some(movement[&member.id].0),
                yaw: if playing { 0. } else { movement[&member.id].1 },
                pose: poses.get(&member.id).and_then(|pose| pose.as_deref()),
            })
            .collect::<Vec<_>>();
        let aspect = if playing {
            640. / 720.
        } else {
            self.selector.lobby.stage_aspect.max(1.)
        };
        state.multiplayer_stage.render(
            &state.renderer.device,
            &state.renderer.queue,
            seconds,
            &dancers,
            playing,
            aspect,
        )?;
        self.selector
            .lobby
            .stage_labels
            .clone_from(&state.multiplayer_stage.labels);
        Ok(())
    }
}

#[cfg(test)]
mod walk_tests {
    use super::LocalWalk;
    use std::time::{Duration, Instant};
    #[test]
    fn local_walk_starts_immediately_and_eases_to_the_final_stop() {
        let now = Instant::now();
        let mut member = sdo_net::Member {
            id: 1,
            slot: 0,
            name: "A".into(),
            x: 0.5,
            y: 0.65,
            facing: [0, 1],
            walking: [0, 0],
            ready: false,
            state_us: 1000,
        };
        let mut walk = LocalWalk::new(&member, now);
        walk.turn([1, 0], now, Some(1000));
        let after = now + Duration::from_millis(20);
        let x = walk.sample(after).0[0];
        assert!((x - 0.5056).abs() < 0.00001);
        walk.turn([0, 0], after, Some(21000));
        member.walking = [1, 0];
        member.state_us = 20_000;
        walk.observe(&member, after, Some(22000));
        assert_eq!(walk.sample(after).0[0], x);
        member.walking = [0, 0];
        member.state_us = 51_000;
        member.x = 0.510;
        walk.observe(&member, after, Some(60_000));
        assert!((walk.sample(after).0[0] - x).abs() < 0.00001);
        assert_eq!(
            walk.sample(after + Duration::from_millis(51)).0[0],
            member.x
        );
        assert!(!walk.sample(after).2);
    }
}
