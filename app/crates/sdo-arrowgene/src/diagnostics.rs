//! Explicit content and audio diagnostics, using the same provider as normal play.
use crate::cli::Options;
use anyhow::{Context, Result, ensure};
use sdo_asset_loader::{AssetLoader, request as asset};
use sdo_audio::{AudioPlayer, DecodedSong};
use sdo_dance::SongDance;
use std::time::{Duration, Instant};

pub(crate) fn audio_test(song: DecodedSong, seconds: f64, start: f64) -> Result<()> {
    let rms = (song
        .samples
        .iter()
        .map(|&s| (s as f64).powi(2))
        .sum::<f64>()
        / song.samples.len() as f64)
        .sqrt();
    ensure!(rms > 0.00001, "Decoded song is silent");
    let player = AudioPlayer::new(song, start)?;
    println!(
        "Output: {} | decoded PCM RMS {:.6}",
        player.device_name, rms
    );
    player.set_paused(false)?;
    let begin = Instant::now();
    let mut previous = start;
    while begin.elapsed().as_secs_f64() < seconds {
        std::thread::sleep(Duration::from_millis(25));
        player.check()?;
        let position = player.position();
        ensure!(
            position + 0.002 >= previous,
            "Audio output clock moved backwards"
        );
        previous = position;
    }
    let end = player.position();
    ensure!(
        end - start > seconds * 0.6,
        "Audio output clock did not advance"
    );
    player.set_paused(true)?;
    let pause = player.position();
    std::thread::sleep(Duration::from_millis(100));
    ensure!(
        (player.position() - pause).abs() < 0.001,
        "Pause moved the song clock"
    );
    player.seek(start + 1.0)?;
    ensure!(
        (player.position() - (start + 1.0)).abs() < 0.002,
        "Seek clock mismatch"
    );
    player.set_paused(false)?;
    std::thread::sleep(Duration::from_millis(150));
    player.check()?;
    ensure!(player.position() > start + 1.0, "Clock did not resume");
    player.set_paused(true)?;
    player.seek((player.duration() - 0.08).max(0.0))?;
    player.set_paused(false)?;
    std::thread::sleep(Duration::from_millis(180));
    player.check()?;
    ensure!(
        (player.position() - player.duration()).abs() < 0.002,
        "Audio clock did not stop at end of song"
    );
    player.set_paused(true)?;
    println!(
        "PASS: output clock advanced {:.3}s; pause, seek, resume and end-of-song passed",
        end - start
    );
    Ok(())
}
pub(crate) fn audit_dances(content: &AssetLoader, options: &Options) -> Result<()> {
    let catalog = content.request(asset::Catalog)?;
    let characters = content.request(asset::Characters(&Default::default()))?;
    let character = if let Some(id) = options.character.as_deref() {
        characters.preset(id)?
    } else {
        characters
            .presets
            .first()
            .context("No character available to verify choreography")?
    };
    let bones = character.skeleton.bones.len();
    let mut clean = 0;
    let mut unavailable = 0;
    let mut rows = Vec::new();
    for song in &catalog.songs {
        let result = (|| -> Result<SongDance> {
            let dance = song
                .dance_moves_paths
                .as_deref()
                .context("No dance script")?;
            let pattern = song.pattern("k", 4).or_else(|_| song.pattern("t", 4))?;
            let chart = content.request(asset::Chart {
                id: &pattern.file,
                difficulty: options.difficulty,
                bpm: pattern.bpm.unwrap_or(song.bpm),
            })?;
            content.request(asset::Dance {
                id: dance,
                chart: &chart,
                difficulty: options.difficulty,
                bones,
            })
        })();
        match result {
            Ok(d) if d.issues.is_empty() => clean += 1,
            Ok(d) => {
                if !d.has_motion() {
                    unavailable += 1;
                }
                rows.push(serde_json::json!({"id":song.id,"title":song.title,"playable_motion":d.has_motion(),"issues":d.issues}));
            }
            Err(e) => {
                unavailable += 1;
                rows.push(serde_json::json!({"id":song.id,"title":song.title,"playable_motion":false,"issues":[format!("{e:#}")]}));
            }
        }
    }
    println!(
        "{}",
        serde_json::to_string_pretty(
            &serde_json::json!({"songs":catalog.songs.len(),"fully_supported_songs":clean,"songs_with_gaps":rows.len(),"songs_without_playable_motion":unavailable,"issues":rows})
        )?
    );
    ensure!(
        rows.is_empty(),
        "Some songs have choreography gaps; see audit report"
    );
    Ok(())
}
