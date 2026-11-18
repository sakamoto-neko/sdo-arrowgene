//! Animation001 MOT: explicit bone IDs and sparse rotation/scale/position keys.
use crate::{CharacterSkeleton, character::Reader};
use anyhow::{Result, ensure};
use glam::{Mat4, Quat, Vec3};
#[derive(Debug)]
struct Key<T> {
    frame: f32,
    value: T,
}
#[derive(Debug)]
struct Track {
    rotation: Vec<Key<Quat>>,
    scale: Vec<Key<Vec3>>,
    translation: Vec<Key<Vec3>>,
}
#[derive(Debug)]
pub struct CharacterMotion {
    tracks: Vec<Track>,
    pub last_frame: f32,
}
fn interval<T>(keys: &[Key<T>], frame: f32) -> (&T, &T, f32) {
    let end = keys.partition_point(|k| k.frame <= frame);
    if end == 0 {
        return (&keys[0].value, &keys[0].value, 0.);
    }
    if end == keys.len() {
        let v = &keys[end - 1].value;
        return (v, v, 0.);
    }
    let (a, b) = (&keys[end - 1], &keys[end]);
    (
        &a.value,
        &b.value,
        ((frame - a.frame) / (b.frame - a.frame)).clamp(0., 1.),
    )
}
impl CharacterMotion {
    pub fn sha256(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut hash = Sha256::new();
        hash.update(self.last_frame.to_le_bytes());
        hash.update((self.tracks.len() as u64).to_le_bytes());
        for track in &self.tracks {
            hash.update((track.rotation.len() as u64).to_le_bytes());
            for key in &track.rotation {
                hash.update(key.frame.to_le_bytes());
                for v in key.value.to_array() {
                    hash.update(v.to_le_bytes());
                }
            }
            for keys in [&track.scale, &track.translation] {
                hash.update((keys.len() as u64).to_le_bytes());
                for key in keys {
                    hash.update(key.frame.to_le_bytes());
                    for v in key.value.to_array() {
                        hash.update(v.to_le_bytes());
                    }
                }
            }
        }
        format!("{:x}", hash.finalize())
    }
    pub fn parse(raw: &[u8]) -> Result<Self> {
        let mut r = Reader { raw, at: 0 };
        ensure!(r.take(12)? == b"Animation001", "Unsupported MOT format");
        let n = r.u32()? as usize;
        ensure!((1..=512).contains(&n), "Invalid motion bone count");
        let mut tracks: Vec<Option<Track>> = (0..n).map(|_| None).collect();
        fn keys<T>(
            r: &mut Reader<'_>,
            n: usize,
            mut read: impl FnMut(&mut Reader<'_>) -> Result<T>,
        ) -> Result<Vec<Key<T>>> {
            ensure!(n > 0 && n <= 1_000_000, "Invalid MOT key count");
            let mut keys = Vec::new();
            let mut previous = -1.;
            for _ in 0..n {
                let value = read(r)?;
                let frame = r.f32()?;
                ensure!(frame >= 0. && frame > previous, "MOT frames must increase");
                previous = frame;
                keys.push(Key { frame, value });
            }
            Ok(keys)
        }
        for _ in 0..n {
            let id = r.u32()? as usize;
            ensure!(
                id < n && tracks[id].is_none(),
                "Invalid or duplicate MOT bone ID"
            );
            ensure!(r.u32()? == 0, "Unsupported MOT track flags");
            let nr = r.u32()? as usize;
            let ns = r.u32()? as usize;
            let nt = r.u32()? as usize;
            let rotation = keys(&mut r, nr, |r| {
                let q = Quat::from_xyzw(r.f32()?, r.f32()?, r.f32()?, r.f32()?);
                ensure!(q.length_squared() > 0.01, "Zero MOT quaternion");
                Ok(q.normalize())
            })?;
            let vector = |r: &mut Reader<'_>| Ok(Vec3::new(r.f32()?, r.f32()?, r.f32()?));
            let scale = keys(&mut r, ns, vector)?;
            let translation = keys(&mut r, nt, vector)?;
            tracks[id] = Some(Track {
                rotation,
                scale,
                translation,
            });
        }
        let last_frame = r.f32()?;
        ensure!(last_frame >= 0., "Negative MOT end frame");
        r.done()?;
        let tracks = tracks.into_iter().map(Option::unwrap).collect::<Vec<_>>();
        ensure!(
            tracks
                .iter()
                .all(|t| t.rotation.last().unwrap().frame <= last_frame + 0.01
                    && t.scale.last().unwrap().frame <= last_frame + 0.01
                    && t.translation.last().unwrap().frame <= last_frame + 0.01),
            "MOT key exceeds end frame"
        );
        Ok(Self { tracks, last_frame })
    }
    pub fn bone_count(&self) -> usize {
        self.tracks.len()
    }
    pub fn supports_bones(&self, count: usize) -> bool {
        self.bone_count() == count
    }
    pub fn pose(&self, rig: &CharacterSkeleton, frame: f32) -> Result<Vec<Mat4>> {
        ensure!(
            rig.bones.len() == self.tracks.len(),
            "{}-bone motion cannot animate a {}-bone character",
            self.tracks.len(),
            rig.bones.len()
        );
        ensure!(frame.is_finite(), "Invalid motion frame");
        let locals = self
            .tracks
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let (a, b, f) = interval(&t.rotation, frame);
                // MOT quaternion keys use the inverse rotation convention of HRC's matrices.
                let rotation = a.slerp(*b, f).conjugate();
                let (a, b, f) = interval(&t.scale, frame);
                let scale = a.lerp(*b, f);
                let (a, b, f) = interval(&t.translation, frame);
                let mut translation = a.lerp(*b, f);
                // A solo note-field view keeps horizontal root travel centered on the stage.
                if rig.bones[i].parent.is_none() {
                    translation.x = rig.bones[i].local.w_axis.x;
                    translation.z = rig.bones[i].local.w_axis.z;
                }
                Mat4::from_scale_rotation_translation(scale, rotation, translation)
            })
            .collect::<Vec<_>>();
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
                .map(|p| global(p, rig, locals, out))
                .unwrap_or(Mat4::IDENTITY)
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CharacterBone;
    fn w(b: &mut Vec<u8>, v: u32) {
        b.extend(v.to_le_bytes());
    }
    fn f(b: &mut Vec<u8>, v: f32) {
        b.extend(v.to_le_bytes());
    }
    fn example() -> Vec<u8> {
        let mut b = b"Animation001".to_vec();
        w(&mut b, 2);
        for id in [1, 0] {
            for v in [id, 0, 2, 1, 1] {
                w(&mut b, v);
            }
            for (q, t) in [
                (Quat::IDENTITY, 0.),
                (
                    if id == 1 {
                        Quat::from_rotation_z(std::f32::consts::FRAC_PI_2)
                    } else {
                        Quat::IDENTITY
                    },
                    10.,
                ),
            ] {
                for v in q.to_array() {
                    f(&mut b, v);
                }
                f(&mut b, t);
            }
            for v in [1., 1., 1., 0.] {
                f(&mut b, v);
            }
            for v in [0., if id == 0 { 20. } else { 1. }, 0., 0.] {
                f(&mut b, v);
            }
        }
        f(&mut b, 10.);
        b
    }
    #[test]
    fn explicit_bone_ids_interpolation_and_mot_rotation_convention() {
        let m = CharacterMotion::parse(&example()).unwrap();
        let rig = CharacterSkeleton {
            source: "rig.HRC".into(),
            bones: vec![
                CharacterBone {
                    name: "Root".into(),
                    parent: None,
                    local: Mat4::IDENTITY,
                    global: Mat4::IDENTITY,
                },
                CharacterBone {
                    name: "Child".into(),
                    parent: Some(0),
                    local: Mat4::IDENTITY,
                    global: Mat4::IDENTITY,
                },
            ],
        };
        let posed = m.pose(&rig, 5.).unwrap();
        let p = posed[1].transform_point3(Vec3::X);
        assert!((p.x - 0.5f32.sqrt()).abs() < 0.0001);
        assert!((p.y - (21. - 0.5f32.sqrt())).abs() < 0.0001);
        assert_eq!(posed[0].w_axis.y, 20.);
        assert_eq!(m.pose(&rig, 5.).unwrap(), posed);
        assert_eq!(m.pose(&rig, 100.).unwrap(), m.pose(&rig, 10.).unwrap());
        assert!(CharacterMotion::parse(&example()[..50]).is_err());
        let mut wrong = rig;
        wrong.bones.pop();
        assert!(m.pose(&wrong, 0.).is_err());
    }
}
