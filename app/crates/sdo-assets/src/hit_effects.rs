//! In-memory note-effect frames; JSON discovery is handled by UiAssets.
use crate::TextureId;
#[derive(Debug, Clone)]
pub struct HitAnimation {
    pub id: String,
    pub label: String,
    pub lanes: usize,
    pub frames: Vec<HitFrame>,
}
#[derive(Debug, Clone, Copy)]
pub struct HitFrame {
    pub texture: TextureId,
    pub additive: bool,
}
impl HitAnimation {
    pub fn frame(&self, lane: usize, elapsed: f64) -> Option<HitFrame> {
        if elapsed < 0.0 || !elapsed.is_finite() || self.lanes == 0 {
            return None;
        }
        let per_lane = self.frames.len() / self.lanes;
        let frame = (elapsed * 30.0).floor() as usize;
        if frame >= per_lane {
            return None;
        }
        self.frames
            .get((lane % self.lanes) * per_lane + frame)
            .copied()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hit_animation_uses_each_lanes_frames_and_finishes_without_looping() {
        let animation = HitAnimation {
            id: "test".into(),
            label: "test".into(),
            lanes: 4,
            frames: (0..48)
                .map(|i| HitFrame {
                    texture: TextureId(i),
                    additive: true,
                })
                .collect(),
        };
        assert_eq!(animation.frame(0, 0.0).unwrap().texture, TextureId(0));
        assert_eq!(animation.frame(1, 0.1).unwrap().texture, TextureId(15));
        assert_eq!(animation.frame(3, 0.1).unwrap().texture, TextureId(39));
        assert!(animation.frame(0, 0.4).is_none());
        assert!(animation.frame(0, -1.0).is_none());
    }
}
