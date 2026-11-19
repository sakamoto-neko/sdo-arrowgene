//! Browser composition root. All menus, gameplay, wardrobe and room scenes run in App.
use crate::{app::App, cli::Options, scores::ScoreStore, settings::SettingsStore};
use anyhow::{Context, Result};
use sdo_asset_loader::request as asset;
use std::{path::PathBuf, sync::Arc};
use wasm_bindgen::{JsCast, prelude::*};
use winit::{
    event_loop::EventLoop,
    platform::web::{EventLoopExtWebSys, WindowAttributesExtWebSys},
    window::Window,
};

#[wasm_bindgen]
pub fn enable_audio() -> Result<(), String> {
    sdo_platform::audio::unlock().map_err(|e| e.to_string())
}
#[wasm_bindgen]
pub async fn start_client(endpoint: String, socket: String) -> Result<(), String> {
    start(endpoint, socket).await.map_err(|e| format!("{e:#}"))
}
async fn start(endpoint: String, socket: String) -> Result<()> {
    std::panic::set_hook(Box::new(|info| {
        web_sys::console::error_1(&info.to_string().into())
    }));
    let content = sdo_asset_loader::browser::BrowserAssets::open(&endpoint).await?;
    let catalog = content.request(asset::Catalog)?;
    let ui = content.request(asset::Ui)?;
    let mut settings = SettingsStore::load(PathBuf::from("settings.json"));
    settings.initialize_sources();
    if !settings.values.outfits.is_empty() {
        let _ = sdo_platform::assets::hydrate(|| {
            content.request(asset::Characters(&settings.values.outfits))
        })
        .await;
    }
    let options = Options {
        remote_asset_url: Some(endpoint.clone()),
        ..Options::default()
    };
    let mut app = App::new(
        content,
        options,
        catalog,
        ui,
        None,
        settings,
        (
            ScoreStore::load("score.json".into()),
            ScoreStore::load("multiplayer-scores.json".into()),
        ),
    )?;
    app.selector
        .source_reports
        .push(sdo_asset_loader::sources::SourceReport {
            name: endpoint.clone(),
            songs: app.selector.catalog.songs.len(),
            skins: app.selector.ui_choices.skins.len(),
            characters: app.characters.presets.len(),
            issues: app.selector.character_issues.clone(),
        });
    app.selector.lobby.preferences.address = socket;
    app.selector.remote_override = Some(endpoint);
    let event_loop = EventLoop::new()?;
    let document = web_sys::window()
        .and_then(|w| w.document())
        .context("Missing document")?;
    let canvas = document
        .get_element_by_id("game")
        .context("Missing game canvas")?
        .dyn_into::<web_sys::HtmlCanvasElement>()
        .map_err(|_| anyhow::anyhow!("Invalid canvas"))?;
    canvas.set_hidden(false);
    // On the web the DOM canvas already exists, so initialize its surface asynchronously before starting events.
    #[allow(deprecated)]
    let window = Arc::new(
        event_loop.create_window(
            Window::default_attributes()
                .with_canvas(Some(canvas))
                .with_prevent_default(true),
        )?,
    );
    let size = window.inner_size();
    let (surface, device, queue, _) = sdo_render::surface::DisplaySurface::new(
        window.clone(),
        [size.width, size.height],
        app.options.frame_latency,
    )
    .await?;
    app.attach_window(window, surface, device, queue)?;
    app.network_wake = Some(event_loop.create_proxy());
    event_loop.spawn_app(app);
    Ok(())
}
