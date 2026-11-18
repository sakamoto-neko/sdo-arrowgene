//! Performance selects choreography, a miss reaction, then rest; blends use local joints.
use crate::{DancePose, SongDance};
use anyhow::{Result, ensure};
use glam::Mat4;
use sdo_assets::{CharacterMotion, CharacterSkeleton};
use std::{path::PathBuf, sync::Arc};

#[derive(Debug, Clone)]
pub struct ReactionMotion {
    pub(crate) name: String,
    pub(crate) frames_per_second: f32,
    pub(crate) motion: Arc<CharacterMotion>,
}
impl ReactionMotion {
    pub fn new(
        name: String,
        frames_per_second: f32,
        motion: Arc<CharacterMotion>,
        bones: usize,
    ) -> Result<Self> {
        ensure!(
            frames_per_second.is_finite() && frames_per_second > 0.,
            "Animation {name} has an invalid playback rate"
        );
        ensure!(
            motion.supports_bones(bones),
            "Animation {name} has {} bones; the character needs {bones}",
            motion.bone_count()
        );
        ensure!(motion.last_frame > 0., "Animation {name} has no duration");
        Ok(Self {
            name,
            frames_per_second,
            motion,
        })
    }
    fn duration(&self) -> f64 {
        f64::from(self.motion.last_frame) / f64::from(self.frames_per_second)
    }
    fn sample(&self, elapsed: f64, looping: bool, rig: &CharacterSkeleton) -> Result<DancePose> {
        let frame = elapsed.max(0.) * f64::from(self.frames_per_second);
        let last = f64::from(self.motion.last_frame);
        let frame = if looping {
            frame.rem_euclid(last)
        } else {
            frame.min(last)
        } as f32;
        Ok(DancePose {
            globals: self.motion.pose(rig, frame)?,
            motion: self.name.clone(),
            frame,
        })
    }
}
#[derive(Clone)]
struct Transition {
    started_at: f64,
    from: Vec<Mat4>,
}
/// Owns playback state, with all motion files already loaded by DanceLibrary.
/// Time is the audio clock during play/pause; a failed stage may supply a continuing
/// presentation clock for idle. Never pass a miss-frozen choreography timestamp.
#[derive(Clone)]
pub struct DancePlayer {
    dance: Option<Arc<SongDance>>,
    dance_starts_at: Option<f64>,
    idle: Option<ReactionMotion>,
    miss: Option<ReactionMotion>,
    play_miss: bool,
    dancing: Option<bool>,
    entered_at: f64,
    last_time: Option<f64>,
    last_rig: Option<PathBuf>,
    last_pose: Option<DancePose>,
    transition: Option<Transition>,
}
impl DancePlayer {
    pub fn new(
        dance: Option<SongDance>,
        idle: Option<ReactionMotion>,
        miss: Option<ReactionMotion>,
    ) -> Self {
        Self {
            dance_starts_at: dance.as_ref().and_then(SongDance::starts_at),
            dance: dance.map(Arc::new),
            idle,
            miss,
            play_miss: false,
            dancing: None,
            entered_at: 0.,
            last_time: None,
            last_rig: None,
            last_pose: None,
            transition: None,
        }
    }
    pub fn has_choreography(&self) -> bool {
        self.dance.as_ref().is_some_and(|dance| dance.has_motion())
    }
    pub fn waiting_for_choreography(&self, time: f64) -> bool {
        self.dance_starts_at.is_some_and(|start| time < start)
    }
    pub fn reset(&mut self) {
        self.dancing = None;
        self.play_miss = false;
        self.entered_at = 0.;
        self.last_time = None;
        self.last_rig = None;
        self.last_pose = None;
        self.transition = None;
    }
    pub fn sample(
        &mut self,
        time: f64,
        dancing: bool,
        rig: &CharacterSkeleton,
    ) -> Result<Option<DancePose>> {
        ensure!(time.is_finite(), "Invalid dance playback time");
        if self.last_time.is_some_and(|previous| time < previous)
            || self
                .last_rig
                .as_ref()
                .is_some_and(|previous| *previous != rig.source)
        {
            self.reset();
        }
        let dancing = dancing && self.dance_starts_at.is_some_and(|start| time >= start);
        if self.dancing != Some(dancing) {
            self.play_miss = !dancing && self.dancing == Some(true);
            self.transition = self.last_pose.as_ref().map(|pose| Transition {
                started_at: time,
                from: pose.globals.clone(),
            });
            self.entered_at = time;
            self.dancing = Some(dancing);
        }
        let mut pose = if dancing {
            // Recovery always samples the current music time, even during the blend.
            self.dance.as_ref().unwrap().sample(time, rig)?
        } else {
            self.rest_pose(time - self.entered_at, rig)?
        };
        if let (Some(pose), Some(transition)) = (&mut pose, &self.transition) {
            let fraction = ((time - transition.started_at) / 0.2).clamp(0., 1.) as f32;
            if fraction < 1. {
                // Smoothstep gives the start/end of the crossfade zero blend velocity.
                let weight = smoothstep(fraction);
                pose.globals = blend_globals(rig, &transition.from, &pose.globals, weight)?;
            } else {
                self.transition = None;
            }
        }
        self.last_time = Some(time);
        self.last_rig = Some(rig.source.clone());
        self.last_pose = pose.clone();
        Ok(pose)
    }

    fn rest_pose(&self, elapsed: f64, rig: &CharacterSkeleton) -> Result<Option<DancePose>> {
        let miss = self.miss.as_ref().filter(|_| self.play_miss);
        if let Some(miss) = miss
            && elapsed < miss.duration()
        {
            return miss.sample(elapsed, false, rig).map(Some);
        }
        // Derive the boundary from the clip duration, not the last render time.
        // Repeated misses cannot restart the reaction or delay the rest loop.
        let idle_elapsed = elapsed - miss.map_or(0., ReactionMotion::duration);
        let mut pose = self
            .idle
            .as_ref()
            .map(|idle| idle.sample(idle_elapsed, true, rig))
            .transpose()?;
        if let Some(miss) = miss {
            let last = miss.sample(miss.duration(), false, rig)?;
            if let Some(idle) = &mut pose {
                let fraction = (idle_elapsed / 0.2).clamp(0., 1.) as f32;
                if fraction < 1. {
                    idle.globals =
                        blend_globals(rig, &last.globals, &idle.globals, smoothstep(fraction))?;
                }
            } else {
                // Optional presets without a rest loop retain the final reaction pose.
                pose = Some(last);
            }
        }
        Ok(pose)
    }
}

fn smoothstep(fraction: f32) -> f32 {
    fraction * fraction * (3. - 2. * fraction)
}

/// Interpolating global matrices collapses rotated limbs. Recover each local
/// transform, interpolate its quaternion/scale/translation, then rebuild globals.
fn blend_globals(
    rig: &CharacterSkeleton,
    from: &[Mat4],
    to: &[Mat4],
    weight: f32,
) -> Result<Vec<Mat4>> {
    ensure!(
        from.len() == rig.bones.len() && to.len() == rig.bones.len(),
        "Transition pose does not match character skeleton"
    );
    if weight <= 0. {
        return Ok(from.to_vec());
    }
    if weight >= 1. {
        return Ok(to.to_vec());
    }
    let locals = rig
        .bones
        .iter()
        .enumerate()
        .map(|(i, bone)| {
            let local = |pose: &[Mat4]| {
                bone.parent
                    .map_or(pose[i], |parent| pose[parent].inverse() * pose[i])
            };
            let a = local(from);
            let b = local(to);
            ensure!(
                a.is_finite() && b.is_finite(),
                "Invalid transition at bone {i}"
            );
            let (scale_a, rotation_a, translation_a) = a.to_scale_rotation_translation();
            let (scale_b, rotation_b, translation_b) = b.to_scale_rotation_translation();
            let result = Mat4::from_scale_rotation_translation(
                scale_a.lerp(scale_b, weight),
                rotation_a.normalize().slerp(rotation_b.normalize(), weight),
                translation_a.lerp(translation_b, weight),
            );
            ensure!(result.is_finite(), "Invalid blended transform at bone {i}");
            Ok(result)
        })
        .collect::<Result<Vec<_>>>()?;
    fn global(
        i: usize,
        rig: &CharacterSkeleton,
        locals: &[Mat4],
        out: &mut [Option<Mat4>],
    ) -> Mat4 {
        if let Some(m) = out[i] {
            return m;
        }
        let m = rig.bones[i]
            .parent
            .map_or(Mat4::IDENTITY, |p| global(p, rig, locals, out))
            * locals[i];
        out[i] = Some(m);
        m
    }
    let mut out = vec![None; locals.len()];
    for i in 0..out.len() {
        global(i, rig, &locals, &mut out);
    }
    Ok(out.into_iter().map(Option::unwrap).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;
    use sdo_assets::CharacterBone;

    #[test]
    fn rotated_joint_blends_preserve_limb_length_and_handle_unordered_parents() -> Result<()> {
        let child = Mat4::from_translation(Vec3::Y * 2.);
        let rig = CharacterSkeleton {
            source: "rig.HRC".into(),
            bones: vec![
                CharacterBone {
                    name: "child".into(),
                    local: child,
                    global: child,
                    parent: Some(1),
                },
                CharacterBone {
                    name: "root".into(),
                    local: Mat4::IDENTITY,
                    global: Mat4::IDENTITY,
                    parent: None,
                },
            ],
        };
        let rotated = Mat4::from_rotation_z(std::f32::consts::PI);
        let from = [child, Mat4::IDENTITY];
        let to = [rotated * child, rotated];
        for weight in [0., 0.25, 0.5, 0.75, 1.] {
            let pose = blend_globals(&rig, &from, &to, weight)?;
            assert!(
                (pose[0]
                    .w_axis
                    .truncate()
                    .distance(pose[1].w_axis.truncate())
                    - 2.)
                    .abs()
                    < 0.0001
            );
            for m in pose {
                assert!((m.determinant() - 1.).abs() < 0.0001);
            }
        }
        Ok(())
    }
}
