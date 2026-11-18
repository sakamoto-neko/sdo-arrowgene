//! Fixed stage-front cameras sample the same standing direction and animated pose.
use anyhow::Result;
use sdo_asset_loader::{AssetLoader, request as asset};
use sdo_assets::{CharacterAnimation, CharacterAssets, CharacterMotion, CharacterPreset};
use sdo_render::{
    Renderer,
    character::{CharacterCamera, CharacterRenderer},
};
use std::sync::Arc;

pub(crate) struct RoomCamera {
    gpu: sdo_render::Readback,
    renderer: CharacterRenderer,
    idle: Option<(Arc<CharacterMotion>, f32)>,
    walk: Option<(Arc<CharacterMotion>, f32)>,
    moving: bool,
    entered_at: f64,
    last_time: f64,
    yaw: f32,
}
impl RoomCamera {
    pub fn new(
        renderer: &Renderer,
        assets: &CharacterAssets,
        preset: &CharacterPreset,
        content: &AssetLoader,
    ) -> Self {
        Self::with_gpu(renderer.readback(), assets, preset, content)
    }
    fn with_gpu(
        gpu: sdo_render::Readback,
        assets: &CharacterAssets,
        preset: &CharacterPreset,
        content: &AssetLoader,
    ) -> Self {
        let load = |definition: &Option<CharacterAnimation>| {
            definition.as_ref().and_then(|definition| {
                let motion = content.request(asset::Motion(&definition.motion)).ok()?;
                (motion.supports_bones(preset.skeleton.bones.len())
                    && motion.last_frame > 0.
                    && definition.frames_per_second.is_finite()
                    && definition.frames_per_second > 0.)
                    .then_some((motion, definition.frames_per_second))
            })
        };
        Self {
            renderer: CharacterRenderer::new_room_preview(&gpu.device, &gpu.queue, assets),
            gpu,
            idle: load(&preset.idle),
            walk: load(&preset.walk),
            moving: false,
            entered_at: 0.,
            last_time: 0.,
            yaw: 0.,
        }
    }
    pub fn frame(
        &mut self,
        _renderer: &Renderer,
        preset: &CharacterPreset,
        seconds: f64,
        facing: [i8; 2],
        walking: [i8; 2],
    ) -> Result<sdo_render::character::Portrait> {
        self.frame_at(preset, seconds, facing, walking)
    }
    fn frame_at(
        &mut self,
        preset: &CharacterPreset,
        seconds: f64,
        facing: [i8; 2],
        walking: [i8; 2],
    ) -> Result<sdo_render::character::Portrait> {
        let moving = walking != [0, 0];
        if moving != self.moving {
            self.entered_at = seconds;
            self.moving = moving;
        }
        let motion = if moving {
            self.walk.as_ref().or(self.idle.as_ref())
        } else {
            self.idle.as_ref()
        };
        let pose = motion
            .map(|(motion, fps)| {
                let frame = ((seconds - self.entered_at).max(0.) * f64::from(*fps)) as f32
                    % motion.last_frame;
                motion.pose(&preset.skeleton, frame)
            })
            .transpose()?;
        let dt = (seconds - self.last_time).clamp(0., 0.1) as f32;
        self.last_time = seconds;
        let target = f32::from(facing[0]).atan2(f32::from(facing[1]));
        let delta = (target - self.yaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)
            - std::f32::consts::PI;
        self.yaw += delta * (dt * 14.).min(1.);
        self.capture(preset, self.yaw, pose.as_deref())
    }
    fn capture(
        &mut self,
        preset: &CharacterPreset,
        yaw: f32,
        pose: Option<&[glam::Mat4]>,
    ) -> Result<sdo_render::character::Portrait> {
        // Both fixed cameras see the same character turn, including its back.
        self.renderer.render_head_pose(
            &self.gpu.device,
            &self.gpu.queue,
            &preset.choice.id,
            CharacterCamera {
                yaw,
                ..Default::default()
            },
            pose,
        )?;
        let mut rgba = self
            .gpu
            .camera_rgba(&self.renderer.texture, sdo_render::character::PORTRAIT_SIZE)?;
        self.renderer.render_room_pose(
            &self.gpu.device,
            &self.gpu.queue,
            &preset.choice.id,
            CharacterCamera {
                yaw,
                ..Default::default()
            },
            pose,
        )?;
        let mut avatar = self
            .gpu
            .camera_rgba(&self.renderer.texture, sdo_render::character::AVATAR_SIZE)?;
        // The display is the same size as these views. Drop only the two lowest
        // color bits to keep a full eight-person room affordable to stream.
        for pixels in [&mut rgba, &mut avatar] {
            for pixel in pixels.as_chunks_mut::<4>().0 {
                for channel in &mut pixel[..3] {
                    *channel &= 0xfc;
                }
            }
        }
        let mut character = preset.choice.name.clone();
        while character.len() > 80 {
            character.pop();
        }
        let portrait = sdo_render::character::Portrait {
            character,
            rgba,
            avatar,
        };
        portrait.validate()?;
        Ok(portrait)
    }
}
