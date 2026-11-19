//! Decoded song playback and a latency-aware audio-output clock.
use crate::DecodedSong;
use anyhow::{Context, Result, bail, ensure};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

struct Shared {
    start_ns: AtomicU64,
    paused: AtomicBool,
    seek: AtomicU64,
    seek_origin: AtomicU64,
    sequence: AtomicU64,
    generation: AtomicU64,
    base_generation: AtomicU64,
    base_frame: AtomicU64,
    base_ns: AtomicU64,
    ready: AtomicBool,
    failed: AtomicBool,
}
pub struct AudioPlayer {
    _device: cpal::Device,
    stream: cpal::Stream,
    shared: Arc<Shared>,
    epoch: Instant,
    rate: u32,
    frames: u64,
    pub device_name: String,
}
impl AudioPlayer {
    pub fn new(song: DecodedSong, seconds: f64) -> Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .context("No audio output device is available")?;
        let device_name = device
            .description()
            .map(|d| d.name().to_owned())
            .unwrap_or_else(|_| "default output".into());
        let default = device.default_output_config()?;
        // Prefer the song's rate so that no resampling is needed.
        let config = device
            .supported_output_configs()?
            .find(|c| {
                c.channels() as usize == song.channels
                    && c.min_sample_rate() <= song.sample_rate
                    && c.max_sample_rate() >= song.sample_rate
                    && c.sample_format() == cpal::SampleFormat::F32
            })
            .map(|c| c.with_sample_rate(song.sample_rate))
            .unwrap_or(default);
        let format = config.sample_format();
        let config: cpal::StreamConfig = config.into();
        let pcm = resample(song, config.sample_rate, config.channels as usize);
        let frames = (pcm.len() / config.channels as usize) as u64;
        let origin = (seconds.max(0.0) * config.sample_rate as f64).round() as u64;
        let shared = Arc::new(Shared {
            start_ns: AtomicU64::new(0),
            paused: AtomicBool::new(true),
            seek: AtomicU64::new(origin.min(frames)),
            seek_origin: AtomicU64::new(origin.min(frames)),
            sequence: AtomicU64::new(0),
            generation: AtomicU64::new(0),
            base_generation: AtomicU64::new(u64::MAX),
            base_frame: AtomicU64::new(origin.min(frames)),
            base_ns: AtomicU64::new(0),
            ready: AtomicBool::new(false),
            failed: AtomicBool::new(false),
        });
        let epoch = Instant::now();
        let stream = match format {
            cpal::SampleFormat::F32 => build::<f32>(&device, &config, pcm, shared.clone(), epoch)?,
            cpal::SampleFormat::I16 => build::<i16>(&device, &config, pcm, shared.clone(), epoch)?,
            cpal::SampleFormat::U16 => build::<u16>(&device, &config, pcm, shared.clone(), epoch)?,
            other => bail!("Unsupported audio output sample format {other:?}"),
        };
        Ok(Self {
            _device: device,
            stream,
            shared,
            epoch,
            rate: config.sample_rate,
            frames,
            device_name,
        })
    }
    pub fn duration(&self) -> f64 {
        self.frames as f64 / self.rate as f64
    }
    pub fn paused(&self) -> bool {
        self.shared.paused.load(Ordering::Acquire)
    }
    pub fn position(&self) -> f64 {
        self.position_at(Instant::now())
    }
    pub fn position_at(&self, captured: Instant) -> f64 {
        if !self.shared.ready.load(Ordering::Acquire) || self.paused() {
            return self.shared.seek_origin.load(Ordering::Acquire) as f64 / self.rate as f64;
        }
        let generation = self.shared.generation.load(Ordering::Acquire);
        let (frame, ns, base_generation) = loop {
            let before = self.shared.sequence.load(Ordering::Acquire);
            if before & 1 != 0 {
                std::hint::spin_loop();
                continue;
            }
            let frame = self.shared.base_frame.load(Ordering::Relaxed);
            let ns = self.shared.base_ns.load(Ordering::Relaxed);
            let base_generation = self.shared.base_generation.load(Ordering::Relaxed);
            if before == self.shared.sequence.load(Ordering::Acquire) {
                break (frame, ns, base_generation);
            }
        };
        if generation & 1 != 0 || base_generation != generation {
            return self.shared.seek_origin.load(Ordering::Acquire) as f64 / self.rate as f64;
        }
        let now = captured.saturating_duration_since(self.epoch).as_nanos() as f64;
        let origin = self.shared.seek_origin.load(Ordering::Acquire) as f64;
        (frame as f64 + (now - ns as f64) * self.rate as f64 / 1e9)
            .clamp(origin, self.frames as f64)
            / self.rate as f64
    }
    /// Submit silence until the first sample's output presentation deadline.
    pub fn schedule_start(&self, deadline: Instant) -> Result<()> {
        ensure!(deadline > Instant::now(), "Audio start deadline has passed");
        self.set_paused(true)?;
        self.seek(0.)?;
        let ns = deadline
            .duration_since(self.epoch)
            .as_nanos()
            .min(u64::MAX as u128) as u64;
        self.shared.start_ns.store(ns, Ordering::Release);
        self.shared.paused.store(false, Ordering::Release);
        self.stream.play()?;
        Ok(())
    }
    pub fn set_paused(&self, paused: bool) -> Result<()> {
        if paused == self.paused() {
            return Ok(());
        }
        if paused {
            let frame = (self.position() * self.rate as f64).round() as u64;
            self.stream.pause()?;
            self.shared.paused.store(true, Ordering::Release);
            self.shared.start_ns.store(0, Ordering::Release);
            self.set_cursor(frame);
        } else {
            self.shared.paused.store(false, Ordering::Release);
            self.stream.play()?;
        }
        Ok(())
    }
    fn set_cursor(&self, frame: u64) {
        let frame = frame.min(self.frames);
        self.shared.generation.fetch_add(1, Ordering::AcqRel);
        self.shared.ready.store(false, Ordering::Release);
        self.shared.seek_origin.store(frame, Ordering::Release);
        self.shared.seek.store(frame, Ordering::Release);
        self.shared.generation.fetch_add(1, Ordering::Release);
    }
    pub fn seek(&self, seconds: f64) -> Result<()> {
        ensure!(seconds.is_finite(), "Invalid audio seek position");
        let was_paused = self.paused();
        if !was_paused {
            self.stream.pause()?;
        }
        self.set_cursor((seconds.max(0.0) * self.rate as f64).round() as u64);
        if !was_paused {
            self.stream.play()?;
        }
        Ok(())
    }
    pub fn check(&self) -> Result<()> {
        ensure!(
            !self.shared.failed.load(Ordering::Acquire),
            "Audio device stream failed; restart playback after checking the output device"
        );
        Ok(())
    }
}
fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    pcm: Vec<f32>,
    shared: Arc<Shared>,
    epoch: Instant,
) -> Result<cpal::Stream>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let error_shared = shared.clone();
    let channels = config.channels as usize;
    let rate = config.sample_rate;
    let mut cursor = 0usize;
    Ok(device.build_output_stream(
        config,
        move |output: &mut [T], info: &cpal::OutputCallbackInfo| {
            let generation = shared.generation.load(Ordering::Acquire);
            if generation & 1 != 0 {
                output.fill(T::from_sample(0.0));
                return;
            }
            let callback_time = epoch.elapsed();
            let seek = shared.seek.swap(u64::MAX, Ordering::AcqRel);
            if seek != u64::MAX {
                cursor = seek as usize;
            }
            if shared.paused.load(Ordering::Acquire) {
                output.fill(T::from_sample(0.0));
                return;
            }
            let timestamp = info.timestamp();
            let delay = timestamp
                .playback
                .duration_since(&timestamp.callback)
                .unwrap_or(Duration::ZERO);
            let presentation = (callback_time + delay).as_nanos().min(u64::MAX as u128) as u64;
            let silence = scheduled_silence_frames(
                shared.start_ns.load(Ordering::Acquire),
                presentation,
                rate,
                output.len() / channels,
            );
            output[..silence * channels].fill(T::from_sample(0.0));
            if silence * channels == output.len() {
                return;
            }
            let start = cursor;
            for frame in output[silence * channels..].chunks_mut(channels) {
                for (channel, sample) in frame.iter_mut().enumerate() {
                    *sample = T::from_sample(
                        pcm.get(cursor * channels + channel).copied().unwrap_or(0.0),
                    );
                }
                cursor = cursor.saturating_add(1);
            }
            shared.sequence.fetch_add(1, Ordering::AcqRel);
            shared.base_generation.store(generation, Ordering::Relaxed);
            shared.base_frame.store(start as u64, Ordering::Relaxed);
            shared.base_ns.store(
                presentation.saturating_add((silence as f64 * 1e9 / rate as f64).round() as u64),
                Ordering::Relaxed,
            );
            shared.sequence.fetch_add(1, Ordering::Release);
            shared.ready.store(true, Ordering::Release);
        },
        move |_| {
            error_shared.failed.store(true, Ordering::Release);
        },
        None,
    )?)
}
fn resample(song: DecodedSong, rate: u32, channels: usize) -> Vec<f32> {
    if rate == song.sample_rate && channels == song.channels {
        return song.samples;
    }
    let source_frames = song.samples.len() / song.channels;
    let frames = (source_frames as f64 * rate as f64 / song.sample_rate as f64).round() as usize;
    let mut result = Vec::with_capacity(frames * channels);
    for frame in 0..frames {
        let pos = frame as f64 * song.sample_rate as f64 / rate as f64;
        let left = (pos.floor() as usize).min(source_frames - 1);
        let right = (left + 1).min(source_frames - 1);
        let fraction = (pos - left as f64) as f32;
        for channel in 0..channels {
            let value = if channels == 1 && song.channels > 1 {
                (0..song.channels)
                    .map(|c| {
                        song.samples[left * song.channels + c] * (1.0 - fraction)
                            + song.samples[right * song.channels + c] * fraction
                    })
                    .sum::<f32>()
                    / song.channels as f32
            } else {
                let c = channel.min(song.channels - 1);
                song.samples[left * song.channels + c] * (1.0 - fraction)
                    + song.samples[right * song.channels + c] * fraction
            };
            result.push(value);
        }
    }
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resampling_preserves_duration_and_channels() {
        let song = DecodedSong {
            samples: vec![0.5, -0.5, 1.0, -1.0],
            channels: 2,
            sample_rate: 2,
        };
        let output = resample(song, 4, 2);
        assert_eq!(output.len(), 8);
        assert_eq!(&output[0..2], &[0.5, -0.5]);
    }
}

fn scheduled_silence_frames(deadline: u64, presentation: u64, rate: u32, frames: usize) -> usize {
    ((deadline.saturating_sub(presentation) as u128 * rate as u128).div_ceil(1_000_000_000))
        .min(frames as u128) as usize
}
#[cfg(test)]
mod scheduling_tests {
    #[test]
    fn scheduled_start_splits_output_buffer_at_sample_boundary() {
        assert_eq!(
            super::scheduled_silence_frames(2_000_000_000, 1_000_000_000, 48000, 512),
            512
        );
        assert_eq!(
            super::scheduled_silence_frames(1_005_000_000, 1_000_000_000, 48000, 512),
            240
        );
        assert_eq!(
            super::scheduled_silence_frames(0, 1_000_000_000, 48000, 512),
            0
        );
    }
}

#[cfg(test)]
mod output_tests {
    use super::*;
    #[test]
    fn decoded_audio_identity_includes_recording_and_format() {
        let mut song = DecodedSong {
            samples: vec![0.; 16],
            channels: 1,
            sample_rate: 48000,
        };
        let hash = song.sha256();
        song.samples[0] = 0.25;
        assert_ne!(hash, song.sha256());
        song.samples[0] = 0.;
        song.sample_rate = 44100;
        assert_ne!(hash, song.sha256());
        song.sample_rate = 48000;
        song.channels = 2;
        assert_ne!(hash, song.sha256());
    }
    #[test]
    #[ignore = "Requires an audio output device"]
    fn scheduled_audio_clock_stays_at_zero_then_tracks_the_deadline() -> Result<()> {
        let player = AudioPlayer::new(
            DecodedSong {
                samples: vec![0.; 48000],
                channels: 1,
                sample_rate: 48000,
            },
            0.,
        )?;
        let deadline = Instant::now() + Duration::from_millis(250);
        player.schedule_start(deadline)?;
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(player.position(), 0.);
        std::thread::sleep(Duration::from_millis(300));
        player.check()?;
        let expected = Instant::now().duration_since(deadline).as_secs_f64();
        assert!(
            (player.position() - expected).abs() < 0.05,
            "Audio position {} vs deadline elapsed {expected}",
            player.position()
        );
        player.set_paused(true)?;
        Ok(())
    }
}
