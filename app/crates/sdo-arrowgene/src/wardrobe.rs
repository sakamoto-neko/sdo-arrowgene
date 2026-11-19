//! One background loader and a bounded GPU thumbnail cache for visible shop items.
use anyhow::Result;
use sdo_asset_loader::{AssetLoader, request};
use sdo_assets::{CharacterAssets, ClothingPreviewKey};
use sdo_render::character::{CharacterCamera, CharacterRenderer, ITEM_PREVIEW_SIZE};
use sdo_ui::{ClothingThumbnail, SelectorUi, WardrobeView};
use std::{
    collections::BTreeMap,
    sync::{Arc, mpsc},
};

const CACHE_CAPACITY: usize = 128;
struct CachedPreview {
    _texture: Option<wgpu::Texture>,
    thumbnail: ClothingThumbnail,
    used: u64,
}
pub struct WardrobeThumbnails {
    jobs: sdo_platform::jobs::Jobs<
        ClothingPreviewKey,
        (ClothingPreviewKey, Result<Arc<CharacterAssets>>),
    >,
    pending: Option<ClothingPreviewKey>,
    renderer: CharacterRenderer,
    cache: BTreeMap<ClothingPreviewKey, CachedPreview>,
    tick: u64,
}
impl WardrobeThumbnails {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, assets: AssetLoader) -> Self {
        let jobs = sdo_platform::jobs::Jobs::new(1, move |key: ClothingPreviewKey| {
            let result = assets.request(request::ClothingPreview(&key));
            (key, result)
        });
        Self {
            jobs,
            pending: None,
            renderer: CharacterRenderer::new_item_preview(device, queue),
            cache: BTreeMap::new(),
            tick: 0,
        }
    }
    /// Runs after UI layout, which reports only the rows currently on screen.
    /// Disk reads and mesh/texture decoding occur exclusively on the worker.
    pub fn update(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        ui: &mut SelectorUi,
        view: &mut WardrobeView,
        active: bool,
    ) {
        self.tick += 1;
        let visible = if active {
            view.visible_previews().to_vec()
        } else {
            vec![]
        };
        for key in &visible {
            if let Some(entry) = self.cache.get_mut(key) {
                entry.used = self.tick;
            }
        }
        let completed = match self.jobs.poll() {
            Ok(result) => Some(result),
            Err(mpsc::TryRecvError::Disconnected) => self
                .pending
                .take()
                .map(|key| (key, Err(anyhow::anyhow!("Preview loader stopped")))),
            Err(mpsc::TryRecvError::Empty) => None,
        };
        if let Some((key, result)) = completed {
            self.pending = None;
            // Scrolling or changing presets cannot apply a stale preview to a row.
            if visible.contains(&key) {
                let rendered =
                    result.and_then(|assets| self.render(device, queue, ui, &assets, &key));
                let (texture, thumbnail) = match rendered {
                    Ok((texture, thumbnail)) => (Some(texture), thumbnail),
                    Err(error) => (None, ClothingThumbnail::Failed(format!("{error:#}"))),
                };
                view.set_thumbnail(key.clone(), thumbnail.clone());
                self.cache.insert(
                    key,
                    CachedPreview {
                        _texture: texture,
                        thumbnail,
                        used: self.tick,
                    },
                );
            }
        }
        // Keep visible items plus recent thumbnails; unregister evicted egui
        // textures too, so browsing thousands of items cannot retain them all.
        while self.cache.len() > CACHE_CAPACITY {
            let victim = self
                .cache
                .iter()
                .filter(|(key, _)| !visible.contains(key))
                .min_by_key(|(_, entry)| entry.used)
                .map(|(key, _)| key.clone());
            let Some(key) = victim else {
                break;
            };
            let entry = self.cache.remove(&key).unwrap();
            if let ClothingThumbnail::Ready(id) = entry.thumbnail {
                ui.unregister_preview(id);
            }
            view.remove_thumbnail(&key);
        }
        if self.pending.is_none()
            && let Some(key) = visible.iter().find(|key| !self.cache.contains_key(*key))
        {
            if self.jobs.send(key.clone()).is_ok() {
                self.pending = Some(key.clone());
            } else {
                let thumbnail = ClothingThumbnail::Failed("Preview loader stopped".into());
                view.set_thumbnail(key.clone(), thumbnail.clone());
                self.cache.insert(
                    key.clone(),
                    CachedPreview {
                        _texture: None,
                        thumbnail,
                        used: self.tick,
                    },
                );
            }
        }
    }
    fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        ui: &mut SelectorUi,
        assets: &CharacterAssets,
        key: &ClothingPreviewKey,
    ) -> Result<(wgpu::Texture, ClothingThumbnail)> {
        self.renderer.sync_assets(device, queue, assets);
        self.renderer.render(
            device,
            queue,
            &key.item,
            CharacterCamera {
                yaw: -0.35,
                elevation: 0.24,
                zoom: 1.,
            },
        )?;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("cached wardrobe thumbnail"),
            size: wgpu::Extent3d {
                width: ITEM_PREVIEW_SIZE[0],
                height: ITEM_PREVIEW_SIZE[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.renderer.texture.format(),
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: sdo_render::character::preview_view_formats(
                self.renderer.texture.format(),
            ),
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_texture(
            self.renderer.texture.as_image_copy(),
            texture.as_image_copy(),
            texture.size(),
        );
        queue.submit([encoder.finish()]);
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(wgpu::TextureFormat::Rgba8Unorm),
            ..Default::default()
        });
        let id = ui.register_character_preview(device, &view);
        Ok((texture, ClothingThumbnail::Ready(id)))
    }
    #[cfg(not(target_arch = "wasm32"))]
    pub fn is_ready(&self, view: &WardrobeView) -> bool {
        view.visible_previews()
            .iter()
            .all(|key| self.cache.contains_key(key))
    }
}
