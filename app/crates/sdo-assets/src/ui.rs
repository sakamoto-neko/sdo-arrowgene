//! Discover JSON-defined UI components and decode their images into one shared bank.
use crate::curated::CollectionPaths;
use crate::{
    FourKeySkin, GameplayUi, HitAnimation, HitFrame, LaneFrames, Texture, TextureId, TextureStore,
    resolve_resource, score::ScoreDefinition,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

const DIRECTIONS: [&str; 4] = ["left", "down", "up", "right"];

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiSelection {
    pub skin: String,
    pub board: String,
    pub effect: String,
    pub score: String,
}
#[derive(Debug, Clone, Serialize)]
pub struct UiChoice {
    pub id: String,
    pub name: String,
}
#[derive(Debug, Clone, Default, Serialize)]
pub struct UiChoices {
    pub skins: Vec<UiChoice>,
    pub boards: Vec<UiChoice>,
    pub effects: Vec<UiChoice>,
    pub scores: Vec<UiChoice>,
}
#[derive(Debug, Clone, Serialize)]
pub struct AssetIssue {
    pub component: String,
    pub path: PathBuf,
    pub message: String,
}
impl std::fmt::Display for AssetIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} {}: {}",
            self.component,
            self.path.display(),
            self.message
        )
    }
}
pub struct NoteSkin {
    pub choice: UiChoice,
    pub entry: PathBuf,
    legacy_id: String,
    pub lanes: [LaneFrames; 4],
}
impl NoteSkin {
    pub fn catalog_id(&self) -> &str {
        &self.legacy_id
    }
    /// Construct normalized frames from any provider. `entry` identifies the
    /// source for diagnostics; it does not need to be an on-disk file.
    pub fn new(
        choice: UiChoice,
        catalog_id: String,
        entry: PathBuf,
        lanes: [LaneFrames; 4],
    ) -> Self {
        Self {
            choice,
            entry,
            legacy_id: catalog_id,
            lanes,
        }
    }
}
pub struct NoteBoard {
    pub choice: UiChoice,
    pub entry: PathBuf,
    pub image: TextureId,
}
pub struct NoteEffect {
    pub choice: UiChoice,
    pub entry: PathBuf,
    pub animation: HitAnimation,
}
pub struct ScoreUi {
    pub choice: UiChoice,
    pub entry: PathBuf,
    pub ui: GameplayUi,
}
#[derive(Debug, Clone)]
pub struct HealthBar {
    pub choice: UiChoice,
    pub frame: TextureId,
    pub fill: TextureId,
    pub full: TextureId,
    pub fill_offset: [f32; 2],
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HealthDefinition {
    name: String,
    frame: String,
    fill: String,
    full: String,
    fill_offset: [f32; 2],
}
pub struct UiAssets {
    pub root: PathBuf,
    pub textures: Arc<Vec<Texture>>,
    pub resources: Vec<PathBuf>,
    pub skins: Vec<NoteSkin>,
    pub boards: Vec<NoteBoard>,
    pub effects: Vec<NoteEffect>,
    pub scores: Vec<ScoreUi>,
    pub health_bars: Vec<HealthBar>,
    pub issues: Vec<AssetIssue>,
}
#[derive(Serialize)]
pub struct UiReport {
    pub root: PathBuf,
    pub skins: usize,
    pub boards: usize,
    pub effects: usize,
    pub scores: usize,
    pub health_bars: usize,
    pub decoded_textures: usize,
    pub decoded_bytes: usize,
    pub resource_files: usize,
    pub choices: UiChoices,
    pub issues: Vec<AssetIssue>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SkinDefinition {
    schema_version: u32,
    id: String,
    name: String,
    directions: BTreeMap<String, DirectionDefinition>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DirectionDefinition {
    normal: NormalDefinition,
    hold: HoldDefinition,
    receptor: ReceptorDefinition,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NormalDefinition {
    #[serde(rename = "default")]
    regular: Vec<String>,
    eighth: Vec<String>,
    sixteenth: Vec<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HoldDefinition {
    head: Vec<String>,
    body: Vec<String>,
    tail: Vec<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceptorDefinition {
    idle: Vec<String>,
    pressed: Vec<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BoardDefinition {
    name: String,
    image: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EffectDefinition {
    name: String,
    sequence: Vec<String>,
}

fn json<T: serde::de::DeserializeOwned>(loader: &mut TextureStore, path: &Path) -> Result<T> {
    loader.resources.insert(path.to_owned());
    serde_json::from_slice(&fs::read(path)?).with_context(|| format!("Invalid {}", path.display()))
}
fn choice(base: &Path, path: &Path, name: String) -> Result<UiChoice> {
    ensure!(!name.trim().is_empty(), "Component name is empty");
    let relative = path
        .parent()
        .context("Entry has no parent")?
        .strip_prefix(base)?;
    let id = if relative.as_os_str().is_empty() {
        "default".into()
    } else {
        relative.to_string_lossy().replace('\\', "/")
    };
    Ok(UiChoice { id, name })
}
fn sequence(loader: &mut TextureStore, parent: &Path, names: &[String]) -> Result<Vec<TextureId>> {
    names
        .iter()
        .map(|name| loader.image(resolve_resource(parent, name)?))
        .collect()
}
fn discover(base: &Path, entry: &str, issues: &mut Vec<AssetIssue>, kind: &str) -> Vec<PathBuf> {
    fn walk(path: &Path, entry: &str, out: &mut Vec<PathBuf>) -> Result<()> {
        let mut children = fs::read_dir(path)?.collect::<std::io::Result<Vec<_>>>()?;
        children.sort_by_key(|p| p.file_name());
        for child in children {
            let ty = child.file_type()?;
            ensure!(
                !ty.is_symlink(),
                "Symlinked UI component: {}",
                child.path().display()
            );
            if ty.is_dir() {
                walk(&child.path(), entry, out)?;
            } else if child
                .file_name()
                .to_string_lossy()
                .eq_ignore_ascii_case(entry)
            {
                out.push(child.path().canonicalize()?);
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    if let Err(error) = walk(base, entry, &mut files) {
        issues.push(AssetIssue {
            component: kind.into(),
            path: base.into(),
            message: format!("{error:#}"),
        });
    }
    if files.is_empty() {
        issues.push(AssetIssue {
            component: kind.into(),
            path: base.into(),
            message: format!("No {entry} definitions found"),
        });
    }
    files
}

impl UiAssets {
    pub(crate) fn load(paths: &CollectionPaths) -> Result<Self> {
        Self::load_root(paths.assets().join("ui"))
    }
    /// This root is the curated UI tree, never a regional/source fallback.
    pub fn load_root(root: impl AsRef<Path>) -> Result<Self> {
        let root = root
            .as_ref()
            .canonicalize()
            .context("Cannot open assets/ui")?;
        let mut loader = TextureStore::from_root(&root)?;
        let font = loader.utility_font();
        let mut issues = Vec::new();
        let mut skins = Vec::new();
        let mut boards = Vec::new();
        let mut effects = Vec::new();
        let mut scores = Vec::new();
        let mut health_bars = Vec::new();
        macro_rules! load_components {
            ($folder:literal, $entry:literal, $kind:literal, $target:ident, $load:expr) => {{
                let base = root.join($folder);
                let mut ids = HashSet::new();
                for path in discover(&base, $entry, &mut issues, $kind) {
                    let result: Result<_> = ($load)(&mut loader, &base, &path);
                    match result {
                        Ok(component) if ids.insert(component.choice.id.to_ascii_lowercase()) => {
                            $target.push(component)
                        }
                        Ok(_) => issues.push(AssetIssue {
                            component: $kind.into(),
                            path,
                            message: "Duplicate component folder ID".into(),
                        }),
                        Err(error) => issues.push(AssetIssue {
                            component: $kind.into(),
                            path,
                            message: format!("{error:#}"),
                        }),
                    }
                }
            }};
        }
        load_components!(
            "note/skin",
            "note_skin.json",
            "Note skin",
            skins,
            |loader: &mut TextureStore, base: &Path, path: &Path| {
                let definition: SkinDefinition = json(loader, path)?;
                ensure!(
                    definition.schema_version == 1,
                    "Unsupported noteskin schema {}",
                    definition.schema_version
                );
                ensure!(
                    definition.directions.len() == 4,
                    "A skin needs all four directions"
                );
                let parent = path.parent().unwrap();
                let mut lanes = Vec::new();
                for direction in DIRECTIONS {
                    let d = definition
                        .directions
                        .get(direction)
                        .with_context(|| format!("Missing {direction} direction"))?;
                    for (label, frames) in [
                        ("normal", &d.normal.regular),
                        ("hold head", &d.hold.head),
                        ("hold body", &d.hold.body),
                        ("hold tail", &d.hold.tail),
                    ] {
                        ensure!(
                            frames.len() == 4,
                            "{direction} {label} needs four keyframes"
                        );
                    }
                    ensure!(
                        d.normal.eighth.len() == 4 || d.normal.eighth.is_empty(),
                        "Invalid eighth-note keyframes"
                    );
                    ensure!(
                        d.normal.sixteenth.len() == 4 || d.normal.sixteenth.is_empty(),
                        "Invalid sixteenth-note keyframes"
                    );
                    ensure!(
                        d.receptor.idle.len() == 2 && matches!(d.receptor.pressed.len(), 2 | 5),
                        "Invalid {direction} receptor frames"
                    );
                    lanes.push(LaneFrames {
                        normal: sequence(loader, parent, &d.normal.regular)?,
                        eighth: sequence(loader, parent, &d.normal.eighth)?,
                        sixteenth: sequence(loader, parent, &d.normal.sixteenth)?,
                        hold_head: sequence(loader, parent, &d.hold.head)?,
                        hold_body: sequence(loader, parent, &d.hold.body)?,
                        hold_tail: sequence(loader, parent, &d.hold.tail)?,
                        receptor: sequence(loader, parent, &d.receptor.idle)?,
                        pressed: sequence(loader, parent, &d.receptor.pressed)?,
                    });
                }
                Ok(NoteSkin {
                    choice: choice(base, path, definition.name)?,
                    entry: path.into(),
                    legacy_id: definition.id,
                    lanes: lanes.try_into().unwrap(),
                })
            }
        );
        load_components!(
            "note/board",
            "note_board.json",
            "Note board",
            boards,
            |loader: &mut TextureStore, base: &Path, path: &Path| {
                let definition: BoardDefinition = json(loader, path)?;
                let image =
                    loader.image(resolve_resource(path.parent().unwrap(), &definition.image)?)?;
                Ok(NoteBoard {
                    choice: choice(base, path, definition.name)?,
                    entry: path.into(),
                    image,
                })
            }
        );
        load_components!(
            "note/effect",
            "note_effect.json",
            "Note effect",
            effects,
            |loader: &mut TextureStore, base: &Path, path: &Path| {
                let definition: EffectDefinition = json(loader, path)?;
                ensure!(!definition.sequence.is_empty(), "Effect sequence is empty");
                let key = path.to_string_lossy().to_ascii_lowercase();
                ensure!(
                    !["o2jam", "effect_six", "six_eft", "drum"]
                        .iter()
                        .any(|name| key.contains(name)),
                    "Effect is outside four-key arrow mode"
                );
                let c = choice(base, path, definition.name)?;
                let ids = sequence(loader, path.parent().unwrap(), &definition.sequence)?;
                let lanes = if ids.len().is_multiple_of(4) { 4 } else { 1 };
                // The native receptor-burst pass adds light regardless of image format.
                // PNG is a container, not a blend-mode declaration (some PNGs are opaque).
                let frames = ids
                    .into_iter()
                    .map(|texture| HitFrame {
                        texture,
                        additive: true,
                    })
                    .collect();
                let animation = HitAnimation {
                    id: c.id.clone(),
                    label: c.name.clone(),
                    lanes,
                    frames,
                };
                Ok(NoteEffect {
                    choice: c,
                    entry: path.into(),
                    animation,
                })
            }
        );
        load_components!(
            "score",
            "score.json",
            "Score UI",
            scores,
            |loader: &mut TextureStore, base: &Path, path: &Path| {
                loader.resources.insert(path.into());
                let definition = ScoreDefinition::parse(&fs::read(path)?)?;
                let parent = path.parent().unwrap();
                let ui = GameplayUi {
                    judgments: [
                        sequence(loader, parent, &definition.effects["perfect"])?,
                        sequence(loader, parent, &definition.effects["cool"])?,
                        sequence(loader, parent, &definition.effects["bad"])?,
                        sequence(loader, parent, &definition.effects["miss"])?,
                    ],
                    combo: sequence(loader, parent, &definition.effects["combo"])?,
                    combo_digits: sequence(loader, parent, &definition.effects["combo_digits"])?,
                    score_digits: sequence(
                        loader,
                        parent,
                        &(0..10)
                            .map(|i| definition.numbers[&i.to_string()].clone())
                            .collect::<Vec<_>>(),
                    )?,
                    font,
                };
                Ok(ScoreUi {
                    choice: choice(
                        base,
                        path,
                        definition.name.unwrap_or_else(|| {
                            if path.parent() == Some(base) {
                                "Default score".into()
                            } else {
                                path.parent()
                                    .unwrap()
                                    .file_name()
                                    .unwrap()
                                    .to_string_lossy()
                                    .into_owned()
                            }
                        }),
                    )?,
                    entry: path.into(),
                    ui,
                })
            }
        );
        if root.join("health").exists() {
            load_components!(
                "health",
                "health.json",
                "Health bar",
                health_bars,
                |loader: &mut TextureStore, base: &Path, path: &Path| {
                    let d: HealthDefinition = json(loader, path)?;
                    let parent = path.parent().unwrap();
                    let frame = loader.image(resolve_resource(parent, &d.frame)?)?;
                    let fill = loader.image(resolve_resource(parent, &d.fill)?)?;
                    let full = loader.image(resolve_resource(parent, &d.full)?)?;
                    let bounds = &loader.textures[frame.0];
                    for id in [fill, full] {
                        let image = &loader.textures[id.0];
                        ensure!(
                            d.fill_offset.iter().all(|v| v.is_finite() && *v >= 0.0)
                                && d.fill_offset[0] + image.width as f32 <= bounds.width as f32
                                && d.fill_offset[1] + image.height as f32 <= bounds.height as f32,
                            "Health fill exceeds its frame"
                        );
                    }
                    ensure!(
                        loader.textures[fill.0].width == loader.textures[full.0].width
                            && loader.textures[fill.0].height == loader.textures[full.0].height,
                        "Health fill images must have matching dimensions"
                    );
                    Ok(HealthBar {
                        choice: choice(base, path, d.name)?,
                        frame,
                        fill,
                        full,
                        fill_offset: d.fill_offset,
                    })
                }
            );
        }
        Ok(Self {
            root,
            textures: Arc::new(loader.textures),
            resources: loader.resources.into_iter().collect(),
            skins,
            boards,
            effects,
            scores,
            health_bars,
            issues,
        })
    }
    pub fn ensure_ready(&self) -> Result<()> {
        ensure!(
            !self.skins.is_empty() && !self.boards.is_empty() && !self.scores.is_empty(),
            "UI cannot be built: need a valid note skin, note board and score definition. {}",
            self.issues
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ")
        );
        Ok(())
    }
    pub fn choices(&self) -> UiChoices {
        UiChoices {
            skins: self.skins.iter().map(|x| x.choice.clone()).collect(),
            boards: self.boards.iter().map(|x| x.choice.clone()).collect(),
            effects: self.effects.iter().map(|x| x.choice.clone()).collect(),
            scores: self.scores.iter().map(|x| x.choice.clone()).collect(),
        }
    }
    pub fn report(&self) -> UiReport {
        UiReport {
            root: self.root.clone(),
            skins: self.skins.len(),
            boards: self.boards.len(),
            effects: self.effects.len(),
            scores: self.scores.len(),
            health_bars: self.health_bars.len(),
            decoded_textures: self.textures.len(),
            decoded_bytes: self.textures.iter().map(|t| t.rgba.len()).sum(),
            resource_files: self.resources.len(),
            choices: self.choices(),
            issues: self.issues.clone(),
        }
    }
    pub fn selection(
        &self,
        skin: &str,
        board: Option<&str>,
        effect: Option<&str>,
        score: Option<&str>,
    ) -> Result<UiSelection> {
        self.ensure_ready()?;
        let selected_skin = if skin == "default" {
            self.skins
                .iter()
                .find(|s| s.legacy_id == "001")
                .or(self.skins.first())
        } else {
            self.skins.iter().find(|s| {
                s.choice.id.eq_ignore_ascii_case(skin)
                    || s.legacy_id == skin
                    || s.choice.name.eq_ignore_ascii_case(skin)
            })
        }
        .with_context(|| format!("Unknown note skin {skin:?}; use --verify-ui"))?;
        fn select<'a>(
            choices: impl Iterator<Item = &'a UiChoice>,
            requested: Option<&str>,
        ) -> Result<String> {
            let choices = choices.collect::<Vec<_>>();
            let found = if let Some(id) = requested {
                choices
                    .iter()
                    .find(|c| c.id.eq_ignore_ascii_case(id) || c.name.eq_ignore_ascii_case(id))
                    .copied()
            } else {
                choices
                    .iter()
                    .find(|c| c.id == "default")
                    .copied()
                    .or(choices.first().copied())
            };
            Ok(found
                .with_context(|| format!("Unavailable UI component {:?}", requested))?
                .id
                .clone())
        }
        let selected_effect = match effect {
            Some("none") => "none".into(),
            Some(id) => select(self.effects.iter().map(|e| &e.choice), Some(id))?,
            None => self
                .effects
                .iter()
                .find(|e| e.choice.id == "sparkle")
                .or(self.effects.first())
                .map(|e| e.choice.id.clone())
                .unwrap_or_else(|| "none".into()),
        };
        Ok(UiSelection {
            skin: selected_skin.choice.id.clone(),
            board: select(self.boards.iter().map(|b| &b.choice), board)?,
            effect: selected_effect,
            score: select(self.scores.iter().map(|s| &s.choice), score)?,
        })
    }
    pub fn compose(&self, selection: &UiSelection) -> Result<FourKeySkin> {
        let skin = self
            .skins
            .iter()
            .find(|s| s.choice.id == selection.skin)
            .context("Selected skin unavailable")?;
        let board = self
            .boards
            .iter()
            .find(|s| s.choice.id == selection.board)
            .context("Selected board unavailable")?;
        let score = self
            .scores
            .iter()
            .find(|s| s.choice.id == selection.score)
            .context("Selected score UI unavailable")?;
        let effect = if selection.effect == "none" {
            None
        } else {
            Some(
                self.effects
                    .iter()
                    .find(|s| s.choice.id == selection.effect)
                    .context("Selected effect unavailable")?
                    .animation
                    .clone(),
            )
        };
        Ok(FourKeySkin {
            definition: skin.entry.clone(),
            resources: self.resources.clone(),
            textures: self.textures.clone(),
            lanes: skin.lanes.clone(),
            board: board.image,
            ui: score.ui.clone(),
            hit_effect: effect,
            health: self.health_bars.first().cloned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);
    struct Fixture(PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn write(path: &Path, value: serde_json::Value) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
    }
    fn image(path: &Path, color: [u8; 4]) {
        image::RgbaImage::from_pixel(2, 2, image::Rgba(color))
            .save(path)
            .unwrap();
    }
    fn fixture() -> Fixture {
        let root = std::env::temp_dir().join(format!(
            "sdo-json-ui-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        let skin = root.join("note/skin/first");
        let direction = json!({"normal":{"default":vec!["a.PNG";4],"eighth":vec!["b.PNG";4],"sixteenth":[]},
            "hold":{"head":vec!["b.PNG";4],"body":vec!["a.PNG";4],"tail":vec!["b.PNG";4]},"receptor":{"idle":["a.PNG","a.PNG"],"pressed":["a.PNG","b.PNG"]}});
        write(
            &skin.join("note_skin.json"),
            json!({"schema_version":1,"id":"001","name":"First skin","directions":DIRECTIONS.map(|d|(d,direction.clone())).into_iter().collect::<BTreeMap<_,_>>()}),
        );
        image(&skin.join("a.PNG"), [255, 0, 0, 255]);
        image(&skin.join("b.PNG"), [0, 255, 0, 255]);
        for folder in [
            "note/board",
            "note/board/other",
            "note/effect/glow",
            "score",
            "score/other",
        ] {
            let dir = root.join(folder);
            fs::create_dir_all(&dir).unwrap();
            fs::copy(skin.join("a.PNG"), dir.join("a.PNG")).unwrap();
            fs::copy(skin.join("b.PNG"), dir.join("b.PNG")).unwrap();
        }
        write(
            &root.join("note/board/note_board.json"),
            json!({"name":"Default board","image":"a.PNG"}),
        );
        write(
            &root.join("note/board/other/note_board.json"),
            json!({"name":"Other board","image":"b.PNG"}),
        );
        write(
            &root.join("note/effect/glow/note_effect.json"),
            json!({"name":"Glow","sequence":["a.PNG","b.PNG","a.PNG","b.PNG","a.PNG","b.PNG","a.PNG","b.PNG"]}),
        );
        for (folder, color) in [("score", "a.PNG"), ("score/other", "b.PNG")] {
            write(
                &root.join(folder).join("score.json"),
                json!({"numbers":(0..10).map(|i|(i.to_string(),color)).collect::<BTreeMap<_,_>>(),
                "effects":{"perfect":[color],"cool":[color],"bad":[color],"miss":[color],"combo":[color,color],"combo_digits":vec![color;10]}}),
            );
        }
        Fixture(root)
    }
    #[test]
    fn json_components_are_in_memory_and_switch_without_reading_files_again() -> Result<()> {
        let fixture = fixture();
        let assets = UiAssets::load_root(&fixture.0)?;
        assets.ensure_ready()?;
        assert!(assets.issues.is_empty());
        assert_eq!(
            (
                assets.skins.len(),
                assets.boards.len(),
                assets.effects.len(),
                assets.scores.len()
            ),
            (1, 2, 1, 2)
        );
        assert_eq!(assets.textures.len(), 4); // White, utility font and two deduplicated images.
        assert!(assets.resources.iter().all(|p| p.starts_with(&assets.root)));
        let first = assets.selection("default", None, None, None)?;
        assert_eq!(first.board, "default");
        assert_eq!(first.score, "default");
        let a = assets.compose(&first)?;
        assert_ne!(a.note_frame(0, 0), a.hold_head(0, 0));
        assert_eq!(a.note_variant(0, 0, 2), a.hold_head(0, 0));
        assert_eq!(a.note_variant(0, 0, 1), a.note_frame(0, 0)); // An absent optional block uses the supplied normal head.
        assert_eq!(a.pressed_receptor(0, 100), a.hold_head(0, 0));
        assert_eq!(a.receptor(0, 0), a.receptor(0, 1));
        let offline = fixture.0.with_extension("offline");
        fs::rename(&fixture.0, &offline)?;
        let second = assets.selection("first", Some("other"), Some("none"), Some("other"))?;
        let b = assets.compose(&second)?;
        fs::rename(offline, &fixture.0)?;
        assert!(Arc::ptr_eq(&a.textures, &b.textures));
        assert_ne!(a.board, b.board);
        assert_ne!(a.ui.score_digits, b.ui.score_digits);
        assert!(b.hit_effect.is_none());
        assert!(a.hit_effect.as_ref().unwrap().frame(3, 0.0).is_some());
        Ok(())
    }
    #[test]
    fn named_folders_preserve_defaults_and_select_by_catalog_id_or_name() -> Result<()> {
        let fixture = fixture();
        let classic = fixture.0.join("note/skin/classic_1");
        fs::rename(fixture.0.join("note/skin/first"), &classic)?;
        let mut definition: serde_json::Value =
            serde_json::from_slice(&fs::read(classic.join("note_skin.json"))?)?;
        definition["name"] = json!("Classic 1");
        write(&classic.join("note_skin.json"), definition.clone());
        let aura = fixture.0.join("note/skin/aura_1");
        definition["id"] = json!("010");
        definition["name"] = json!("Aura 1");
        write(&aura.join("note_skin.json"), definition);
        let sparkle = fixture.0.join("note/effect/sparkle");
        write(
            &sparkle.join("note_effect.json"),
            json!({"name":"Sparkle","sequence":["a.PNG"]}),
        );
        for folder in [&aura, &sparkle] {
            for name in ["a.PNG", "b.PNG"] {
                fs::copy(classic.join(name), folder.join(name))?;
            }
        }
        let assets = UiAssets::load_root(&fixture.0)?;
        let selection = assets.selection("default", None, None, None)?;
        assert_eq!(selection.skin, "classic_1");
        assert_eq!(selection.effect, "sparkle");
        for requested in ["001", "CLASSIC_1", "Classic 1"] {
            assert_eq!(
                assets.selection(requested, None, None, None)?.skin,
                "classic_1"
            );
        }
        assert_eq!(assets.selection("010", None, None, None)?.skin, "aura_1");
        assert_eq!(
            assets
                .selection("default", None, Some("Sparkle"), None)?
                .effect,
            "sparkle"
        );
        Ok(())
    }
    #[test]
    fn invalid_components_are_flagged_and_legacy_folders_are_never_fallbacks() -> Result<()> {
        let fixture = fixture();
        let bad = fixture.0.join("note/skin/broken/note_skin.json");
        let mut definition: serde_json::Value =
            serde_json::from_slice(&fs::read(fixture.0.join("note/skin/first/note_skin.json"))?)?;
        definition["directions"]["left"]["normal"]["default"] = json!(vec!["../outside.PNG"; 4]);
        write(&bad, definition);
        let bad = bad.canonicalize()?;
        let assets = UiAssets::load_root(&fixture.0)?;
        assert_eq!(assets.skins.len(), 1);
        assert!(
            assets
                .issues
                .iter()
                .any(|issue| issue.path == bad && issue.message.contains("Invalid resource path"))
        );
        assets.ensure_ready()?;
        fs::remove_dir_all(fixture.0.join("note/board"))?;
        // An old regional board image cannot satisfy the JSON-defined board component.
        fs::create_dir_all(fixture.0.join("sdo-x/NOTEIMAGE"))?;
        image(
            &fixture.0.join("sdo-x/NOTEIMAGE/NOTES_BOARD1.PNG"),
            [255; 4],
        );
        let assets = UiAssets::load_root(&fixture.0)?;
        assert!(assets.boards.is_empty());
        assert!(assets.ensure_ready().is_err());
        assert!(
            assets
                .issues
                .iter()
                .any(|issue| issue.component == "Note board")
        );
        Ok(())
    }
    #[test]
    fn health_images_are_preloaded_and_invalid_layout_is_reported() -> Result<()> {
        let fixture = fixture();
        let entry = fixture.0.join("health/health.json");
        let definition = json!({"name":"Health","frame":"frame.PNG","fill":"fill.PNG",
            "full":"fill.PNG","fill_offset":[1,1]});
        write(&entry, definition.clone());
        image::RgbaImage::from_pixel(4, 4, image::Rgba([0, 0, 0, 255]))
            .save(entry.parent().unwrap().join("frame.PNG"))?;
        image(
            &entry.parent().unwrap().join("fill.PNG"),
            [255, 40, 80, 255],
        );
        let assets = UiAssets::load_root(&fixture.0)?;
        assert!(assets.issues.is_empty());
        assert_eq!(assets.report().health_bars, 1);
        fs::remove_dir_all(entry.parent().unwrap())?;
        let skin = assets.compose(&assets.selection("default", None, None, None)?)?;
        let bar = skin.health.as_ref().unwrap();
        assert_eq!(skin.texture(bar.fill).width, 2);
        assert_eq!(bar.fill_offset, [1., 1.]);
        let mut invalid = definition;
        invalid["fill_offset"] = json!([3, 3]);
        write(&entry, invalid);
        image::RgbaImage::from_pixel(4, 4, image::Rgba([0, 0, 0, 255]))
            .save(entry.parent().unwrap().join("frame.PNG"))?;
        image(
            &entry.parent().unwrap().join("fill.PNG"),
            [255, 40, 80, 255],
        );
        let assets = UiAssets::load_root(&fixture.0)?;
        assert!(assets.health_bars.is_empty());
        assert!(
            assets
                .issues
                .iter()
                .any(|i| i.message.contains("exceeds its frame"))
        );
        Ok(())
    }
}
