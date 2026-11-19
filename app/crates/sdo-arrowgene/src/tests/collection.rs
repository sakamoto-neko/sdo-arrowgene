//! Optional regressions against an external curated collection; no assets are shipped.
use crate::{
    app::App,
    cli::Options,
    scores::ScoreStore,
    session::{PlaySession, ScoreSaveState, TailClock},
    settings::SettingsStore,
    wardrobe::WardrobeThumbnails,
};
use anyhow::{Context, Result, ensure};
use sdo_asset_loader::{self as loader, AssetLoader, curated::CuratedProvider, request as asset};
use sdo_assets::{UiAssets, curated::CollectionPaths};
use sdo_audio::{AudioPlayer, DecodedSong};
use sdo_config::{ScoreKey, ScrollSpeed};
use sdo_dance::{DanceLibrary, DancePlayer, SongDance};
use sdo_gameplay::{
    Practice,
    input::{Binding, InputState},
};
use sdo_render::{
    Renderer, Sprite,
    character::{CharacterCamera, CharacterRenderer},
};
use sdo_song::{Catalog, Chart, Difficulty};
use sdo_ui::notes::NoteDisplay;
use sdo_ui::{SelectorUi, SongSelector};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use winit::keyboard::KeyCode;

#[test]
fn end_clock_handles_offsets_pause_and_restart() {
    let now = Instant::now();
    let mut clock = TailClock::default();
    assert_eq!(clock.position(1.5, 2., false, now), 1.5);
    assert_eq!(clock.position(2., 2., false, now), 2.);
    assert_eq!(
        clock.position(2., 2., true, now + Duration::from_millis(100)),
        2.1
    );
    assert_eq!(
        clock.position(2., 2., true, now + Duration::from_secs(10)),
        2.1
    );
    assert_eq!(
        clock.position(2., 2., false, now + Duration::from_secs(10)),
        2.1
    );
    let position = clock.position(2., 2., false, now + Duration::from_millis(10200));
    assert!((position - 2.3).abs() < 1e-9);
    let mut practice = Practice::from_notes(
        Vec::new(),
        2.,
        sdo_gameplay::judgment::find("keyboard").unwrap(),
    );
    practice.advance_to(position - 0.1, [false; 4], false);
    assert!(
        practice.finished(),
        "A positive offset must not prevent the playback clock finishing"
    );
    assert_eq!(
        clock.position(0., 2., false, now + Duration::from_secs(11)),
        0.
    );
    assert_eq!(
        clock.position(2., 2., false, now + Duration::from_secs(13)),
        2.
    );
}

#[test]
#[ignore = "Requires local assets and an audio output device"]
fn escape_during_the_outro_keeps_completed_scores_and_retries_failed_writes() -> Result<()> {
    use sdo_gameplay::{Lane, Note};
    let temp = std::env::temp_dir().join(format!("sdo-score-escape-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp);
    std::fs::create_dir_all(&temp)?;
    let score_path = temp.join("score.json");
    let paths = CollectionPaths::from_env()?;
    let catalog = Catalog::load(&paths)?;
    let assets = sdo_assets::curated::ui(&paths)?;
    let mut app = collection_app(
        paths,
        Options::default(),
        catalog,
        assets,
        None,
        SettingsStore::load(temp.join("settings.json")),
        ScoreStore::load(score_path.clone()),
    )?;
    let key = ScoreKey::new(
        "00001",
        "k",
        0,
        "keyboard",
        &app.selector
            .catalog
            .select("00001", None, "k", 4)?
            .pattern
            .sha256,
    );
    let session = || -> Result<PlaySession> {
        Ok(PlaySession {
            tail_clock: TailClock::default(),
            score_key: key.clone(),
            score_save: ScoreSaveState::Pending,
            from_start: true,
            practice: Practice::from_notes(
                vec![Note {
                    lane: Lane::Left,
                    start: 1.,
                    end: 2.,
                }],
                10.,
                sdo_gameplay::judgment::find("keyboard").unwrap(),
            ),
            input: InputState::default(),
            display: NoteDisplay::default(),
            // Keep audio paused; seeking drives the real keyboard path deterministically.
            audio: AudioPlayer::new(
                DecodedSong {
                    samples: vec![0.; 48_000 * 2 * 10],
                    channels: 2,
                    sample_rate: 48_000,
                },
                1.,
            )?,
            title: "Score persistence test".into(),
            dance: DancePlayer::new(None, None, None),
            failed_at: None,
            dance_errors: Vec::new(),
        })
    };
    app.session = Some(session()?);
    app.keyboard(KeyCode::KeyD, true, false)?;
    assert!(!score_path.exists(), "The hold has not been released yet");
    app.session.as_mut().unwrap().audio.seek(2.)?;
    app.keyboard(KeyCode::KeyD, false, false)?;
    let played = app.session.as_ref().unwrap();
    assert_eq!(played.score_save, ScoreSaveState::Saved);
    assert!(!played.practice.finished());
    assert!(played.audio.position() < played.audio.duration());
    assert_eq!(
        ScoreStore::load(score_path.clone())
            .book
            .best(&key)
            .unwrap()
            .points,
        68
    );
    app.record_completed_score(false);
    app.record_completed_score(false);
    app.keyboard(KeyCode::Escape, true, false)?;
    assert!(app.session.is_none());
    assert_eq!(app.selector.scores.best(&key).unwrap().points, 68);
    assert_eq!(ScoreStore::load(score_path.clone()).book.scores.len(), 1);

    app.session = Some(session()?);
    app.keyboard(KeyCode::KeyD, true, false)?;
    app.keyboard(KeyCode::Escape, true, false)?;
    assert_eq!(
        app.scores.book.scores.len(),
        1,
        "Abandoning an unfinished hold adds no score"
    );

    // A failed write must remain retryable, without claiming the score was saved.
    let backup = temp.join("score-backup.json");
    std::fs::rename(&score_path, &backup)?;
    std::fs::create_dir(&score_path)?;
    app.session = Some(session()?);
    app.keyboard(KeyCode::KeyD, true, false)?;
    app.session.as_mut().unwrap().audio.seek(2.)?;
    app.keyboard(KeyCode::KeyD, false, false)?;
    assert!(matches!(
        app.session.as_ref().unwrap().score_save,
        ScoreSaveState::Failed(_)
    ));
    assert!(app.selector.error.is_some());
    assert_eq!(app.scores.book.scores.len(), 1);
    std::fs::remove_dir(&score_path)?;
    std::fs::rename(&backup, &score_path)?;
    app.record_completed_score(false);
    assert_eq!(
        app.scores.book.scores.len(),
        1,
        "Frames must not repeatedly retry a failed write"
    );
    app.keyboard(KeyCode::Escape, true, false)?;
    assert!(app.selector.error.is_none());
    assert_eq!(app.selector.scores.scores.len(), 2);
    assert_eq!(*ScoreStore::load(score_path).book, *app.scores.book);
    drop(app);
    std::fs::remove_dir_all(temp)?;
    Ok(())
}

#[test]
#[ignore = "Requires local assets and a Metal/wgpu device"]
fn settings_scores_and_missed_holds_survive_restart_and_render() -> Result<()> {
    use sdo_assets::{CharacterSlot, ClothingCategory};
    use sdo_gameplay::{Lane, Note};
    use sdo_ui::WardrobeChange;
    let temp = std::env::temp_dir().join(format!("sdo-persistence-render-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp);
    std::fs::create_dir_all(&temp)?;
    let settings_path = temp.join("settings.json");
    let score_path = temp.join("score.json");
    let open = || -> Result<App> {
        let paths = CollectionPaths::from_env()?;
        let catalog = Catalog::load(&paths)?;
        let assets = sdo_assets::curated::ui(&paths)?;
        collection_app(
            paths,
            Options::default(),
            catalog,
            assets,
            None,
            SettingsStore::load(settings_path.clone()),
            ScoreStore::load(score_path.clone()),
        )
    };
    let mut app = open()?;
    assert_eq!(app.selector.selected().unwrap().id, "00001");
    app.equip_character(&WardrobeChange::Set {
        slot: CharacterSlot::Shoes,
        item: Some("000130_MAN_SHOES_.MSH".into()),
    })?;
    app.selector.character = "female".into();
    app.equip_character(&WardrobeChange::Set {
        slot: CharacterSlot::Shoes,
        item: Some("000115_WOMAN_SHOES.MSH".into()),
    })?;
    app.selector.query = "00041".into();
    app.selector.refilter();
    app.selector.difficulty = 2;
    app.selector.sort = sdo_ui::SongSort::Level;
    app.selector.sort_descending = true;
    app.selector.judgment = "pad".into();
    app.selector.scroll_speed = ScrollSpeed::new(5)?;
    app.selector.timing_offset_ms = -42.;
    app.selector.appearance =
        app.content
            .request(asset::Ui)?
            .selection("paw_print_1", None, Some("none"), None)?;
    app.sync_appearance()?;
    app.selector.character_yaw = 0.3;
    app.selector.character_zoom = 1.2;
    app.selector
        .wardrobe
        .browse(ClothingCategory::Shoes, "000115");
    let expected = app.settings.capture(&app.selector, &app.characters);
    app.sync_settings(true);
    assert!(app.selector.error.is_none(), "{:?}", app.selector.error);
    let key = ScoreKey::new(
        "00041",
        "k",
        2,
        "pad",
        &app.selector
            .catalog
            .select("00041", None, "k", 4)?
            .pattern
            .sha256,
    );
    let mut attempt = Practice::from_notes(
        vec![Note {
            lane: Lane::Left,
            start: 1.,
            end: 2.,
        }],
        3.,
        sdo_gameplay::judgment::find("pad").unwrap(),
    );
    attempt.advance_to(1., [false; 4], false);
    attempt.press(Lane::Left);
    attempt.advance_to(2., [true, false, false, false], false);
    attempt.release(Lane::Left);
    attempt.advance_to(3., [false; 4], false);
    assert!(app.scores.record(key.clone(), &attempt, true, false)?);
    drop(app);
    let mut app = open()?;
    assert!(app.selector.error.is_none(), "{:?}", app.selector.error);
    assert_eq!(
        app.settings.capture(&app.selector, &app.characters),
        expected
    );
    assert_eq!(app.scores.book.best(&key).unwrap().points, 68);
    assert_eq!(
        app.characters.preset("male")?.equipment[&CharacterSlot::Shoes],
        "000130_MAN_SHOES.MSH"
    );
    assert_eq!(
        app.characters.preset("female")?.equipment[&CharacterSlot::Shoes],
        "000115_WOMAN_SHOES.MSH"
    );

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let (_, device, queue, _) = pollster::block_on(sdo_render::request_device(&instance, None))?;
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut renderer = Renderer::new(device, queue, format, &app.skin);
    let mut ui = SelectorUi::new(&renderer.device, format, None);
    let target = renderer.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Persistent score selector"),
        size: wgpu::Extent3d {
            width: 1100,
            height: 760,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    for _ in 0..3 {
        ui.draw(
            &renderer.device,
            &renderer.queue,
            &target.create_view(&Default::default()),
            [1100, 760],
            None,
            &mut app.selector,
            "Preview",
            (0., 0.),
        );
    }
    let artifacts = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../artifacts");
    renderer.save_texture(
        &artifacts.join("scores-selection.png"),
        &target,
        [1100, 760],
    )?;

    // Same real noteskin: missed head, early drop, currently held, upcoming hold.
    let mut practice = Practice::from_notes(
        Lane::ALL
            .into_iter()
            .map(|lane| Note {
                lane,
                start: if lane == Lane::Right { 3. } else { 1. },
                end: 4.,
            })
            .collect(),
        6.,
        sdo_gameplay::judgment::find("keyboard").unwrap(),
    );
    practice.advance_to(1., [false; 4], false);
    practice.press(Lane::Down);
    practice.press(Lane::Up);
    practice.advance_to(1.5, [false, true, true, false], false);
    practice.release(Lane::Down);
    practice.advance_to(2., [false, false, true, false], false);
    let mut input = InputState::default();
    input.update(Binding::J, true, 1.);
    let mut scene = NoteDisplay::default().scene(&app.skin, &practice, &input);
    for lane in [0, 1] {
        assert!(scene.sprites.iter().any(|sprite| {
            app.skin.lanes[lane].hold_body.contains(&sprite.texture)
                && sprite.color == [0.35, 0.35, 0.35, 1.]
        }));
    }
    sdo_ui::notes::personal_best(
        &mut scene,
        &app.skin,
        app.scores.book.best(&key).map(|score| score.points),
    );
    renderer.screenshot(
        &artifacts.join("missed-holds-and-personal-best.png"),
        [960, 720],
        &scene,
    )?;
    let mut completed = NoteDisplay::default().scene(&app.skin, &attempt, &InputState::default());
    sdo_ui::notes::personal_best(&mut completed, &app.skin, Some(attempt.score().points));
    sdo_ui::notes::score_save_status(&mut completed, &app.skin, true);
    renderer.screenshot(&artifacts.join("score-saved.png"), [960, 720], &completed)?;
    app.equip_character(&WardrobeChange::Reset)?;
    app.sync_settings(true);
    drop(app);
    let reset = open()?;
    assert!(!reset.characters.outfit_overrides().contains_key("female"));
    assert!(reset.characters.outfit_overrides().contains_key("male"));
    std::fs::remove_dir_all(temp)?;
    Ok(())
}
#[test]
#[ignore = "Requires local wardrobe assets and a Metal/wgpu device"]
fn wardrobe_equipment_updates_registered_preview_and_animated_models() -> Result<()> {
    use sdo_assets::CharacterSlot;
    use sdo_ui::WardrobeChange;
    let paths = CollectionPaths::from_env()?;
    let assets = sdo_assets::curated::ui(&paths)?;
    let catalog = Catalog::load(&paths)?;
    let mut app = collection_app(
        paths,
        Options::default(),
        catalog,
        assets,
        None,
        SettingsStore::default(),
        ScoreStore::default(),
    )?;
    app.selector.character_view = true;
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let (_, device, queue, _) = pollster::block_on(sdo_render::request_device(&instance, None))?;
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let renderer = Renderer::new(device, queue, format, &app.skin);
    let mut character = CharacterRenderer::new(&renderer.device, &renderer.queue, &app.characters);
    let mut ui = SelectorUi::new(&renderer.device, format, None);
    let mut thumbnails =
        WardrobeThumbnails::new(&renderer.device, &renderer.queue, app.content.clone());
    app.selector.character_texture =
        Some(ui.register_character_preview(&renderer.device, &character.ui_view));
    let target = renderer.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Wardrobe screenshot"),
        size: wgpu::Extent3d {
            width: 1100,
            height: 760,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let folder = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../artifacts");
    let capture = |label: &str, character: &CharacterRenderer| -> Result<image::RgbaImage> {
        let path = folder.join(format!("wardrobe-{label}.png"));
        renderer.save_texture(
            &path,
            &character.texture,
            sdo_render::character::PREVIEW_SIZE,
        )?;
        Ok(image::open(path)?.into_rgba8())
    };
    for id in ["male", "female"] {
        app.selector.character = id.into();
        character.render(
            &renderer.device,
            &renderer.queue,
            id,
            CharacterCamera::default(),
        )?;
        let before = capture(&format!("{id}-before"), &character)?;
        let changes = if id == "male" {
            vec![
                (CharacterSlot::Hair, "000001_MAN_HAIR.MSH"),
                (CharacterSlot::Top, "000044_MAN_COAT.MSH"),
                (CharacterSlot::Pants, "000087_MAN_PANT.MSH"),
                // This used to equip the partial, ankle-less shoe mesh.
                (CharacterSlot::Shoes, "000130_MAN_SHOES_.MSH"),
            ]
        } else {
            vec![
                (CharacterSlot::Hair, "000025_WOMAN_HAIR.MSH"),
                (CharacterSlot::Outfit, "000258_WOMAN_ONE.MSH"),
                (CharacterSlot::Shoes, "000115_WOMAN_SHOES_.MSH"),
            ]
        };
        for (slot, item) in changes {
            app.equip_character(&WardrobeChange::Set {
                slot,
                item: Some(item.into()),
            })?;
            assert_eq!(
                app.characters
                    .preset(id)?
                    .equipment
                    .get(&slot)
                    .map(String::as_str),
                Some(app.characters.wardrobe.item(item)?.id.as_str())
            );
        }
        if id == "male" {
            let shoes = app
                .characters
                .preset(id)?
                .parts
                .iter()
                .find(|part| part.source.ends_with("000130_MAN_SHOES.MSH"))
                .expect("The primary shoe mesh must be equipped, including its ankle geometry");
            assert_eq!(
                shoes
                    .surfaces
                    .iter()
                    .map(|surface| surface.vertices.len())
                    .sum::<usize>(),
                232
            );
            assert!(
                shoes
                    .surfaces
                    .iter()
                    .any(|surface| surface.vertices.len() == 70)
            );
            assert!(
                !app.characters
                    .preset(id)?
                    .parts
                    .iter()
                    .any(|part| part.source.ends_with("000130_MAN_SHOES_.MSH"))
            );
        }
        character.sync_assets(&renderer.device, &renderer.queue, &app.characters);
        character.render(
            &renderer.device,
            &renderer.queue,
            id,
            CharacterCamera::default(),
        )?;
        let after = capture(&format!("{id}-equipped"), &character)?;
        assert_ne!(
            before, after,
            "Equipping clothes did not change the rendered model"
        );
        app.selector.wardrobe.browse(
            sdo_assets::ClothingCategory::Shoes,
            if id == "male" { "00013" } else { "00011" },
        );
        let shoe_id = if id == "male" {
            "000130_MAN_SHOES.MSH"
        } else {
            "000115_WOMAN_SHOES.MSH"
        };
        let key = sdo_assets::ClothingPreviewKey {
            preset: id.into(),
            item: shoe_id.into(),
        };
        let preview = app.content.request(asset::ClothingPreview(&key))?;
        assert!(
            preview.presets[0].parts[0]
                .source
                .file_stem()
                .unwrap()
                .to_str()
                .unwrap()
                .ends_with('_')
        );
        if id == "male" {
            assert_eq!(
                preview.presets[0].parts[0]
                    .surfaces
                    .iter()
                    .map(|s| s.vertices.len())
                    .sum::<usize>(),
                162
            );
        }
        let equipment_before_previews = app.characters.preset(id)?.equipment.clone();
        // Existing character texture stays usable while thumbnails load separately.
        let started = Instant::now();
        let mut passes = 0;
        loop {
            ui.draw(
                &renderer.device,
                &renderer.queue,
                &target.create_view(&Default::default()),
                [1100, 760],
                None,
                &mut app.selector,
                "",
                (0., 0.),
            );
            passes += 1;
            let ready = thumbnails.is_ready(&app.selector.wardrobe);
            thumbnails.update(
                &renderer.device,
                &renderer.queue,
                &mut ui,
                &mut app.selector.wardrobe,
                true,
            );
            if passes >= 3 && ready {
                break;
            }
            ensure!(
                started.elapsed().as_secs() < 20,
                "Timed out loading item previews"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(matches!(
            app.selector.wardrobe.thumbnail(&key),
            Some(sdo_ui::ClothingThumbnail::Ready(_))
        ));
        assert_eq!(
            app.selector
                .wardrobe
                .visible_previews()
                .iter()
                .filter(|k| k.item == shoe_id)
                .count(),
            1
        );
        assert_eq!(
            app.characters.preset(id)?.equipment,
            equipment_before_previews
        );
        renderer.save_texture(
            &folder.join(format!("wardrobe-{id}.png")),
            &target,
            [1100, 760],
        )?;
        let preset = app.characters.preset(id)?;
        let motion = app
            .content
            .request(asset::Motion(&preset.idle.as_ref().unwrap().motion))?;
        let globals = motion.pose(&preset.skeleton, 10.)?;
        character.render_pose(
            &renderer.device,
            &renderer.queue,
            id,
            CharacterCamera::default(),
            Some(&globals),
        )?;
        assert_ne!(
            after,
            capture(&format!("{id}-animated"), &character)?,
            "Equipped model did not animate"
        );
        // The original equipped-avatar path uses the primary shoe with both
        // separates and full outfits. Exercise actual assets in each context.
        let shoe_id = app.characters.preset(id)?.equipment[&CharacterSlot::Shoes].clone();
        let combinations = if id == "male" {
            vec![
                (CharacterSlot::Pants, "000001_MAN_PANT.MSH", "trousers"),
                (CharacterSlot::Outfit, "000730_MAN_ONE.MSH", "outfit"),
                (CharacterSlot::Pants, "000087_MAN_PANT.MSH", "shorts"),
            ]
        } else {
            vec![
                (CharacterSlot::Pants, "000111_WOMAN_PANT.MSH", "pants"),
                (CharacterSlot::Outfit, "000258_WOMAN_ONE.MSH", "dress"),
            ]
        };
        for (slot, item, label) in combinations {
            app.equip_character(&WardrobeChange::Set {
                slot,
                item: Some(item.into()),
            })?;
            let preset = app.characters.preset(id)?;
            assert_eq!(preset.equipment[&CharacterSlot::Shoes], shoe_id);
            assert!(
                preset
                    .parts
                    .iter()
                    .any(|part| part.source.ends_with(&shoe_id))
            );
            if slot == CharacterSlot::Outfit {
                assert!(!preset.equipment.contains_key(&CharacterSlot::Pants));
                assert!(!preset.equipment.contains_key(&CharacterSlot::Top));
            } else {
                assert!(!preset.equipment.contains_key(&CharacterSlot::Outfit));
            }
            character.sync_assets(&renderer.device, &renderer.queue, &app.characters);
            character.render(
                &renderer.device,
                &renderer.queue,
                id,
                CharacterCamera::default(),
            )?;
            let pixels = capture(&format!("{id}-{label}"), &character)?;
            assert_ne!(before, pixels);
            if label == "shorts" || label == "dress" {
                assert_eq!(
                    after, pixels,
                    "Switching back must restore the entire outfit exactly"
                );
            }
        }
        let equipment = app.characters.preset(id)?.equipment.clone();
        let error = app.equip_character(&WardrobeChange::Set {
            slot: CharacterSlot::Top,
            item: Some("000004_MAN_COAT.MSH".into()),
        });
        assert!(error.is_err());
        assert_eq!(app.characters.preset(id)?.equipment, equipment);
        app.equip_character(&WardrobeChange::Reset)?;
        character.sync_assets(&renderer.device, &renderer.queue, &app.characters);
        character.render(
            &renderer.device,
            &renderer.queue,
            id,
            CharacterCamera::default(),
        )?;
        assert_eq!(
            before,
            capture(&format!("{id}-restored"), &character)?,
            "Reset did not restore the original model"
        );
    }
    println!(
        "PASS: male/female outfits equip, render through the existing UI texture, animate, retain complete shoes across pants/dress changes, reject invalid items, and reset exactly"
    );
    Ok(())
}
#[test]
#[ignore = "Requires local character/dance assets and a Metal/wgpu device"]
fn song_intros_animate_rest_before_choreography_on_both_characters() -> Result<()> {
    let paths = CollectionPaths::from_env()?;
    let catalog = Catalog::load(&paths)?;
    for song in &catalog.songs {
        for pattern in &song.patterns {
            assert!(
                pattern.level_error.is_none(),
                "{}: {:?}",
                pattern.file,
                pattern.level_error
            );
        }
    }
    assert_eq!(
        catalog.select("00001", None, "k", 4)?.pattern.levels,
        [Some(3), Some(4), Some(5)]
    );
    assert_eq!(
        catalog.select("00001", None, "t", 4)?.pattern.levels,
        [Some(1), Some(3), Some(5)]
    );
    let characters = sdo_assets::curated::characters(&paths)?;
    let assets = sdo_assets::curated::ui(&paths)?;
    let skin = assets.compose(&assets.selection("default", None, Some("none"), None)?)?;
    let mut library = collection_dances(&paths);
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let (_, device, queue, _) = pollster::block_on(sdo_render::request_device(&instance, None))?;
    let mut renderer = Renderer::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb, &skin);
    let mut dancer = CharacterRenderer::new(&renderer.device, &renderer.queue, &characters);
    let texture = renderer.register_external_texture(&dancer.view);
    let artifacts = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../artifacts");
    for id in ["00001", "00041", "01518"] {
        let selected = catalog.select(id, None, "k", 4)?;
        let files = selected.resolve(&paths)?;
        let chart = Chart::load_with_bpm(&files.pattern, Difficulty::Easy, selected.bpm())?;
        for preset in &characters.presets {
            let dance = library.load(
                &std::fs::read(sdo_assets::curated::resolve_resource(
                    paths.assets(),
                    selected.song.dance_moves_paths.as_deref().unwrap(),
                )?)?,
                &chart.raw,
                0,
                preset.skeleton.bones.len(),
            )?;
            let start = dance.starts_at().context("No choreography")?;
            assert!(start > 0.35, "Fixture needs an introductory rest interval");
            let live = dance.sample(start + 0.25, &preset.skeleton)?.unwrap();
            let definition = preset.idle.as_ref().context("Missing rest animation")?;
            let idle = library.load_reaction(definition, preset.skeleton.bones.len())?;
            let miss = preset
                .miss
                .as_ref()
                .map(|definition| library.load_reaction(definition, preset.skeleton.bones.len()))
                .transpose()?;
            let mut player = DancePlayer::new(Some(dance), Some(idle), miss);
            let mut first = None;
            let mut practice = Practice::from_notes(
                chart.notes.clone(),
                selected.song.duration,
                sdo_gameplay::judgment::find("keyboard").unwrap(),
            )
            .with_bpm(chart.bpm)
            .with_tempo(chart.tempo.clone());
            for (index, time) in [0., 0.35].into_iter().enumerate() {
                let pose = player.sample(time, true, &preset.skeleton)?.unwrap();
                assert_eq!(pose.motion, definition.motion);
                assert!(player.waiting_for_choreography(time));
                if let Some(previous) = &first {
                    assert_ne!(
                        &pose.globals, previous,
                        "Intro froze for {id}/{}",
                        preset.choice.id
                    );
                } else {
                    first = Some(pose.globals.clone());
                }
                dancer.render_pose(
                    &renderer.device,
                    &renderer.queue,
                    &preset.choice.id,
                    CharacterCamera::default(),
                    Some(&pose.globals),
                )?;
                practice.advance_to(time, [false; 4], true);
                let mut scene =
                    NoteDisplay::default().scene(&skin, &practice, &InputState::default());
                scene.push(Sprite::new(texture, [7., 180., 320., 360.]));
                sdo_ui::notes::dance_status(&mut scene, &skin, "GET READY", false);
                renderer.screenshot(
                    &artifacts.join(format!("intro-{id}-{}-{index}.png", preset.choice.id)),
                    [960, 720],
                    &scene,
                )?;
            }
            let rest = player
                .sample(start - 0.05, true, &preset.skeleton)?
                .unwrap();
            assert_eq!(
                player
                    .sample(start, true, &preset.skeleton)?
                    .unwrap()
                    .globals,
                rest.globals
            );
            let playing = player
                .sample(start + 0.25, true, &preset.skeleton)?
                .unwrap();
            assert_eq!(playing.globals, live.globals);
            assert_eq!(playing.motion, live.motion);
            player.reset();
            assert_eq!(
                player.sample(0., true, &preset.skeleton)?.unwrap().motion,
                definition.motion
            );
        }
    }
    println!(
        "PASS: all catalog headers read; K/T levels verified; male and female rest animate through three song intros and blend into live choreography"
    );
    Ok(())
}

#[test]
#[ignore = "Requires local character/dance assets and a Metal/wgpu device"]
fn miss_idle_and_recovery_render_on_both_presets() -> Result<()> {
    use sdo_gameplay::{Lane, Note};
    let paths = CollectionPaths::from_env()?;
    let catalog = Catalog::load(&paths)?;
    let chars = sdo_assets::curated::characters(&paths)?;
    let ui = sdo_assets::curated::ui(&paths)?;
    let skin = ui.compose(&ui.selection("default", None, None, None)?)?;
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let (_, device, queue, _) = pollster::block_on(sdo_render::request_device(&instance, None))?;
    let renderer = Renderer::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb, &skin);
    let mut character = CharacterRenderer::new(&renderer.device, &renderer.queue, &chars);
    let folder = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../artifacts");
    let mut library = collection_dances(&paths);
    let mut rows = Vec::new();
    // Exercise a current song and a song repaired from legacy choreography.
    for song in ["01518", "00011"] {
        let selected = catalog.select(song, None, "k", 4)?;
        let files = selected.resolve(&paths)?;
        let gn = std::fs::read(&files.pattern)?;
        let entry = sdo_assets::curated::resolve_resource(
            paths.assets(),
            selected.song.dance_moves_paths.as_deref().unwrap(),
        )?;
        for preset in &chars.presets {
            let dance =
                library.load(&std::fs::read(&entry)?, &gn, 0, preset.skeleton.bones.len())?;
            let start = if song == "01518" {
                8.
            } else {
                let mut found = None;
                for group in &dance.groups {
                    let mut offset = 0.;
                    for clip in &group.clips {
                        if clip.motion.is_some() && clip.seconds > 4. {
                            found = Some(group.start + offset + 0.1);
                            break;
                        }
                        offset += clip.seconds;
                    }
                    if found.is_some() {
                        break;
                    }
                }
                found.context("No long choreography clip")?
            };
            let expected = dance.sample(start + 6.3, &preset.skeleton)?.unwrap();
            let definition = preset.idle.as_ref().context("Missing preset idle")?;
            assert_eq!(
                definition.motion,
                if preset.choice.id == "male" {
                    "MALE_REST.MOT"
                } else {
                    "FEMALE_REST.MOT"
                }
            );
            let idle_source = library.motion(&definition.motion)?;
            let miss_definition = preset
                .miss
                .as_ref()
                .context("Missing preset miss reaction")?;
            assert_eq!(
                miss_definition.motion,
                if preset.choice.id == "male" {
                    "MALE_MISS.MOT"
                } else {
                    "FEMALE_MISS.MOT"
                }
            );
            let miss_source = library.motion(&miss_definition.motion)?;
            let miss_seconds =
                f64::from(miss_source.last_frame) / f64::from(miss_definition.frames_per_second);
            let rest_start = start + 0.5 + miss_seconds;
            let rest_seconds =
                f64::from(idle_source.last_frame) / f64::from(definition.frames_per_second);
            let mut player = DancePlayer::new(
                Some(dance),
                Some(library.load_reaction(definition, preset.skeleton.bones.len())?),
                Some(library.load_reaction(miss_definition, preset.skeleton.bones.len())?),
            );
            let mut practice = Practice::from_notes(
                vec![
                    Note {
                        lane: Lane::Left,
                        start: start + 0.1,
                        end: start + 0.1,
                    },
                    Note {
                        lane: Lane::Left,
                        start: start + 1.,
                        end: start + 1.,
                    },
                    Note {
                        lane: Lane::Up,
                        start: start + 6.,
                        end: start + 7.,
                    },
                ],
                start + 10.,
                sdo_gameplay::judgment::find("keyboard").unwrap(),
            );
            practice.seek_preview(start);
            let mut pictures = Vec::new();
            let mut previous = None;
            for (label, time, hit, release) in [
                ("dance", start, false, false),
                ("miss", start + 0.5, false, false),
                ("transition", start + 0.6, false, false),
                ("miss-a", start + 0.8, false, false),
                ("miss-b", start + 1.5, false, false),
                ("rest-transition", rest_start, false, false),
                ("idle-a", rest_start + 0.3, false, false),
                ("idle-b", rest_start + 0.8, false, false),
                ("idle-loop", rest_start + rest_seconds + 0.25, false, false),
                ("recover", start + 6., true, false),
                ("recovered", start + 6.3, false, false),
                ("hold-miss", start + 6.4, false, true),
                ("hold-reaction", start + 6.7, false, false),
                ("hold-idle", start + 6.4 + miss_seconds + 0.3, false, false),
            ] {
                practice.advance_to(time, [false, false, true, false], false);
                if hit {
                    practice.press(Lane::Up);
                }
                if release {
                    practice.release(Lane::Up);
                }
                let pose = player
                    .sample(time, practice.dancing(), &preset.skeleton)?
                    .unwrap();
                if matches!(label, "miss" | "recover" | "hold-miss") {
                    assert_eq!(
                        Some(&pose.globals),
                        previous.as_ref(),
                        "State change snapped pose"
                    );
                }
                if label.starts_with("idle") || label == "hold-idle" {
                    assert_eq!(pose.motion, definition.motion);
                    assert!(!practice.dancing());
                    assert_eq!(
                        pose.globals,
                        idle_source.pose(&preset.skeleton, pose.frame)?,
                        "Settled idle differs from its original MOT"
                    );
                }
                if matches!(label, "miss-a" | "miss-b" | "hold-reaction") {
                    assert_eq!(pose.motion, miss_definition.motion);
                    assert_eq!(
                        pose.globals,
                        miss_source.pose(&preset.skeleton, pose.frame)?
                    );
                }
                if label == "miss-b" {
                    assert!(
                        (pose.frame - 30.).abs() < 0.001,
                        "Repeated miss restarted the clip"
                    );
                }
                if label == "idle-b" {
                    assert!((pose.frame - 24.).abs() < 0.001);
                }
                if label == "idle-loop" {
                    assert!((pose.frame - 7.5).abs() < 0.001);
                    practice.set_paused(true);
                    practice.advance_to(time + 0.1, [false; 4], false);
                    assert_eq!(
                        pose.globals,
                        player
                            .sample(time, practice.dancing(), &preset.skeleton)?
                            .unwrap()
                            .globals
                    );
                    practice.set_paused(false);
                }
                if label == "recovered" {
                    assert!(practice.dancing());
                    assert_eq!(pose.globals, expected.globals);
                    assert_eq!(pose.frame, expected.frame);
                }
                // Native MOT translations can differ from the neutral mesh (notably toes).
                // Test rigid transforms, not equality with that unrelated bind pose.
                // The synthetic rotated-joint test verifies limb length through blending.
                for (i, bone) in preset.skeleton.bones.iter().enumerate() {
                    let transform = pose.globals[i];
                    assert!(transform.is_finite());
                    assert!(
                        (transform.determinant() - 1.).abs() < 0.001,
                        "{song} {} {label}: collapsed/scaled {}",
                        preset.choice.id,
                        bone.name
                    );
                }
                character.render_pose(
                    &renderer.device,
                    &renderer.queue,
                    &preset.choice.id,
                    CharacterCamera::default(),
                    Some(&pose.globals),
                )?;
                let path = folder.join(format!("idle-{song}-{}-{label}.png", preset.choice.id));
                renderer.save_texture(
                    &path,
                    &character.texture,
                    sdo_render::character::PREVIEW_SIZE,
                )?;
                let pixels = image::open(path)?.into_rgba8();
                let bg = pixels.get_pixel(0, 0);
                assert!(pixels.pixels().filter(|p| *p != bg).count() > 5000);
                pictures.push(pixels);
                previous = Some(pose.globals);
            }
            assert_ne!(pictures[3], pictures[4], "Miss reaction did not animate");
            assert_ne!(pictures[6], pictures[7], "Idle stopped animating");
            let [w, h] = sdo_render::character::PREVIEW_SIZE;
            let mut strip = image::RgbaImage::new(w * 6, h);
            for (column, index) in [0, 3, 4, 6, 7, 10].into_iter().enumerate() {
                image::imageops::replace(
                    &mut strip,
                    &pictures[index],
                    i64::from(w) * column as i64,
                    0,
                );
            }
            strip.save(folder.join(format!("idle-{song}-{}-sequence.png", preset.choice.id)))?;
            rows.push(serde_json::json!({
                "song": song, "preset": preset.choice.id, "idle": definition.motion,
                "miss": miss_definition.motion, "miss_seconds": miss_seconds,
                "crossfade_seconds": 0.2, "idle_frames_per_second": definition.frames_per_second,
                "miss_idle_loop_hold_recovery_pause_proportions": "passed",
            }));
        }
    }
    std::fs::write(
        folder.join("idle-playback-validation.json"),
        serde_json::to_vec_pretty(&rows)?,
    )?;
    println!(
        "PASS: both presets transition dance/miss/rest/dance without stretching; one-shot reactions, repeated misses, idle loops, hold releases, pause and live-time recovery validated"
    );
    Ok(())
}
#[test]
#[ignore = "Requires the local dance assets and a Metal/wgpu device"]
fn song_dance_uses_audio_time_and_restores_each_presets_preview() -> Result<()> {
    let paths = CollectionPaths::from_env()?;
    let catalog = Catalog::load(&paths)?;
    let selected = catalog.select("01518", None, "k", 4)?;
    let files = selected.resolve(&paths)?;
    let chart = Chart::load_with_bpm(&files.pattern, Difficulty::Easy, selected.bpm())?;
    let mut library = collection_dances(&paths);
    let (dance, issues) = load_song_dance(
        &mut library,
        &paths,
        selected.song,
        &chart,
        Difficulty::Easy,
    );
    assert!(issues.is_empty());
    let dance = dance.unwrap();
    assert_eq!(dance.unit_ms, 391);
    let chars = sdo_assets::curated::characters(&paths)?;
    let ui = sdo_assets::curated::ui(&paths)?;
    let skin = ui.compose(&ui.selection("default", None, None, None)?)?;
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let (_, device, queue, _) = pollster::block_on(sdo_render::request_device(&instance, None))?;
    let renderer = Renderer::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb, &skin);
    let mut character = CharacterRenderer::new(&renderer.device, &renderer.queue, &chars);
    let folder = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../artifacts");
    let capture = |name: &str, c: &CharacterRenderer| -> Result<image::RgbaImage> {
        let p = folder.join(format!("dance-test-{name}.png"));
        renderer.save_texture(&p, &c.texture, sdo_render::character::PREVIEW_SIZE)?;
        Ok(image::open(p)?.into_rgba8())
    };
    character.render(
        &renderer.device,
        &renderer.queue,
        "female",
        CharacterCamera::default(),
    )?;
    let bind = capture("bind-female", &character)?;
    for id in ["male", "female"] {
        let preset = chars.preset(id)?;
        let mut images = Vec::new();
        for time in [8., 9., 8.] {
            let pose = dance.sample(time, &preset.skeleton)?.unwrap();
            assert!(
                pose.globals[51].w_axis.y > pose.globals[5].w_axis.y,
                "Head/foot orientation inverted"
            );
            character.render_pose(
                &renderer.device,
                &renderer.queue,
                id,
                CharacterCamera::default(),
                Some(&pose.globals),
            )?;
            images.push(capture(&format!("{id}-{time}"), &character)?);
        }
        assert_ne!(images[0], images[1], "Character did not animate");
        assert_eq!(
            images[0], images[2],
            "Seeking/restarting to the same audio time changed the pose"
        );
    }
    character.render(
        &renderer.device,
        &renderer.queue,
        "male",
        CharacterCamera::default(),
    )?;
    character.render(
        &renderer.device,
        &renderer.queue,
        "female",
        CharacterCamera::default(),
    )?;
    assert_eq!(
        bind,
        capture("restored-female", &character)?,
        "Returning to preview did not restore bind pose"
    );
    println!(
        "PASS: both presets animate; absolute audio-time samples are repeatable; preview pose restores after preset changes"
    );
    Ok(())
}
#[test]
#[ignore = "Requires the local character presets and a Metal/wgpu device"]
fn character_presets_and_camera_render_distinct_nonempty_models() -> Result<()> {
    let paths = CollectionPaths::from_env()?;
    let assets = sdo_assets::curated::ui(&paths)?;
    let skin = assets.compose(&assets.selection("default", None, None, None)?)?;
    let characters = sdo_assets::curated::characters(&paths)?;
    ensure!(characters.issues.is_empty(), "{:?}", characters.issues);
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let (_, device, queue, _) = pollster::block_on(sdo_render::request_device(&instance, None))?;
    let renderer = Renderer::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb, &skin);
    let mut character = CharacterRenderer::new(&renderer.device, &renderer.queue, &characters);
    let mut pictures = Vec::new();
    for (id, yaw) in [
        ("male", 0.0),
        ("female", 0.0),
        ("female", 1.1),
        ("male", std::f32::consts::PI),
        ("female", std::f32::consts::PI),
    ] {
        let camera = CharacterCamera {
            yaw,
            ..Default::default()
        };
        character.render(&renderer.device, &renderer.queue, id, camera)?;
        let portrait =
            renderer.thumbnail_rgba(&character.texture, sdo_render::character::PORTRAIT_SIZE)?;
        sdo_render::character::Portrait {
            avatar: Vec::new(),
            character: id.into(),
            rgba: portrait.clone(),
        }
        .validate()?;
        assert!(portrait.as_chunks::<4>().0.iter().any(|pixel| pixel[3] > 0));
        let out = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../../artifacts/character-gpu-{id}-{yaw}.png"));
        renderer.save_texture(
            &out,
            &character.texture,
            sdo_render::character::PREVIEW_SIZE,
        )?;
        let pixels = image::open(&out)?.into_rgba8();
        let background = pixels.get_pixel(0, 0);
        let foreground = pixels.pixels().filter(|p| *p != background).count();
        assert!(foreground > 5000, "Empty character render: {id}/{yaw}");
        pictures.push(pixels);
    }
    assert_ne!(
        pictures[0], pictures[1],
        "Preset switch did not change the model"
    );
    assert_ne!(
        pictures[1], pictures[2],
        "Camera rotation did not change the model"
    );
    println!(
        "PASS: male/female preset switching and camera rotation produced distinct, nonempty GPU renders"
    );
    Ok(())
}
#[test]
#[ignore = "Requires the local character preset and a Metal/wgpu device"]
fn mixed_alpha_surfaces_occlude_front_and_back_independent_of_triangle_order() -> Result<()> {
    use sdo_assets::{CharacterVertex, Texture, TextureId};
    use std::sync::Arc;
    let paths = CollectionPaths::from_env()?;
    let ui = sdo_assets::curated::ui(&paths)?;
    let skin = ui.compose(&ui.selection("default", None, None, None)?)?;
    let mut characters = sdo_assets::curated::characters(&paths)?;
    characters.presets.truncate(1);
    let preset = &mut characters.presets[0];
    let mut part = (*preset.parts[0]).clone();
    let mut surface = part.surfaces[0].clone();
    surface.texture = TextureId(0);
    surface.diffuse = [1.; 4];
    surface.vertices.clear();
    // Front red and back blue in one mesh. The unused green texel has
    // fractional alpha, reproducing the old whole-mesh classification.
    for (z, u) in [(-0.3, 0.125), (0.3, 0.375)] {
        for (x, y) in [(-1., -1.), (1., -1.), (1., 1.), (-1., 1.)] {
            surface.vertices.push(CharacterVertex {
                position: [x, y, z],
                normal: [0., 0., -1.],
                uv: [u, 0.5],
            });
        }
    }
    preset.bounds = [[-1., -1., -0.3], [1., 1., 0.3]];
    let id = preset.choice.id.clone();
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let (_, device, queue, _) = pollster::block_on(sdo_render::request_device(&instance, None))?;
    let renderer = Renderer::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb, &skin);
    for alpha in [255, 128] {
        characters.textures = Arc::new(vec![Texture {
            source: "depth-regression".into(),
            width: 4,
            height: 1,
            rgba: vec![255, 0, 0, alpha, 0, 0, 255, 255, 0, 255, 0, 128, 0, 0, 0, 0],
        }]);
        for yaw in [0., std::f32::consts::PI] {
            let mut centers = Vec::new();
            for reverse in [false, true] {
                surface.indices = if reverse {
                    vec![4, 5, 6, 4, 6, 7, 0, 1, 2, 0, 2, 3]
                } else {
                    vec![0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7]
                };
                part.surfaces = vec![surface.clone()];
                characters.presets[0].parts = vec![Arc::new(part.clone())];
                let mut character =
                    CharacterRenderer::new(&renderer.device, &renderer.queue, &characters);
                character.render(
                    &renderer.device,
                    &renderer.queue,
                    &id,
                    CharacterCamera {
                        yaw,
                        ..Default::default()
                    },
                )?;
                let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
                    "../../artifacts/depth-regression-{alpha}-{yaw}-{reverse}.png"
                ));
                renderer.save_texture(
                    &out,
                    &character.texture,
                    sdo_render::character::PREVIEW_SIZE,
                )?;
                let pixels = image::open(&out)?.into_rgba8();
                centers.push(*pixels.get_pixel(pixels.width() / 2, pixels.height() / 2));
            }
            assert_eq!(centers[0], centers[1], "Triangle order changed occlusion");
            let p = centers[0].0;
            if yaw != 0. {
                assert!(p[2] > 100 && p[0] < 5, "Back must occlude front: {p:?}");
            } else if alpha == 255 {
                assert!(p[0] > 100 && p[2] < 5, "Front must occlude back: {p:?}");
            } else {
                assert!(
                    p[0] > 50 && p[2] > 50,
                    "Partial alpha must still blend: {p:?}"
                );
            }
        }
    }
    Ok(())
}

#[test]
#[ignore = "Requires the local noteskins and a Metal/wgpu device"]
fn hold_caps_stay_visible_in_real_skins_through_the_end_of_the_hold() -> Result<()> {
    let paths = CollectionPaths::from_env()?;
    let assets = sdo_assets::curated::ui(&paths)?;
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    for name in ["retro_rainbow", "paw_print_1", "default"] {
        let skin = assets.compose(&assets.selection(name, None, Some("none"), None)?)?;
        let (_, device, queue, _) =
            pollster::block_on(sdo_render::request_device(&instance, None))?;
        let mut renderer = Renderer::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb, &skin);
        let mut scene = sdo_render::Scene::new(480., 800.);
        for (row, length) in [150., 80., 30., 1.].into_iter().enumerate() {
            let top = row as f32 * 200.;
            let head_y = top + 45.;
            for (lane, frames) in skin.lanes.iter().enumerate() {
                let x = 70. + lane as f32 * 110.;
                let receptor = skin.pressed_receptor(lane, 20);
                let image = skin.texture(receptor);
                scene.push(Sprite::new(
                    receptor,
                    [
                        x - image.width as f32 / 2.,
                        head_y - image.height as f32 / 2.,
                        image.width as f32,
                        image.height as f32,
                    ],
                ));
                let sprites = sdo_ui::notes::hold_sprites(
                    &skin.textures,
                    frames,
                    row,
                    [x, head_y, head_y + length],
                    1.,
                    [top, top + 190.],
                );
                let head = sprites.last().context("Hold head disappeared")?;
                assert_eq!(head.texture, frames.hold_head[row % frames.hold_head.len()]);
                let join = head_y;
                assert!(
                    sprites[..sprites.len() - 1]
                        .iter()
                        .all(|s| s.rect[1] >= join - 0.001)
                );
                scene.sprites.extend(sprites);
            }
        }
        let out = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../../artifacts/hold-caps-{name}.png"));
        renderer.screenshot(&out, [480, 800], &scene)?;
    }
    Ok(())
}

#[test]
#[ignore = "Requires the local effect collection and a Metal/wgpu device"]
fn render_hit_animation_and_validate_all_effect_choices() -> Result<()> {
    let paths = CollectionPaths::from_env()?;
    let assets = sdo_assets::curated::ui(&paths)?;
    let mut selection = assets.selection("001", None, None, None)?;
    let mut skin = assets.compose(&selection)?;
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let (_, device, queue, _) = pollster::block_on(sdo_render::request_device(&instance, None))?;
    let mut renderer = Renderer::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb, &skin);
    let target = renderer.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("UI component switching smoke test"),
        size: wgpu::Extent3d {
            width: 960,
            height: 720,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let display = NoteDisplay::default();
    let input = InputState::default();
    let notes = sdo_gameplay::Lane::ALL
        .into_iter()
        .flat_map(|lane| {
            [
                sdo_gameplay::Note {
                    lane,
                    start: 1.0,
                    end: 1.0,
                },
                sdo_gameplay::Note {
                    lane,
                    start: 2.0,
                    end: 3.0,
                },
            ]
        })
        .collect();
    let mut sample = Practice::from_notes(
        notes,
        4.0,
        sdo_gameplay::judgment::find("keyboard").unwrap(),
    );
    sample.seek_preview(0.5);
    for choice in &assets.skins {
        selection.skin = choice.choice.id.clone();
        skin = assets.compose(&selection)?;
        assert!(Arc::ptr_eq(&assets.textures, &skin.textures));
        renderer.sync_textures(&skin);
        renderer.draw(&view, [960, 720], &display.scene(&skin, &sample, &input))?;
    }
    sample.advance_to(1.05, [false; 4], true);
    let mut count = 0;
    for effect in &assets.effects {
        selection.effect = effect.choice.id.clone();
        skin = assets.compose(&selection)?;
        renderer.sync_textures(&skin);
        let animation = skin.hit_effect.as_ref().unwrap();
        assert!(animation.frame(0, 0.0).is_some());
        assert!(animation.frame(3, 0.0).is_some());
        renderer.draw(&view, [960, 720], &display.scene(&skin, &sample, &input))?;
        count += 1;
    }
    selection = assets.selection("001", None, Some("sparkle"), None)?;
    skin = assets.compose(&selection)?;
    // All four notes hit together with no held physical keys.
    let notes = sdo_gameplay::Lane::ALL
        .into_iter()
        .map(|lane| sdo_gameplay::Note {
            lane,
            start: 1.0,
            end: 1.0,
        })
        .collect();
    let mut practice = Practice::from_notes(
        notes,
        3.0,
        sdo_gameplay::judgment::find("keyboard").unwrap(),
    );
    let input = InputState::default();
    let display = NoteDisplay::default();
    for (index, time) in [(0, 1.001), (2, 1.075), (5, 1.175), (10, 1.342), (12, 1.45)] {
        practice.advance_to(time, [false; 4], true);
        let scene = display.scene(&skin, &practice, &input);
        let additive = scene
            .sprites
            .iter()
            .filter(|s| s.blend == sdo_render::BlendMode::Additive)
            .count();
        assert_eq!(additive, if index < 12 { 4 } else { 0 });
        renderer.screenshot(
            &PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join(format!("../../artifacts/key-hit-{index:02}.png")),
            [960, 720],
            &scene,
        )?;
    }
    println!(
        "PASS: {} skins and {count} effects switched in memory and rendered on GPU; four-lane animation frames verified",
        assets.skins.len()
    );
    Ok(())
}
#[test]
#[ignore = "Requires local songs, audio output, and a Metal/wgpu device"]
fn song_loading_progress_errors_holds_tempo_and_speed() -> Result<()> {
    let paths = CollectionPaths::from_env()?;
    let assets = sdo_assets::curated::ui(&paths)?;
    let catalog = Catalog::load(&paths)?;
    let mut app = collection_app(
        paths,
        Options::default(),
        catalog,
        assets,
        None,
        SettingsStore::default(),
        ScoreStore::default(),
    )?;
    assert_eq!(app.selector.selected().unwrap().id, "00001");
    let settings =
        std::env::temp_dir().join(format!("sdo-session-speed-{}.json", std::process::id()));
    app.settings = SettingsStore::load(settings.clone());
    app.selector.scroll_speed = ScrollSpeed::new(3)?;
    app.sync_settings(true);
    assert_eq!(sdo_config::load_scroll_speed(&settings)?.value(), 3);
    app.selector.query = "00041".into();
    app.selector.refilter();
    app.start_selected()?;
    assert!(app.session.is_none() && app.selector.loading.is_some());

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let (_, device, queue, _) = pollster::block_on(sdo_render::request_device(&instance, None))?;
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut ui = SelectorUi::new(&device, format, None);
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("loading and error screenshots"),
        size: wgpu::Extent3d {
            width: 1100,
            height: 760,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let renderer = Renderer::new(device, queue, format, &app.skin);
    let folder = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../artifacts");
    let mut capture = |selector: &mut SongSelector, name: &str| -> Result<()> {
        for _ in 0..20 {
            ui.draw(
                &renderer.device,
                &renderer.queue,
                &texture.create_view(&Default::default()),
                [1100, 760],
                None,
                selector,
                "",
                (0., 0.),
            );
        }
        renderer.save_texture(&folder.join(name), &texture, [1100, 760])
    };
    capture(&mut app.selector, "song-loading.png")?;
    // The worker can finish, but audio waits for a presented loading frame.
    let began = Instant::now();
    let mut previous = 0.;
    while app.prepared_song.is_none() {
        app.poll_loading();
        assert!(app.selector.error.is_none(), "{:?}", app.selector.error);
        let progress = app.selector.loading.as_ref().unwrap().progress;
        assert!(progress >= previous && progress <= 1.);
        previous = progress;
        assert!(began.elapsed() < Duration::from_secs(20));
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(app.session.is_none());
    assert_eq!(previous, 1.);
    let chart = &app.prepared_song.as_ref().unwrap().chart;
    assert!(chart.tempo.len() > 1);
    assert!((chart.tempo[1].at - 21.8181818).abs() < 0.001);
    assert!((chart.tempo[1].bpm - 137.7).abs() < 0.001);
    app.loading_frame_presented = true;
    app.poll_loading();
    assert!(app.selector.loading.is_none());
    assert_eq!(app.session.as_ref().unwrap().display.speed.value(), 3);
    app.keyboard(KeyCode::Equal, true, false)?;
    assert_eq!(app.session.as_ref().unwrap().display.speed.value(), 4);
    app.session.as_mut().unwrap().restart()?;
    assert_eq!(app.session.as_ref().unwrap().display.speed.value(), 4);
    assert_eq!(sdo_config::load_scroll_speed(&settings)?.value(), 4);
    app.return_to_selector()?;

    app.selector.query = "01518".into();
    app.selector.refilter();
    app.selector.difficulty = 2;
    app.start_selected()?;
    wait_for_session(&mut app);
    let session = app.session.as_ref().unwrap();
    assert_eq!(session.display.speed.value(), 4);
    let holds = session
        .practice
        .notes()
        .filter(|(n, _)| n.is_hold())
        .collect::<Vec<_>>();
    assert_eq!(holds.len(), 133);
    assert!((holds[0].0.start - 1.548387).abs() < 0.001);
    app.return_to_selector()?;

    // Cancelled work cannot activate later, even after its result is queued.
    app.start_selected()?;
    app.cancel_loading();
    app.poll_loading();
    assert!(app.session.is_none() && app.selector.loading.is_none());
    // A missing optional reaction is reported while the chart remains playable.
    let index = app
        .characters
        .presets
        .iter()
        .position(|p| p.choice.id == app.selector.character)
        .unwrap();
    for role in ["idle", "miss"] {
        let preset = &mut Arc::make_mut(&mut app.characters).presets[index];
        let definition = if role == "idle" {
            &mut preset.idle
        } else {
            &mut preset.miss
        };
        let definition = definition.as_mut().unwrap();
        let original = definition.motion.clone();
        let missing = format!("missing-test-{role}.MOT");
        definition.motion = missing.clone();
        app.start_selected()?;
        wait_for_session(&mut app);
        let errors = &app.session.as_ref().unwrap().dance_errors;
        assert!(
            errors
                .iter()
                .any(|error| error.contains(&missing) && error.contains(role)),
            "{errors:?}"
        );
        app.return_to_selector()?;
        let preset = &mut Arc::make_mut(&mut app.characters).presets[index];
        let definition = if role == "idle" {
            &mut preset.idle
        } else {
            &mut preset.miss
        };
        definition.as_mut().unwrap().motion = original;
    }
    // Missing resources exercise the real worker -> UI error path.
    let song = app
        .selector
        .catalog
        .songs
        .iter_mut()
        .find(|s| s.id == "01518")
        .unwrap();
    for pattern in &mut song.patterns {
        pattern.file = "music/missing-test-chart.gn".into();
    }
    app.start_selected()?;
    let began = Instant::now();
    while app.selector.loading.is_some() {
        app.poll_loading();
        assert!(began.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(app.session.is_none());
    let error = app
        .selector
        .error
        .as_deref()
        .context("Missing load error")?;
    assert!(error.contains("missing-test-chart.gn"), "{error}");
    capture(&mut app.selector, "song-load-error.png")?;
    app.preview_audio.stop();
    std::fs::remove_file(settings)?;
    println!(
        "PASS: first song, background loading/progress, loading-frame gate, Super Dancer tempo, Holy Knight holds, cancel, error popup, persisted live speed"
    );
    Ok(())
}

fn wait_for_session(app: &mut App) {
    let start = Instant::now();
    app.loading_frame_presented = true;
    while app.session.is_none() {
        app.poll_loading();
        assert!(app.selector.error.is_none(), "{:?}", app.selector.error);
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "Load did not complete"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn wait_for_preview(app: &mut App) {
    let begin = Instant::now();
    while app.preview_audio.progress().0 < 0.1 {
        app.preview_audio.update(true);
        assert!(
            begin.elapsed() < Duration::from_secs(8),
            "Preview failed: {}",
            app.preview_audio.status()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}
#[test]
#[ignore = "Requires the local SDO collection and an audio output device"]
fn selector_preview_switch_play_return_and_loop() -> Result<()> {
    let paths = CollectionPaths::from_env()?;
    let options = Options::default();
    let assets = sdo_assets::curated::ui(&paths)?;
    let catalog = Catalog::load(&paths)?;
    let short_id = catalog
        .songs
        .iter()
        .filter(|s| s.preview.is_some())
        .min_by(|a, b| {
            a.preview
                .as_ref()
                .unwrap()
                .duration
                .total_cmp(&b.preview.as_ref().unwrap().duration)
        })
        .context("No previews")?
        .id
        .clone();
    let mut app = collection_app(
        paths,
        options,
        catalog,
        assets,
        None,
        SettingsStore::default(),
        ScoreStore::default(),
    )?;
    app.sync_preview();
    wait_for_preview(&mut app);
    let first_id = app.preview_id.clone();
    app.selector.query = short_id;
    app.selector.refilter();
    app.sync_preview();
    assert_ne!(app.preview_id, first_id);
    assert_eq!(
        app.preview_audio.progress(),
        (0.0, 0.0),
        "Old preview must stop immediately"
    );
    wait_for_preview(&mut app);
    let mut previous = app.preview_audio.progress().0;
    let mut looped = false;
    let deadline = Instant::now() + Duration::from_secs_f64(app.preview_audio.progress().1 + 2.0);
    while Instant::now() < deadline {
        app.preview_audio.update(true);
        let pos = app.preview_audio.progress().0;
        if pos + 0.3 < previous {
            looped = true;
            break;
        }
        previous = pos;
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(looped, "Preview did not loop");
    app.preview_audio.update(false);
    let paused = app.preview_audio.progress().0;
    std::thread::sleep(Duration::from_millis(80));
    assert!((app.preview_audio.progress().0 - paused).abs() < 0.01);
    app.selector.query = "Holy Knight".into();
    app.selector.refilter();
    app.sync_preview();
    app.start_selected()?;
    wait_for_session(&mut app);
    assert!(app.session.is_some());
    assert_eq!(app.preview_audio.progress(), (0.0, 0.0));
    std::thread::sleep(Duration::from_millis(180));
    assert!(app.session.as_ref().unwrap().audio.position() > 0.15);
    app.return_to_selector()?;
    assert!(app.session.is_none());
    wait_for_preview(&mut app);
    app.preview_audio.stop();
    println!(
        "PASS: distinct preview selection, immediate stop, loop, focus pause, full-song playback and return"
    );
    Ok(())
}
#[test]
#[ignore = "Requires the local assets, audio output and a Metal/wgpu device"]
fn native_health_failure_stops_audio_and_restart_restores_play() -> Result<()> {
    let paths = CollectionPaths::from_env()?;
    let assets = sdo_assets::curated::ui(&paths)?;
    let catalog = Catalog::load(&paths)?;
    let mut app = collection_app(
        paths,
        Options::default(),
        catalog,
        assets,
        None,
        SettingsStore::default(),
        ScoreStore::default(),
    )?;
    app.start_selected()?;
    wait_for_session(&mut app);
    let session = app.session.as_mut().unwrap();
    session.practice = Practice::from_notes(
        (1..=24)
            .map(|i| sdo_gameplay::Note {
                lane: sdo_gameplay::Lane::Left,
                start: i as f64,
                end: i as f64,
            })
            .collect(),
        30.,
        sdo_gameplay::judgment::find("keyboard").unwrap(),
    )
    .with_health_difficulty(sdo_gameplay::HealthDifficulty::Easy);
    session.practice.advance_to(23.5, [false; 4], false);
    session.advance(0., false)?;
    assert!(session.practice.failed() && session.practice.paused());
    session.pause(false)?;
    assert!(
        session.practice.paused(),
        "Pause key must not revive a failed attempt"
    );
    let stopped = session.audio.position();
    let rig = &app.characters.preset(&app.selector.character)?.skeleton;
    let idle_start = session
        .character_pose(rig)?
        .context("Missing failed-stage idle")?;
    std::thread::sleep(Duration::from_millis(80));
    assert!((session.audio.position() - stopped).abs() < 0.01);
    let idle_next = session.character_pose(rig)?.unwrap();
    assert_ne!(
        idle_start.globals, idle_next.globals,
        "Stage failure froze idle"
    );
    assert!(idle_next.frame > idle_start.frame);
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let (_, device, queue, _) = pollster::block_on(sdo_render::request_device(&instance, None))?;
    let mut renderer = Renderer::new(
        device,
        queue,
        wgpu::TextureFormat::Rgba8UnormSrgb,
        &app.skin,
    );
    let bar = app
        .skin
        .health
        .as_ref()
        .context("Missing original health artwork")?;
    let scene = session
        .display
        .scene(&app.skin, &session.practice, &session.input);
    assert!(scene.sprites.iter().any(|s| s.texture == bar.frame));
    assert!(
        !scene
            .sprites
            .iter()
            .any(|s| s.texture == bar.fill || s.texture == bar.full)
    );
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../artifacts/health-failed.png");
    renderer.screenshot(&path, [960, 720], &scene)?;
    session.restart()?;
    assert!(session.failed_at.is_none());
    let restarted = session
        .character_pose(rig)?
        .context("Missing restarted dance")?;
    assert_eq!(
        restarted.motion,
        app.characters
            .preset(&app.selector.character)?
            .idle
            .as_ref()
            .unwrap()
            .motion,
        "Restart returns to the intro rest loop until authored choreography begins"
    );
    assert!(session.dance.waiting_for_choreography(session.dance_time()));
    assert_eq!(session.practice.health().value(), 1000);
    assert!(!session.practice.paused() && session.practice.dancing());
    assert_eq!(session.practice.score().missed, 0);
    std::thread::sleep(Duration::from_millis(80));
    assert!(session.audio.position() > 0.);
    session.pause(true)?;
    println!(
        "PASS: native depletion pauses audio, empty bar renders, restart restores full health"
    );
    Ok(())
}

fn collection_app(
    paths: CollectionPaths,
    options: Options,
    _catalog: Catalog,
    _ui: UiAssets,
    session: Option<PlaySession>,
    settings: SettingsStore,
    scores: ScoreStore,
) -> Result<App> {
    let content = AssetLoader::new(CuratedProvider::from_paths(paths)?);
    let catalog = content.request(asset::Catalog)?;
    let ui = content.request(asset::Ui)?;
    App::new(
        content,
        options,
        catalog,
        ui,
        session,
        settings,
        (scores, Default::default()),
    )
}
fn collection_dances(paths: &CollectionPaths) -> DanceLibrary {
    let root = paths.assets().join("character/motion");
    DanceLibrary::new(move |name| {
        Ok(Arc::new(sdo_assets::CharacterMotion::parse(
            &std::fs::read(sdo_assets::curated::resolve_resource(&root, name)?)?,
        )?))
    })
}
fn load_song_dance(
    _library: &mut DanceLibrary,
    paths: &CollectionPaths,
    song: &sdo_song::Song,
    chart: &Chart,
    difficulty: Difficulty,
) -> (Option<SongDance>, Vec<String>) {
    let content = AssetLoader::new(CuratedProvider::from_paths(paths.clone()).unwrap());
    loader::load_dance(
        &content,
        song.dance_moves_paths.as_deref(),
        chart,
        difficulty,
        55,
    )
}

#[test]
#[ignore = "Requires local songs and an audio output device"]
fn multiplayer_local_loading_barrier_scheduled_start_inputs_and_forfeit() -> Result<()> {
    use sdo_ui::LobbyAction;
    let paths = CollectionPaths::from_env()?;
    let content = AssetLoader::new(CuratedProvider::from_paths(paths)?);
    let catalog = content.request(asset::Catalog)?;
    let ui = content.request(asset::Ui)?;
    let make = || {
        App::new(
            content.clone(),
            Options::default(),
            catalog.clone(),
            ui.clone(),
            None,
            SettingsStore::default(),
            Default::default(),
        )
    };
    let mut a = make()?;
    let mut b = make()?;
    a.selector.timing_offset_ms = 80.;
    b.selector.timing_offset_ms = -120.;
    a.selector.lobby.scroll_speed = ScrollSpeed::new(2)?;
    b.selector.lobby.scroll_speed = ScrollSpeed::new(7)?;
    let server = sdo_multiplayer::Server::bind("127.0.0.1:0")?;
    for (app, name) in [(&mut a, "Alice"), (&mut b, "Bob")] {
        app.selector.lobby.preferences.address = server.address.to_string();
        app.selector.lobby.preferences.name = name.into();
        app.lobby_action(&LobbyAction::Connect)?;
    }
    fn until(a: &mut App, b: &mut App, predicate: impl Fn(&App, &App) -> bool) -> Result<()> {
        let began = Instant::now();
        loop {
            for app in [&mut *a, &mut *b] {
                app.poll_multiplayer();
                app.loading_frame_presented = true;
                app.poll_loading();
                if app.session.is_none() {
                    app.sync_preview();
                    app.preview_audio.update(app.preview_active());
                }
                ensure!(
                    app.selector.error.is_none(),
                    "Desktop multiplayer failed: {:?}",
                    app.selector.error
                );
                ensure!(
                    app.online.client.is_some(),
                    "Desktop multiplayer disconnected: {}",
                    app.selector.lobby.status
                );
            }
            if predicate(a, b) {
                return Ok(());
            }
            ensure!(
                began.elapsed() < Duration::from_secs(60),
                "Timed out waiting for desktop lobby/loading"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    until(&mut a, &mut b, |a, b| {
        a.selector.lobby.player.is_some()
            && b.selector.lobby.player.is_some()
            && a.selector.lobby.synchronized
            && b.selector.lobby.synchronized
    })?;
    a.selector.lobby.room_name = "Playback test".into();
    a.lobby_action(&LobbyAction::Create)?;
    until(&mut a, &mut b, |a, b| {
        a.selector.lobby.room().is_some() && !b.selector.lobby.directory.is_empty()
    })?;
    let room = a.selector.lobby.room().unwrap().id;
    b.lobby_action(&LobbyAction::Join(room))?;
    until(&mut a, &mut b, |a, b| {
        a.selector.lobby.room().unwrap().members.len() == 2 && b.selector.lobby.room().is_some()
    })?;
    let room_song = a.selector.selected().unwrap().id.clone();
    let browse_song = a
        .selector
        .catalog
        .songs
        .iter()
        .find(|song| song.id != room_song && song.preview.is_some() && song.pattern("k", 4).is_ok())
        .context("Need another multiplayer preview song")?
        .id
        .clone();
    a.selector.lobby.song_picker_open = true;
    a.selector.lobby.song_preview = Some(browse_song.clone());
    until(&mut a, &mut b, |a, _| a.preview_audio.progress().1 > 0.)?;
    assert_eq!(a.preview_song().unwrap().id, browse_song);
    assert_eq!(a.selector.selected().unwrap().id, room_song);
    assert!(a.selector.lobby.room().unwrap().chart.is_none());
    a.selector.lobby.song_preview = Some(room_song.clone());
    a.sync_preview();
    assert_eq!(
        a.preview_audio.progress(),
        (0., 0.),
        "Switching multiplayer previews stops the old audio"
    );
    until(&mut a, &mut b, |a, _| a.preview_audio.progress().1 > 0.)?;
    assert_eq!(a.preview_song().unwrap().id, room_song);
    a.selector.lobby.song_picker_open = false;
    a.selector.lobby.song_preview = None;
    a.sync_preview();
    assert_eq!(
        a.preview_audio.progress(),
        (0., 0.),
        "Cancelling an unconfigured room returns to silence"
    );
    a.selector.select_room_song(&room_song)?;
    a.start_room_selected()?;
    until(&mut a, &mut b, |a, b| {
        a.online.prepared.is_some()
            && b.selector
                .lobby
                .room()
                .and_then(|r| r.chart.as_ref())
                .is_some()
    })?;
    let chosen = a.online.prepared.as_ref().unwrap().0.clone();
    let local = b
        .selector
        .catalog
        .songs
        .iter_mut()
        .find(|song| {
            song.patterns
                .iter()
                .any(|p| p.sha256 == chosen.pattern_sha256)
        })
        .unwrap();
    local
        .patterns
        .iter_mut()
        .find(|p| p.sha256 == chosen.pattern_sha256)
        .unwrap()
        .bpm = Some(chosen.bpm + 10.);
    b.lobby_action(&LobbyAction::Ready)?;
    until(&mut a, &mut b, |a, b| {
        b.online.prepared.is_some()
            && a.selector
                .lobby
                .room()
                .unwrap()
                .members
                .iter()
                .all(|m| m.ready)
    })?;
    let host_spec = a.online.prepared.as_ref().unwrap().0.clone();
    assert_eq!(b.online.prepared.as_ref().unwrap().0, host_spec);
    assert_eq!(
        a.online.prepared.as_ref().unwrap().1.display.speed.value(),
        2
    );
    assert_eq!(
        b.online.prepared.as_ref().unwrap().1.display.speed.value(),
        7
    );
    b.selector.move_by(1);
    b.focused = false;
    until(&mut a, &mut b, |a, b| {
        a.preview_audio.progress().1 > 0. && b.preview_audio.progress().1 > 0.
    })?;
    assert_eq!(a.preview_song().unwrap().id, b.preview_song().unwrap().id);
    assert_ne!(
        b.selector.selected().unwrap().id,
        b.preview_song().unwrap().id
    );
    assert!(b.preview_active());
    let self_id = a.selector.lobby.player.unwrap();
    let initial = a
        .selector
        .lobby
        .room()
        .unwrap()
        .members
        .iter()
        .find(|m| m.id == self_id)
        .unwrap()
        .x;
    a.selector.lobby.walk_direction = [1, 0];
    a.sync_room_walk()?;
    until(&mut a, &mut b, |a, b| {
        a.selector
            .lobby
            .room()
            .unwrap()
            .members
            .iter()
            .any(|m| m.id == self_id && m.x > initial + 0.02)
            && b.selector
                .lobby
                .room()
                .unwrap()
                .members
                .iter()
                .any(|m| m.id == self_id && m.x > initial + 0.02)
    })?;
    a.selector.lobby.stop_walking();
    a.sync_room_walk()?;
    until(&mut a, &mut b, |_, b| {
        b.selector
            .lobby
            .room()
            .unwrap()
            .members
            .iter()
            .any(|m| m.id == self_id && m.walking == [0, 0])
    })?;
    a.selector.lobby.scroll_speed = ScrollSpeed::new(3)?;
    b.selector.lobby.scroll_speed = ScrollSpeed::new(6)?;
    // The host has no Ready button: Start must prepare its local session if needed.
    a.online.prepared = None;
    a.lobby_action(&LobbyAction::Start)?;
    assert!(a.online.start_when_ready);
    until(&mut a, &mut b, |a, b| {
        a.session.is_some() && b.session.is_some()
    })?;
    assert!(
        !b.focused,
        "An unfocused client must enter the gameplay screen"
    );
    assert!(b.online_running() && b.session.is_some());
    assert_eq!(a.session.as_ref().unwrap().display.speed.value(), 3);
    assert_eq!(b.session.as_ref().unwrap().display.speed.value(), 6);
    assert_eq!(a.preview_audio.progress(), (0., 0.));
    assert_eq!(b.preview_audio.progress(), (0., 0.));
    assert_eq!(a.session.as_ref().unwrap().audio.position(), 0.);
    assert_eq!(b.session.as_ref().unwrap().audio.position(), 0.);
    a.keyboard(KeyCode::KeyR, true, false)?;
    assert_eq!(a.online.sequence, 0);
    until(&mut a, &mut b, |_, b| b.online_input_open())?;
    assert!(!b.online_started());
    assert_eq!(b.session.as_ref().unwrap().audio.position(), 0.);
    b.keyboard(KeyCode::KeyF, true, false)?;
    b.keyboard(KeyCode::KeyF, false, false)?;
    assert_eq!(
        b.online.sequence, 2,
        "Negative calibration accepts pre-roll input before audio starts"
    );
    until(&mut a, &mut b, |a, b| {
        a.online_started()
            && b.online_started()
            && a.session.as_ref().unwrap().audio.position() > 0.15
            && b.session.as_ref().unwrap().audio.position() > 0.15
    })?;
    let a_time = a.session.as_ref().unwrap().audio.position();
    let b_time = b.session.as_ref().unwrap().audio.position();
    ensure!(
        (a_time - b_time).abs() < 0.05,
        "Desktop audio clocks diverged: {a_time} vs {b_time}"
    );
    a.keyboard(KeyCode::KeyD, true, false)?;
    a.keyboard(KeyCode::ArrowLeft, true, false)?;
    a.keyboard(KeyCode::KeyD, false, false)?;
    a.keyboard(KeyCode::ArrowLeft, false, false)?;
    assert_eq!(
        a.online.sequence, 2,
        "Only logical transitions go over the network"
    );
    until(&mut a, &mut b, |a, _| {
        !a.selector.lobby.standings.is_empty()
    })?;
    assert_eq!(
        a.session.as_ref().unwrap().score_save,
        ScoreSaveState::Pending
    );
    b.keyboard(KeyCode::KeyD, true, false)?;
    let before = b.session.as_ref().unwrap().audio.position();
    let sequence = b.online.sequence;
    b.window_focus(false)?;
    assert!(b.session.is_some() && b.online_running());
    assert!(!b.session.as_ref().unwrap().audio.paused());
    assert_eq!(
        b.online.sequence,
        sequence + 1,
        "Focus loss must send a held-lane release"
    );
    std::thread::sleep(Duration::from_millis(40));
    assert!(b.session.as_ref().unwrap().audio.position() > before);
    b.window_focus(true)?;
    a.keyboard(KeyCode::Escape, true, false)?;
    assert!(
        a.session.is_some(),
        "Escape cannot remove an active participant"
    );
    a.net_send(sdo_net::Command::Forfeit {
        round: a.online.round.unwrap().0,
    })?;
    assert!(
        b.session.is_some() && b.online_running(),
        "One forfeit must not stop another player's playback"
    );
    until(&mut a, &mut b, |_, b| {
        b.selector.lobby.standings.iter().any(|s| s.forfeited)
    })?;
    b.net_send(sdo_net::Command::Forfeit {
        round: b.online.round.unwrap().0,
    })?;
    until(&mut a, &mut b, |a, b| {
        a.selector.lobby.results && b.selector.lobby.results
    })?;
    assert!(
        a.session.is_some() && b.session.is_some(),
        "Final results stay over gameplay"
    );
    assert!(a.online_running() && b.online_running());
    a.lobby_action(&LobbyAction::ConfirmResults)?;
    assert!(a.session.is_none() && !a.online_running());
    assert!(
        b.session.is_some() && b.selector.lobby.results,
        "Each client acknowledges independently"
    );
    b.lobby_action(&LobbyAction::ConfirmResults)?;
    assert!(b.session.is_none() && !b.online_running());
    assert!(a.scores.book.scores.is_empty() && b.scores.book.scores.is_empty());
    assert!(
        a.multiplayer_scores.book.scores.is_empty() && b.multiplayer_scores.book.scores.is_empty()
    );
    println!(
        "PASS: both desktop loaders verified {}, audio clocks differ by {:.1} ms, logical inputs and independent forfeit work",
        host_spec.pattern_sha256,
        (a_time - b_time).abs() * 1000.
    );
    Ok(())
}

#[test]
#[ignore = "Requires the local character library and a GPU"]
fn room_reference_layout_renders_closeups_and_walking_stage() -> Result<()> {
    use sdo_net::{ChartSpec, ChatMessage, Member, Room};
    let paths = CollectionPaths::from_env()?;
    let content = AssetLoader::new(CuratedProvider::from_paths(paths)?);
    let mut app = App::new(
        content.clone(),
        Options::default(),
        content.request(asset::Catalog)?,
        content.request(asset::Ui)?,
        None,
        SettingsStore::default(),
        Default::default(),
    )?;
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let (_, device, queue, _) = pollster::block_on(sdo_render::request_device(&instance, None))?;
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let renderer = Renderer::new(device, queue, format, &app.skin);
    let mut portraits = Vec::new();
    for (index, preset) in app.characters.presets.iter().take(2).enumerate() {
        let direction = if index == 0 { [1, 0] } else { [-1, 0] };
        let walk = preset
            .walk
            .as_ref()
            .expect("Original room walk must be configured");
        let motion = app.content.request(asset::Motion(&walk.motion))?;
        assert!(motion.supports_bones(preset.skeleton.bones.len()));
        let mut cameras =
            crate::room::RoomCamera::new(&renderer, &app.characters, preset, &app.content);
        let idle = cameras.frame(&renderer, preset, 0., [0, 1], [0, 0])?;
        let right = cameras.frame(&renderer, preset, 0.2, direction, direction)?;
        let picture = cameras.frame(&renderer, preset, 0.4, direction, direction)?;
        assert_eq!(picture.avatar.len(), sdo_render::character::AVATAR_BYTES);
        assert_ne!(
            idle.rgba, picture.rgba,
            "The fixed portrait must show the character turning and animating"
        );
        assert_ne!(
            right.avatar, picture.avatar,
            "Original walk animation must advance"
        );
        let (pixels, _) = picture.rgba.as_chunks::<4>();
        assert!(
            pixels.iter().filter(|p| p[3] > 0).count() > 1000,
            "The head must remain in frame"
        );
        let [width, height] = sdo_render::character::PORTRAIT_SIZE;
        let center = ((height / 2 * width + width / 2) * 4) as usize;
        let nose = &idle.rgba[center..center + 4];
        assert!(
            nose[0] > 120 && nose[1] > 85 && nose[2] > 40 && nose[3] > 200,
            "Facing the stage front must show the face: {nose:?}"
        );
        let left = cameras.frame(&renderer, preset, 0.6, [-direction[0], 0], [0, 0])?;
        assert_ne!(
            left.rgba, picture.rgba,
            "Turning left must change the character seen by the fixed portrait camera"
        );
        // Freeze the bind pose so only the standing direction changes.
        let mut fixed_preset = preset.clone();
        fixed_preset.idle = None;
        fixed_preset.walk = None;
        let mut fixed =
            crate::room::RoomCamera::new(&renderer, &app.characters, &fixed_preset, &app.content);
        let front = fixed.frame(&renderer, &fixed_preset, 0., [0, 1], [0, 0])?;
        let back = fixed.frame(&renderer, &fixed_preset, 0.1, [0, -1], [0, 0])?;
        assert_ne!(front.avatar, back.avatar, "The stage must show the back");
        assert_ne!(
            front.rgba, back.rgba,
            "Turning away must show the back in the profile too"
        );
        let back_center = &back.rgba[center..center + 4];
        assert!(
            back_center[0] < nose[0] && back_center[3] > 200,
            "The back-facing profile must show hair rather than a tracked face: {back_center:?}"
        );
        let began = std::time::Instant::now();
        let directions = [
            [0, 1],
            [1, 1],
            [1, 0],
            [1, -1],
            [0, -1],
            [-1, -1],
            [-1, 0],
            [-1, 1],
        ];
        for step in 1..=24 {
            let direction = directions[(step - 1) as usize % directions.len()];
            let frame = cameras.frame(
                &renderer,
                preset,
                0.6 + step as f64 * 0.08,
                direction,
                direction,
            )?;
            let [width, height] = sdo_render::character::AVATAR_SIZE;
            for y in 0..height {
                for x in 0..width {
                    if x < 2 || y < 2 || x >= width - 2 || y >= height - 2 {
                        assert_eq!(
                            frame.avatar[((y * width + x) * 4 + 3) as usize],
                            0,
                            "{} walking {direction:?} frame {step} touches the camera edge at ({x}, {y})",
                            preset.choice.name
                        );
                    }
                }
            }
            frame.validate()?;
        }
        println!(
            "{}: local diagnostic capture average {:.1} ms",
            preset.choice.name,
            began.elapsed().as_secs_f64() * 1000. / 24.
        );
        portraits.push(picture);
    }
    assert_eq!(portraits.len(), 2);
    assert_ne!(portraits[0].rgba, portraits[1].rgba);
    let song = app.selector.selected().unwrap();
    let pattern = song.pattern("k", 4)?;
    let levels = pattern.levels;
    let spec = ChartSpec {
        pattern_sha256: pattern.sha256.clone(),
        audio_sha256: "a".repeat(64),
        dance_sha256: None,
        title: song.title.clone(),
        kind: "k".into(),
        difficulty: 0,
        judgment: "keyboard".into(),
        bpm: song.bpm,
        duration_us: (song.duration * 1e6) as u64,
    };
    app.selector.lobby.open = true;
    app.selector.lobby.player = Some(1);
    app.selector.lobby.synchronized = true;
    app.selector.lobby.matching_chart = true;
    app.selector.lobby.preview_status = "Room preview playing".into();
    app.selector.lobby.joined_room = Some(Room {
        keys: 4,
        levels,
        password_protected: false,
        id: 1,
        name: "Evening Practice".into(),
        host: 1,
        members: vec![
            Member {
                state_us: 0,
                x: 0.28,
                y: 0.65,
                facing: [1, 0],
                walking: [0, 0],
                slot: 0,
                id: 1,
                name: "Alice".into(),
                ready: true,
            },
            Member {
                state_us: 0,
                x: 0.66,
                y: 0.65,
                facing: [-1, 0],
                walking: [1, 0],
                slot: 1,
                id: 2,
                name: "Bob".into(),
                ready: false,
            },
        ],
        chart: Some(spec),
        round: None,
        start_us: None,
    });
    for (index, portrait) in portraits.into_iter().enumerate() {
        app.selector
            .lobby
            .receive_portrait(index as u64 + 1, Some(portrait));
    }
    app.selector.lobby.receive_chat(
        Some(1),
        ChatMessage {
            id: 1,
            player: Some(1),
            name: "Alice".into(),
            text: "Welcome! Choose your speed and ready up.".into(),
            error: false,
        },
    );
    app.selector.lobby.receive_chat(
        Some(1),
        ChatMessage {
            id: 2,
            player: Some(2),
            name: "Bob".into(),
            text: "The preview is playing here too.".into(),
            error: false,
        },
    );
    let target = renderer.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("room layout"),
        size: wgpu::Extent3d {
            width: 1100,
            height: 760,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let mut ui = SelectorUi::new(&renderer.device, format, None);
    app.selector.lobby.private_server = true;
    for _ in 0..8 {
        ui.draw(
            &renderer.device,
            &renderer.queue,
            &target.create_view(&Default::default()),
            [1100, 760],
            None,
            &mut app.selector,
            "Preview playing",
            (5., 14.),
        );
    }
    let folder = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../artifacts");
    renderer.save_texture(&folder.join("room-lobby.png"), &target, [1100, 760])?;
    app.selector.lobby.private_server = false;
    for _ in 0..3 {
        ui.draw(
            &renderer.device,
            &renderer.queue,
            &target.create_view(&Default::default()),
            [1100, 760],
            None,
            &mut app.selector,
            "Preview",
            (0., 0.),
        );
    }
    renderer.save_texture(
        &folder.join("dedicated-room-host.png"),
        &target,
        [1100, 760],
    )?;
    app.selector.lobby.player = Some(2);
    for _ in 0..3 {
        ui.draw(
            &renderer.device,
            &renderer.queue,
            &target.create_view(&Default::default()),
            [1100, 760],
            None,
            &mut app.selector,
            "Preview",
            (0., 0.),
        );
    }
    renderer.save_texture(&folder.join("room-client.png"), &target, [1100, 760])?;
    app.selector.lobby.player = Some(1);
    app.selector.lobby.private_server = true;
    app.selector.lobby.song_picker_open = true;
    for _ in 0..3 {
        ui.draw(
            &renderer.device,
            &renderer.queue,
            &target.create_view(&Default::default()),
            [1100, 760],
            None,
            &mut app.selector,
            "Preview playing",
            (5., 14.),
        );
    }
    renderer.save_texture(&folder.join("room-song-picker.png"), &target, [1100, 760])?;
    app.selector.lobby.song_picker_open = false;
    // The player's settings are already inline in the room sidebar.
    for _ in 0..3 {
        ui.draw(
            &renderer.device,
            &renderer.queue,
            &target.create_view(&Default::default()),
            [1100, 760],
            None,
            &mut app.selector,
            "Preview",
            (0., 0.),
        );
    }
    renderer.save_texture(
        &folder.join("room-player-settings.png"),
        &target,
        [1100, 760],
    )?;
    let mut character_preview = sdo_render::character::CharacterRenderer::new(
        &renderer.device,
        &renderer.queue,
        &app.characters,
    );
    character_preview.render(
        &renderer.device,
        &renderer.queue,
        &app.selector.character,
        sdo_render::character::CharacterCamera::default(),
    )?;
    let character_texture =
        ui.register_character_preview(&renderer.device, &character_preview.ui_view);
    app.selector.character_texture = Some(character_texture);
    app.selector.character_view = true;
    for _ in 0..3 {
        ui.draw(
            &renderer.device,
            &renderer.queue,
            &target.create_view(&Default::default()),
            [1100, 760],
            None,
            &mut app.selector,
            "Preview",
            (0., 0.),
        );
    }
    renderer.save_texture(
        &folder.join("multiplayer-character.png"),
        &target,
        [1100, 760],
    )?;
    app.selector.character_view = false;
    app.selector.character_texture = None;
    ui.unregister_preview(character_texture);
    app.selector.lobby.private_server = false;
    let previous = app.selector.lobby.joined_room.take().unwrap();
    app.selector.lobby.directory = vec![sdo_net::RoomSummary {
        id: previous.id,
        name: previous.name,
        occupants: previous.members.len(),
        playing: false,
        password_protected: true,
    }];
    app.selector.lobby.player = Some(99);
    for _ in 0..3 {
        ui.draw(
            &renderer.device,
            &renderer.queue,
            &target.create_view(&Default::default()),
            [1100, 760],
            None,
            &mut app.selector,
            "Preview",
            (0., 0.),
        );
    }
    renderer.save_texture(&folder.join("room-password-list.png"), &target, [1100, 760])?;
    app.selector.lobby.create_room_open = true;
    app.selector.lobby.room_name = "New practice room".into();
    for _ in 0..3 {
        ui.draw(
            &renderer.device,
            &renderer.queue,
            &target.create_view(&Default::default()),
            [1100, 760],
            None,
            &mut app.selector,
            "Preview",
            (0., 0.),
        );
    }
    renderer.save_texture(
        &folder.join("multiplayer-create-room.png"),
        &target,
        [1100, 760],
    )?;
    app.selector.lobby.create_room_open = false;
    app.selector.lobby.private_server = true;
    app.selector
        .lobby
        .request_room_password(1, "Evening Practice".into());
    app.selector.lobby.join_password = "example".into();
    for _ in 0..3 {
        ui.draw(
            &renderer.device,
            &renderer.queue,
            &target.create_view(&Default::default()),
            [1100, 760],
            None,
            &mut app.selector,
            "Preview",
            (0., 0.),
        );
    }
    renderer.save_texture(
        &folder.join("private-room-password.png"),
        &target,
        [1100, 760],
    )?;
    app.selector.lobby.clear_password_prompt();
    app.selector.lobby.private_server = false;
    app.selector.lobby.player = None;
    app.selector.lobby.server_menu = sdo_ui::ServerMenu::Choose;
    for _ in 0..3 {
        ui.draw(
            &renderer.device,
            &renderer.queue,
            &target.create_view(&Default::default()),
            [1100, 760],
            None,
            &mut app.selector,
            "Preview",
            (0., 0.),
        );
    }
    renderer.save_texture(&folder.join("multiplayer-menu.png"), &target, [1100, 760])?;
    app.selector.lobby.server_menu = sdo_ui::ServerMenu::Join;
    for _ in 0..3 {
        ui.draw(
            &renderer.device,
            &renderer.queue,
            &target.create_view(&Default::default()),
            [1100, 760],
            None,
            &mut app.selector,
            "Preview",
            (0., 0.),
        );
    }
    renderer.save_texture(&folder.join("multiplayer-join.png"), &target, [1100, 760])?;
    app.selector.lobby.server_menu = sdo_ui::ServerMenu::Host;

    app.selector.lobby.room_name = "Friends".into();
    app.selector.lobby.room_password = "example".into();
    for _ in 0..3 {
        ui.draw(
            &renderer.device,
            &renderer.queue,
            &target.create_view(&Default::default()),
            [1100, 760],
            None,
            &mut app.selector,
            "Preview",
            (0., 0.),
        );
    }
    renderer.save_texture(
        &folder.join("private-server-hosting.png"),
        &target,
        [1100, 760],
    )?;
    app.selector.lobby.room_password.clear();
    app.selector.lobby.open = false;
    app.selector.settings_open = true;
    for _ in 0..3 {
        ui.draw(
            &renderer.device,
            &renderer.queue,
            &target.create_view(&Default::default()),
            [1100, 760],
            None,
            &mut app.selector,
            "Preview",
            (0., 0.),
        );
    }
    renderer.save_texture(&folder.join("settings-menu.png"), &target, [1100, 760])?;
    println!(
        "PASS: close-up 3D portraits, distinct walking poses, eight slots and anchored chat rendered"
    );
    Ok(())
}
