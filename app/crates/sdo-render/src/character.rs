//! Reusable wgpu character preview: curated geometry, depth, materials and orbit camera.
use anyhow::{Context, Result, ensure};
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use sdo_assets::{CharacterAssets, CharacterSlot, CharacterSurface};
use wgpu::util::DeviceExt;
pub mod stage;

/// WebGL2 cannot reinterpret an sRGB attachment as a linear egui texture view.
pub fn preview_format(device: &wgpu::Device) -> wgpu::TextureFormat {
    if device.adapter_info().backend == wgpu::Backend::Gl {
        wgpu::TextureFormat::Rgba8Unorm
    } else {
        wgpu::TextureFormat::Rgba8UnormSrgb
    }
}
pub fn preview_view_formats(format: wgpu::TextureFormat) -> &'static [wgpu::TextureFormat] {
    if format == wgpu::TextureFormat::Rgba8UnormSrgb {
        &[wgpu::TextureFormat::Rgba8Unorm]
    } else {
        &[]
    }
}

pub const PREVIEW_SIZE: [u32; 2] = [640, 720];
pub const ITEM_PREVIEW_SIZE: [u32; 2] = [160, 160];
pub const PORTRAIT_SIZE: [u32; 2] = [96, 96];
pub const PORTRAIT_BYTES: usize = 96 * 96 * 4;
pub const AVATAR_SIZE: [u32; 2] = [192, 192];
pub const AVATAR_BYTES: usize = 192 * 192 * 4;
/// Local capture utility for diagnostics; deliberately has no wire serialization.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Portrait {
    pub character: String,
    pub rgba: Vec<u8>,
    pub avatar: Vec<u8>,
}
impl Portrait {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.rgba.len() == PORTRAIT_BYTES
                && (self.avatar.is_empty() || self.avatar.len() == AVATAR_BYTES),
            "Invalid local capture"
        );
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CharacterCamera {
    pub yaw: f32,
    pub elevation: f32,
    pub zoom: f32,
}
impl Default for CharacterCamera {
    fn default() -> Self {
        Self {
            yaw: 0.,
            elevation: 0.04,
            zoom: 1.,
        }
    }
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    position: [f32; 3],
    normal: [f32; 3],
    uv: [f32; 2],
    diffuse: [f32; 4],
}
const ATTRS: [wgpu::VertexAttribute; 4] =
    wgpu::vertex_attr_array![0=>Float32x3,1=>Float32x3,2=>Float32x2,3=>Float32x4];
struct Surface {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    count: u32,
    texture: usize,
    transparent: bool,
    center: Vec3,
    source: CharacterSurface,
    face: bool,
}
/// Temporary CPU skinning storage shared by every surface drawn by this renderer.
#[derive(Default)]
struct SkinUpload {
    matrices: Vec<Mat4>,
    vertices: Vec<Vertex>,
}
impl SkinUpload {
    fn upload(
        &mut self,
        queue: &wgpu::Queue,
        surface: &mut Surface,
        pose: Option<&[Mat4]>,
        model: Mat4,
    ) {
        self.vertices.clear();
        let mut sum = Vec3::ZERO;
        let mut vertex = |v: sdo_assets::CharacterVertex| {
            let position = Vec3::from_array(v.position);
            sum += position;
            Vertex {
                position: model.transform_point3(position).to_array(),
                normal: model
                    .transform_vector3(Vec3::from_array(v.normal))
                    .normalize_or_zero()
                    .to_array(),
                uv: v.uv,
                diffuse: surface.source.diffuse,
            }
        };
        if let Some(globals) = pose {
            self.vertices.extend(
                surface
                    .source
                    .skin_vertices(globals, &mut self.matrices)
                    .map(&mut vertex),
            );
        } else {
            self.vertices
                .extend(surface.source.vertices.iter().copied().map(&mut vertex));
        }
        queue.write_buffer(&surface.vertices, 0, bytemuck::cast_slice(&self.vertices));
        surface.center = model.transform_point3(sum / self.vertices.len().max(1) as f32);
    }
}
struct Preset {
    id: String,
    surfaces: Vec<Surface>,
    bounds: [Vec3; 2],
    bone_count: usize,
    head_center: Vec3,
    animated: bool,
}
#[derive(Clone, Copy, PartialEq)]
enum PreviewMode {
    Standard,
    Head,
    Room,
}
pub struct CharacterRenderer {
    presets: Vec<Preset>,
    textures: Vec<wgpu::BindGroup>,
    texture_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    opaque: wgpu::RenderPipeline,
    transparent: wgpu::RenderPipeline,
    camera: wgpu::Buffer,
    camera_group: wgpu::BindGroup,
    camera_layout: wgpu::BindGroupLayout,
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub ui_view: wgpu::TextureView,
    _depth: wgpu::Texture,
    depth: wgpu::TextureView,
    skin_upload: SkinUpload,
    last: Option<(String, CharacterCamera)>,
    size: [u32; 2],
    fit_item: bool,
}
impl CharacterRenderer {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, assets: &CharacterAssets) -> Self {
        Self::with_target(device, queue, Some(assets), PREVIEW_SIZE, false)
    }
    pub fn new_room_preview(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        assets: &CharacterAssets,
    ) -> Self {
        Self::with_target(device, queue, Some(assets), [256, 320], false)
    }
    /// Shares the character materials/depth pipeline; frames one item in a square.
    pub fn new_item_preview(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        Self::with_target(device, queue, None, ITEM_PREVIEW_SIZE, true)
    }
    fn with_target(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        assets: Option<&CharacterAssets>,
        size: [u32; 2],
        fit_item: bool,
    ) -> Self {
        let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("character camera"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("character textures"),
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
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("character material sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            ..Default::default()
        });
        let textures = assets
            .map(|assets| Self::upload_textures(device, queue, assets, &texture_layout, &sampler))
            .unwrap_or_default();
        let camera = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("character camera matrix"),
            contents: bytemuck::cast_slice(&Mat4::IDENTITY.to_cols_array()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let camera_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("character camera"),
            layout: &camera_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera.as_entire_binding(),
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("character pipeline layout"),
            bind_group_layouts: &[Some(&camera_layout), Some(&texture_layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::include_wgsl!("character.wgsl"));
        let make = |transparent| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(if transparent {
                    "character translucent surfaces"
                } else {
                    "character solid/cutout surfaces"
                }),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Vertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &ATTRS,
                    })],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(if transparent {
                        "fs_translucent"
                    } else {
                        "fs_solid"
                    }),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: preview_format(device),
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: Some(!transparent),
                    depth_compare: Some(wgpu::CompareFunction::LessEqual),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let opaque = make(false);
        let transparent = make(true);
        let presets = assets
            .map(|assets| Self::upload_presets(device, assets))
            .unwrap_or_default();
        let target = |label, format, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: size[0],
                    height: size[1],
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: preview_view_formats(format),
            })
        };
        let texture = target(
            "character preview",
            preview_format(device),
            wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC,
        );
        let view = texture.create_view(&Default::default());
        let ui_view = texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(wgpu::TextureFormat::Rgba8Unorm),
            ..Default::default()
        });
        let depth_texture = target(
            "character depth",
            wgpu::TextureFormat::Depth32Float,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        );
        let depth = depth_texture.create_view(&Default::default());
        Self {
            presets,
            textures,
            texture_layout,
            sampler,
            opaque,
            transparent,
            camera,
            camera_group,
            camera_layout,
            texture,
            view,
            ui_view,
            _depth: depth_texture,
            depth,
            skin_upload: SkinUpload::default(),
            last: None,
            size,
            fit_item,
        }
    }
    fn upload_textures(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        assets: &CharacterAssets,
        texture_layout: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
    ) -> Vec<wgpu::BindGroup> {
        assets
            .textures
            .iter()
            .map(|source| {
                let t = device.create_texture(&wgpu::TextureDescriptor {
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
                    t.as_image_copy(),
                    &source.rgba,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(source.width * 4),
                        rows_per_image: Some(source.height),
                    },
                    t.size(),
                );
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("character material"),
                    layout: texture_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(
                                &t.create_view(&Default::default()),
                            ),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(sampler),
                        },
                    ],
                })
            })
            .collect()
    }
    fn upload_presets(device: &wgpu::Device, assets: &CharacterAssets) -> Vec<Preset> {
        assets
            .presets
            .iter()
            .map(|preset| {
                let face_resource = preset
                    .equipment
                    .get(&CharacterSlot::Face)
                    .and_then(|id| assets.wardrobe.item(id).ok())
                    .map(|item| item.resource.as_str());
                let surfaces: Vec<Surface> = preset
                    .parts
                    .iter()
                    .flat_map(|part| {
                        let face =
                            face_resource.is_some_and(|resource| part.source.ends_with(resource));
                        part.surfaces.iter().map(move |surface| (surface, face))
                    })
                    .map(|(s, face)| {
                        let verts = s
                            .vertices
                            .iter()
                            .map(|v| Vertex {
                                position: v.position,
                                normal: v.normal,
                                uv: v.uv,
                                diffuse: s.diffuse,
                            })
                            .collect::<Vec<_>>();
                        let center = s
                            .vertices
                            .iter()
                            .map(|v| Vec3::from_array(v.position))
                            .sum::<Vec3>()
                            / s.vertices.len().max(1) as f32;
                        Surface {
                            vertices: device.create_buffer_init(
                                &wgpu::util::BufferInitDescriptor {
                                    label: Some("character surface vertices"),
                                    contents: bytemuck::cast_slice(&verts),
                                    usage: wgpu::BufferUsages::VERTEX
                                        | wgpu::BufferUsages::COPY_DST,
                                },
                            ),
                            indices: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                                label: Some("character surface indices"),
                                contents: bytemuck::cast_slice(&s.indices),
                                usage: wgpu::BufferUsages::INDEX,
                            }),
                            count: s.indices.len() as u32,
                            texture: s.texture.0,
                            transparent: s.diffuse[3] < 1.
                                || assets.textures[s.texture.0]
                                    .rgba
                                    .as_chunks::<4>()
                                    .0
                                    .iter()
                                    .any(|p| p[3] < 255),
                            center,
                            source: s.clone(),
                            face,
                        }
                    })
                    .collect();
                let mut min = Vec3::splat(f32::INFINITY);
                let mut max = Vec3::splat(f32::NEG_INFINITY);
                for vertex in surfaces
                    .iter()
                    .filter(|s| s.face)
                    .flat_map(|s| &s.source.vertices)
                {
                    let point = Vec3::from_array(vertex.position);
                    min = min.min(point);
                    max = max.max(point);
                }
                let span = max - min;
                // Use the bind-pose nose height for fixed head framing. Animation
                // and turning must never move or orbit this camera.
                let face_anchor = surfaces
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| s.face)
                    .flat_map(|(surface, s)| {
                        s.source
                            .vertices
                            .iter()
                            .enumerate()
                            .map(move |(vertex, v)| (surface, vertex, v.position))
                    })
                    .filter(|(_, _, p)| {
                        p[0] >= min.x + span.x * 0.3
                            && p[0] <= max.x - span.x * 0.3
                            && p[1] >= min.y + span.y * 0.2
                            && p[1] <= max.y - span.y * 0.2
                    })
                    .min_by(|a, b| a.2[2].total_cmp(&b.2[2]))
                    .map(|(surface, vertex, _)| (surface, vertex));
                let bounds = preset.bounds.map(Vec3::from_array);
                let mut head_center = (bounds[0] + bounds[1]) * 0.5;
                head_center.y = face_anchor
                    .map(|(surface, vertex)| surfaces[surface].source.vertices[vertex].position[1])
                    .or_else(|| {
                        preset
                            .skeleton
                            .bones
                            .iter()
                            .find(|bone| bone.name.to_ascii_lowercase().contains("head"))
                            .map(|bone| bone.global.w_axis.y + (bounds[1].y - bounds[0].y) * 0.04)
                    })
                    .unwrap_or(head_center.y + (bounds[1].y - bounds[0].y) * 0.32);
                Preset {
                    head_center,
                    id: preset.choice.id.clone(),
                    surfaces,
                    bounds,
                    bone_count: preset.skeleton.bones.len(),
                    animated: false,
                }
            })
            .collect()
    }
    /// Replace validated outfit data while retaining the registered preview target.
    pub fn sync_assets(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        assets: &CharacterAssets,
    ) {
        self.textures =
            Self::upload_textures(device, queue, assets, &self.texture_layout, &self.sampler);
        self.presets = Self::upload_presets(device, assets);
        self.last = None;
    }
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        id: &str,
        camera: CharacterCamera,
    ) -> Result<()> {
        self.render_pose(device, queue, id, camera, None)
    }
    pub fn render_pose(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        id: &str,
        camera: CharacterCamera,
        pose: Option<&[Mat4]>,
    ) -> Result<()> {
        self.render_mode(device, queue, id, camera, pose, PreviewMode::Standard)
    }
    pub fn render_head(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        id: &str,
    ) -> Result<()> {
        self.render_mode(
            device,
            queue,
            id,
            CharacterCamera::default(),
            None,
            PreviewMode::Head,
        )
    }
    pub fn render_head_pose(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        id: &str,
        camera: CharacterCamera,
        pose: Option<&[Mat4]>,
    ) -> Result<()> {
        self.render_mode(device, queue, id, camera, pose, PreviewMode::Head)
    }
    pub fn render_room_pose(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        id: &str,
        camera: CharacterCamera,
        pose: Option<&[Mat4]>,
    ) -> Result<()> {
        self.render_mode(device, queue, id, camera, pose, PreviewMode::Room)
    }
    fn render_mode(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        id: &str,
        camera: CharacterCamera,
        pose: Option<&[Mat4]>,
        mode: PreviewMode,
    ) -> Result<()> {
        let preset = self
            .presets
            .iter_mut()
            .find(|p| p.id == id)
            .context("Character preset not uploaded")?;
        if mode == PreviewMode::Standard
            && pose.is_none()
            && !preset.animated
            && self
                .last
                .as_ref()
                .is_some_and(|(p, c)| p == id && *c == camera)
        {
            return Ok(());
        }

        if let Some(pose) = pose {
            ensure!(
                pose.len() == preset.bone_count && pose.iter().all(Mat4::is_finite),
                "Invalid character pose"
            );
        }
        let standing_center = (preset.bounds[0] + preset.bounds[1]) * 0.5;
        // Room cameras stay in front of the stage. Turn the entire posed model,
        // including its normals, around the same standing axis in both views.
        let model = if mode == PreviewMode::Standard {
            Mat4::IDENTITY
        } else {
            Mat4::from_translation(standing_center)
                * Mat4::from_rotation_y(-camera.yaw)
                * Mat4::from_translation(-standing_center)
        };
        if mode != PreviewMode::Standard || pose.is_some() || preset.animated {
            for surface in &mut preset.surfaces {
                self.skin_upload.upload(queue, surface, pose, model);
            }
        }
        // A later standard preview must restore the original geometry.
        preset.animated = pose.is_some() || mode != PreviewMode::Standard;
        let span = preset.bounds[1] - preset.bounds[0];
        let center = if mode == PreviewMode::Head {
            preset.head_center
        } else {
            standing_center
        };
        let aspect = match mode {
            PreviewMode::Head => 1.,
            PreviewMode::Room => 1.,
            PreviewMode::Standard => self.size[0] as f32 / self.size[1] as f32,
        };
        let viewport = crate::camera_viewport(self.size, aspect);
        let distance = if mode == PreviewMode::Head {
            span.y * 0.40
        } else if mode == PreviewMode::Room {
            // The camera scale is based on bind height and stays fixed through
            // turning and arm/leg movement. Horizontal travel never changes it.
            span.y * 1.75
        } else if self.fit_item {
            (span.length() * 0.5 / 21f32.to_radians().sin() * 1.08).max(0.2)
        } else {
            span.y.max(span.x / aspect) * 1.45
        } * camera.zoom.clamp(0.55, 2.5);
        let view_yaw = if mode == PreviewMode::Standard {
            camera.yaw
        } else {
            0.
        };
        let eye = center
            + Vec3::new(
                (view_yaw + std::f32::consts::PI).sin() * camera.elevation.cos(),
                camera.elevation.sin(),
                (view_yaw + std::f32::consts::PI).cos() * camera.elevation.cos(),
            ) * distance;
        let mvp = glam::camera::lh::proj::directx::perspective(
            42f32.to_radians(),
            aspect,
            0.1,
            distance + span.length() * 4.,
        ) * glam::camera::lh::view::look_at_mat4(eye, center, Vec3::Y);
        queue.write_buffer(&self.camera, 0, bytemuck::cast_slice(&mvp.to_cols_array()));
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("character preview"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(if mode == PreviewMode::Standard {
                            wgpu::Color {
                                r: 0.018,
                                g: 0.026,
                                b: 0.042,
                                a: 1.,
                            }
                        } else {
                            wgpu::Color::TRANSPARENT
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            let [x, y, width, height] = viewport;
            pass.set_viewport(x as f32, y as f32, width as f32, height as f32, 0., 1.);
            pass.set_scissor_rect(x, y, width, height);
            pass.set_bind_group(0, &self.camera_group, &[]);
            let mut ordered = preset.surfaces.iter().collect::<Vec<_>>();
            ordered.sort_by(|a, b| {
                a.transparent.cmp(&b.transparent).then_with(|| {
                    if a.transparent {
                        b.center
                            .distance_squared(eye)
                            .total_cmp(&a.center.distance_squared(eye))
                    } else {
                        std::cmp::Ordering::Equal
                    }
                })
            });
            // Mixed-alpha textures contain solid clothing/skin as well as soft
            // edges. Their solid fragments must occlude other triangles, even
            // triangles in the same mesh. Classify fragments, not whole meshes.
            for (translucent, pipeline) in [(false, &self.opaque), (true, &self.transparent)] {
                pass.set_pipeline(pipeline);
                for s in &ordered {
                    if translucent && !s.transparent {
                        continue;
                    }
                    pass.set_bind_group(1, &self.textures[s.texture], &[]);
                    pass.set_vertex_buffer(0, s.vertices.slice(..));
                    pass.set_index_buffer(s.indices.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..s.count, 0, 0..1);
                }
            }
        }
        queue.submit([encoder.finish()]);
        self.last = (mode == PreviewMode::Standard).then(|| (id.into(), camera));
        Ok(())
    }
}
