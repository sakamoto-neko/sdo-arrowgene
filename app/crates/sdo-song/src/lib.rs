//! Song identities, chart metadata and decoded four-key timelines.
#[cfg(feature = "catalog")]
pub mod catalog;
pub mod chart;
#[cfg(feature = "catalog")]
pub use catalog::*;
pub use chart::{Chart, ChartHeader, Difficulty};
