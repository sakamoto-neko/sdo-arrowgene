//! Filesystem helpers for the curated provider. Frontends use sdo-asset-loader.
use sdo_platform::assets as fs;
mod paths;
pub(crate) mod textures;
use crate::{CharacterAssets, ClothingPreviewKey, OutfitOverrides, UiAssets};
use anyhow::{Context, Result, bail, ensure};
pub use paths::{CollectionPaths, DATA_ENV, resource_path};
use std::path::{Path, PathBuf};

pub fn ui(paths: &CollectionPaths) -> Result<UiAssets> {
    UiAssets::load(paths)
}
pub fn characters(paths: &CollectionPaths) -> Result<CharacterAssets> {
    CharacterAssets::load(paths)
}
pub fn with_outfits(base: &CharacterAssets, outfits: &OutfitOverrides) -> Result<CharacterAssets> {
    base.with_outfit_overrides(outfits)
}
pub fn clothing_preview(
    base: &CharacterAssets,
    key: &ClothingPreviewKey,
) -> Result<CharacterAssets> {
    base.preview_source().load(key)
}

/// Reproduces Windows case-insensitive lookup on case-sensitive filesystems, too.
pub fn resolve_resource(root: &Path, logical: &str) -> Result<PathBuf> {
    let relative = resource_path(logical)?;
    let root = fs::canonicalize(root).context("Cannot open resource root")?;
    let exact = root.join(&relative);
    let found = if fs::is_file(&exact) {
        exact
    } else {
        let mut cursor = root.clone();
        for part in relative.components() {
            let part = part.as_os_str();
            let direct = cursor.join(part);
            if fs::exists(&direct) {
                cursor = direct;
                continue;
            }
            let wanted = part.to_string_lossy();
            let matches = fs::read_dir(&cursor)?
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .eq_ignore_ascii_case(&wanted)
                })
                .collect::<Vec<_>>();
            if matches.len() != 1 {
                bail!(
                    "Missing or ambiguous resource {} under {}",
                    wanted,
                    cursor.display()
                );
            }
            cursor = matches[0].path();
        }
        cursor
    };
    let found = fs::canonicalize(&found)?;
    ensure!(
        found.starts_with(&root) && fs::is_file(&found),
        "Resource escapes its component directory"
    );
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[test]
    fn case_insensitive_lookup_and_symlink_containment() {
        let root = std::env::temp_dir().join(format!("sdo-assets-test-{}", std::process::id()));
        fs::create_dir_all(root.join("NOTEIMAGE")).unwrap();
        fs::write(root.join("NOTEIMAGE/TEST.AN"), b"x.png").unwrap();
        assert_eq!(
            resolve_resource(&root, r"noteimage\test.an").unwrap(),
            root.join("NOTEIMAGE/TEST.AN").canonicalize().unwrap()
        );
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/etc/hosts", root.join("escape")).unwrap();
            assert!(resolve_resource(&root, "escape").is_err());
        }
        fs::remove_dir_all(root).unwrap();
    }
}
