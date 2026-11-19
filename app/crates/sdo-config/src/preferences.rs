use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::path::{Path, PathBuf};
#[cfg(not(target_arch = "wasm32"))]
use std::{
    fs,
    io::Write,
    sync::atomic::{AtomicU64, Ordering},
};

/// Visual scroll multiplier; never alters song playback or judgment times.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub struct ScrollSpeed(u8);
impl Default for ScrollSpeed {
    fn default() -> Self {
        Self(1)
    }
}
impl ScrollSpeed {
    pub fn new(value: u8) -> Result<Self> {
        ensure!(
            (1..=8).contains(&value),
            "Scroll speed must be from 1x to 8x"
        );
        Ok(Self(value))
    }
    pub fn value(self) -> u8 {
        self.0
    }
    pub fn adjusted(self, delta: i8) -> Self {
        Self((self.0 as i16 + delta as i16).clamp(1, 8) as u8)
    }
    pub fn multiplier(self) -> f32 {
        self.0 as f32
    }
}
impl TryFrom<u8> for ScrollSpeed {
    type Error = anyhow::Error;
    fn try_from(value: u8) -> Result<Self> {
        Self::new(value)
    }
}
impl From<ScrollSpeed> for u8 {
    fn from(value: ScrollSpeed) -> Self {
        value.0
    }
}

/// Independent of the collection root and the process's working directory.
pub fn settings_path() -> Result<PathBuf> {
    beside_executable(&std::env::current_exe().context("Cannot locate the executable")?)
}
fn beside_executable(executable: &Path) -> Result<PathBuf> {
    Ok(executable
        .parent()
        .context("Executable has no parent directory")?
        .join("settings.json"))
}
#[cfg(not(target_arch = "wasm32"))]
pub fn load_preferences<T: DeserializeOwned + Default>(path: &Path) -> Result<T> {
    match fs::read(path) {
        Ok(raw) => serde_json::from_slice(&raw)
            .with_context(|| format!("Invalid settings {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Default::default()),
        Err(e) => Err(e).with_context(|| format!("Cannot read settings {}", path.display())),
    }
}
pub fn load_scroll_speed(path: &Path) -> Result<ScrollSpeed> {
    let document: serde_json::Map<String, serde_json::Value> = load_preferences(path)?;
    let Some(value) = document.get("scroll_speed") else {
        return Ok(ScrollSpeed::default());
    };
    let value = value
        .as_u64()
        .context("scroll_speed must be an integer from 1 to 8")?;
    ensure!(value <= 8, "Scroll speed must be from 1x to 8x");
    ScrollSpeed::new(value as u8)
}
pub fn save_scroll_speed(path: &Path, speed: ScrollSpeed) -> Result<()> {
    let mut document: serde_json::Map<String, serde_json::Value> = load_preferences(path)?;
    document.insert("scroll_speed".into(), serde_json::json!(speed.value()));
    save_preferences(path, &document)
}
/// Replace a complete document only after its temporary file has been flushed.
#[cfg(not(target_arch = "wasm32"))]
pub fn save_preferences<T: Serialize>(path: &Path, document: &T) -> Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let parent = path.parent().context("Settings path has no parent")?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(
        ".settings-{}-{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| -> Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        serde_json::to_writer_pretty(&mut file, document)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.with_context(|| format!("Cannot save settings {}", path.display()))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn speed_bounds_and_persistence_preserve_other_preferences() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("sdo-speed-test-{}", std::process::id()));
        fs::create_dir_all(&dir)?;
        let path = dir.join("settings.json");
        fs::write(&path, br#"{"unrelated":"keep","scroll_speed":3}"#)?;
        assert_eq!(load_scroll_speed(&path)?.value(), 3);
        let speed = ScrollSpeed::new(3)?.adjusted(1);
        save_scroll_speed(&path, speed)?;
        assert_eq!(load_scroll_speed(&path)?, speed);
        assert_eq!(
            load_preferences::<serde_json::Value>(&path)?["unrelated"],
            "keep"
        );
        assert_eq!(speed.adjusted(-100).value(), 1);
        assert_eq!(speed.adjusted(100).value(), 8);
        assert!(ScrollSpeed::new(0).is_err() && ScrollSpeed::new(9).is_err());
        fs::write(&path, br#"{"scroll_speed":-1}"#)?;
        assert!(load_scroll_speed(&path).is_err());
        fs::remove_dir_all(dir)?;
        Ok(())
    }
    #[test]
    fn settings_follow_the_executable() -> Result<()> {
        assert_eq!(
            beside_executable(Path::new("/some/install/sdo-arrowgene"))?,
            Path::new("/some/install/settings.json")
        );
        assert_eq!(settings_path()?.parent(), std::env::current_exe()?.parent());
        assert!(serde_json::from_str::<ScrollSpeed>("0").is_err());
        assert!(serde_json::from_str::<ScrollSpeed>("9").is_err());
        assert_eq!(serde_json::from_str::<ScrollSpeed>("8")?.value(), 8);
        Ok(())
    }
}

#[cfg(target_arch = "wasm32")]
pub fn load_preferences<T: DeserializeOwned + Default>(path: &Path) -> Result<T> {
    sdo_platform::storage::get(path)?
        .map(|s| serde_json::from_str(&s))
        .transpose()
        .map(|v| v.unwrap_or_default())
        .map_err(Into::into)
}
#[cfg(target_arch = "wasm32")]
pub fn save_preferences<T: Serialize>(path: &Path, document: &T) -> Result<()> {
    sdo_platform::storage::set(path, &serde_json::to_string(document)?)
}
