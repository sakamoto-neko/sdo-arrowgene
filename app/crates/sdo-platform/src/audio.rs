//! Browser audio buffers use the Web Audio clock for both playback and input.
use anyhow::{Context, Result, ensure};
use std::cell::{Cell, RefCell};
use web_sys::{AudioBuffer, AudioBufferSourceNode, AudioContext};
use web_time::Instant;
thread_local! {static CONTEXT: RefCell<Option<AudioContext>> = const { RefCell::new(None) };}
fn js(e: wasm_bindgen::JsValue) -> anyhow::Error {
    anyhow::anyhow!("Web Audio: {e:?}")
}
fn context() -> Result<AudioContext> {
    CONTEXT.with_borrow_mut(|slot| {
        if let Some(ctx) = slot {
            return Ok(ctx.clone());
        }
        let ctx = AudioContext::new().map_err(js)?;
        *slot = Some(ctx.clone());
        Ok(ctx)
    })
}
pub fn unlock() -> Result<()> {
    let _resume = context()?.resume().map_err(js)?;
    Ok(())
}
#[wasm_bindgen::prelude::wasm_bindgen]
extern "C" {
    #[wasm_bindgen(catch,js_name=sdoDecodeRecording)]
    async fn decode_recording(
        blob: web_sys::Blob,
    ) -> Result<wasm_bindgen::JsValue, wasm_bindgen::JsValue>;
}
/// Decode in a worker and return interleaved PCM for the shared song preparation path.
pub async fn decode(path: &str) -> Result<(u32, usize, Vec<f32>)> {
    let data = js_sys::Array::from(
        &decode_recording(crate::asset_cache::blob(path).await?)
            .await
            .map_err(js)?,
    );
    let rate = data.get(0).as_f64().context("Missing sample rate")? as u32;
    let channels = data.get(1).as_f64().context("Missing channel count")? as usize;
    Ok((
        rate,
        channels,
        js_sys::Float32Array::new(&data.get(2)).to_vec(),
    ))
}

pub struct Output {
    ctx: AudioContext,
    buffer: AudioBuffer,
    node: RefCell<Option<AudioBufferSourceNode>>,
    origin: Cell<f64>,
    when: Cell<f64>,
    pub device_name: String,
}
impl Output {
    pub fn new(samples: &[f32], channels: usize, rate: u32, start: f64) -> Result<Self> {
        let ctx = context()?;
        let buffer = ctx
            .create_buffer(
                channels.try_into()?,
                (samples.len() / channels).try_into()?,
                rate as f32,
            )
            .map_err(js)?;
        for i in 0..channels {
            let channel = samples
                .iter()
                .skip(i)
                .step_by(channels)
                .copied()
                .collect::<Vec<_>>();
            buffer.copy_to_channel(&channel, i as i32).map_err(js)?;
        }
        let out = Self {
            ctx,
            buffer,
            node: Default::default(),
            origin: Cell::new(start),
            when: Cell::new(0.),
            device_name: "Web Audio".into(),
        };
        out.set_paused(false)?;
        Ok(out)
    }
    fn play(&self, at: f64) -> Result<()> {
        let node = self.ctx.create_buffer_source().map_err(js)?;
        node.set_buffer(Some(&self.buffer));
        node.connect_with_audio_node(&self.ctx.destination())
            .map_err(js)?;
        node.start_with_when_and_grain_offset(at, self.origin.get())
            .map_err(js)?;
        self.when.set(at);
        self.node.replace(Some(node));
        Ok(())
    }
    fn stop(&self) {
        if let Some(n) = self.node.take() {
            let _ = web_sys::AudioScheduledSourceNode::stop(&n);
            let _ = n.disconnect();
        }
    }
    pub fn duration(&self) -> f64 {
        self.buffer.duration()
    }
    pub fn paused(&self) -> bool {
        self.node.borrow().is_none()
    }
    pub fn position(&self) -> f64 {
        self.position_at(Instant::now())
    }
    pub fn position_at(&self, at: Instant) -> f64 {
        let elapsed = if self.paused() {
            0.
        } else {
            (self.ctx.current_time()
                - self.when.get()
                - Instant::now().saturating_duration_since(at).as_secs_f64())
            .max(0.)
        };
        (self.origin.get() + elapsed).min(self.duration())
    }
    pub fn schedule_start(&self, deadline: Instant) -> Result<()> {
        let wait = deadline
            .checked_duration_since(Instant::now())
            .context("Start deadline passed")?;
        self.stop();
        self.origin.set(0.);
        self.play(self.ctx.current_time() + wait.as_secs_f64())
    }
    pub fn set_paused(&self, paused: bool) -> Result<()> {
        if self.paused() == paused {
            return Ok(());
        }
        if paused {
            self.origin.set(self.position());
            self.stop();
            Ok(())
        } else {
            self.play(self.ctx.current_time())
        }
    }
    pub fn seek(&self, time: f64) -> Result<()> {
        ensure!(time.is_finite(), "Invalid seek");
        let playing = !self.paused();
        self.stop();
        self.origin.set(time.clamp(0., self.duration()));
        if playing {
            self.play(self.ctx.current_time())?
        }
        Ok(())
    }
    pub fn check(&self) -> Result<()> {
        ensure!(
            self.ctx.state() != web_sys::AudioContextState::Closed,
            "Audio context closed"
        );
        Ok(())
    }
}
impl Drop for Output {
    fn drop(&mut self) {
        self.stop();
    }
}
