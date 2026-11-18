//! Single owner of the song asset pipeline and its motion cache.
//! All disk parsing/decoding runs here; the window observes progress and activates audio.
use crate::{AssetLoader, request};
use anyhow::{Context, Result, ensure};
use sdo_assets::CharacterAnimation;
use sdo_audio::DecodedSong;
use sdo_dance::{ReactionMotion, SongDance};
use sdo_song::chart::{Chart, Difficulty};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
    mpsc,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadStage {
    Chart,
    Audio,
    Dance,
    Ready,
}
impl LoadStage {
    pub fn label(self) -> &'static str {
        match self {
            Self::Chart => "Reading chart",
            Self::Audio => "Decoding audio",
            Self::Dance => "Preparing dance",
            Self::Ready => "Ready to play",
        }
    }
}
#[derive(Debug, Clone, Copy)]
pub struct LoadProgress {
    pub stage: LoadStage,
    pub fraction: f32,
}
pub struct LoadRequest<C = ()> {
    pub context: C,
    pub audio: String,
    pub verify_audio: bool,
    pub pattern: String,
    pub expected_pattern_sha256: Option<String>,
    pub dance: Option<String>,
    pub idle: Option<CharacterAnimation>,
    pub miss: Option<CharacterAnimation>,
    pub character_bones: usize,
    pub bpm: f64,
    pub duration: f64,
    pub difficulty: Difficulty,
}
pub struct PreparedSong<C = ()> {
    pub context: C,
    pub chart: Chart,
    pub pattern_sha256: String,
    pub audio: DecodedSong,
    pub audio_sha256: Option<String>,
    pub dance: Option<SongDance>,
    pub idle: Option<ReactionMotion>,
    pub miss: Option<ReactionMotion>,
    pub dance_errors: Vec<String>,
    pub difficulty: Difficulty,
}
enum Event<C> {
    Progress(u64, LoadProgress),
    Complete(u64, Box<Result<PreparedSong<C>>>),
}
pub struct SongAssetLoader<C: Send + 'static = ()> {
    requests: mpsc::Sender<(u64, LoadRequest<C>)>,
    events: mpsc::Receiver<Event<C>>,
    generation: Arc<AtomicU64>,
    active: Option<u64>,
    pub progress: LoadProgress,
}
impl<C: Send + 'static> SongAssetLoader<C> {
    pub fn new(assets: AssetLoader) -> Self {
        let (requests, jobs) = mpsc::channel::<(u64, LoadRequest<C>)>();
        let (events, receiver) = mpsc::channel();
        let generation = Arc::new(AtomicU64::new(0));
        let current = generation.clone();
        std::thread::spawn(move || {
            while let Ok((mut id, mut request)) = jobs.recv() {
                // Coalesce queued selections; a cancelled load cannot win a later request.
                while let Ok((next_id, next_request)) = jobs.try_recv() {
                    id = next_id;
                    request = next_request;
                }
                if current.load(Ordering::Acquire) != id {
                    continue;
                }
                let send = |stage, fraction| {
                    if current.load(Ordering::Acquire) != id {
                        return false;
                    }
                    events
                        .send(Event::Progress(id, LoadProgress { stage, fraction }))
                        .is_ok()
                };
                let result = prepare(&assets, request, &send);
                if current.load(Ordering::Acquire) == id
                    && events.send(Event::Complete(id, Box::new(result))).is_err()
                {
                    break;
                }
            }
        });
        Self {
            requests,
            events: receiver,
            generation,
            active: None,
            progress: LoadProgress {
                stage: LoadStage::Chart,
                fraction: 0.,
            },
        }
    }
    pub fn request(&mut self, request: LoadRequest<C>) -> Result<()> {
        let id = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        self.active = Some(id);
        self.progress = LoadProgress {
            stage: LoadStage::Chart,
            fraction: 0.,
        };
        self.requests
            .send((id, request))
            .map_err(|_| anyhow::anyhow!("Song loader is unavailable"))
    }
    pub fn cancel(&mut self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.active = None;
    }
    pub fn poll(&mut self) -> Option<Result<PreparedSong<C>>> {
        let mut completed = None;
        loop {
            let event = match self.events.try_recv() {
                Ok(event) => event,
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    if self.active.take().is_some() {
                        completed = Some(Err(anyhow::anyhow!(
                            "Song loader stopped before completing the request"
                        )));
                    }
                    break;
                }
            };
            match event {
                Event::Progress(id, progress) if self.active == Some(id) => {
                    self.progress = progress
                }
                Event::Complete(id, result) if self.active == Some(id) => {
                    self.active = None;
                    completed = Some(*result);
                }
                _ => {}
            }
        }
        completed
    }
}
impl<C: Send + 'static> Drop for SongAssetLoader<C> {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn prepare<C>(
    assets: &AssetLoader,
    request: LoadRequest<C>,
    progress: &impl Fn(LoadStage, f32) -> bool,
) -> Result<PreparedSong<C>> {
    ensure!(progress(LoadStage::Chart, 0.), "Loading cancelled");
    let chart = assets
        .request(request::Chart {
            id: &request.pattern,
            difficulty: request.difficulty,
            bpm: request.bpm,
        })
        .context("Could not load the selected chart")?;
    let pattern_sha256 = chart.pattern_sha256();
    if let Some(expected) = &request.expected_pattern_sha256 {
        ensure!(
            *expected == pattern_sha256,
            "Pattern file changed or does not match the expected SHA-256; reload assets before playing"
        );
    }
    ensure!(progress(LoadStage::Audio, 0.1), "Loading cancelled");
    let audio = assets
        .request(request::Audio {
            id: &request.audio,
            expected_seconds: request.duration,
            progress: &mut |fraction| progress(LoadStage::Audio, 0.1 + fraction * 0.75),
        })
        .context("Could not decode the song audio")?;
    let audio_sha256 = request.verify_audio.then(|| audio.sha256());
    ensure!(progress(LoadStage::Dance, 0.85), "Loading cancelled");
    let mut reaction_errors = Vec::new();
    let mut reaction = |role: &str, definition: Option<&CharacterAnimation>| {
        definition.and_then(|animation| {
            match assets.request(request::Reaction {
                animation,
                bones: request.character_bones,
            }) {
                Ok(motion) => Some(motion),
                Err(error) => {
                    reaction_errors.push(format!(
                        "Character {role} omitted ({}): {error:#}",
                        animation.motion
                    ));
                    None
                }
            }
        })
    };
    let idle = reaction("idle", request.idle.as_ref());
    let miss = reaction("miss", request.miss.as_ref());
    let (dance, mut dance_errors) = if request.character_bones == 0 {
        (None, Vec::new())
    } else {
        load_dance(
            assets,
            request.dance.as_deref(),
            &chart,
            request.difficulty,
            request.character_bones,
        )
    };
    dance_errors.extend(reaction_errors);
    ensure!(progress(LoadStage::Ready, 1.), "Loading cancelled");
    Ok(PreparedSong {
        context: request.context,
        chart,
        pattern_sha256,
        audio_sha256,
        audio,
        dance,
        dance_errors,
        idle,
        miss,
        difficulty: request.difficulty,
    })
}

pub fn load_dance(
    assets: &AssetLoader,
    name: Option<&str>,
    chart: &Chart,
    difficulty: Difficulty,
    bones: usize,
) -> (Option<SongDance>, Vec<String>) {
    let loaded = (|| -> Result<SongDance> {
        let name = name.context("Song has no dance script")?;
        assets.request(request::Dance {
            id: name,
            chart,
            difficulty,
            bones,
        })
    })();
    match loaded {
        Ok(dance) => {
            let issues = dance.issues.clone();
            (Some(dance), issues)
        }
        Err(error) => (None, vec![format!("{error:#}")]),
    }
}
