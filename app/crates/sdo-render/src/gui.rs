//! GPU backend for egui scenes and overlays.
pub struct GuiRenderer {
    renderer: egui_wgpu::Renderer,
}
impl GuiRenderer {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        Self {
            renderer: egui_wgpu::Renderer::new(device, format, Default::default()),
        }
    }
    pub fn register(&mut self, device: &wgpu::Device, view: &wgpu::TextureView) -> egui::TextureId {
        self.renderer
            .register_native_texture(device, view, wgpu::FilterMode::Linear)
    }
    pub fn unregister(&mut self, id: egui::TextureId) {
        self.renderer.free_texture(&id);
    }
    #[allow(clippy::too_many_arguments)]
    pub fn paint(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        size: [u32; 2],
        context: &egui::Context,
        mut output: egui::FullOutput,
        overlay: bool,
    ) {
        for (id, deltas) in output.textures_delta.set.drain() {
            for delta in deltas {
                self.renderer.update_texture(device, queue, id, &delta);
            }
        }
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: size,
            pixels_per_point: output.pixels_per_point,
        };
        let jobs = context.tessellate(output.shapes, output.pixels_per_point);
        let mut encoder = device.create_command_encoder(&Default::default());
        let callbacks = self
            .renderer
            .update_buffers(device, queue, &mut encoder, &jobs, &screen);
        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Song selector"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: if overlay {
                                wgpu::LoadOp::Load
                            } else {
                                wgpu::LoadOp::Clear(wgpu::Color {
                                    r: 0.006,
                                    g: 0.01,
                                    b: 0.02,
                                    a: 1.0,
                                })
                            },
                            store: wgpu::StoreOp::Store,
                        },
                        depth_slice: None,
                    })],
                    ..Default::default()
                })
                .forget_lifetime();
            self.renderer.render(&mut pass, &jobs, &screen);
        }
        queue.submit(callbacks.into_iter().chain([encoder.finish()]));
        for id in &output.textures_delta.free {
            self.renderer.free_texture(id);
        }
    }
}
