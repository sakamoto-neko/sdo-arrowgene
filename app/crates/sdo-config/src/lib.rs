//! User settings and score persistence, independent of content storage.
mod preferences;
mod scores;
pub use preferences::{
    ScrollSpeed, load_preferences, load_scroll_speed, save_preferences, save_scroll_speed,
    settings_path,
};
pub use scores::{ScoreBook, ScoreKey, ScoreRecord};
