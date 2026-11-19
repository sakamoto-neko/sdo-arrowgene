//! Worker entry point for decoding recordings off the 3D client thread.
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct Recording(sdo_audio::DecodedSong);

#[wasm_bindgen]
impl Recording {
    pub fn decode(bytes: &[u8]) -> Result<Recording, String> {
        sdo_audio::DecodedSong::decode(bytes)
            .map(Self)
            .map_err(|e| e.to_string())
    }
    pub fn rate(&self) -> u32 {
        self.0.sample_rate
    }
    pub fn channels(&self) -> usize {
        self.0.channels
    }
    pub fn samples(self) -> Vec<f32> {
        self.0.samples
    }
}
