//! Generic alpha-blended textured quads on wgpu; no gameplay/window dependencies.
pub mod character;
pub mod gui;
pub mod surface;
use anyhow::{Context, Result, ensure};
use bytemuck::{Pod, Zeroable};
use sdo_assets::{FourKeySkin, Texture, TextureId};
use std::{
    path::Path,
    sync::{Arc, mpsc},
};
use wgpu::util::DeviceExt;

#[derive(Debug, Clone)]
pub struct Sprite {
    pub texture: TextureId,
    pub rect: [f32; 4],
    pub uv: [f32; 4],
    pub color: [f32; 4],
    pub blend: BlendMode,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlendMode {
    Alpha,
    Additive,
}
impl Sprite {
    pub fn new(texture: TextureId, rect: [f32; 4]) -> Self {
        Self {
            texture,
            rect,
            uv: [0.0, 0.0, 1.0, 1.0],
            color: [1.0; 4],
            blend: BlendMode::Alpha,
        }
    }
    /// Place a native scene in a viewport without changing aspect ratios or UVs.
    pub fn transformed(mut self, [x, y]: [f32; 2], scale: f32) -> Self {
        self.rect = [
            x + self.rect[0] * scale,
            y + self.rect[1] * scale,
            self.rect[2] * scale,
            self.rect[3] * scale,
        ];
        self
    }
    /// Crop geometry and UVs together, preserving hold/tap texture alignment.
    pub fn clip_vertical(mut self, top: f32, bottom: f32) -> Option<Self> {
        let [_, y, _, h] = self.rect;
        if h <= 0.0 {
            return None;
        }
        let lo = y.max(top);
        let hi = (y + h).min(bottom);
        if hi <= lo {
            return None;
        }
        let v = self.uv[1];
        let range = self.uv[3] - v;
        self.uv[1] = v + range * (lo - y) / h;
        self.uv[3] = v + range * (hi - y) / h;
        self.rect[1] = lo;
        self.rect[3] = hi - lo;
        Some(self)
    }
}
pub struct Scene {
    pub width: f32,
    pub height: f32,
    pub sprites: Vec<Sprite>,
}
/// A cloned GPU context for background camera capture. Readbacks can wait on
/// this worker without ever making the window thread wait for the GPU.
#[derive(Clone)]
pub struct Readback {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}
impl Scene {
    pub fn new(width: f32, height: f32) -> Self {
        Self {
            width,
            height,
            sprites: Vec::new(),
        }
    }
    pub fn push(&mut self, sprite: Sprite) {
        self.sprites.push(sprite);
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    position: [f32; 2],
    uv: [f32; 2],
    color: [f32; 4],
}
const ATTRIBUTES: [wgpu::VertexAttribute; 3] =
    wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4];
struct GpuTexture {
    _texture: Option<wgpu::Texture>,
    bind_group: wgpu::BindGroup,
}

pub async fn request_device(
    instance: &wgpu::Instance,
    surface: Option<&wgpu::Surface<'_>>,
) -> Result<(wgpu::Adapter, wgpu::Device, wgpu::Queue, String)> {
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: surface,
            force_fallback_adapter: false,
            ..Default::default()
        })
        .await
        .context("No compatible wgpu adapter available")?;
    let info = adapter.get_info();
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("SDO Arrowgene device"),
            ..Default::default()
        })
        .await
        .context("Could not create the wgpu device")?;
    Ok((
        adapter,
        device,
        queue,
        format!("{} ({:?})", info.name, info.backend),
    ))
}

pub struct Renderer {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    additive_pipeline: wgpu::RenderPipeline,
    texture_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    textures: Vec<GpuTexture>,
    texture_bank: Arc<Vec<Texture>>,
    vertices: wgpu::Buffer,
    vertex_capacity: usize,
    staging: Vec<Vertex>,
}
impl Renderer {
    pub fn new(
        device: wgpu::Device,
        queue: wgpu::Queue,
        format: wgpu::TextureFormat,
        skin: &FourKeySkin,
    ) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sprite textures"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sprite layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::include_wgsl!("sprite.wgsl"));
        let make_pipeline = |blend| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("original SDO textured sprites"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Vertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &ATTRIBUTES,
                    })],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(blend),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let pipeline = make_pipeline(wgpu::BlendState::ALPHA_BLENDING);
        let additive_pipeline = make_pipeline(wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::SrcAlpha,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent::OVER,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("linear clamp sprite sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let textures = skin
            .textures
            .iter()
            .map(|source| upload_sprite_texture(&device, &queue, &layout, &sampler, source))
            .collect();
        let vertex_capacity = 4096;
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sprite vertices"),
            contents: &vec![0u8; vertex_capacity * std::mem::size_of::<Vertex>()],
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        });
        Self {
            device,
            queue,
            pipeline,
            additive_pipeline,
            texture_layout: layout,
            sampler,
            textures,
            texture_bank: skin.textures.clone(),
            vertices,
            vertex_capacity,
            staging: Vec::new(),
        }
    }
    pub fn sync_textures(&mut self, skin: &FourKeySkin) {
        if Arc::ptr_eq(&self.texture_bank, &skin.textures) {
            return;
        }
        self.textures = skin
            .textures
            .iter()
            .map(|source| {
                upload_sprite_texture(
                    &self.device,
                    &self.queue,
                    &self.texture_layout,
                    &self.sampler,
                    source,
                )
            })
            .collect();
        self.texture_bank = skin.textures.clone();
    }
    /// Register a render target as a sprite without copying pixels through the CPU.
    pub fn register_external_texture(&mut self, view: &wgpu::TextureView) -> TextureId {
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("external sprite texture"),
            layout: &self.texture_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        let id = TextureId(self.textures.len());
        self.textures.push(GpuTexture {
            _texture: None,
            bind_group,
        });
        id
    }
    pub fn draw(&mut self, view: &wgpu::TextureView, size: [u32; 2], scene: &Scene) -> Result<()> {
        ensure!(
            size[0] > 0 && size[1] > 0 && scene.width > 0.0 && scene.height > 0.0,
            "Cannot render an empty viewport"
        );
        let scale = (size[0] as f32 / scene.width).min(size[1] as f32 / scene.height);
        let offset = [
            (size[0] as f32 - scene.width * scale) * 0.5,
            (size[1] as f32 - scene.height * scale) * 0.5,
        ];
        let vertices = &mut self.staging;
        vertices.clear();
        vertices.reserve(scene.sprites.len() * 6);
        for sprite in &scene.sprites {
            ensure!(
                sprite.texture.0 < self.textures.len(),
                "Unknown texture ID {}",
                sprite.texture.0
            );
            let [x, y, w, h] = sprite.rect;
            let [u0, v0, u1, v1] = sprite.uv;
            let corners = [
                ([x, y], [u0, v0]),
                ([x + w, y], [u1, v0]),
                ([x + w, y + h], [u1, v1]),
                ([x, y + h], [u0, v1]),
            ];
            for i in [0, 1, 2, 2, 3, 0] {
                let (p, uv) = corners[i];
                let position = [
                    (p[0] * scale + offset[0]) / size[0] as f32 * 2.0 - 1.0,
                    1.0 - (p[1] * scale + offset[1]) / size[1] as f32 * 2.0,
                ];
                vertices.push(Vertex {
                    position,
                    uv,
                    color: sprite.color,
                });
            }
        }
        if vertices.len() > self.vertex_capacity {
            self.vertex_capacity = vertices.len().next_power_of_two();
            self.vertices = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("grown sprite vertices"),
                size: (self.vertex_capacity * std::mem::size_of::<Vertex>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if !vertices.is_empty() {
            self.queue
                .write_buffer(&self.vertices, 0, bytemuck::cast_slice(vertices));
        }
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("note display"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("4-key sprites"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.009,
                            g: 0.014,
                            b: 0.026,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_vertex_buffer(0, self.vertices.slice(..));
            let mut blend = BlendMode::Alpha;
            let mut first = 0;
            for run in scene
                .sprites
                .chunk_by(|a, b| a.texture == b.texture && a.blend == b.blend)
            {
                let sprite = &run[0];
                if sprite.blend != blend {
                    blend = sprite.blend;
                    pass.set_pipeline(match blend {
                        BlendMode::Alpha => &self.pipeline,
                        BlendMode::Additive => &self.additive_pipeline,
                    });
                }
                pass.set_bind_group(0, &self.textures[sprite.texture.0].bind_group, &[]);
                let end = first + (run.len() * 6) as u32;
                pass.draw(first..end, 0..1);
                first = end;
            }
        }
        self.queue.submit([encoder.finish()]);
        Ok(())
    }
    pub fn screenshot(&mut self, path: &Path, size: [u32; 2], scene: &Scene) -> Result<()> {
        ensure!(size[0] > 0 && size[1] > 0, "Invalid capture dimensions");
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen note display"),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        self.draw(&texture.create_view(&Default::default()), size, scene)?;
        self.save_texture(path, &texture, size)
    }
    /// Save an already rendered texture, including UI drawn by another component.
    pub fn texture_rgba(&self, texture: &wgpu::Texture) -> Result<Vec<u8>> {
        self.readback().texture_rgba(texture)
    }
    pub fn readback(&self) -> Readback {
        Readback {
            device: self.device.clone(),
            queue: self.queue.clone(),
        }
    }
    /// Preserve a camera's fixed viewport, independent of animated bounds.
    pub fn camera_rgba(&self, texture: &wgpu::Texture, size: [u32; 2]) -> Result<Vec<u8>> {
        self.readback().camera_rgba(texture, size)
    }
}
impl Readback {
    pub fn texture_rgba(&self, texture: &wgpu::Texture) -> Result<Vec<u8>> {
        let size = [texture.width(), texture.height()];
        let padded = (size[0] * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("screenshot readback"),
            size: padded as u64 * size[1] as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(size[1]),
                },
            },
            texture.size(),
        );
        self.queue.submit([encoder.finish()]);
        let (tx, rx) = mpsc::channel();
        buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device.poll(wgpu::PollType::wait_indefinitely())?;
        rx.recv()
            .context("GPU screenshot callback did not complete")??;
        let mapped = buffer.slice(..).get_mapped_range()?;
        let mut rgba = Vec::with_capacity((size[0] * size[1] * 4) as usize);
        for row in mapped.chunks_exact(padded as usize) {
            rgba.extend_from_slice(&row[..(size[0] * 4) as usize]);
        }
        drop(mapped);
        buffer.unmap();
        Ok(rgba)
    }
    /// Preserve a camera's fixed viewport. Animated silhouette bounds must never
    /// change the capture scale or move its aim point away from the image center.
    pub fn camera_rgba(&self, texture: &wgpu::Texture, size: [u32; 2]) -> Result<Vec<u8>> {
        ensure!(size[0] > 0 && size[1] > 0, "Invalid camera dimensions");
        let raw = self.texture_rgba(texture)?;
        let image = image::RgbaImage::from_raw(texture.width(), texture.height(), raw)
            .context("Invalid rendered camera")?;
        Ok(camera_image(&image, size).into_raw())
    }
}
impl Renderer {
    /// Read a rendered preview and fit it into a small, transparent thumbnail.
    pub fn thumbnail_rgba(&self, texture: &wgpu::Texture, size: [u32; 2]) -> Result<Vec<u8>> {
        ensure!(size[0] > 0 && size[1] > 0, "Invalid thumbnail dimensions");
        let raw = self.texture_rgba(texture)?;
        let mut image = image::RgbaImage::from_raw(texture.width(), texture.height(), raw)
            .context("Invalid rendered texture")?;
        let mut bounds = [image.width(), image.height(), 0, 0];
        for (x, y, pixel) in image.enumerate_pixels() {
            if pixel[3] > 8 {
                bounds[0] = bounds[0].min(x);
                bounds[1] = bounds[1].min(y);
                bounds[2] = bounds[2].max(x);
                bounds[3] = bounds[3].max(y);
            }
        }
        if bounds[2] >= bounds[0] && bounds[3] >= bounds[1] {
            image = image::imageops::crop_imm(
                &image,
                bounds[0],
                bounds[1],
                bounds[2] - bounds[0] + 1,
                bounds[3] - bounds[1] + 1,
            )
            .to_image();
        }
        let scale =
            (size[0] as f64 / image.width() as f64).min(size[1] as f64 / image.height() as f64);
        let width = (image.width() as f64 * scale).round().max(1.) as u32;
        let height = (image.height() as f64 * scale).round().max(1.) as u32;
        let thumb =
            image::imageops::resize(&image, width, height, image::imageops::FilterType::Triangle);
        let mut output = image::RgbaImage::new(size[0], size[1]);
        image::imageops::replace(
            &mut output,
            &thumb,
            ((size[0] - width) / 2) as i64,
            ((size[1] - height) / 2) as i64,
        );
        Ok(output.into_raw())
    }
    pub fn save_texture(&self, path: &Path, texture: &wgpu::Texture, size: [u32; 2]) -> Result<()> {
        let rgba = self.texture_rgba(texture)?;
        let (pixels, _) = rgba.as_chunks::<4>();
        ensure!(
            pixels
                .iter()
                .any(|pixel| pixel[0] > 60 || pixel[1] > 60 || pixel[2] > 60),
            "Rendered screenshot is blank"
        );
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        image::save_buffer(path, &rgba, size[0], size[1], image::ColorType::Rgba8)?;
        Ok(())
    }
}
fn camera_image(image: &image::RgbaImage, size: [u32; 2]) -> image::RgbaImage {
    let [x, y, width, height] = camera_viewport(
        [image.width(), image.height()],
        size[0] as f32 / size[1] as f32,
    );
    let view = image::imageops::crop_imm(image, x, y, width, height);
    image::imageops::resize(
        &view.to_image(),
        size[0],
        size[1],
        image::imageops::FilterType::Triangle,
    )
}
/// Shared by projection and capture: a fixed, centered viewport at the desired aspect.
fn camera_viewport(size: [u32; 2], aspect: f32) -> [u32; 4] {
    let width = size[0].min((size[1] as f32 * aspect).round().max(1.) as u32);
    let height = size[1].min((size[0] as f32 / aspect).round().max(1.) as u32);
    [(size[0] - width) / 2, (size[1] - height) / 2, width, height]
}
fn upload_sprite_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    source: &sdo_assets::Texture,
) -> GpuTexture {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: source.source.to_str(),
        size: wgpu::Extent3d {
            width: source.width,
            height: source.height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        texture.as_image_copy(),
        &source.rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(source.width * 4),
            rows_per_image: Some(source.height),
        },
        texture.size(),
    );
    let view = texture.create_view(&Default::default());
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: source.source.to_str(),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    });
    GpuTexture {
        _texture: Some(texture),
        bind_group,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn camera_capture_preserves_center_and_scale_when_limbs_extend() {
        let mut narrow = image::RgbaImage::new(256, 320);
        for y in 100..220 {
            for x in 125..131 {
                narrow.put_pixel(x, y, image::Rgba([255, 0, 0, 255]));
            }
        }
        let mut wide = narrow.clone();
        for x in 24..232 {
            if !(110..146).contains(&x) {
                wide.put_pixel(x, 160, image::Rgba([255, 255, 255, 255]));
            }
        }
        for size in [[192, 192], [96, 96]] {
            let a = camera_image(&narrow, size);
            let b = camera_image(&wide, size);
            let red_pixels = |image: &image::RgbaImage| {
                image
                    .enumerate_pixels()
                    .filter(|(_, _, p)| p[0] > 200 && p[1] == 0 && p[2] == 0 && p[3] > 0)
                    .map(|(x, y, _)| (x, y))
                    .collect::<Vec<_>>()
            };
            let a = red_pixels(&a);
            let b = red_pixels(&b);
            assert!(!a.is_empty());
            assert_eq!(
                a, b,
                "Extending limbs must not rescale or shift the body/face camera"
            );
            let center = a.iter().map(|(x, _)| *x as f32).sum::<f32>() / a.len() as f32;
            assert!((center - (size[0] - 1) as f32 * 0.5).abs() < 1.);
        }
    }
    #[test]
    fn clipping_keeps_uvs_in_sync() {
        let s = Sprite::new(TextureId(1), [0.0, 0.0, 10.0, 100.0])
            .clip_vertical(25.0, 75.0)
            .unwrap();
        assert_eq!(s.rect, [0.0, 25.0, 10.0, 50.0]);
        assert_eq!(s.uv, [0.0, 0.25, 1.0, 0.75]);
        assert!(s.clip_vertical(100.0, 150.0).is_none());
    }
}
