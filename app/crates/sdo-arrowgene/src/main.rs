//! Desktop composition root. Content providers never depend on this binary.
mod app;
mod cli;
mod diagnostics;
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

fn main() -> anyhow::Result<()> {
    launch::run()
}

#[cfg(test)]
#[path = "tests/collection.rs"]
mod integration_tests;
#[cfg(test)]
#[path = "tests/multiplayer_scene.rs"]
mod multiplayer_scene_tests;
