//! Curated filesystem locations and checked native relative references.
use anyhow::{Context, Result, bail};
use sdo_platform::assets as fs;
use std::{
    env,
    path::{Component, Path, PathBuf},
};

pub const DATA_ENV: &str = "SDO_ARROWGENE_DATA";
#[derive(Debug, Clone)]
pub struct CollectionPaths {
    root: PathBuf,
    assets: PathBuf,
}
impl CollectionPaths {
    pub fn from_env() -> Result<Self> {
        let root = env::var_os(DATA_ENV)
            .context("Set SDO_ARROWGENE_DATA to the SDO Arrowgene collection directory")?;
        Self::new(root)
    }
    pub fn new(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        let root = fs::canonicalize(&root).with_context(|| {
            format!(
                "Cannot open the SDO collection directory {}",
                root.display()
            )
        })?;
        for dir in ["assets", "assets/music"] {
            if !root.join(dir).is_dir() {
                bail!("Missing {dir} directory under {}", root.display());
            }
        }
        let assets = root.join("assets");
        Ok(Self { root, assets })
    }
    /// Open the Custom Assets directory itself, independent of its parent name.
    pub fn from_assets(assets: impl Into<PathBuf>) -> Result<Self> {
        let assets = fs::canonicalize(Into::<PathBuf>::into(assets))
            .context("Cannot open Custom Assets folder")?;
        if !fs::is_dir(&assets) {
            bail!("Custom Assets path must be a directory");
        }
        let root = assets.parent().unwrap_or(&assets).to_owned();
        Ok(Self { root, assets })
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    /// Runtime resources, including music.json and its relative music paths.
    pub fn assets(&self) -> &Path {
        &self.assets
    }
}

/// Windows resource names use backslashes. Reject absolute paths and traversal.
pub fn resource_path(value: &str) -> Result<PathBuf> {
    let value = value.replace('\\', "/");
    if value.is_empty()
        || value.contains(':')
        || value.contains('\0')
        || value.split('/').any(|x| x == "..")
    {
        bail!("Invalid resource path {value:?}");
    }
    let path = PathBuf::from(value);
    if path
        .components()
        .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
    {
        bail!("Expected a relative resource path: {}", path.display());
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn windows_paths_are_relative() {
        assert_eq!(
            resource_path(r"NoteImage_5\judgeline.an").unwrap(),
            PathBuf::from("NoteImage_5/judgeline.an")
        );
        for bad in ["../secret", r"foo\..\secret", "/absolute", "C:/Windows", ""] {
            assert!(resource_path(bad).is_err(), "{bad}");
        }
    }
}
