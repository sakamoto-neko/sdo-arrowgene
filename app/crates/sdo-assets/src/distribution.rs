//! Browser-readable index of the operator's published collection. File URLs use SHA-256.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

pub const VERSION: u32 = 1;
pub const MAX_FILE: u64 = 256 * 1024 * 1024;
pub const MAX_MANIFEST: u64 = 16 * 1024 * 1024;
pub const DEFAULT_UI: [&str; 5] = [
    "ui/note/skin/classic_1/note_skin.json",
    "ui/note/board/note_board.json",
    "ui/note/effect/sparkle/note_effect.json",
    "ui/score/score.json",
    "ui/health/health.json",
];

pub use sdo_platform::remote::AssetFile;
#[derive(Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    /// Lowercase logical path to its case-preserving filename and content identity.
    pub files: BTreeMap<String, AssetFile>,
    pub charts: BTreeMap<String, [Option<u16>; 3]>,
    pub dance_motions: BTreeMap<String, Vec<String>>,
}

pub fn key(path: &str) -> String {
    path.replace('\\', "/").to_ascii_lowercase()
}

fn reserved_name(part: &str) -> bool {
    let stem = part
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end()
        .to_ascii_uppercase();
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL") {
        return true;
    }
    stem.strip_prefix("COM")
        .or_else(|| stem.strip_prefix("LPT"))
        .is_some_and(|suffix| {
            matches!(
                suffix,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        })
}

fn safe_path(path: &str) -> bool {
    !path.split('/').any(|part| {
        part.is_empty()
            || part.starts_with('.')
            || part.ends_with([' ', '.'])
            || part
                .chars()
                .any(|c| c.is_control() || "\\:*?\"<>|".contains(c))
            || reserved_name(part)
    })
}
pub fn permitted(path: &str) -> bool {
    if !safe_path(path) {
        return false;
    }
    let path = key(path);
    if path == "music.json" || DEFAULT_UI.contains(&path.as_str()) {
        return true;
    }
    let Some((dir, name)) = path.rsplit_once('/') else {
        return false;
    };
    let ext = Path::new(name)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    (path.starts_with("music/") && matches!(ext, "ogg" | "gn" | "dps"))
        || (dir == "character/motion" && ext == "mot")
        || (DEFAULT_UI
            .iter()
            .any(|entry| entry.rsplit_once('/').unwrap().0 == dir)
            && matches!(ext, "bmp" | "png" | "dds" | "tga"))
}
pub fn essential(id: &str) -> bool {
    id == "music.json" || id.starts_with("ui/")
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct CharacterManifest {
    pub files: BTreeMap<String, AssetFile>,
    pub initial: std::collections::BTreeSet<String>,
}
impl CharacterManifest {
    pub fn validate(&self) -> Result<()> {
        for (id, file) in &self.files {
            ensure!(
                file.valid_metadata(MAX_FILE)
                    && *id == key(&file.path)
                    && character_resource(&file.path),
                "Invalid character asset: {id}"
            );
        }
        ensure!(
            self.initial.iter().all(|id| self.files.contains_key(id)),
            "Unknown initial character asset"
        );
        Ok(())
    }
}
pub fn character_resource(path: &str) -> bool {
    if !safe_path(path) {
        return false;
    }
    let key = key(path);
    let Some((dir, name)) = key.rsplit_once('/') else {
        return false;
    };
    let ext = Path::new(name)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("");
    match dir {
        "character" => ext == "hrc",
        "character/skin" => matches!(ext, "msh" | "dds" | "bmp" | "png" | "tga"),
        "character/motion" => ext == "mot",
        dir if dir == "character/presets" || dir.starts_with("character/presets/") => ext == "json",
        _ => false,
    }
}
impl Manifest {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.version == VERSION,
            "Unsupported asset manifest version {}",
            self.version
        );
        for (id, file) in &self.files {
            ensure!(
                *id == key(&file.path) && permitted(&file.path) && file.valid_metadata(MAX_FILE),
                "Invalid asset manifest entry: {id}"
            );
        }
        for id in self.charts.keys() {
            ensure!(
                id.starts_with("music/") && id.ends_with(".gn") && self.files.contains_key(id),
                "Invalid chart metadata: {id}"
            );
        }
        for (id, motions) in &self.dance_motions {
            ensure!(
                id.starts_with("music/") && id.ends_with(".dps") && self.files.contains_key(id),
                "Invalid dance metadata: {id}"
            );
            ensure!(
                motions
                    .iter()
                    .all(|m| m.starts_with("character/motion/") && self.files.contains_key(m)),
                "Invalid motion list: {id}"
            );
        }
        for path in DEFAULT_UI.into_iter().chain(["music.json"]) {
            ensure!(
                self.files.contains_key(path),
                "Missing essential asset: {path}"
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restricts_distribution_to_defaults_and_song_files() {
        for path in DEFAULT_UI.into_iter().chain([
            "music.json",
            "ui/note/skin/classic_1/arrow.png",
            "music/SDoM00001.ogg",
            "music/SDoM00001.GN",
            "music/SDoM00001.DPS",
            "character/motion/2600.MOT",
        ]) {
            assert!(permitted(path), "{path}");
        }
        for path in [
            "ui/note/skin/alternate/arrow.png",
            "ui/note/effect/other/effect.json",
            "character/MANBONE.HRC",
            "character/outfit/jacket.png",
            "music/artwork.png",
            "music/../other.ogg",
            "music/CON.ogg",
            "/music/song.ogg",
        ] {
            assert!(!permitted(path), "{path}");
        }
    }
}
