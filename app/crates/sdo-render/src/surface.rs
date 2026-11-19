//! Window-surface configuration, recovery and presentation.
use anyhow::{Context, Result, bail};
pub struct DisplaySurface {
    surface: wgpu::Surface<'static>,
    instance: wgpu::Instance,
    config: wgpu::SurfaceConfiguration,
}
pub struct Frame {
    texture: wgpu::SurfaceTexture,
    reconfigure: bool,
}
impl Frame {
    pub fn view(&self) -> wgpu::TextureView {
        self.texture.texture.create_view(&Default::default())
    }
}
impl DisplaySurface {
    pub async fn new(
        target: impl Into<wgpu::SurfaceTarget<'static>>,
        size: [u32; 2],
        frame_latency: u32,
    ) -> Result<(Self, wgpu::Device, wgpu::Queue, String)> {
        let instance = wgpu::util::new_instance_with_webgpu_detection(
            wgpu::InstanceDescriptor::new_without_display_handle(),
        )
        .await;
        let surface = instance.create_surface(target)?;
        let (adapter, device, queue, name) =
            crate::request_device(&instance, Some(&surface)).await?;
        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .or_else(|| caps.formats.first().copied())
            .context("Surface has no formats")?;
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            color_space: wgpu::SurfaceColorSpace::Auto,
            format,
            width: size[0].max(1),
            height: size[1].max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            desired_maximum_frame_latency: frame_latency,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
        };
        surface.configure(&device, &config);
        Ok((
            Self {
                surface,
                instance,
                config,
            },
            device,
            queue,
            name,
        ))
    }
    pub fn format(&self) -> wgpu::TextureFormat {
        self.config.format
    }
    pub fn size(&self) -> [u32; 2] {
        [self.config.width, self.config.height]
    }
    pub fn resize(&mut self, device: &wgpu::Device, size: [u32; 2]) {
        self.config.width = size[0].max(1);
        self.config.height = size[1].max(1);
        self.surface.configure(device, &self.config);
    }
    pub fn acquire(
        &mut self,
        device: &wgpu::Device,
        target: impl Into<wgpu::SurfaceTarget<'static>>,
    ) -> Result<Option<Frame>> {
        let (frame, reconfigure) = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) => (frame, false),
            wgpu::CurrentSurfaceTexture::Suboptimal(frame) => (frame, true),
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(None);
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(device, &self.config);
                return Ok(None);
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                self.surface = self.instance.create_surface(target)?;
                self.surface.configure(device, &self.config);
                return Ok(None);
            }
            wgpu::CurrentSurfaceTexture::Validation => bail!("wgpu surface validation failed"),
        };
        Ok(Some(Frame {
            texture: frame,
            reconfigure,
        }))
    }
    pub fn present(&self, frame: Frame, device: &wgpu::Device, queue: &wgpu::Queue) {
        queue.present(frame.texture);
        if frame.reconfigure {
            self.surface.configure(device, &self.config);
        }
    }
}
