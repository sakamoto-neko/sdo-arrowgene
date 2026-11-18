//! Select the provider, restore user state, then start the desktop application.
use crate::{
    app::{App, run_app},
    cli::Options,
    diagnostics::{audio_test, audit_dances},
    scores::ScoreStore,
    session::{PlaySession, ScoreSaveState, TailClock},
    settings::SettingsStore,
    wardrobe::WardrobeThumbnails,
};
use anyhow::{Result, ensure};
use sdo_asset_loader::{self as loader, request as asset, sources};
use sdo_audio::AudioPlayer;
use sdo_config::ScoreKey;
use sdo_dance::DancePlayer;
use sdo_gameplay::input::InputState;
use sdo_render::{
    Renderer, Sprite,
    character::{CharacterCamera, CharacterRenderer},
};
use sdo_ui::SelectorUi;
use sdo_ui::notes::NoteDisplay;
use std::time::Instant;

pub(crate) fn run() -> Result<()> {
    let Some(options) = Options::parse()? else {
        return Ok(());
    };
    let settings_path = sdo_config::settings_path()?;
    let scores = ScoreStore::load(settings_path.with_file_name("score.json"));
    let mut settings = SettingsStore::load(settings_path);
    settings.initialize_sources();
    let (content, source_reports) = sources::open(settings.values.asset_paths.as_ref().unwrap())?;
    if options.verify_dances {
        return audit_dances(&content, &options);
    }
    if options.verify_characters {
        let characters = content.request(asset::Characters(&Default::default()))?;
        println!("{}", serde_json::to_string_pretty(&characters.report())?);
        ensure!(
            !characters.presets.is_empty() && characters.issues.is_empty(),
            "Some character presets could not be built; see issues above"
        );
        return Ok(());
    }
    let assets = content.request(asset::Ui)?;
    for issue in &assets.issues {
        eprintln!("Unavailable UI: {issue}");
    }
    if options.verify || options.list_ui {
        println!("{}", serde_json::to_string_pretty(&assets.report())?);
        assets.ensure_ready()?;
        if options.verify {
            ensure!(
                assets.issues.is_empty(),
                "Some UI components could not be built; see issues above"
            );
        }
        return Ok(());
    }
    if options.list_hit_effects {
        for effect in &assets.effects {
            println!("{}\t{}", effect.choice.id, effect.choice.name);
        }
        return Ok(());
    }
    if options.asset_plan {
        let files = assets
            .resources
            .iter()
            .map(|path| {
                path.strip_prefix(&assets.root)
                    .map(|p| p.to_string_lossy().replace('\\', "/"))
            })
            .collect::<std::result::Result<Vec<_>, _>>()?;
        println!(
            "{}",
            serde_json::to_string_pretty(
                &serde_json::json!({"runtime_root":"assets/ui","files":files})
            )?
        );
        return Ok(());
    }
    let catalog = content.request(asset::Catalog)?;
    if options.verify_catalog {
        let stats = loader::verify_catalog(&content, &catalog)?;
        println!(
            "Validated music.json: {} songs, {} patterns, {} source mappings / {} original IDs, {} pattern BPM overrides, {} previews; all referenced files exist",
            stats.songs,
            stats.patterns,
            stats.source_mappings,
            stats.original_ids,
            stats.bpm_overrides,
            stats.previews
        );
        return Ok(());
    }
    if options.list_songs {
        let query = options.search.as_deref().unwrap_or("").to_lowercase();
        println!("ID\tTitle\tArtist\tPatterns / BPM\tOriginal source IDs");
        let mut count = 0;
        for song in catalog.songs_for_source(options.music_region.as_deref())? {
            let sources = song
                .sources
                .iter()
                .map(|(country, source)| format!("{}:{}", country, source.id.as_slice().join("/")))
                .collect::<Vec<_>>()
                .join(", ");
            let text =
                format!("{} {} {} {}", song.id, song.title, song.artist, sources).to_lowercase();
            if !query.is_empty() && !text.contains(&query) {
                continue;
            }
            let patterns = song
                .patterns
                .iter()
                .map(|p| format!("{}{}:{}", p.kind, p.mode, p.bpm.unwrap_or(song.bpm)))
                .collect::<Vec<_>>()
                .join(" ");
            println!(
                "{}\t{}\t{}\t{}\t{}",
                song.id, song.title, song.artist, patterns, sources
            );
            count += 1;
        }
        println!("{count} matching catalog songs");
        return Ok(());
    }
    let open_selector = options.selector
        || (options.song.is_none()
            && !options.autoplay
            && !options.verify_song
            && options.audio_test.is_none()
            && options.screenshot.is_none());
    let mut app = App::new(content, options, catalog, assets, None, settings, scores)?;
    app.selector.source_reports = source_reports;
    if open_selector {
        let screenshot = app.options.screenshot.clone();
        if let Some(path) = screenshot {
            let instance =
                wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
            let (_, device, queue, name) =
                pollster::block_on(sdo_render::request_device(&instance, None))?;
            let format = wgpu::TextureFormat::Rgba8UnormSrgb;
            let mut ui = SelectorUi::new(&device, format, None);
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("Selector screenshot"),
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
            let mut character_renderer =
                CharacterRenderer::new(&renderer.device, &renderer.queue, &app.characters);
            let mut thumbnails =
                WardrobeThumbnails::new(&renderer.device, &renderer.queue, app.content.clone());
            if !app.selector.character.is_empty() {
                character_renderer.render(
                    &renderer.device,
                    &renderer.queue,
                    &app.selector.character,
                    CharacterCamera::default(),
                )?;
                app.selector.character_texture = Some(
                    ui.register_character_preview(&renderer.device, &character_renderer.ui_view),
                );
            }
            // Settle layout and wait for the visible thumbnails, including failures.
            let started = Instant::now();
            let mut passes = 0;
            loop {
                ui.draw(
                    &renderer.device,
                    &renderer.queue,
                    &texture.create_view(&Default::default()),
                    [1100, 760],
                    None,
                    &mut app.selector,
                    "Preview plays when selected",
                    (0.0, 0.0),
                );
                passes += 1;
                let ready = thumbnails.is_ready(&app.selector.wardrobe);
                thumbnails.update(
                    &renderer.device,
                    &renderer.queue,
                    &mut ui,
                    &mut app.selector.wardrobe,
                    app.selector.character_view,
                );
                if passes >= 3 && ready {
                    break;
                }
                ensure!(
                    started.elapsed().as_secs() < 20,
                    "Timed out preparing wardrobe thumbnails"
                );
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            renderer.save_texture(&path, &texture, [1100, 760])?;
            println!("GPU: {name} | Song selector rendered to {}", path.display());
            return Ok(());
        }
        return run_app(app);
    }
    let options = &app.options;
    let content = &app.content;
    let catalog = &app.selector.catalog;
    let selected = if let Some(id) = options.song.as_deref() {
        catalog.select(id, options.music_region.as_deref(), &options.chart, 4)?
    } else {
        catalog.preferred(
            "Holy Knight",
            options.music_region.as_deref(),
            &options.chart,
            4,
        )?
    };
    let song_info = selected.song;
    let chart = content.request(asset::Chart {
        id: &selected.pattern.file,
        difficulty: options.difficulty,
        bpm: selected.bpm(),
    })?;
    if let Some(warning) = &chart.note_count_warning {
        eprintln!(
            "Warning: {} {:?}: {warning}",
            selected.pattern.file, options.difficulty
        );
    }
    let pattern_sha256 = chart.pattern_sha256();
    let song = content.request(asset::Audio {
        id: &selected.song.audio,
        expected_seconds: selected.song.duration,
        progress: &mut |_| true,
    })?;
    let duration = song.duration();
    println!(
        "Song {}: {} - {} | {:.3} BPM | level {} | {} notes | {} events | audio {:.3}s / {} Hz / {} channels",
        song_info.id,
        song_info.title,
        song_info.artist,
        chart.bpm,
        chart.level,
        chart.notes.len(),
        chart.events.len(),
        duration,
        song.sample_rate,
        song.channels
    );
    println!(
        "GN header field 12: {} (unresolved); {} trailing bytes preserved. Scoring/holds use practice behavior; native tap timing can be selected.",
        chart.header_field_12, chart.trailing_bytes
    );
    if chart.additional_lane_events > 0 {
        println!(
            "Chart has {} events outside the four-key lanes; playing its four-key subset and retaining all raw events.",
            chart.additional_lane_events
        );
    }
    ensure!(options.start < duration, "--start exceeds song duration");
    if options.verify_song {
        return Ok(());
    }
    if let Some(seconds) = options.audio_test {
        return audio_test(song, seconds, options.start);
    }
    let skin = &app.skin;
    let preset = sdo_gameplay::judgment::find(&options.preset).unwrap();
    let (dance, dance_errors) = loader::load_dance(
        content,
        song_info.dance_moves_paths.as_deref(),
        &chart,
        options.difficulty,
        app.characters
            .preset(&app.selector.character)
            .map_or(0, |c| c.skeleton.bones.len()),
    );
    let characters = &app.characters;
    let selected_character = &app.selector.character;
    let character = characters.preset(selected_character).ok();
    let idle = character
        .and_then(|c| c.idle.as_ref())
        .map(|definition| {
            content.request(asset::Reaction {
                animation: definition,
                bones: character.map_or(0, |c| c.skeleton.bones.len()),
            })
        })
        .and_then(Result::ok);
    let miss = character
        .and_then(|c| c.miss.as_ref())
        .map(|definition| {
            content.request(asset::Reaction {
                animation: definition,
                bones: character.map_or(0, |c| c.skeleton.bones.len()),
            })
        })
        .and_then(Result::ok);
    let mut dance = DancePlayer::new(dance, idle, miss);
    let mut practice = chart.into_practice(duration, preset, options.difficulty);
    let input = InputState::default();
    let display = NoteDisplay {
        speed: app.selector.scroll_speed,
    };
    if let Some(path) = &options.screenshot {
        ensure!(options.time < duration, "--time exceeds song duration");
        if options.autoplay {
            practice.advance_to(options.time, [false; 4], true);
        } else {
            practice.seek_preview(options.time);
        }
        if let Some(judgment) = options.preview {
            practice.show_preview_judgment(judgment);
        }
        let mut scene = display.scene(skin, &practice, &input);
        sdo_ui::notes::personal_best(
            &mut scene,
            skin,
            app.scores
                .book
                .best(&ScoreKey::new(
                    &song_info.id,
                    &options.chart,
                    options.difficulty.index(),
                    &options.preset,
                    &pattern_sha256,
                ))
                .map(|score| score.points),
        );
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let (_adapter, device, queue, adapter) =
            pollster::block_on(sdo_render::request_device(&instance, None))?;
        println!("GPU: {adapter}");
        let mut renderer = Renderer::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb, skin);
        if let Some(character) = character {
            let mut dancer = CharacterRenderer::new(&renderer.device, &renderer.queue, characters);
            let pose = dance.sample(options.time, practice.dancing(), &character.skeleton)?;
            dancer.render_pose(
                &renderer.device,
                &renderer.queue,
                &character.choice.id,
                CharacterCamera::default(),
                pose.as_ref().map(|p| p.globals.as_slice()),
            )?;
            let texture = renderer.register_external_texture(&dancer.view);
            scene.push(Sprite::new(texture, [7., 180., 320., 360.]));
            let status = if dance_errors.is_empty() {
                if dance.has_choreography() {
                    "DANCING".into()
                } else {
                    "DANCE UNAVAILABLE".into()
                }
            } else {
                format!("DANCE GAPS: {}", dance_errors.len())
            };
            sdo_ui::notes::dance_status(&mut scene, skin, &status, !dance_errors.is_empty());
        }
        renderer.screenshot(path, [960, 720], &scene)?;
        println!(
            "Rendered {} original/utility sprites to {} | score {:?}",
            scene.sprites.len(),
            path.display(),
            practice.score()
        );
        return Ok(());
    }
    if options.start > 0.0 {
        practice.seek_preview((options.start - options.offset).max(0.0));
    }
    let audio = AudioPlayer::new(song, options.start)?;
    let session = PlaySession {
        tail_clock: TailClock::default(),
        score_key: ScoreKey::new(
            &song_info.id,
            &options.chart,
            options.difficulty.index(),
            &options.preset,
            &pattern_sha256,
        ),
        score_save: ScoreSaveState::Pending,
        from_start: options.start == 0.,
        practice,
        input,
        display,
        audio,
        dance,
        failed_at: None,
        dance_errors,
        title: format!(
            "{} / {} - {}",
            song_info.id, song_info.title, song_info.artist
        ),
    };
    app.session = Some(session);
    run_app(app)
}
