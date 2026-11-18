//! Optional end-to-end scene verification against the local curated collection.
use anyhow::{Result, ensure};
use sdo_asset_loader::{AssetLoader, curated::CuratedProvider, request as asset};
use sdo_assets::curated::CollectionPaths;
use sdo_gameplay::{Practice, input::InputState};
use sdo_render::{
    Renderer,
    character::stage::{Dancer, StageModel, StageRenderer},
};
use std::time::Instant;

#[test]
#[ignore = "Requires local assets, saved outfits and a wgpu device"]
fn saved_outfits_resolve_on_clients_with_default_wardrobes() -> Result<()> {
    let paths = CollectionPaths::from_env()?;
    let settings: crate::settings::Settings = serde_json::from_slice(&std::fs::read(
        paths.root().join("app/target/debug/settings.json"),
    )?)?;
    ensure!(!settings.outfits.is_empty(), "No saved outfits to verify");
    let content = AssetLoader::new(CuratedProvider::from_paths(paths)?);
    let defaults = content.request(asset::Characters(&Default::default()))?;
    let equipped = content.request(asset::Characters(&settings.outfits))?;
    let ui = content.request(asset::Ui)?;
    let skin = content.request(asset::Skin(&ui.selection("default", None, None, None)?))?;
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let (_, device, queue, _) = pollster::block_on(sdo_render::request_device(&instance, None))?;
    let renderer = Renderer::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb, &skin);
    let worker = crate::scene::SceneLoader::new(renderer.readback(), content);
    let receive = || -> Result<crate::scene::Loaded> {
        let start = Instant::now();
        loop {
            if let Some(loaded) = worker.poll() {
                return Ok(loaded);
            }
            ensure!(
                start.elapsed().as_secs() < 30,
                "Saved outfit loading timed out"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    };
    for (index, preset) in equipped.presets.iter().enumerate() {
        worker.request(crate::scene::Load {
            player: 1,
            generation: index as u64 * 2,
            desired: None,
            assets: equipped.clone(),
            selected: preset.choice.id.clone(),
            song: None,
            upload: false,
        })?;
        let (local, _) = receive()?.result?;
        worker.request(crate::scene::Load {
            player: 2,
            generation: index as u64 * 2 + 1,
            desired: Some(local.appearance.clone()),
            assets: defaults.clone(),
            selected: String::new(),
            song: None,
            upload: true,
        })?;
        let (remote, model) = receive()?.result?;
        ensure!(
            model.is_some(),
            "Remote character was not prepared for rendering"
        );
        assert_eq!(local.appearance, remote.appearance);
        println!(
            "Verified equipped {} resolves on the other client",
            preset.choice.id
        );
    }
    Ok(())
}

#[test]
#[ignore = "Requires the local collection and a wgpu device"]
fn eight_local_models_camera_portraits_and_results_render_without_network_images() -> Result<()> {
    let paths = CollectionPaths::from_env()?;
    let artifacts = paths.root().join("app/artifacts");
    let content = AssetLoader::new(CuratedProvider::from_paths(paths)?);
    let assets = content.request(asset::Characters(&Default::default()))?;
    ensure!(!assets.presets.is_empty(), "No local characters");
    let ui_assets = content.request(asset::Ui)?;
    let skin = content.request(asset::Skin(
        &ui_assets.selection("default", None, None, None)?,
    ))?;
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let (_, device, queue, _) = pollster::block_on(sdo_render::request_device(&instance, None))?;
    let mut renderer = Renderer::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb, &skin);
    // Exercise the real local/remote descriptor resolver, including fingerprints
    // and authored choreography; it must not need any images from a peer.
    let catalog = content.request(asset::Catalog)?;
    let selection = catalog.select("00001", None, "k", 4)?;
    let song = crate::scene::SceneSong {
        spec: sdo_net::ChartSpec {
            pattern_sha256: selection.pattern.sha256.clone(),
            audio_sha256: "a".repeat(64),
            dance_sha256: None,
            title: selection.song.title.clone(),
            kind: "k".into(),
            difficulty: 0,
            judgment: "keyboard".into(),
            bpm: selection.bpm(),
            duration_us: (selection.song.duration * 1e6) as u64,
        },
        chart: selection.pattern.file.clone(),
        dance: selection.song.dance_moves_paths.clone(),
    };
    let worker = crate::scene::SceneLoader::new(renderer.readback(), content.clone());
    let receive = || -> Result<crate::scene::Loaded> {
        let began = Instant::now();
        loop {
            if let Some(loaded) = worker.poll() {
                return Ok(loaded);
            }
            ensure!(began.elapsed().as_secs() < 30, "Asset loading timed out");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    };
    worker.request(crate::scene::Load {
        player: 1,
        generation: 1,
        desired: None,
        assets: assets.clone(),
        selected: assets.presets[0].choice.id.clone(),
        song: Some(song.clone()),
        upload: true,
    })?;
    let (mut local, _) = receive()?.result?;
    let packet = serde_json::to_vec(&sdo_net::Command::SetAppearance {
        appearance: Some(local.appearance.clone()),
    })?;
    ensure!(
        packet.len() < 4096,
        "Appearance is no longer a small state packet"
    );
    worker.request(crate::scene::Load {
        player: 2,
        generation: 2,
        desired: Some(local.appearance.clone()),
        assets: assets.clone(),
        selected: String::new(),
        song: Some(song),
        upload: false,
    })?;
    let (mut remote, _) = receive()?.result?;
    assert_eq!(local.appearance, remote.appearance);
    let a = local
        .playback
        .as_mut()
        .unwrap()
        .sample(8., None, &local.preset.skeleton)?
        .unwrap();
    let b = remote
        .playback
        .as_mut()
        .unwrap()
        .sample(8., None, &remote.preset.skeleton)?
        .unwrap();
    assert_eq!(
        a.globals, b.globals,
        "Local and resolved remote actors must sample identical poses"
    );
    let mut stage = StageRenderer::new(&renderer.device, &renderer.queue, &assets);
    let mut poses = vec![];
    for slot in 0..8 {
        let preset = &assets.presets[slot % assets.presets.len()];
        stage.set_model(
            slot as u64 + 1,
            StageModel::new(
                &renderer.device,
                &renderer.queue,
                &assets,
                &preset.choice.id,
            )?,
        );
        let pose = preset
            .idle
            .as_ref()
            .map(|definition| {
                let motion = content.request(asset::Motion(&definition.motion))?;
                motion.pose(
                    &preset.skeleton,
                    definition.frames_per_second.min(motion.last_frame),
                )
            })
            .transpose()?;
        poses.push(pose);
    }
    let dancers = (0..8)
        .map(|slot| Dancer {
            id: slot as u64 + 1,
            slot: slot as u8,
            position: None,
            yaw: 0.,
            pose: poses[slot].as_deref(),
        })
        .collect::<Vec<_>>();
    stage.render(&renderer.device, &renderer.queue, 0., &dancers, false, 1.6)?;
    renderer.save_texture(
        &artifacts.join("local-room-head-atlas.png"),
        &stage.heads,
        [1536, 192],
    )?;
    let head_pixels = renderer.texture_rgba(&stage.heads)?;
    for slot in 0..8 {
        let count = (0..192)
            .flat_map(|y| (0..192).map(move |x| (y * 1536 + slot * 192 + x) * 4 + 3))
            .filter(|i| head_pixels[*i] > 0)
            .count();
        ensure!(count > 300, "Empty local portrait in slot {slot}");
    }
    stage.render(
        &renderer.device,
        &renderer.queue,
        0.,
        &dancers,
        true,
        640. / 720.,
    )?;
    let first = renderer.texture_rgba(&stage.character.texture)?;
    stage.render(
        &renderer.device,
        &renderer.queue,
        12.,
        &dancers,
        true,
        640. / 720.,
    )?;
    let second = renderer.texture_rgba(&stage.character.texture)?;
    ensure!(first != second, "The shared camera must move");
    let texture = renderer.register_external_texture(&stage.character.view);
    let mut practice = Practice::new();
    practice.advance_to(3., [false; 4], true);
    let display = sdo_ui::notes::NoteDisplay::default();
    let scene = display.multiplayer_scene(&skin, &practice, &InputState::default(), texture);
    renderer.screenshot(&artifacts.join("multiplayer-stage.png"), [960, 720], &scene)?;
    let room: sdo_net::Room = serde_json::from_value(serde_json::json!({
        "keys":4,"levels":[1,2,3],"password_protected":false,"id":1,"name":"Multiplayer",
        "host":1,"members":(0..8).map(|slot|serde_json::json!({"id":slot+1,"slot":slot,"name":format!("Dancer {}",slot+1),"x":0.5,"y":0.5,"facing":[0,1],"walking":[0,0],"ready":false})).collect::<Vec<_>>(),
        "chart":null,"round":null,"start_us":null
    }))?;
    let standings = (1..=8)
        .map(|id| sdo_net::Standing {
            player: id,
            points: 10000 - id * 100,
            max_combo: 30,
            failed: id == 8,
            ..Default::default()
        })
        .collect::<Vec<_>>();
    let target = renderer.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("multiplayer results preview"),
        size: wgpu::Extent3d {
            width: 960,
            height: 720,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let mut ui =
        sdo_ui::SelectorUi::new(&renderer.device, wgpu::TextureFormat::Rgba8UnormSrgb, None);
    for _ in 0..3 {
        renderer.draw(&view, [960, 720], &scene)?;
        ui.draw_round(
            &renderer.device,
            &renderer.queue,
            &view,
            [960, 720],
            None,
            sdo_ui::RoundHud {
                room: &room,
                player: Some(1),
                standings: &standings,
                title: "Multiplayer round",
                seconds: 36.,
                duration: 36.,
                countdown: None,
                failed: false,
                results: true,
                disconnected: false,
            },
        );
    }
    renderer.save_texture(
        &artifacts.join("multiplayer-results.png"),
        &target,
        [960, 720],
    )?;
    let began = Instant::now();
    for frame in 0..60 {
        stage.render(
            &renderer.device,
            &renderer.queue,
            frame as f64 / 60.,
            &dancers,
            true,
            640. / 720.,
        )?;
    }
    renderer.device.poll(wgpu::PollType::wait_indefinitely())?;
    println!(
        "Eight full local models: {:.2} ms/frame including GPU completion; no camera readback in live rendering",
        began.elapsed().as_secs_f64() * 1000. / 60.
    );
    Ok(())
}
