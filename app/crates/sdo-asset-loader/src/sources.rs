use serde::{Deserialize, Serialize};
/// The sole runtime library location. SDO installations are import inputs only.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SourceSettings {
    pub path: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub remote_asset_url: String,
}
impl<'de> Deserialize<'de> for SourceSettings {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Document {
            path: Option<String>,
            remote_asset_url: String,
            custom_assets: Option<LegacyCustom>,
        }
        #[derive(Deserialize)]
        struct LegacyCustom {
            path: String,
        }
        let value = Document::deserialize(deserializer)?;
        Ok(Self {
            remote_asset_url: value.remote_asset_url,
            path: value
                .path
                .or_else(|| value.custom_assets.map(|c| c.path))
                .unwrap_or_default(),
        })
    }
}
#[derive(Debug, Clone, Default)]
pub struct SourceReport {
    pub name: String,
    pub songs: usize,
    pub skins: usize,
    pub characters: usize,
    pub issues: Vec<String>,
}
#[cfg(not(target_arch = "wasm32"))]
pub use crate::sources_native::{initial_load, open};
