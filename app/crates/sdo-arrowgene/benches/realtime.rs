//! Run with cargo bench -p sdo-arrowgene --bench realtime.
//! Uses the curated collection selected by SDO_ARROWGENE_DATA, a GPU and an audio device.
use anyhow::{Result, bail, ensure};
use sdo_asset_loader::{AssetLoader, curated::CuratedProvider, request as asset};
use sdo_assets::curated::CollectionPaths;
use sdo_gameplay::{Lane, Note, Practice, input::InputState};
use sdo_multiplayer::Server;
use sdo_net::{ChartSpec, Command, Event, SongTime};
use sdo_platform::net::{Client, ClientEvent};
use sdo_render::{
    Renderer, Sprite,
    character::{
        CharacterCamera, CharacterRenderer,
        stage::{Dancer, StageModel, StageRenderer},
    },
};
use std::thread::sleep;
use std::time::{Duration, Instant};
fn report(name: &str, mut samples: Vec<f64>) {
    samples.sort_by(f64::total_cmp);
    assert!(!samples.is_empty(), "No samples for {name}");
    let q = |p: f64| samples[(samples.len() as f64 * p).ceil() as usize - 1];
    println!(
        "{name}: n={} p50={:.3} p95={:.3} p99={:.3} ms",
        samples.len(),
        q(0.5),
        q(0.95),
        q(0.99)
    );
}
fn elapsed_ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.
}
fn wait_for(c: &mut Client, f: impl Fn(&Event) -> bool) -> Result<Event> {
    let t = Instant::now();
    loop {
        for e in c.poll() {
            match e {
                ClientEvent::Message(e) if f(&e) => return Ok(e),
                ClientEvent::Message(Event::Error(s)) | ClientEvent::Disconnected(s) => {
                    bail!("{s}")
                }
                _ => (),
            }
        }
        ensure!(t.elapsed().as_secs() < 10, "network timeout");
        sleep(Duration::from_micros(250));
    }
}

fn network(collection: &CollectionPaths, with_room_traffic: bool) -> Result<()> {
    let server = Server::bind("127.0.0.1:0")?;
    let mut sender = Client::connect(server.address.to_string(), "A".into())?;
    let mut receiver = Client::connect(server.address.to_string(), "B".into())?;
    wait_for(&mut sender, |e| matches!(e, Event::Welcome { .. }))?;
    wait_for(&mut receiver, |e| matches!(e, Event::Welcome { .. }))?;
    sender.send(Command::Create {
        name: "Review".into(),
        password: String::new(),
    })?;
    let Event::RoomState(Some(room)) =
        wait_for(&mut sender, |e| matches!(e, Event::RoomState(Some(_))))?
    else {
        unreachable!()
    };
    receiver.send(Command::Join {
        room: room.id,
        password: String::new(),
    })?;
    wait_for(
        &mut receiver,
        |e| matches!(e,Event::RoomState(Some(r)) if r.members.len()==2),
    )?;
    let raw = std::fs::read(collection.assets().join("music/sdom00001K.gn"))?;
    let spec = ChartSpec {
        pattern_sha256: sdo_song::chart::pattern_sha256(&raw),
        audio_sha256: "a".repeat(64),
        dance_sha256: None,
        title: "Review".into(),
        kind: "k".into(),
        difficulty: 0,
        judgment: "keyboard".into(),
        bpm: 139.,
        duration_us: 186234195,
    };
    sender.send(Command::Offer {
        spec: spec.clone(),
        raw,
    })?;
    wait_for(
        &mut receiver,
        |e| matches!(e,Event::RoomState(Some(r)) if r.chart.is_some()),
    )?;
    for client in [&sender, &receiver] {
        client.send(Command::Ready {
            spec: spec.clone(),
            offset_us: 0,
        })?
    }
    wait_for(
        &mut sender,
        |e| matches!(e,Event::RoomState(Some(r)) if r.members.iter().all(|m|m.ready)),
    )?;
    sender.send(Command::Start)?;
    let Event::Started {
        round, start_us, ..
    } = wait_for(&mut sender, |e| matches!(e, Event::Started { .. }))?
    else {
        unreachable!()
    };
    let address = server.address.to_string();
    let traffic = if with_room_traffic {
        let mut client = Client::connect(address, "C".into())?;
        wait_for(&mut client, |e| matches!(e, Event::Welcome { .. }))?;
        client.send(Command::Create {
            name: "Load Room".into(),
            password: String::new(),
        })?;
        wait_for(&mut client, |e| matches!(e, Event::RoomState(Some(_))))?;
        let spec = spec.clone();
        let raw = std::fs::read(collection.assets().join("music/sdom00001K.gn"))?;
        Some(std::thread::spawn(move || -> Result<()> {
            for i in 0_u32..55 {
                client.send(Command::Walk {
                    direction: [if i.is_multiple_of(2) { 1 } else { -1 }, 0],
                })?;
                if i.is_multiple_of(6) {
                    client.send(Command::Offer {
                        spec: spec.clone(),
                        raw: raw.clone(),
                    })?;
                }
                for event in client.poll() {
                    if let ClientEvent::Message(Event::Error(e)) | ClientEvent::Disconnected(e) =
                        event
                    {
                        bail!("{e}");
                    }
                }
                sleep(Duration::from_millis(180));
            }
            Ok(())
        }))
    } else {
        None
    };
    let origin = Instant::now();
    while sender.server_us().unwrap_or(0) < start_us + 20000 {
        sender.poll();
        receiver.poll();
        ensure!(origin.elapsed().as_secs() < 6);
        sleep(Duration::from_millis(1))
    }
    let mut sequence = 0;
    for (label, poll_us) in [
        ("peer input loopback, fast poll", 250),
        ("peer input loopback, 60 Hz poll", 16667),
    ] {
        let mut starts = std::collections::BTreeMap::new();
        let mut times = Vec::new();
        let mut next_send = Instant::now();
        let mut next_poll = next_send;
        let limit = sequence + 150;
        let epoch = Instant::now();
        while times.len() < 150 {
            let now = Instant::now();
            if sequence < limit && now >= next_send {
                sequence += 1;
                starts.insert(sequence, now);
                sender.send(Command::Input {
                    round,
                    sequence,
                    song_time: SongTime::new((sender.server_us().unwrap() - start_us) as f64 / 1e6)
                        .unwrap(),
                    lane: 0,
                    down: sequence % 2 == 1,
                })?;
                next_send += Duration::from_micros(20500);
            }
            sender.poll();
            if now >= next_poll {
                for e in receiver.poll() {
                    match e {
                        ClientEvent::Message(Event::Input { sequence, .. }) => {
                            times.push(elapsed_ms(starts.remove(&sequence).unwrap()))
                        }
                        ClientEvent::Message(Event::Error(e)) | ClientEvent::Disconnected(e) => {
                            bail!("{e}")
                        }
                        _ => (),
                    }
                }
                next_poll += Duration::from_micros(poll_us);
            }
            ensure!(
                epoch.elapsed().as_secs() < 9,
                "input timeout {sequence}, {} received",
                times.len()
            );
            sleep(Duration::from_micros(100));
        }
        report(
            &format!("{label}, second room traffic {with_room_traffic}"),
            times,
        );
    }
    if let Some(worker) = traffic {
        worker
            .join()
            .map_err(|_| anyhow::anyhow!("Background room worker panicked"))??;
    }
    Ok(())
}
fn simulation() {
    for count in [1000, 20000] {
        let notes = (0..count)
            .map(|i| Note {
                lane: Lane::ALL[i % 4],
                start: 5. + i as f64 * 0.05,
                end: 5. + i as f64 * 0.05,
            })
            .collect();
        let practice = Practice::from_notes(
            notes,
            1200.,
            sdo_gameplay::judgment::find("keyboard").unwrap(),
        );
        let mut replays = (0..8)
            .map(|_| sdo_multiplayer::Replay::new(practice.clone()))
            .collect::<Vec<_>>();
        let mut times = vec![];
        for i in 0..900 {
            let t = Instant::now();
            for replay in &mut replays {
                replay.advance(i * 5000);
                std::hint::black_box(replay.terminal());
            }
            if i >= 100 {
                times.push(elapsed_ms(t))
            }
        }
        report(&format!("8 server replays, {count} future notes"), times);
    }
}
fn audio() -> Result<()> {
    let player = sdo_audio::AudioPlayer::new(
        sdo_audio::DecodedSong {
            channels: 2,
            sample_rate: 48000,
            samples: vec![0.; 48000 * 2 * 5],
        },
        0.,
    )?;
    let at = Instant::now() + Duration::from_millis(300);
    player.schedule_start(at)?;
    let mut errors = vec![];
    while Instant::now() < at + Duration::from_secs(3) {
        let t = Instant::now();
        let pos = player.position_at(t);
        if t > at + Duration::from_millis(150) {
            errors.push((pos - t.duration_since(at).as_secs_f64()).abs() * 1000.)
        }
        player.check()?;
        sleep(Duration::from_millis(5));
    }
    report(
        &format!("{} software clock abs error", player.device_name),
        errors,
    );
    Ok(())
}
fn render(collection: &CollectionPaths) -> Result<()> {
    let content = AssetLoader::new(CuratedProvider::from_paths(collection.clone())?);
    let assets = content.request(asset::Characters(&Default::default()))?;
    let ui = content.request(asset::Ui)?;
    let skin = content.request(asset::Skin(&ui.selection("default", None, None, None)?))?;
    let preset = assets.preset("male")?;
    let idle = preset.idle.as_ref().unwrap();
    let motion = content.request(asset::Motion(&idle.motion))?;
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let (_, d, q, name) = pollster::block_on(sdo_render::request_device(&instance, None))?;
    println!("GPU {name}");
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut r = Renderer::new(d, q, format, &skin);
    let target = r.device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: 960,
            height: 720,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let mut solo = CharacterRenderer::new(&r.device, &r.queue, &assets);
    let solo_id = r.register_external_texture(&solo.view);
    let mut stage = StageRenderer::new(&r.device, &r.queue, &assets);
    let stage_id = r.register_external_texture(&stage.character.view);
    for i in 0..8 {
        stage.set_model(i, StageModel::new(&r.device, &r.queue, &assets, "male")?);
    }
    let chart = sdo_song::Chart::parse_with_bpm(
        std::fs::read(collection.assets().join("music/sdom00001K.gn"))?,
        sdo_song::Difficulty::Hard,
        Some(139.),
    )?;
    println!("Render chart hard {} notes", chart.notes.len());
    let base = chart.into_practice(
        186.234,
        sdo_gameplay::judgment::find("keyboard").unwrap(),
        sdo_song::Difficulty::Hard,
    );
    let field = sdo_ui::notes::NoteDisplay::default();
    let input = InputState::default();
    for n in [0, 1, 8] {
        let mut p = base.clone();
        p.advance_to(40., [false; 4], true);
        let mut cpu = vec![];
        let mut total = vec![];
        for i in 0..220 {
            let t = Instant::now();
            let seconds = 40. + i as f64 / 120.;
            p.advance_to(seconds, [false; 4], true);
            let frame = ((i as f32 / 120.) * idle.frames_per_second) % motion.last_frame.max(1.);
            let scene = if n == 8 {
                let poses = (0..8)
                    .map(|_| motion.pose(&preset.skeleton, frame))
                    .collect::<Result<Vec<_>>>()?;
                let dancers = poses
                    .iter()
                    .enumerate()
                    .map(|(slot, pose)| Dancer {
                        id: slot as u64,
                        slot: slot as u8,
                        position: None,
                        yaw: 0.,
                        pose: Some(pose),
                    })
                    .collect::<Vec<_>>();
                stage.render(&r.device, &r.queue, seconds, &dancers, true, 640. / 720.)?;
                field.multiplayer_scene(&skin, &p, &input, stage_id)
            } else {
                let mut scene = field.scene(&skin, &p, &input);
                if n == 1 {
                    let pose = motion.pose(&preset.skeleton, frame)?;
                    solo.render_pose(
                        &r.device,
                        &r.queue,
                        "male",
                        CharacterCamera::default(),
                        Some(&pose),
                    )?;
                    scene.push(Sprite::new(solo_id, [7., 180., 320., 360.]));
                }
                scene
            };
            r.draw(&view, [960, 720], &scene)?;
            let submit = elapsed_ms(t);
            r.device.poll(wgpu::PollType::wait_indefinitely())?;
            if i >= 20 {
                cpu.push(submit);
                total.push(elapsed_ms(t))
            }
        }
        report(&format!("actors {n} CPU through submit"), cpu);
        report(&format!("actors {n} through GPU completion"), total);
    }
    Ok(())
}
fn main() -> Result<()> {
    let collection = CollectionPaths::from_env()?;
    simulation();
    active_referees()?;
    network(&collection, false)?;
    network(&collection, true)?;
    audio()?;
    render(&collection)
}
/// Eight active players with chords, holds, a 50 ms delivery delay and grade publication.
fn active_referees() -> Result<()> {
    let notes = (0..20_000)
        .map(|i| {
            let start = 1. + (i / 2) as f64 * 0.1;
            Note {
                lane: Lane::ALL[i % 4],
                start,
                end: if i % 4 == 0 { start + 0.05 } else { start },
            }
        })
        .collect();
    let practice = Practice::from_notes(
        notes,
        1002.,
        sdo_gameplay::judgment::find("keyboard").unwrap(),
    );
    let mut players = (0..8)
        .map(|_| sdo_multiplayer::Replay::new(practice.clone()))
        .collect::<Vec<_>>();
    let mut times = Vec::new();
    let mut sequence = 0;
    for tick in 0_u64..1200 {
        let now = tick * 5000;
        let began = Instant::now();
        for player in &mut players {
            player.advance(now);
            if tick >= 210 && (tick - 210).is_multiple_of(10) {
                let head = ((tick - 210) / 20) * 100_000 + 1_000_000;
                let down = (tick - 210).is_multiple_of(20);
                let edge =
                    SongTime::new((head + if down { 0 } else { 50_000 }) as f64 / 1e6).unwrap();
                let lane = (((tick - 210) / 20) % 2 * 2) as u8;
                player.input(sequence + 1, edge, lane, down)?;
                player.input(sequence + 2, edge, lane + 1, down)?;
            }
            if tick.is_multiple_of(20) {
                std::hint::black_box(player.judgment_changes());
            }
        }
        if tick >= 210 && (tick - 210).is_multiple_of(10) {
            sequence += 2;
        }
        if tick >= 210 {
            times.push(elapsed_ms(began));
        }
    }
    ensure!(
        players.iter().all(|p| p.standing(1).points > 0),
        "Missing benchmark inputs"
    );
    report(
        "8 active server replays, 20000 notes, late chords and holds",
        times,
    );
    Ok(())
}
