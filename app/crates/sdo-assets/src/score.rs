//! Typed, named score UI definitions. Only JSON/image files are read.
use crate::curated::resource_path;
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use std::collections::BTreeMap;

const EFFECTS: [&str; 6] = ["perfect", "cool", "bad", "miss", "combo", "combo_digits"];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ScoreDefinition {
    #[serde(default)]
    pub(super) name: Option<String>,
    pub(super) numbers: BTreeMap<String, String>,
    pub(super) effects: BTreeMap<String, Vec<String>>,
}
impl ScoreDefinition {
    pub(super) fn parse(bytes: &[u8]) -> Result<Self> {
        let definition: Self = serde_json::from_slice(bytes).context("Invalid score.json")?;
        ensure!(
            definition.numbers.len() == 10,
            "score.json needs numbers 0 through 9"
        );
        for digit in 0..10 {
            let name = definition
                .numbers
                .get(&digit.to_string())
                .with_context(|| format!("Missing score number {digit}"))?;
            resource_path(name)?;
        }
        ensure!(
            definition.effects.len() == EFFECTS.len(),
            "score.json needs six named effects"
        );
        for effect in EFFECTS {
            let frames = definition
                .effects
                .get(effect)
                .with_context(|| format!("Missing score effect {effect}"))?;
            ensure!(!frames.is_empty(), "Empty score effect {effect}");
            for frame in frames {
                resource_path(frame)?;
            }
        }
        ensure!(
            definition.effects["combo_digits"].len() == 10,
            "Combo digits need ten frames"
        );
        Ok(definition)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    fn example() -> serde_json::Value {
        let numbers = (0..10)
            .map(|digit| {
                (
                    digit.to_string(),
                    if digit % 2 == 0 { "b.PNG" } else { "a.PNG" },
                )
            })
            .collect::<BTreeMap<_, _>>();
        serde_json::json!({
            "numbers": numbers,
            "effects": {
                "miss": ["b.PNG"], "cool": ["b.PNG"], "bad": ["a.PNG"],
                "perfect": ["a.PNG"], "combo": ["b.PNG", "b.PNG", "a.PNG"],
                "combo_digits": vec!["a.PNG"; 10]
            }
        })
    }

    #[test]
    fn named_sequences_and_configured_number_paths_load_without_legacy_lists() -> Result<()> {
        let root = std::env::temp_dir().join(format!("sdo-score-json-{}", std::process::id()));
        let score = root.join("score");
        fs::create_dir_all(&score)?;
        image::RgbaImage::from_pixel(1, 1, image::Rgba([255, 0, 0, 255]))
            .save(score.join("a.PNG"))?;
        image::RgbaImage::from_pixel(1, 1, image::Rgba([0, 255, 0, 255]))
            .save(score.join("b.PNG"))?;
        fs::write(score.join("score.json"), serde_json::to_vec(&example())?)?;
        let assets = crate::UiAssets::load_root(&root)?;
        let ui = &assets.scores[0].ui;
        assert_eq!(ui.combo.len(), 3);
        assert_eq!(ui.combo[0], ui.combo[1]);
        assert_ne!(ui.combo[1], ui.combo[2]);
        assert_eq!(ui.judgments[0][0], ui.score_digits[1]); // Perfect -> a
        assert_eq!(ui.judgments[1][0], ui.score_digits[0]); // Cool -> b
        assert_eq!(ui.judgments[2][0], ui.score_digits[1]); // Bad -> a
        assert_eq!(ui.judgments[3][0], ui.score_digits[0]); // Miss -> b
        assert_eq!(assets.resources.len(), 3); // JSON and two PNGs, no DGE/AN.
        assert_eq!(ui.score_digits.len(), 10);
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn missing_numbers_empty_sequences_and_unsafe_paths_are_rejected() {
        let mut missing = example();
        missing["numbers"].as_object_mut().unwrap().remove("9");
        assert!(ScoreDefinition::parse(&serde_json::to_vec(&missing).unwrap()).is_err());
        let mut empty = example();
        empty["effects"]["cool"] = serde_json::json!([]);
        assert!(ScoreDefinition::parse(&serde_json::to_vec(&empty).unwrap()).is_err());
        let mut unsafe_path = example();
        unsafe_path["effects"]["combo"] = serde_json::json!(["../escape.PNG"]);
        assert!(ScoreDefinition::parse(&serde_json::to_vec(&unsafe_path).unwrap()).is_err());
    }

    #[test]
    #[ignore = "Requires the curated local score folder"]
    fn curated_score_json_loads_all_original_frames() -> Result<()> {
        let root = crate::curated::CollectionPaths::from_env()?
            .assets()
            .join("ui");
        let assets = crate::UiAssets::load_root(&root)?;
        let ui = &assets.scores[0].ui;
        assert_eq!(ui.judgments.each_ref().map(Vec::len), [21; 4]);
        assert_eq!(ui.combo.len(), 21);
        assert_eq!(ui.combo_digits.len(), 10);
        assert_eq!(ui.score_digits.len(), 10);
        let resources = assets
            .resources
            .iter()
            .filter(|p| p.starts_with(root.join("score")))
            .collect::<Vec<_>>();
        assert_eq!(resources.len(), 26); // score.json + the 25 original PNGs.
        assert!(
            resources
                .iter()
                .all(|path| path.extension().is_some_and(
                    |ext| ext.eq_ignore_ascii_case("json") || ext.eq_ignore_ascii_case("png")
                ))
        );
        Ok(())
    }
}
