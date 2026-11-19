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

#[derive(Debug, Serialize, Deserialize)]
pub struct AssetFile {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}
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

pub fn permitted(path: &str) -> bool {
    if path.split('/').any(|part| {
        part.is_empty()
            || part.starts_with('.')
            || part.ends_with([' ', '.'])
            || part
                .chars()
                .any(|c| c.is_control() || "\\:*?\"<>|".contains(c))
            || reserved_name(part)
    }) {
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
impl Manifest {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.version == VERSION,
            "Unsupported asset manifest version {}",
            self.version
        );
        for (id, file) in &self.files {
            ensure!(
                *id == key(&file.path)
                    && permitted(&file.path)
                    && file.bytes <= MAX_FILE
                    && file.sha256.len() == 64
                    && file
                        .sha256
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
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
