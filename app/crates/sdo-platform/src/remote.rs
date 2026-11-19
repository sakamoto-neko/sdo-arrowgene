//! Endpoint and integrity rules shared by disk and browser asset caches.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AssetFile {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}
impl AssetFile {
    pub fn valid_metadata(&self, limit: u64) -> bool {
        self.bytes <= limit
            && self.sha256.len() == 64
            && self
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }
    pub fn url(&self, endpoint: &Url) -> Result<Url> {
        Ok(endpoint.join(&format!("files/{}", self.sha256))?)
    }
    pub fn matches(&self, len: u64, digest: Sha256) -> bool {
        len == self.bytes && format!("{:x}", digest.finalize()) == self.sha256
    }
}
pub fn endpoint(url: &str) -> Result<Url> {
    let mut endpoint = Url::parse(url.trim()).context("Invalid asset server URL")?;
    ensure!(
        matches!(endpoint.scheme(), "http" | "https")
            && endpoint.host().is_some()
            && endpoint.query().is_none()
            && endpoint.fragment().is_none()
            && endpoint.username().is_empty()
            && endpoint.password().is_none(),
        "Use an HTTP(S) asset URL without credentials, query, or fragment"
    );
    if !endpoint.path().ends_with('/') {
        endpoint.path_segments_mut().unwrap().push("");
    }
    Ok(endpoint)
}
