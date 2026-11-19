use anyhow::{Context, Result};
use std::path::Path;
fn storage() -> Result<web_sys::Storage> {
    web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .context("Browser save storage unavailable")
}
fn key(path: &Path) -> Result<String> {
    let location = web_sys::window()
        .context("Missing browser window")?
        .location();
    Ok(format!(
        "sdo:{}:{}",
        location.pathname().unwrap_or_default(),
        path.display()
    ))
}
pub fn get(path: &Path) -> Result<Option<String>> {
    storage()?
        .get_item(&key(path)?)
        .map_err(|e| anyhow::anyhow!("Cannot read save: {e:?}"))
}
pub fn set(path: &Path, value: &str) -> Result<()> {
    storage()?
        .set_item(&key(path)?, value)
        .map_err(|e| anyhow::anyhow!("Cannot write save: {e:?}"))
}
