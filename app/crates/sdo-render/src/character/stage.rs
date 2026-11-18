//! Locally rendered multi-character scene; the network carries no pixels.
use super::*;
use std::collections::BTreeMap;

pub const SIZE: [u32; 2] = [1280, 900];
pub const HEAD_SIZE: u32 = 192;
pub fn camera_yaw(seconds: f64) -> f32 {
    (seconds as f32 * 0.13).sin() * 0.24
}

pub struct Dancer<'a> {
    pub id: u64,
    pub slot: u8,
    pub position: Option<[f32; 2]>,
    pub yaw: f32,
    pub pose: Option<&'a [Mat4]>,
}
/// Prepared off the window thread. Upload only this actor's referenced materials.
pub struct StageModel {
    preset: Preset,
    textures: Vec<wgpu::BindGroup>,
}
impl StageModel {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        assets: &CharacterAssets,
        id: &str,
    ) -> Result<Self> {
        let mut subset = assets.clone();
        subset.presets = vec![assets.preset(id)?.clone()];
        let mut preset = CharacterRenderer::upload_presets(device, &subset).remove(0);
        let mut used = BTreeMap::new();
        for surface in &mut preset.surfaces {
            let next = used.len();
            surface.texture = *used.entry(surface.texture).or_insert(next);
        }
        let mut sources = used
            .iter()
            .map(|(source, index)| {
                let t = &assets.textures[*source];
                (
                    *index,
                    sdo_assets::Texture {
                        source: t.source.clone(),
                        width: t.width,
                        height: t.height,
                        rgba: t.rgba.clone(),
                    },
                )
            })
            .collect::<Vec<_>>();
        sources.sort_by_key(|(index, _)| *index);
        subset.textures =
            std::sync::Arc::new(sources.into_iter().map(|(_, texture)| texture).collect());
        let layout = material_layout(device);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            ..Default::default()
        });
        let textures =
            CharacterRenderer::upload_textures(device, queue, &subset, &layout, &sampler);
        Ok(Self { preset, textures })
    }
}
fn material_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("stage materials"),
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
    })
}
struct HeadCamera {
    buffer: wgpu::Buffer,
    group: wgpu::BindGroup,
}
pub struct StageRenderer {
    pub character: CharacterRenderer,
    models: BTreeMap<u64, StageModel>,
    pub labels: BTreeMap<u64, [f32; 2]>,
    white: wgpu::BindGroup,
    floor: wgpu::Buffer,
    floor_count: u32,
    pub heads: wgpu::Texture,
    pub head_ui_view: wgpu::TextureView,
    head_view: wgpu::TextureView,
    _head_depth: wgpu::Texture,
    head_depth: wgpu::TextureView,
    head_cameras: Vec<HeadCamera>,
}
fn position(dancer: &Dancer<'_>) -> Vec3 {
    dancer.position.map_or_else(
        || {
            Vec3::new(
                (f32::from(dancer.slot % 4) - 1.5) * 1.65,
                0.,
                f32::from(dancer.slot / 4) * 2.2 - 1.1,
            )
        },
        |[x, y]| Vec3::new((x - 0.5) * 8., 0., (0.5 - y) * 4.),
    )
}
impl StageRenderer {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, _assets: &CharacterAssets) -> Self {
        let character = CharacterRenderer::with_target(device, queue, None, SIZE, false);
        let white_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("stage white"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
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
            white_texture.as_image_copy(),
            &[255; 4],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4),
                rows_per_image: Some(1),
            },
            white_texture.size(),
        );
        let white = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("stage floor material"),
            layout: &character.texture_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(
                        &white_texture.create_view(&Default::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&character.sampler),
                },
            ],
        });
        let mut vertices = vec![];
        let mut quad = |p: [Vec3; 4], color: [f32; 4], normal: Vec3| {
            for i in [0, 1, 2, 0, 2, 3] {
                vertices.push(Vertex {
                    position: p[i].to_array(),
                    normal: normal.to_array(),
                    uv: [0., 0.],
                    diffuse: color,
                });
            }
        };
        quad(
            [
                Vec3::new(-5., 0., -3.),
                Vec3::new(5., 0., -3.),
                Vec3::new(5., 0., 3.),
                Vec3::new(-5., 0., 3.),
            ],
            [0.02, 0.035, 0.06, 1.],
            Vec3::Y,
        );
        quad(
            [
                Vec3::new(-9., 0., 4.),
                Vec3::new(9., 0., 4.),
                Vec3::new(9., 7., 4.),
                Vec3::new(-9., 7., 4.),
            ],
            [0.009, 0.017, 0.035, 1.],
            -Vec3::Z,
        );
        for i in 0..=8 {
            let x = -5. + i as f32 * 1.25;
            let color = if i == 0 || i == 8 {
                [0.12, 0.42, 0.48, 1.]
            } else {
                [0.06, 0.11, 0.15, 1.]
            };
            quad(
                [
                    Vec3::new(x - 0.012, 0.006, -3.),
                    Vec3::new(x + 0.012, 0.006, -3.),
                    Vec3::new(x + 0.012, 0.006, 3.),
                    Vec3::new(x - 0.012, 0.006, 3.),
                ],
                color,
                Vec3::Y,
            );
        }
        for i in 0..=6 {
            let z = -3. + i as f32;
            let color = if i == 0 || i == 6 {
                [0.12, 0.42, 0.48, 1.]
            } else {
                [0.06, 0.11, 0.15, 1.]
            };
            quad(
                [
                    Vec3::new(-5., 0.008, z - 0.012),
                    Vec3::new(5., 0.008, z - 0.012),
                    Vec3::new(5., 0.008, z + 0.012),
                    Vec3::new(-5., 0.008, z + 0.012),
                ],
                color,
                Vec3::Y,
            );
        }
        for x in [-5., 0., 5.] {
            quad(
                [
                    Vec3::new(x - 0.02, 0.3, 3.98),
                    Vec3::new(x + 0.02, 0.3, 3.98),
                    Vec3::new(x + 0.02, 6., 3.98),
                    Vec3::new(x - 0.02, 6., 3.98),
                ],
                [0.018, 0.035, 0.066, 1.],
                -Vec3::Z,
            );
        }
        let floor_count = vertices.len() as u32;
        let floor = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("static room floor"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let target = |format, usage, views: &[wgpu::TextureFormat]| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("local portrait atlas"),
                size: wgpu::Extent3d {
                    width: HEAD_SIZE * 8,
                    height: HEAD_SIZE,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: views,
            })
        };
        let heads = target(
            wgpu::TextureFormat::Rgba8UnormSrgb,
            wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            &[wgpu::TextureFormat::Rgba8Unorm],
        );
        let head_view = heads.create_view(&Default::default());
        let head_ui_view = heads.create_view(&wgpu::TextureViewDescriptor {
            format: Some(wgpu::TextureFormat::Rgba8Unorm),
            ..Default::default()
        });
        let head_depth_texture = target(
            wgpu::TextureFormat::Depth32Float,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
            &[],
        );
        let head_depth = head_depth_texture.create_view(&Default::default());
        let head_cameras = (0..8)
            .map(|_| {
                let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("portrait camera"),
                    size: 64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("portrait camera"),
                    layout: &character.camera_layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: buffer.as_entire_binding(),
                    }],
                });
                HeadCamera { buffer, group }
            })
            .collect();
        Self {
            character,
            models: BTreeMap::new(),
            labels: BTreeMap::new(),
            white,
            floor,
            floor_count,
            heads,
            head_view,
            head_ui_view,
            _head_depth: head_depth_texture,
            head_depth,
            head_cameras,
        }
    }
    pub fn set_model(&mut self, id: u64, model: StageModel) {
        self.models.insert(id, model);
    }
    pub fn remove_model(&mut self, id: u64) {
        self.models.remove(&id);
    }
    pub fn retain(&mut self, ids: &[u64]) {
        self.models.retain(|id, _| ids.contains(id));
    }
    pub fn has_model(&self, id: u64) -> bool {
        self.models.contains_key(&id)
    }
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        seconds: f64,
        dancers: &[Dancer<'_>],
        playing: bool,
        aspect: f32,
    ) -> Result<()> {
        let yaw = if playing { camera_yaw(seconds) } else { 0. };
        let target = Vec3::new(0., 1.05, 0.);
        let distance = if playing { 12.8 } else { 11.5 };
        let eye = target
            + Vec3::new(
                yaw.sin() * distance,
                4.4 + (if playing {
                    (seconds as f32 * 0.09).sin() * 0.35
                } else {
                    0.
                }),
                -yaw.cos() * distance,
            );
        let mvp = glam::camera::lh::proj::directx::perspective(
            42f32.to_radians(),
            aspect.clamp(0.4, 4.),
            0.1,
            60.,
        ) * glam::camera::lh::view::look_at_mat4(eye, target, Vec3::Y);
        queue.write_buffer(
            &self.character.camera,
            0,
            bytemuck::cast_slice(&mvp.to_cols_array()),
        );
        self.labels = dancers
            .iter()
            .map(|dancer| {
                let clip = mvp * (position(dancer) + Vec3::Y * 2.35).extend(1.);
                (
                    dancer.id,
                    [clip.x / clip.w * 0.5 + 0.5, 0.5 - clip.y / clip.w * 0.5],
                )
            })
            .collect();
        for dancer in dancers {
            let Some(actor) = self.models.get_mut(&dancer.id) else {
                continue;
            };
            let preset = &mut actor.preset;
            if let Some(pose) = dancer.pose {
                ensure!(
                    pose.len() == preset.bone_count && pose.iter().all(Mat4::is_finite),
                    "Invalid stage pose"
                );
            }
            let height = (preset.bounds[1].y - preset.bounds[0].y).max(0.001);
            let center = (preset.bounds[0] + preset.bounds[1]) * 0.5;
            let model = Mat4::from_translation(position(dancer))
                * Mat4::from_scale(Vec3::splat(2. / height))
                * Mat4::from_rotation_y(-dancer.yaw)
                * Mat4::from_translation(-Vec3::new(center.x, preset.bounds[0].y, center.z));
            for surface in &mut preset.surfaces {
                self.character
                    .skin_upload
                    .upload(queue, surface, dancer.pose, model);
            }
            if playing {
                continue;
            }
            let head = model.transform_point3(preset.head_center);
            let head_eye = head + Vec3::new(0., 0.032, -0.8);
            let head_mvp =
                glam::camera::lh::proj::directx::perspective(42f32.to_radians(), 1., 0.1, 10.)
                    * glam::camera::lh::view::look_at_mat4(head_eye, head, Vec3::Y);
            queue.write_buffer(
                &self.head_cameras[dancer.slot as usize].buffer,
                0,
                bytemuck::cast_slice(&head_mvp.to_cols_array()),
            );
        }
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("local multiplayer stage"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.character.view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.0044,
                            g: 0.008,
                            b: 0.0168,
                            a: 1.,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.character.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            pass.set_pipeline(&self.character.opaque);
            pass.set_bind_group(0, &self.character.camera_group, &[]);
            pass.set_bind_group(1, &self.white, &[]);
            pass.set_vertex_buffer(0, self.floor.slice(..));
            pass.draw(0..self.floor_count, 0..1);
            self.draw_actors(&mut pass, dancers, eye);
        }
        if !playing {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("local head portraits"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.head_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.head_depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            for dancer in dancers {
                pass.set_viewport(
                    dancer.slot as f32 * HEAD_SIZE as f32,
                    0.,
                    HEAD_SIZE as f32,
                    HEAD_SIZE as f32,
                    0.,
                    1.,
                );
                pass.set_scissor_rect(dancer.slot as u32 * HEAD_SIZE, 0, HEAD_SIZE, HEAD_SIZE);
                pass.set_bind_group(0, &self.head_cameras[dancer.slot as usize].group, &[]);
                self.draw_actors(&mut pass, std::slice::from_ref(dancer), eye);
            }
        }
        queue.submit([encoder.finish()]);
        Ok(())
    }
    fn draw_actors<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        dancers: &[Dancer<'_>],
        eye: Vec3,
    ) {
        let mut surfaces = dancers
            .iter()
            .filter_map(|d| self.models.get(&d.id))
            .flat_map(|actor| actor.preset.surfaces.iter().map(move |s| (actor, s)))
            .collect::<Vec<_>>();
        surfaces.sort_by(|(_, a), (_, b)| {
            b.center
                .distance_squared(eye)
                .total_cmp(&a.center.distance_squared(eye))
        });
        for (translucent, pipeline) in [
            (false, &self.character.opaque),
            (true, &self.character.transparent),
        ] {
            pass.set_pipeline(pipeline);
            for (actor, surface) in &surfaces {
                if translucent && !surface.transparent {
                    continue;
                }
                pass.set_bind_group(1, &actor.textures[surface.texture], &[]);
                pass.set_vertex_buffer(0, surface.vertices.slice(..));
                pass.set_index_buffer(surface.indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..surface.count, 0, 0..1);
            }
        }
    }
}
