use super::*;
use anyhow::{bail, ensure};
use sdo_assets::{
    GameplayUi, LaneFrames, NoteBoard, NoteSkin, ScoreUi, Texture, TextureId, UiChoice,
    WardrobeCatalog,
};
use sdo_gameplay::{Lane, Note};
use std::{
    collections::BTreeMap,
    sync::mpsc,
    time::{Duration, Instant},
};

/// Deliberately uses archive-style keys that the curated filesystem rejects.
/// This fixture never creates a collection directory or reads a game file.
pub(super) struct MemoryProvider {
    ui: Arc<UiAssets>,
    motion: Arc<CharacterMotion>,
    entered: Option<mpsc::SyncSender<()>>,
    release: Option<Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>>,
}
impl MemoryProvider {
    pub(super) fn new() -> Self {
        let choice = |id: &str| UiChoice {
            id: id.into(),
            name: id.into(),
        };
        let frames = || vec![TextureId(0)];
        let ui = UiAssets {
            root: "memory:".into(),
            resources: Vec::new(),
            issues: Vec::new(),
            textures: Arc::new(vec![Texture {
                source: "memory:texture".into(),
                width: 16,
                height: 16,
                rgba: vec![255; 16 * 16 * 4],
            }]),
            skins: vec![NoteSkin::new(
                choice("memory-skin"),
                "skin:7".into(),
                "memory:skin".into(),
                std::array::from_fn(|_| LaneFrames {
                    normal: frames(),
                    eighth: Vec::new(),
                    sixteenth: Vec::new(),
                    hold_head: frames(),
                    hold_body: frames(),
                    hold_tail: frames(),
                    receptor: frames(),
                    pressed: frames(),
                }),
            )],
            boards: vec![NoteBoard {
                choice: choice("board"),
                entry: "memory:board".into(),
                image: TextureId(0),
            }],
            scores: vec![ScoreUi {
                choice: choice("score"),
                entry: "memory:score".into(),
                ui: GameplayUi {
                    judgments: std::array::from_fn(|_| frames()),
                    combo: frames(),
                    combo_digits: vec![TextureId(0); 10],
                    score_digits: vec![TextureId(0); 10],
                    font: TextureId(0),
                },
            }],
            effects: Vec::new(),
            health_bars: Vec::new(),
        };
        let mut raw = b"Animation001".to_vec();
        for n in [1u32, 0, 0, 1, 1, 2] {
            raw.extend(n.to_le_bytes());
        }
        for v in [
            0_f32, 0., 0., 1., 0., 1., 1., 1., 0., 0., 0., 0., 0., 0., 20., 0., 20., 20.,
        ] {
            raw.extend(v.to_le_bytes());
        }
        Self {
            ui: Arc::new(ui),
            motion: Arc::new(CharacterMotion::parse(&raw).unwrap()),
            entered: None,
            release: None,
        }
    }
}
impl AssetProvider for MemoryProvider {
    fn name(&self) -> &str {
        "Memory archive"
    }
    fn catalog(&self) -> Result<Catalog> {
        use sdo_song::{Pattern, Song, Source, SourceIds};
        Ok(Catalog {
            duration_unit: "seconds".into(),
            source_country_keys: BTreeMap::from([("memory".into(), "Memory".into())]),
            songs: vec![Song {
                id: "cn:recording/1".into(),
                title: "Synthetic fixture".into(),
                artist: "Test".into(),
                duration: 3.,
                bpm: 120.,
                audio: "sac:audio/1".into(),
                preview: None,
                dance_moves_paths: Some("sai:dance/1".into()),
                patterns: vec![Pattern {
                    file: "sai:chart/1".into(),
                    sha256: sdo_song::chart::pattern_sha256(&[]),
                    kind: "k".into(),
                    mode: 4,
                    bpm: None,
                    levels: [Some(1); 3],
                    level_error: None,
                }],
                source: "memory".into(),
                sources: BTreeMap::from([(
                    "memory".into(),
                    Source {
                        id: SourceIds::Single("1".into()),
                    },
                )]),
            }],
        })
    }
    fn ui(&self) -> Result<Arc<UiAssets>> {
        Ok(self.ui.clone())
    }
    fn characters(&self, outfits: &OutfitOverrides) -> Result<Arc<CharacterAssets>> {
        Ok(Arc::new(CharacterAssets::new(
            Vec::new(),
            Arc::new(Vec::new()),
            Arc::new(WardrobeCatalog::default()),
            outfits,
        )?))
    }
    fn clothing_preview(&self, _key: &ClothingPreviewKey) -> Result<Arc<CharacterAssets>> {
        bail!("No clothes in this fixture")
    }
    fn motion(&self, id: &str) -> Result<Arc<CharacterMotion>> {
        ensure!(id == "sac:motion/rest", "Missing motion {id}");
        Ok(self.motion.clone())
    }
    fn chart(&self, id: &str, _difficulty: Difficulty, bpm: f64) -> Result<Chart> {
        ensure!(id == "sai:chart/1", "Missing chart {id}");
        Ok(Chart {
            bpm,
            header_bpm: bpm,
            title: "Test".into(),
            level: 1,
            notes: vec![Note {
                lane: Lane::Left,
                start: 1.,
                end: 2.,
            }],
            tempo: Vec::new(),
            raw_note_events: 1,
            note_count_warning: None,
            events: Vec::new(),
            raw: Vec::new(),
            header_field_12: 0,
            trailing_bytes: 0,
            additional_lane_events: 0,
        })
    }
    fn audio(
        &self,
        id: &str,
        _expected: f64,
        progress: &mut dyn FnMut(f32) -> bool,
    ) -> Result<DecodedSong> {
        ensure!(id == "sac:audio/1" || id == "blocked", "Missing audio {id}");
        if id == "blocked" {
            self.entered.as_ref().unwrap().send(()).unwrap();
            let (lock, changed) = self.release.as_ref().unwrap().as_ref();
            let guard = lock.lock().unwrap();
            let (guard, timeout) = changed
                .wait_timeout_while(guard, Duration::from_secs(5), |released| !*released)
                .unwrap();
            ensure!(
                *guard && !timeout.timed_out(),
                "Timed out waiting for test release"
            );
        }
        for fraction in [0., 0.5, 1.] {
            ensure!(progress(fraction), "Audio loading cancelled");
        }
        Ok(DecodedSong {
            samples: vec![0.; 8_000 * 3],
            channels: 1,
            sample_rate: 8_000,
        })
    }
    fn dance(
        &self,
        id: &str,
        _chart: &Chart,
        _difficulty: Difficulty,
        bones: usize,
    ) -> Result<SongDance> {
        ensure!(id == "sai:dance/1", "Missing dance {id}");
        ensure!(
            bones == 1,
            "The pipeline must use the selected character's bone count"
        );
        Ok(SongDance {
            groups: vec![sdo_dance::DanceGroup {
                start: 0.,
                seconds: 3.,
                clips: Vec::new(),
            }],
            unit_ms: 1000,
            issues: Vec::new(),
            stage_transforms: 0,
        })
    }
    fn file(&self, id: &str) -> Result<Vec<u8>> {
        ensure!(id == "sac:file/1", "Missing file {id}");
        Ok(b"from archive".to_vec())
    }
    fn file_size(&self, id: &str) -> Result<u64> {
        Ok(self.file(id)?.len() as u64)
    }
}
pub(crate) fn assets() -> AssetLoader {
    AssetLoader::new(MemoryProvider::new())
}
fn song(context: &str) -> LoadRequest<String> {
    LoadRequest {
        verify_audio: true,
        context: context.into(),
        audio: "sac:audio/1".into(),
        pattern: "sai:chart/1".into(),
        expected_pattern_sha256: None,
        dance: Some("sai:dance/1".into()),
        idle: Some(CharacterAnimation {
            motion: "sac:motion/rest".into(),
            frames_per_second: 20.,
        }),
        miss: None,
        character_bones: 1,
        bpm: 120.,
        duration: 3.,
        difficulty: Difficulty::Normal,
    }
}
fn wait(loader: &mut SongAssetLoader<String>) -> Result<PreparedSong<String>> {
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(result) = loader.poll() {
            return result;
        }
        ensure!(Instant::now() < until, "Song loader timed out");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn typed_requests_and_preparation_work_without_a_filesystem_layout() -> Result<()> {
    let assets = assets();
    let catalog = assets.request(request::Catalog)?;
    assert_eq!(
        catalog.select("cn:recording/1", None, "k", 4)?.song.audio,
        "sac:audio/1"
    );
    let ui = assets.request(request::Ui)?;
    let mut selector = sdo_ui::SongSelector::new(catalog, None, "cn:recording/1", "k".into(), 0)?;
    selector.set_preview_assets(&ui);
    assert_eq!(selector.selected().unwrap().id, "cn:recording/1");
    let selected = ui.selection("default", None, None, None)?;
    let skin = assets.request(request::Skin(&selected))?;
    assert_eq!(
        skin.texture(skin.lanes[0].normal[0]).rgba.len(),
        16 * 16 * 4
    );
    let motion = assets.request(request::Motion("sac:motion/rest"))?;
    assert!(Arc::ptr_eq(
        &motion,
        &assets.request(request::Motion("sac:motion/rest"))?
    ));
    assert_eq!(
        assets.request(request::File("sac:file/1"))?,
        b"from archive"
    );
    assert!(assets.request(request::File("missing")).is_err());
    assert!(
        assets
            .request(request::Characters(&Default::default()))?
            .presets
            .is_empty()
    );
    let mut loader = SongAssetLoader::new(assets);
    loader.request(song("selected chart"))?;
    let ready = wait(&mut loader)?;
    assert_eq!(ready.context, "selected chart");
    assert!(ready.chart.notes[0].is_hold());
    assert_eq!(ready.audio.duration(), 3.);
    assert!(ready.idle.is_some());
    assert!(ready.dance_errors.is_empty());
    assert_eq!(loader.progress.stage, LoadStage::Ready);
    assert_eq!(loader.progress.fraction, 1.);
    assert!(loader.poll().is_none());
    Ok(())
}

#[test]
fn superseding_a_load_cancels_old_audio_and_returns_only_the_new_context() -> Result<()> {
    let (entered, observed) = mpsc::sync_channel(1);
    let release = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let mut provider = MemoryProvider::new();
    provider.entered = Some(entered);
    provider.release = Some(release.clone());
    let mut loader = SongAssetLoader::new(AssetLoader::new(provider));
    let mut first = song("old");
    first.audio = "blocked".into();
    loader.request(first)?;
    observed.recv_timeout(Duration::from_secs(5))?;
    loader.request(song("new"))?;
    *release.0.lock().unwrap() = true;
    release.1.notify_one();
    assert_eq!(wait(&mut loader)?.context, "new");
    loader.request(song("cancelled"))?;
    loader.cancel();
    assert!(loader.poll().is_none());
    loader.request(song("after cancellation"))?;
    assert_eq!(wait(&mut loader)?.context, "after cancellation");
    Ok(())
}

#[test]
fn provider_errors_surface_and_the_worker_can_handle_another_request() -> Result<()> {
    let mut loader = SongAssetLoader::new(assets());
    let mut broken = song("bad");
    broken.pattern = "missing".into();
    loader.request(broken)?;
    let error = wait(&mut loader).err().expect("Missing chart should fail");
    assert!(format!("{error:#}").contains("Missing chart missing"));
    loader.request(song("retry"))?;
    assert_eq!(wait(&mut loader)?.context, "retry");
    Ok(())
}

#[test]
fn preparation_checks_expected_hash_before_decoding_audio() -> Result<()> {
    let mut loader = SongAssetLoader::new(assets());
    let mut request = song("wrong chart");
    request.expected_pattern_sha256 = Some("f".repeat(64));
    request.audio = "missing".into();
    loader.request(request)?;
    let error = match wait(&mut loader) {
        Ok(_) => panic!("Mismatched chart was accepted"),
        Err(e) => e,
    };
    assert!(format!("{error:#}").contains("SHA-256"));
    let mut request = song("verified chart");
    request.expected_pattern_sha256 = Some(sdo_song::chart::pattern_sha256(&[]));
    loader.request(request)?;
    let ready = wait(&mut loader)?;
    assert_eq!(ready.pattern_sha256, ready.chart.pattern_sha256());
    Ok(())
}
