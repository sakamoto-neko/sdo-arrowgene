//! Shared Vorbis decoding and native audio playback.
mod decode;
pub use decode::DecodedSong;
#[cfg(not(target_arch = "wasm32"))]
mod output;
#[cfg(not(target_arch = "wasm32"))]
pub use output::*;
#[cfg(target_arch = "wasm32")]
pub struct AudioPlayer(sdo_platform::audio::Output);
#[cfg(target_arch = "wasm32")]
impl AudioPlayer {
    pub fn new(song: DecodedSong, start: f64) -> anyhow::Result<Self> {
        Ok(Self(sdo_platform::audio::Output::new(
            &song.samples,
            song.channels,
            song.sample_rate,
            start,
        )?))
    }
}
#[cfg(target_arch = "wasm32")]
impl std::ops::Deref for AudioPlayer {
    type Target = sdo_platform::audio::Output;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
