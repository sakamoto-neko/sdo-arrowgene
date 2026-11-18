//! Background preview decoding. Request generations prevent stale songs from playing.
use crate::{AssetLoader, request};
use sdo_audio::{AudioPlayer, DecodedSong};
use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

struct Request {
    generation: u64,
    id: String,
}
struct Completion {
    generation: u64,
    result: anyhow::Result<DecodedSong>,
}

pub struct PreviewPlayer {
    requests: mpsc::Sender<Request>,
    completed: mpsc::Receiver<Completion>,
    generation: u64,
    current: Arc<AtomicU64>,
    in_flight: Option<u64>,
    pending: Option<(String, Instant)>,
    player: Option<AudioPlayer>,
    status: String,
}
impl PreviewPlayer {
    pub fn new(assets: AssetLoader) -> Self {
        let (requests, rx) = mpsc::channel::<Request>();
        let (tx, completed) = mpsc::channel();
        let current = Arc::new(AtomicU64::new(0));
        let active = current.clone();
        std::thread::spawn(move || {
            while let Ok(mut request) = rx.recv() {
                // Rapid navigation decodes the newest queued request first.
                while let Ok(newer) = rx.try_recv() {
                    request = newer;
                }
                let result = assets.request(request::Audio {
                    id: &request.id,
                    expected_seconds: 0.,
                    progress: &mut |_| active.load(Ordering::Acquire) == request.generation,
                });
                if tx
                    .send(Completion {
                        generation: request.generation,
                        result,
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            requests,
            completed,
            generation: 0,
            current,
            in_flight: None,
            pending: None,
            player: None,
            status: "No preview available".into(),
        }
    }
    pub fn select(&mut self, id: Option<String>) {
        self.stop();
        if let Some(id) = id {
            self.pending = Some((id, Instant::now()));
            self.status = "Loading preview…".into();
        } else {
            self.status = "No preview available".into();
        }
    }
    pub fn stop(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.current.store(self.generation, Ordering::Release);
        self.pending = None;
        self.in_flight = None;
        self.player = None;
    }
    pub fn status(&self) -> &str {
        &self.status
    }
    pub fn progress(&self) -> (f64, f64) {
        self.player
            .as_ref()
            .map(|p| (p.position(), p.duration()))
            .unwrap_or_default()
    }
    pub fn update(&mut self, active: bool) {
        if active
            && self
                .pending
                .as_ref()
                .is_some_and(|(_, since)| since.elapsed() >= Duration::from_millis(120))
        {
            let (id, _) = self.pending.take().unwrap();
            if self
                .requests
                .send(Request {
                    generation: self.generation,
                    id,
                })
                .is_err()
            {
                self.status = "Preview decoder stopped".into();
            } else {
                self.in_flight = Some(self.generation);
            }
        }
        loop {
            let completion = match self.completed.try_recv() {
                Ok(completion) => completion,
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    if self.in_flight.take().is_some() {
                        self.status = "Preview decoder stopped".into();
                    }
                    break;
                }
            };
            if completion.generation != self.generation {
                continue;
            }
            self.in_flight = None;
            match completion
                .result
                .and_then(|song| AudioPlayer::new(song, 0.0))
            {
                Ok(player) => {
                    self.status = "Preview".into();
                    self.player = Some(player);
                }
                Err(error) => {
                    self.status = format!("Preview unavailable: {error:#}");
                }
            }
        }
        if let Some(player) = &self.player {
            let result = (|| -> anyhow::Result<()> {
                player.check()?;
                player.set_paused(!active)?;
                if active && player.position() >= player.duration() {
                    player.seek(0.0)?;
                }
                Ok(())
            })();
            if let Err(error) = result {
                self.status = format!("Preview unavailable: {error:#}");
                self.player = None;
            }
        }
    }
}
impl Drop for PreviewPlayer {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn changing_selection_discards_stale_decode_results_and_restarts_even_for_same_path() {
        let mut player = PreviewPlayer::new(crate::tests::assets());
        player.select(Some("first.ogg".into()));
        let stale = player.generation;
        player.select(Some("second.ogg".into()));
        assert_ne!(player.generation, stale);
        let current = player.generation;
        let (tx, rx) = mpsc::channel();
        player.completed = rx;
        tx.send(Completion {
            generation: stale,
            result: Err(anyhow::anyhow!("stale failure")),
        })
        .unwrap();
        player.update(false);
        assert_eq!(player.status(), "Loading preview…");
        player.select(Some("second.ogg".into()));
        assert_ne!(player.generation, current);
        assert!(player.player.is_none());
        player.select(None);
        assert!(player.pending.is_none());
        assert_eq!(player.status(), "No preview available");
    }
}
