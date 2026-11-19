//! Platform implementations used by the game client and dedicated server.
pub mod assets;
pub mod jobs;
pub mod net;
pub use web_time::{Instant, SystemTime, UNIX_EPOCH};
#[cfg(target_arch = "wasm32")]
pub mod audio;
#[cfg(target_arch = "wasm32")]
pub mod storage;

#[cfg(target_arch = "wasm32")]
pub mod asset_cache;
pub mod remote;
