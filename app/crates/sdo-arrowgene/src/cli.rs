//! Command-line options; no content is opened while parsing arguments.
use anyhow::{Context, Result, bail, ensure};
use sdo_assets::{UiAssets, UiSelection};
use sdo_config::ScrollSpeed;
use sdo_gameplay::Judgment;
use sdo_song::Difficulty;
use std::{collections::HashSet, path::PathBuf};

pub(crate) struct Options {
    pub(crate) specified: HashSet<String>,
    pub(crate) selector: bool,
    pub(crate) character_view: bool,
    pub(crate) character: Option<String>,
    pub(crate) verify_characters: bool,
    pub(crate) verify_dances: bool,
    pub(crate) board: Option<String>,
    pub(crate) score_ui: Option<String>,
    pub(crate) list_ui: bool,
    pub(crate) music_region: Option<String>,
    pub(crate) skin: String,
    pub(crate) hit_effect: Option<String>,
    pub(crate) list_hit_effects: bool,
    pub(crate) song: Option<String>,
    pub(crate) list_songs: bool,
    pub(crate) search: Option<String>,
    pub(crate) verify_catalog: bool,
    pub(crate) asset_plan: bool,
    pub(crate) chart: String,
    pub(crate) difficulty: Difficulty,
    pub(crate) preset: String,
    pub(crate) speed: Option<ScrollSpeed>,
    pub(crate) screenshot: Option<PathBuf>,
    pub(crate) time: f64,
    pub(crate) start: f64,
    pub(crate) offset: f64,
    pub(crate) frames: Option<u64>,
    pub(crate) frame_latency: u32,
    pub(crate) verify: bool,
    pub(crate) verify_song: bool,
    pub(crate) autoplay: bool,
    pub(crate) audio_test: Option<f64>,
    pub(crate) preview: Option<Judgment>,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            specified: HashSet::new(),
            selector: false,
            character_view: false,
            character: None,
            verify_characters: false,
            verify_dances: false,
            board: None,
            score_ui: None,
            list_ui: false,
            music_region: None,
            skin: "default".into(),
            hit_effect: None,
            list_hit_effects: false,
            song: None,
            list_songs: false,
            search: None,
            verify_catalog: false,
            asset_plan: false,
            chart: "k".into(),
            difficulty: Difficulty::Easy,
            preset: "keyboard".into(),
            speed: None,
            screenshot: None,
            time: 8.0,
            start: 0.0,
            offset: 0.0,
            frames: None,
            frame_latency: 2,
            verify: false,
            verify_song: false,
            autoplay: false,
            audio_test: None,
            preview: None,
        }
    }
}
impl Options {
    pub(crate) fn parse() -> Result<Option<Self>> {
        Self::parse_args(std::env::args().skip(1))
    }
    pub(crate) fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Option<Self>> {
        let mut out = Self::default();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            out.specified.insert(arg.clone());
            match arg.as_str() {
                "--help" | "-h" => {
                    println!(
                        "sdo-arrowgene — original SDO four-key assets, real song and GN playback

Options:
  -h, --help            Show this help
  --selector            Open the song selector (default without --song)
  --character-view      Open the 3D character preview and preset selector
  --character ID        Select a JSON character preset (default male)
  --verify-characters   Validate all character preset dependencies without GPU/audio
  --verify-dances       Audit every song's choreography and motion dependencies
  --song ID             Play a global music.json ID; with --selector, preselect it
  --list-songs          List catalog IDs, titles, artists and original source IDs
  --search TEXT         Search catalog titles, artists or IDs (implies --list-songs)
  --verify-catalog      Validate music.json and every referenced music/chart file
  --chart k|t           Chart family (default k)
  --difficulty LEVEL    easy (default), normal, hard
  --board ID            JSON-discovered note board (default first available)
  --score-ui ID         JSON-discovered score artwork (default first available)
  --music-region NAME   Optional original-country filter (sdox/sdo-x, usa, etc.)
  --skin NAME           Note-skin folder, catalog ID or name (default Classic 1)
  --hit-effect ID       Hit effect ID or none (default Sparkle)
  --list-hit-effects    List unique four-key hit effects from all regions
  --judgment ID         Select a judgment preset pad|keyboard (default keyboard)
  --list-judgments      Show presets and client-research limitations
  --offset-ms MS        Delay notes relative to audio by MS (default 0)
  --start SECONDS       Begin playback at a song position
  --autoplay            Demonstrate chart playback and judgment UI
  --verify-ui           Discover and validate every UI component without GPU/audio
  --list-ui             List discovered JSON UI components and any failures
  --asset-plan          List JSON/image resources loaded exclusively from assets/ui
  --verify-song         Decode audio and parse selected GN without playing
  --audio-test SECONDS  Play audio and test its clock/pause/seek
  --screenshot FILE     Render to PNG without opening an audio device/window
  --speed 1..8          Visual scroll multiplier (otherwise use saved setting)
  --time SECONDS        Screenshot position (default 8)
  --preview-judgment J  Screenshot feedback: perfect, cool, bad, miss
  --frames COUNT        Report frame submission intervals and close after COUNT frames
  --frame-latency 1|2    Requested presentation queue depth (default 2)

Selector: search, click or Up/Down; Enter play; Escape quit.
Gameplay: D/F/J/K or arrows; Space pause; R restart; F1 next judgment preset;
-/+ scroll speed; Escape song selector.
Pad and Keyboard use native tap-window tables. Points and hold scoring remain approximate."
                    );
                    return Ok(None);
                }
                "--list-judgments" => {
                    for p in sdo_gameplay::judgment::PRESETS {
                        let w = p.windows(120.0);
                        println!(
                            "{}: {} | at 120 BPM: P +/-{:.2} ms, C +/-{:.2} ms, B +/-{:.2} ms | native_taps={}",
                            p.id,
                            p.name,
                            w[0] * 1000.0,
                            w[1] * 1000.0,
                            w[2] * 1000.0,
                            p.verified_native_taps
                        );
                        println!("  {}", p.evidence);
                    }
                    println!(
                        "Pad and Keyboard are the two selectable timing presets. Score and holds still use practice behavior."
                    );
                    return Ok(None);
                }
                "--board" => out.board = Some(args.next().context("--board needs an ID")?),
                "--score-ui" => out.score_ui = Some(args.next().context("--score-ui needs an ID")?),
                "--list-ui" => out.list_ui = true,
                "--music-region" => {
                    out.music_region = Some(args.next().context("--music-region needs a value")?)
                }
                "--skin" => out.skin = args.next().context("--skin needs a value")?,
                "--hit-effect" => {
                    out.hit_effect = Some(args.next().context("--hit-effect needs an ID")?)
                }
                "--list-hit-effects" => out.list_hit_effects = true,
                "--song" => {
                    out.song = Some(args.next().context("--song needs a global catalog ID")?)
                }
                "--selector" => out.selector = true,
                "--character-view" => {
                    out.character_view = true;
                    out.selector = true;
                }
                "--character" => {
                    out.character = Some(args.next().context("--character needs a preset ID")?)
                }
                "--verify-characters" => out.verify_characters = true,
                "--verify-dances" => out.verify_dances = true,
                "--list-songs" => out.list_songs = true,
                "--search" => {
                    out.search = Some(args.next().context("--search needs text")?);
                    out.list_songs = true;
                }
                "--verify-catalog" => out.verify_catalog = true,
                "--asset-plan" => out.asset_plan = true,
                "--chart" => {
                    out.chart = args
                        .next()
                        .context("--chart needs k or t")?
                        .to_ascii_lowercase()
                }
                "--difficulty" => {
                    out.difficulty =
                        Difficulty::parse(&args.next().context("--difficulty needs a level")?)?
                }
                "--speed" => {
                    out.speed = Some(ScrollSpeed::new(
                        args.next().context("--speed needs 1 through 8")?.parse()?,
                    )?)
                }
                "--judgment" => out.preset = args.next().context("--judgment needs an ID")?,
                "--screenshot" => {
                    out.screenshot = Some(args.next().context("--screenshot needs a path")?.into())
                }
                "--time" => out.time = args.next().context("--time needs seconds")?.parse()?,
                "--start" => out.start = args.next().context("--start needs seconds")?.parse()?,
                "--offset-ms" => {
                    out.offset = args
                        .next()
                        .context("--offset-ms needs milliseconds")?
                        .parse::<f64>()?
                        / 1000.0
                }
                "--frame-latency" => {
                    out.frame_latency = args
                        .next()
                        .context("--frame-latency needs 1 or 2")?
                        .parse()?;
                    ensure!(
                        (1..=2).contains(&out.frame_latency),
                        "--frame-latency needs 1 or 2"
                    );
                }
                "--frames" => {
                    out.frames = Some(args.next().context("--frames needs a count")?.parse()?)
                }
                "--audio-test" => {
                    out.audio_test =
                        Some(args.next().context("--audio-test needs seconds")?.parse()?)
                }
                "--preview-judgment" => {
                    out.preview = Some(
                        match args
                            .next()
                            .context("--preview-judgment needs a grade")?
                            .as_str()
                        {
                            "perfect" => Judgment::Perfect,
                            "cool" | "good" => Judgment::Cool,
                            "bad" => Judgment::Bad,
                            "miss" => Judgment::Miss,
                            _ => bail!("Unknown preview grade"),
                        },
                    )
                }
                "--verify-ui" => out.verify = true,
                "--verify-song" => out.verify_song = true,
                "--autoplay" => out.autoplay = true,
                _ => bail!("Unknown option {arg:?}; use --help"),
            }
        }
        ensure!(
            out.time.is_finite() && out.time >= 0.0 && out.start.is_finite() && out.start >= 0.0,
            "Positions must be finite and nonnegative"
        );
        ensure!(
            out.offset.is_finite() && out.offset.abs() <= 5.0,
            "Offset must be in +/-5000 ms"
        );
        ensure!(out.frames != Some(0), "--frames must be greater than zero");
        ensure!(
            out.audio_test
                .is_none_or(|s| s.is_finite() && (0.5..=30.0).contains(&s)),
            "Audio test must last 0.5..30 seconds"
        );
        ensure!(
            ["k", "t"].contains(&out.chart.as_str()),
            "Chart family must be k or t"
        );
        ensure!(
            sdo_gameplay::judgment::find(&out.preset).is_some(),
            "Unknown or unrecovered judgment preset {:?}; use --list-judgments",
            out.preset
        );
        Ok(Some(out))
    }
    pub(crate) fn ui_selection(&self, assets: &UiAssets) -> Result<UiSelection> {
        assets.selection(
            &self.skin,
            self.board.as_deref(),
            self.hit_effect.as_deref(),
            self.score_ui.as_deref(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn removed_flags_are_rejected() {
        for flag in ["-server", "--server", "--region", "--verify-assets"] {
            let error = Options::parse_args([flag.to_owned(), "value".into()])
                .err()
                .expect("removed flag must fail");
            assert!(
                error
                    .to_string()
                    .contains(&format!("Unknown option {flag:?}"))
            );
        }
    }
}
