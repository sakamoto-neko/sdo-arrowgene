//! Authored DPS choreography sampled from absolute audio time; no frame-delta clock.
use anyhow::{Context, Result, ensure};
mod playback;
use glam::Mat4;
pub use playback::{DancePlayer, ReactionMotion};
mod replay;
pub use replay::ReplayDance;
use sdo_assets::{CharacterAnimation, CharacterMotion, CharacterSkeleton};
use std::{
    collections::{BTreeSet, HashMap},
    sync::Arc,
};
#[derive(Debug)]
pub struct DanceClip {
    pub name: String,
    pub first: f32,
    pub last: f32,
    pub seconds: f64,
    pub translation: [f32; 3],
    pub rotation: f32,
    pub vector: [f32; 3],
    pub flag: u8,
    pub motion: Option<Arc<CharacterMotion>>,
}
#[derive(Debug)]
pub struct DanceGroup {
    pub start: f64,
    pub seconds: f64,
    pub clips: Vec<DanceClip>,
}
#[derive(Debug)]
pub struct SongDance {
    pub groups: Vec<DanceGroup>,
    pub unit_ms: u32,
    pub issues: Vec<String>,
    pub stage_transforms: usize,
}
/// Identity of normalized choreography and every referenced motion, including
/// explicit missing clips. It contains no local paths or package/catalog IDs.
pub fn song_signature(dance: Option<&SongDance>) -> String {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    if let Some(dance) = dance {
        hash.update([1]);
        hash.update(dance.unit_ms.to_le_bytes());
        hash.update((dance.groups.len() as u64).to_le_bytes());
        for group in &dance.groups {
            hash.update(group.start.to_le_bytes());
            hash.update(group.seconds.to_le_bytes());
            hash.update((group.clips.len() as u64).to_le_bytes());
            for clip in &group.clips {
                for value in [clip.first, clip.last, clip.rotation] {
                    hash.update(value.to_le_bytes());
                }
                hash.update(clip.seconds.to_le_bytes());
                for value in clip.translation.into_iter().chain(clip.vector) {
                    hash.update(value.to_le_bytes());
                }
                hash.update([clip.flag]);
                if let Some(motion) = &clip.motion {
                    hash.update([1]);
                    hash.update(motion.sha256().as_bytes());
                } else {
                    hash.update([0]);
                }
            }
        }
    } else {
        hash.update([0]);
    }
    format!("{:x}", hash.finalize())
}
#[derive(Debug, Clone)]
pub struct DancePose {
    pub globals: Vec<Mat4>,
    pub motion: String,
    pub frame: f32,
}
type MotionResolver = dyn Fn(&str) -> Result<Arc<CharacterMotion>> + Send + Sync;
pub struct DanceLibrary {
    load_motion: Box<MotionResolver>,
    motions: HashMap<String, std::result::Result<Arc<CharacterMotion>, String>>,
}
impl DanceLibrary {
    pub fn new(
        load_motion: impl Fn(&str) -> Result<Arc<CharacterMotion>> + Send + Sync + 'static,
    ) -> Self {
        Self {
            load_motion: Box::new(load_motion),
            motions: HashMap::new(),
        }
    }
    pub fn motion(&mut self, name: &str) -> Result<Arc<CharacterMotion>> {
        ensure!(
            !name.contains(['/', '\\', ':']) && name.to_ascii_lowercase().ends_with(".mot"),
            "Invalid MOT filename {name:?}"
        );
        let key = name.to_ascii_lowercase();
        if !self.motions.contains_key(&key) {
            let result = (self.load_motion)(name);
            self.motions
                .insert(key.clone(), result.map_err(|e| format!("{e:#}")));
        }
        self.motions[&key]
            .as_ref()
            .map(Arc::clone)
            .map_err(|s| anyhow::anyhow!("{name}: {s}"))
    }
    /// Load preset reactions through the same validated, cached motion pipeline.
    pub fn load_reaction(
        &mut self,
        definition: &CharacterAnimation,
        bones: usize,
    ) -> Result<ReactionMotion> {
        let motion = self.motion(&definition.motion)?;
        ReactionMotion::new(
            definition.motion.clone(),
            definition.frames_per_second,
            motion,
            bones,
        )
    }

    pub fn load(
        &mut self,
        raw: &[u8],
        gn: &[u8],
        difficulty: usize,
        bones: usize,
    ) -> Result<SongDance> {
        ensure!(
            gn.len() >= 284 && difficulty < 3 && &gn[4..8] == b"gn\0\0",
            "Invalid GN dance timing header"
        );
        let word = |at| u32::from_le_bytes(gn[at..at + 4].try_into().unwrap());
        let measures = word(52 + 4 * difficulty);
        let seconds = word(272 + 4 * difficulty);
        let mut r = Reader { raw, at: 0 };
        let signature = r.take(8)?;
        ensure!(
            signature == b"PAS00002" || signature == b"PAS00003",
            "Unsupported DPS format"
        );
        let stride = if signature == b"PAS00003" { 305 } else { 304 };
        let divisions = r.u32()?;
        ensure!(
            divisions > 0 && measures > 0 && seconds > 0,
            "Zero GN/DPS dance timing divisor"
        );
        let unit_ms =
            (u64::from(seconds) * 1000 / (u64::from(measures) * u64::from(divisions))) as u32;
        ensure!(unit_ms > 0, "DPS unit is shorter than one millisecond");
        r.take(260)?;
        let count = r.u32()? as usize;
        ensure!(count <= raw.len() / 12, "Invalid DPS group count");
        let mut units = 0u64;
        let mut groups = Vec::new();
        let mut issues = BTreeSet::new();
        let mut stage_transforms = 0;
        for _ in 0..count {
            let _authored_start = r.u32()?;
            let length = r.u32()?;
            let n = r.u32()? as usize;
            ensure!(n <= (raw.len() - r.at) / stride, "Truncated DPS clips");
            let mut clips = Vec::new();
            for _ in 0..n {
                let name = std::str::from_utf8(r.take(260)?.split(|v| *v == 0).next().unwrap())
                    .context("Non-UTF-8 DPS motion name")?
                    .to_owned();
                let first = r.u32()? as f32;
                let last = r.u32()? as f32;
                let duration = r.float()?;
                ensure!(
                    last >= first && duration >= 0.,
                    "Invalid DPS frame range/duration"
                );
                let translation = [r.float()?, r.float()?, r.float()?];
                let rotation = r.float()?;
                r.u32()?;
                let vector = [r.float()?, r.float()?, r.float()?];
                let flag = if stride == 305 { r.take(1)?[0] } else { 0 };
                if translation != [0.; 3] || rotation != 0. || vector != [0.; 3] {
                    stage_transforms += 1;
                }
                let motion = match self.motion(&name) {
                    Ok(m) if !m.supports_bones(bones) => {
                        issues.insert(format!("{name}: {}-bone motion is incompatible with the {bones}-bone character",m.bone_count()));
                        None
                    }
                    Ok(m) if first > m.last_frame + 1. || last > m.last_frame + 1. => {
                        issues.insert(format!(
                            "{name}: DPS frames {first}..{last} exceed MOT end {}",
                            m.last_frame
                        ));
                        None
                    }
                    Ok(m) => Some(m),
                    Err(e) => {
                        issues.insert(format!("{e:#}"));
                        None
                    }
                };
                clips.push(DanceClip {
                    name,
                    first,
                    last,
                    seconds: duration as f64,
                    translation,
                    rotation,
                    vector,
                    flag,
                    motion,
                });
            }
            groups.push(DanceGroup {
                start: units as f64 * unit_ms as f64 / 1000.,
                seconds: length as f64 * unit_ms as f64 / 1000.,
                clips,
            });
            units += u64::from(length);
        }
        ensure!(r.at == raw.len(), "Trailing DPS bytes");
        ensure!(!groups.is_empty(), "DPS has no groups");
        Ok(SongDance {
            groups,
            unit_ms,
            issues: issues.into_iter().collect(),
            stage_transforms,
        })
    }
}
impl SongDance {
    /// Empty lead-in groups belong to the rest animation. Choreography begins
    /// at the first playable clip, including preceding clip durations.
    pub fn starts_at(&self) -> Option<f64> {
        for (index, group) in self.groups.iter().enumerate() {
            let next_group = self
                .groups
                .get(index + 1)
                .map_or(f64::INFINITY, |next| next.start);
            let mut at = group.start;
            for clip in &group.clips {
                if at >= next_group {
                    break;
                }
                if clip.motion.is_some() && clip.seconds > 0. {
                    return Some(at);
                }
                at += clip.seconds;
            }
        }
        None
    }
    pub fn sample(&self, time: f64, rig: &CharacterSkeleton) -> Result<Option<DancePose>> {
        ensure!(time.is_finite(), "Invalid dance sample time");
        let g = self
            .groups
            .partition_point(|g| g.start <= time.max(0.))
            .saturating_sub(1);
        let group = &self.groups[g];
        let mut elapsed = (time - group.start).max(0.);
        let mut selected = None;
        for c in &group.clips {
            if elapsed < c.seconds && c.seconds > 0. {
                if c.motion.is_some() {
                    selected = Some((
                        c,
                        c.first + (c.last - c.first) * (elapsed / c.seconds) as f32,
                    ));
                }
                break;
            }
            if c.motion.is_some() {
                selected = Some((c, c.last));
            }
            elapsed -= c.seconds;
        }
        if selected.is_none() {
            selected = self.groups[..g]
                .iter()
                .rev()
                .flat_map(|g| g.clips.iter().rev())
                .find(|c| c.motion.is_some())
                .map(|c| (c, c.last));
        }
        // Use an authored ready pose during an empty lead-in, rather than a T pose.
        if selected.is_none() {
            selected = self
                .groups
                .iter()
                .flat_map(|g| &g.clips)
                .find(|c| c.motion.is_some())
                .map(|c| (c, c.first));
        }
        selected
            .map(|(c, frame)| {
                Ok(DancePose {
                    globals: c.motion.as_ref().unwrap().pose(rig, frame)?,
                    motion: c.name.clone(),
                    frame,
                })
            })
            .transpose()
    }
    pub fn has_motion(&self) -> bool {
        self.groups
            .iter()
            .flat_map(|g| &g.clips)
            .any(|c| c.motion.is_some())
    }
}
struct Reader<'a> {
    raw: &'a [u8],
    at: usize,
}
impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        ensure!(
            n <= self.raw.len().saturating_sub(self.at),
            "Truncated DPS at byte {}",
            self.at
        );
        let v = &self.raw[self.at..self.at + n];
        self.at += n;
        Ok(v)
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn float(&mut self) -> Result<f32> {
        let v = f32::from_le_bytes(self.take(4)?.try_into().unwrap());
        ensure!(v.is_finite(), "Nonfinite DPS value");
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sdo_assets::CharacterBone;
    fn w(b: &mut Vec<u8>, v: u32) {
        b.extend(v.to_le_bytes());
    }
    fn f(b: &mut Vec<u8>, v: f32) {
        b.extend(v.to_le_bytes());
    }
    fn motion() -> CharacterMotion {
        let mut b = b"Animation001".to_vec();
        w(&mut b, 1);
        for v in [0, 0, 1, 1, 2] {
            w(&mut b, v);
        }
        for v in [
            0., 0., 0., 1., 0., 1., 1., 1., 0., 0., 0., 0., 0., 0., 20., 0., 20., 20.,
        ] {
            f(&mut b, v);
        }
        CharacterMotion::parse(&b).unwrap()
    }
    fn clip(b: &mut Vec<u8>, name: &str, first: u32, last: u32, seconds: f32) {
        let mut n = [0; 260];
        n[..name.len()].copy_from_slice(name.as_bytes());
        b.extend(n);
        w(b, first);
        w(b, last);
        f(b, seconds);
        for _ in 0..4 {
            f(b, 0.);
        }
        w(b, 0);
        for _ in 0..3 {
            f(b, 0.);
        }
        b.push(0);
    }
    #[test]
    fn audio_time_sampling_handles_lead_in_groups_holds_pause_and_restart() -> Result<()> {
        let mut raw = b"PAS00003".to_vec();
        w(&mut raw, 4);
        raw.extend([0; 260]);
        w(&mut raw, 3);
        for v in [0, 1, 0, 999, 3, 2] {
            w(&mut raw, v);
        }
        clip(&mut raw, "a.MOT", 0, 10, 1.);
        clip(&mut raw, "a.MOT", 10, 20, 2.);
        for v in [0, 2, 0] {
            w(&mut raw, v);
        }
        let mut gn = vec![0; 284];
        gn[4..8].copy_from_slice(b"gn\0\0");
        gn[52..56].copy_from_slice(&1u32.to_le_bytes());
        gn[272..276].copy_from_slice(&4u32.to_le_bytes());
        let mut library = DanceLibrary {
            load_motion: Box::new(|id| anyhow::bail!("Missing motion {id}")),
            motions: HashMap::from([("a.mot".into(), Ok(Arc::new(motion())))]),
        };
        let dance = library.load(&raw, &gn, 0, 1)?;
        assert_eq!(dance.unit_ms, 1000);
        assert!(dance.issues.is_empty());
        let rig = CharacterSkeleton {
            source: "rig.HRC".into(),
            bones: vec![CharacterBone {
                name: "root".into(),
                parent: None,
                local: Mat4::IDENTITY,
                global: Mat4::IDENTITY,
            }],
        };
        for (time, frame) in [
            (0., 0.),
            (1.5, 5.),
            (2.5, 12.5),
            (4., 20.),
            (2.5, 12.5),
            (0., 0.),
        ] {
            let pose = dance.sample(time, &rig)?.unwrap();
            assert_eq!(pose.frame, frame);
            assert!((pose.globals[0].w_axis.y - frame).abs() < 0.001);
        }
        assert_eq!(
            dance.sample(2.5, &rig)?.unwrap().globals,
            dance.sample(2.5, &rig)?.unwrap().globals
        );
        assert!(library.motion("../source.MOT").is_err());
        gn[52..56].copy_from_slice(&3u32.to_le_bytes());
        assert_eq!(library.load(&raw, &gn, 0, 1)?.unit_ms, 333);
        Ok(())
    }
    fn playback_fixture() -> (DanceLibrary, SongDance, CharacterSkeleton) {
        let motion = Arc::new(motion());
        let library = DanceLibrary {
            load_motion: Box::new(|id| anyhow::bail!("Missing motion {id}")),
            motions: HashMap::from([("a.mot".into(), Ok(motion.clone()))]),
        };
        let dance = SongDance {
            unit_ms: 1000,
            issues: Vec::new(),
            stage_transforms: 0,
            groups: vec![DanceGroup {
                start: 0.,
                seconds: 20.,
                clips: vec![DanceClip {
                    name: "dance.MOT".into(),
                    first: 0.,
                    last: 20.,
                    seconds: 20.,
                    translation: [0.; 3],
                    rotation: 0.,
                    vector: [0.; 3],
                    flag: 0,
                    motion: Some(motion),
                }],
            }],
        };
        let rig = CharacterSkeleton {
            source: "rig.HRC".into(),
            bones: vec![CharacterBone {
                name: "root".into(),
                parent: None,
                local: Mat4::IDENTITY,
                global: Mat4::IDENTITY,
            }],
        };
        (library, dance, rig)
    }
    fn idle_definition() -> CharacterAnimation {
        CharacterAnimation {
            motion: "a.MOT".into(),
            frames_per_second: 30.,
        }
    }
    #[test]
    fn replayed_poses_match_across_frame_rates_and_late_phase_replacement() -> Result<()> {
        use sdo_gameplay::DanceCue;
        let make = || -> Result<(ReplayDance, CharacterSkeleton)> {
            let (mut library, dance, rig) = playback_fixture();
            let idle = library.load_reaction(&idle_definition(), 1)?;
            let miss = library.load_reaction(&idle_definition(), 1)?;
            Ok((
                ReplayDance::new(
                    DancePlayer::new(Some(dance), Some(idle), Some(miss)),
                    Some(0.),
                ),
                rig,
            ))
        };
        let first = Arc::new(DanceCue {
            at: 1.,
            dancing: false,
            previous: None,
        });
        let speculative = Arc::new(DanceCue {
            at: 1.12,
            dancing: true,
            previous: Some(first.clone()),
        });
        let corrected = Arc::new(DanceCue {
            at: 1.06,
            dancing: true,
            previous: Some(first),
        });
        let (mut fast, rig) = make()?;
        let (mut slow, _) = make()?;
        for frame in 0..144 {
            fast.sample(frame as f64 / 120., Some(&speculative), &rig)?;
        }
        for frame in 0..24 {
            slow.sample(frame as f64 / 20., Some(&corrected), &rig)?;
        }
        let a = fast.sample(1.19, Some(&corrected), &rig)?.unwrap();
        let b = slow.sample(1.19, Some(&corrected), &rig)?.unwrap();
        assert_eq!(a.globals, b.globals);
        assert_eq!(a.frame, b.frame);
        for time in [1.2, 1.25, 1.4, 2.0, 5.0] {
            let a = fast.sample(time, Some(&corrected), &rig)?.unwrap();
            let b = slow.sample(time, Some(&corrected), &rig)?.unwrap();
            assert_eq!(a.globals, b.globals);
        }
        Ok(())
    }
    #[test]
    fn miss_blends_to_looping_idle_and_good_hit_resyncs_to_live_music() -> Result<()> {
        let (mut library, dance, rig) = playback_fixture();
        let idle = library.load_reaction(&idle_definition(), 1)?;
        let mut player = DancePlayer::new(Some(dance), Some(idle), None);
        let initial = player.sample(8., true, &rig)?.unwrap();
        assert_eq!(initial.frame, 8.);
        let missed = player.sample(8.1, false, &rig)?.unwrap();
        assert_eq!(missed.globals, initial.globals, "Miss snapped the pose");
        let paused = player.sample(8.1, false, &rig)?.unwrap();
        assert_eq!(paused.globals, missed.globals);
        let middle = player.sample(8.2, false, &rig)?.unwrap();
        assert!(middle.globals[0].w_axis.y > 3. && middle.globals[0].w_axis.y < 8.);
        for (time, frame) in [(8.5, 12.), (8.7, 18.), (8.9, 4.)] {
            let idle = player.sample(time, false, &rig)?.unwrap();
            assert_eq!(idle.motion, "a.MOT");
            assert!((idle.frame - frame).abs() < 0.0001);
            assert!((idle.globals[0].w_axis.y - frame).abs() < 0.0001);
        }
        let before_recovery = player.sample(8.9, false, &rig)?.unwrap();
        let recovering = player.sample(9., true, &rig)?.unwrap();
        assert_eq!(recovering.globals, before_recovery.globals);
        let recovered = player.sample(9.3, true, &rig)?.unwrap();
        assert!((recovered.globals[0].w_axis.y - 9.3).abs() < 0.0001);
        assert_eq!(recovered.motion, "dance.MOT");
        assert_eq!(recovered.frame, 9.3);
        Ok(())
    }
    #[test]
    fn empty_intro_loops_idle_then_blends_into_live_choreography() -> Result<()> {
        let (mut library, mut dance, rig) = playback_fixture();
        dance.groups[0].start = 4.;
        // Include an unavailable lead-in clip before the real motion.
        dance.groups[0].clips.insert(
            0,
            DanceClip {
                name: "missing.MOT".into(),
                first: 0.,
                last: 1.,
                seconds: 1.,
                translation: [0.; 3],
                rotation: 0.,
                vector: [0.; 3],
                flag: 0,
                motion: None,
            },
        );
        dance.groups.insert(
            0,
            DanceGroup {
                start: 0.,
                seconds: 4.,
                clips: Vec::new(),
            },
        );
        assert_eq!(dance.starts_at(), Some(5.));
        let idle = library.load_reaction(&idle_definition(), 1)?;
        let mut miss = library.load_reaction(&idle_definition(), 1)?;
        miss.name = "miss.MOT".into();
        let mut player = DancePlayer::new(Some(dance), Some(idle), Some(miss));
        let first = player.sample(0., true, &rig)?.unwrap();
        let waiting = player.sample(0.3, true, &rig)?.unwrap();
        assert_eq!(first.motion, "a.MOT");
        assert_eq!(waiting.motion, "a.MOT");
        assert_ne!(
            first.globals, waiting.globals,
            "Intro must animate, not hold the first dance frame"
        );
        let before = player.sample(4.9, true, &rig)?.unwrap();
        assert_eq!(before.motion, "a.MOT");
        let start = player.sample(5., true, &rig)?.unwrap();
        assert_eq!(
            start.globals, before.globals,
            "Starting choreography should crossfade from idle"
        );
        let dancing = player.sample(5.3, true, &rig)?.unwrap();
        assert_eq!(dancing.motion, "dance.MOT");
        assert!((dancing.frame - 0.3).abs() < 1e-5);
        player.reset();
        assert_eq!(player.sample(0., true, &rig)?.unwrap().motion, "a.MOT");
        Ok(())
    }
    #[test]
    fn interrupted_transitions_seek_restart_and_character_changes_discard_old_pose() -> Result<()> {
        let (mut library, dance, mut rig) = playback_fixture();
        let mut player = DancePlayer::new(
            Some(dance),
            Some(library.load_reaction(&idle_definition(), 1)?),
            None,
        );
        player.sample(8., true, &rig)?;
        player.sample(8.1, false, &rig)?;
        let interrupted = player.sample(8.2, false, &rig)?.unwrap();
        let recovery = player.sample(8.21, true, &rig)?.unwrap();
        assert_eq!(interrupted.globals, recovery.globals);
        let missed_again = player.sample(8.22, false, &rig)?.unwrap();
        assert_eq!(recovery.globals, missed_again.globals);
        assert_eq!(player.sample(1., true, &rig)?.unwrap().frame, 1.);
        assert_eq!(
            player.sample(1., true, &rig)?.unwrap().globals[0].w_axis.y,
            1.
        );
        player.reset();
        assert_eq!(player.sample(5., false, &rig)?.unwrap().frame, 0.);
        player.sample(5.4, false, &rig)?;
        rig.source = "other.HRC".into();
        assert_eq!(
            player.sample(5.4, false, &rig)?.unwrap().globals[0]
                .w_axis
                .y,
            0.
        );
        assert!(player.sample(f64::NAN, false, &rig).is_err());
        Ok(())
    }
    #[test]
    fn miss_plays_once_then_blends_to_rest_and_recovers_at_current_song_time() -> Result<()> {
        let (mut library, dance, rig) = playback_fixture();
        let idle = library.load_reaction(&idle_definition(), 1)?;
        let mut miss = library.load_reaction(
            &CharacterAnimation {
                frames_per_second: 20.,
                ..idle_definition()
            },
            1,
        )?;
        miss.name = "miss.MOT".into();
        let mut player = DancePlayer::new(Some(dance), Some(idle), Some(miss));
        let initial = player.sample(8., true, &rig)?.unwrap();
        assert_eq!(
            player.sample(8.1, false, &rig)?.unwrap().globals,
            initial.globals
        );
        for (time, frame) in [(8.4, 6.), (8.6, 10.)] {
            let pose = player.sample(time, false, &rig)?.unwrap();
            assert_eq!(pose.motion, "miss.MOT");
            assert!(
                (pose.frame - frame).abs() < 0.0001,
                "Repeated miss restarted the reaction"
            );
            assert!((pose.globals[0].w_axis.y - frame).abs() < 0.0001);
        }
        let boundary = player.sample(9.1, false, &rig)?.unwrap();
        assert!(
            (boundary.globals[0].w_axis.y - 20.).abs() < 0.0001,
            "Miss-to-rest boundary snapped"
        );
        let blend = player.sample(9.2, false, &rig)?.unwrap();
        assert!((blend.globals[0].w_axis.y - 11.5).abs() < 0.0001);
        for (time, frame) in [(9.4, 9.), (9.85, 2.5)] {
            let pose = player.sample(time, false, &rig)?.unwrap();
            assert_eq!(pose.motion, "a.MOT");
            assert!((pose.frame - frame).abs() < 0.0001);
            assert!((pose.globals[0].w_axis.y - frame).abs() < 0.0001);
        }
        let paused = player.sample(9.85, false, &rig)?.unwrap();
        assert_eq!(
            paused.globals,
            player.sample(9.85, false, &rig)?.unwrap().globals
        );
        assert_eq!(
            paused.globals,
            player.sample(9.9, true, &rig)?.unwrap().globals
        );
        let resumed = player.sample(10.2, true, &rig)?.unwrap();
        assert_eq!(resumed.motion, "dance.MOT");
        assert!((resumed.frame - 10.2).abs() < 0.0001);
        Ok(())
    }
    #[test]
    fn recovery_can_interrupt_miss_and_restart_does_not_trigger_a_reaction() -> Result<()> {
        let (mut library, dance, rig) = playback_fixture();
        let miss = library.load_reaction(&idle_definition(), 1)?;
        let mut player = DancePlayer::new(Some(dance), None, Some(miss));
        assert!(
            player.sample(1., false, &rig)?.is_none(),
            "No miss occurred at startup"
        );
        player.sample(2., true, &rig)?;
        player.sample(2.1, false, &rig)?;
        let reaction = player.sample(2.35, false, &rig)?.unwrap();
        assert_eq!(
            player.sample(2.36, true, &rig)?.unwrap().globals,
            reaction.globals
        );
        assert_eq!(player.sample(2.6, true, &rig)?.unwrap().motion, "dance.MOT");
        player.sample(3., false, &rig)?;
        let held = player.sample(4., false, &rig)?.unwrap();
        assert_eq!(
            held.frame, 20.,
            "A one-shot must not loop without a rest clip"
        );
        assert_eq!(
            held.globals,
            player.sample(5., false, &rig)?.unwrap().globals
        );
        player.reset();
        assert!(player.sample(0., false, &rig)?.is_none());
        assert_eq!(player.sample(0., true, &rig)?.unwrap().frame, 0.);
        Ok(())
    }
    #[test]
    fn idle_loading_checks_rate_references_and_skeleton_compatibility() -> Result<()> {
        let (mut library, _, _) = playback_fixture();
        let mut definition = idle_definition();
        library.load_reaction(&definition, 1)?;
        assert!(
            library
                .load_reaction(&definition, 55)
                .unwrap_err()
                .to_string()
                .contains("character needs")
        );
        for fps in [0., -1., f32::NAN, f32::INFINITY] {
            definition.frames_per_second = fps;
            assert!(library.load_reaction(&definition, 1).is_err());
        }
        definition.frames_per_second = 30.;
        definition.motion = "../source.MOT".into();
        assert!(library.load_reaction(&definition, 1).is_err());
        definition.motion = "missing.MOT".into();
        assert!(
            library
                .load_reaction(&definition, 1)
                .unwrap_err()
                .to_string()
                .contains("missing.MOT")
        );
        Ok(())
    }
}
