//! Room authority, verified chart preparation, and deterministic input replay.
use anyhow::{Result, ensure};
use sdo_gameplay::Practice;
use sdo_net::*;
use sdo_song::{Chart, Difficulty};
#[cfg(not(target_arch = "wasm32"))]
mod password;
mod referee;
#[cfg(not(target_arch = "wasm32"))]
mod server;
pub use referee::Referee as Replay;
#[cfg(not(target_arch = "wasm32"))]
pub use server::Server;
pub fn prepare_chart(spec: &ChartSpec, raw: Vec<u8>) -> Result<Practice> {
    spec.validate()?;
    ensure!(
        raw.len() <= MAX_CHART_BYTES,
        "Chart exceeds online size limit"
    );
    ensure!(
        sdo_song::chart::pattern_sha256(&raw) == spec.pattern_sha256,
        "Chart bytes do not match the room SHA-256"
    );
    let chart = Chart::parse_with_bpm(
        raw,
        Difficulty::ALL[spec.difficulty as usize],
        Some(spec.bpm),
    )?;
    ensure!(
        chart.notes.len() <= 20_000,
        "Chart exceeds online note limit"
    );
    let duration = spec.duration_us as f64 / 1e6;
    ensure!(
        chart.notes.iter().all(|n| n.end <= duration + 5.),
        "Chart extends beyond the song"
    );
    Ok(chart.into_practice(
        duration,
        sdo_gameplay::judgment::find(&spec.judgment).unwrap(),
        Difficulty::ALL[spec.difficulty as usize],
    ))
}
