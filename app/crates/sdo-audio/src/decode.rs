use anyhow::{Context, Result, ensure};
use std::io::{Cursor, Read, Seek};

pub struct DecodedSong {
    pub samples: Vec<f32>,
    pub channels: usize,
    pub sample_rate: u32,
}
impl DecodedSong {
    pub fn decode(bytes: impl AsRef<[u8]>) -> Result<Self> {
        Self::decode_with_progress(Cursor::new(bytes.as_ref()), 0., |_| true)
    }
    /// Reports decoded audio frames against the catalog duration; false cancels decoding.
    pub fn decode_with_progress(
        source: impl Read + Seek,
        expected_seconds: f64,
        mut progress: impl FnMut(f32) -> bool,
    ) -> Result<Self> {
        let mut reader = lewton::inside_ogg::OggStreamReader::new(source)
            .context("Cannot decode Vorbis audio")?;
        let channels = reader.ident_hdr.audio_channels as usize;
        let sample_rate = reader.ident_hdr.audio_sample_rate;
        ensure!(channels > 0 && sample_rate > 0, "Invalid Vorbis stream");
        let mut samples = Vec::new();
        let expected = expected_seconds * sample_rate as f64 * channels as f64;
        let mut reported = -1.0f32;
        while let Some(packet) = reader
            .read_dec_packet_itl()
            .context("Vorbis packet decode failed")?
        {
            samples.extend(packet.into_iter().map(|x| x as f32 / 32768.0));
            let fraction = if expected > 0. {
                (samples.len() as f64 / expected).min(0.99) as f32
            } else {
                0.
            };
            if fraction >= reported + 0.01 {
                ensure!(progress(fraction), "Audio loading cancelled");
                reported = fraction;
            }
        }
        ensure!(progress(1.), "Audio loading cancelled");
        ensure!(!samples.is_empty(), "Song decoded to zero audio frames");
        Ok(Self {
            samples,
            channels,
            sample_rate,
        })
    }
    /// Identity of the decoded recording, including its original rate and channels.
    pub fn sha256(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut hash = Sha256::new();
        hash.update((self.channels as u64).to_le_bytes());
        hash.update(self.sample_rate.to_le_bytes());
        for sample in &self.samples {
            hash.update(sample.to_le_bytes());
        }
        format!("{:x}", hash.finalize())
    }
    pub fn duration(&self) -> f64 {
        self.samples.len() as f64 / self.channels as f64 / self.sample_rate as f64
    }
}
