//! Desktop composition root. Content providers never depend on this binary.
mod app;
#[cfg(target_arch = "wasm32")]
mod browser;
mod cli;
#[cfg(not(target_arch = "wasm32"))]
mod diagnostics;
#[cfg(not(target_arch = "wasm32"))]
mod launch;
mod multiplayer;
#[cfg(test)]
mod room;
mod scene;
mod scores;
mod session;
mod settings;
mod sources;
mod wardrobe;

#[cfg(not(target_arch = "wasm32"))]
pub fn run() -> anyhow::Result<()> {
    launch::run()
}

#[cfg(test)]
#[path = "tests/collection.rs"]
mod integration_tests;
#[cfg(test)]
#[path = "tests/multiplayer_scene.rs"]
mod multiplayer_scene_tests;
