//! One native-coordinate note field for gameplay and appearance previews.
use super::{LANE_PITCH, hold_sprites};
use sdo_assets::{FourKeySkin, HitAnimation, LaneFrames, Texture, TextureId};
use sdo_render::{BlendMode, Sprite};

pub const RECEPTOR_Y: f32 = 70.;
pub const NOTE_TOP: f32 = RECEPTOR_Y - 50.;

/// Distances are measured from the receptor in native board pixels.
#[derive(Debug, Clone, Copy)]
pub struct FieldNote {
    pub lane: usize,
    pub head: f32,
    pub tail: Option<f32>,
    pub holding: bool,
    pub missed: bool,
    pub beat_phase: usize,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct LaneFeedback {
    pub held: bool,
    pub pressed_age: Option<f64>,
    pub hit_age: Option<f64>,
}

pub struct NoteField<'a> {
    pub textures: &'a [Texture],
    pub lanes: &'a [LaneFrames; 4],
    pub board: TextureId,
    pub effect: Option<&'a HitAnimation>,
}
impl<'a> From<&'a FourKeySkin> for NoteField<'a> {
    fn from(skin: &'a FourKeySkin) -> Self {
        Self {
            textures: &skin.textures,
            lanes: &skin.lanes,
            board: skin.board,
            effect: skin.hit_effect.as_ref(),
        }
    }
}
impl NoteField<'_> {
    pub fn lane_center(lane: usize) -> f32 {
        49. + lane as f32 * LANE_PITCH
    }

    fn centered(&self, id: TextureId, x: f32, y: f32) -> Sprite {
        let image = &self.textures[id.0];
        let (w, h) = (image.width as f32, image.height as f32);
        Sprite::new(id, [x - w * 0.5, y - h * 0.5, w, h])
    }

    /// Board, receptors, notes and effects always use the same geometry/order.
    /// Callers supply scrolling distances and feedback; lane placement and sprite
    /// geometry are shared by gameplay and the demonstration.
    pub fn sprites(
        &self,
        frame: usize,
        feedback: &[LaneFeedback; 4],
        notes: impl IntoIterator<Item = FieldNote>,
    ) -> Vec<Sprite> {
        let board = &self.textures[self.board.0];
        let top = NOTE_TOP;
        let bottom = board.height as f32 - 12.;
        let mut sprites = vec![Sprite::new(
            self.board,
            [0., 0., board.width as f32, board.height as f32],
        )];
        for (lane, state) in feedback.iter().enumerate() {
            let frames = &self.lanes[lane];
            let id = if state.held || state.pressed_age.is_some_and(|age| age < 0.25) {
                frames.pressed_receptor((state.pressed_age.unwrap_or(0.) * 20.) as usize)
            } else {
                frames.receptor(frame)
            };
            sprites.push(self.centered(id, Self::lane_center(lane), RECEPTOR_Y));
        }
        for note in notes {
            let x = Self::lane_center(note.lane);
            let start_y = RECEPTOR_Y + note.head;
            let end_y = RECEPTOR_Y + note.tail.unwrap_or(note.head);
            if (!note.holding && end_y < RECEPTOR_Y - 55.) || start_y > bottom + 80. {
                continue;
            }
            let head_y = if note.holding { RECEPTOR_Y } else { start_y };
            if note.tail.is_some() {
                sprites.extend(
                    hold_sprites(
                        self.textures,
                        &self.lanes[note.lane],
                        frame,
                        [
                            x,
                            head_y,
                            if note.holding {
                                end_y.max(head_y)
                            } else {
                                end_y
                            },
                        ],
                        1.,
                        [top, bottom],
                    )
                    .into_iter()
                    .map(|mut sprite| {
                        if note.missed {
                            sprite.color = [0.35, 0.35, 0.35, 1.];
                        }
                        sprite
                    }),
                );
            } else {
                let id = self.lanes[note.lane].note_variant(frame, note.beat_phase);
                if let Some(sprite) = self.centered(id, x, head_y).clip_vertical(top, bottom) {
                    sprites.push(sprite);
                }
            }
        }
        if let Some(effect) = self.effect {
            for (lane, state) in feedback.iter().enumerate() {
                if let Some(hit) = state.hit_age.and_then(|age| effect.frame(lane, age)) {
                    let mut sprite =
                        self.centered(hit.texture, Self::lane_center(lane), RECEPTOR_Y);
                    if hit.additive {
                        sprite.blend = BlendMode::Additive;
                    }
                    sprites.push(sprite);
                }
            }
        }
        sprites
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sdo_assets::HitFrame;

    #[test]
    fn native_field_aligns_notes_with_board_lanes_and_preserves_hold_and_effect_order() {
        let textures = [
            (315, 600),
            (80, 80),
            (80, 80),
            (80, 80),
            (80, 80),
            (80, 64),
            (80, 85),
            (80, 80),
            (80, 80),
            (80, 80),
            (128, 128),
            (128, 128),
        ]
        .map(|(width, height)| Texture {
            source: "fixture".into(),
            width,
            height,
            rgba: vec![],
        });
        let lanes = std::array::from_fn(|_| LaneFrames {
            normal: vec![TextureId(1)],
            eighth: vec![TextureId(2)],
            sixteenth: vec![TextureId(3)],
            hold_head: vec![TextureId(4)],
            hold_body: vec![TextureId(5)],
            hold_tail: vec![TextureId(6)],
            receptor: vec![TextureId(7)],
            pressed: vec![TextureId(8), TextureId(9)],
        });
        let effect = HitAnimation {
            id: "hit".into(),
            label: "hit".into(),
            lanes: 1,
            frames: vec![
                HitFrame {
                    texture: TextureId(10),
                    additive: true,
                },
                HitFrame {
                    texture: TextureId(11),
                    additive: true,
                },
            ],
        };
        let field = NoteField {
            textures: &textures,
            lanes: &lanes,
            board: TextureId(0),
            effect: Some(&effect),
        };
        let mut feedback = [LaneFeedback::default(); 4];
        feedback[1] = LaneFeedback {
            held: true,
            pressed_age: Some(0.3),
            hit_age: Some(0.05),
        };
        let notes = (0..4).map(|lane| FieldNote {
            lane,
            head: if lane == 1 { -50. } else { 100. },
            tail: (lane == 1).then_some(75.),
            holding: lane == 1,
            missed: false,
            beat_phase: lane,
        });
        let sprites = field.sprites(0, &feedback, notes);
        assert_eq!(sprites[0].texture, TextureId(0));
        assert_eq!(
            sprites[1..5].iter().map(|s| s.texture).collect::<Vec<_>>(),
            vec![TextureId(7), TextureId(9), TextureId(7), TextureId(7)]
        );
        // Divider positions from the original 315-pixel board, independently of lane_center().
        let boundaries = [14., 83., 152., 221., 290.];
        for (lane, texture) in [1, 4, 2, 3].into_iter().enumerate() {
            let sprite = sprites
                .iter()
                .find(|s| s.texture == TextureId(texture))
                .unwrap();
            for scale in [0.4, 1., 1.5] {
                let placed = sprite.clone().transformed([37., 19.], scale);
                let center = placed.rect[0] + placed.rect[2] * 0.5;
                let lo = 37. + boundaries[lane] * scale;
                let hi = 37. + boundaries[lane + 1] * scale;
                assert!(center > lo && center < hi, "Note is on a divider");
                assert!((center - (lo + hi) * 0.5).abs() <= 0.6 * scale);
            }
        }
        let head_index = sprites
            .iter()
            .position(|s| s.texture == TextureId(4))
            .unwrap();
        let head = &sprites[head_index];
        assert_eq!(head.rect[1] + head.rect[3] * 0.5, RECEPTOR_Y);
        for (index, sprite) in sprites
            .iter()
            .enumerate()
            .filter(|(_, s)| matches!(s.texture.0, 5 | 6))
        {
            assert!(index < head_index);
            assert!(sprite.rect[1] >= RECEPTOR_Y);
        }
        assert_eq!(sprites.last().unwrap().texture, TextureId(11));
        assert_eq!(sprites.last().unwrap().blend, BlendMode::Additive);

        let missed = FieldNote {
            lane: 0,
            head: -100.,
            tail: Some(200.),
            holding: false,
            missed: true,
            beat_phase: 0,
        };
        let failed = field.sprites(0, &[LaneFeedback::default(); 4], [missed]);
        assert!(
            failed.len() > 5,
            "A missed head must leave the remaining hold visible"
        );
        assert!(
            failed[5..]
                .iter()
                .all(|sprite| sprite.color == [0.35, 0.35, 0.35, 1.])
        );
        assert!(
            failed[..5].iter().all(|sprite| sprite.color == [1.; 4]),
            "Miss tint must not affect the board or receptors"
        );
        let later = field.sprites(
            0,
            &[LaneFeedback::default(); 4],
            [FieldNote {
                head: -250.,
                tail: Some(50.),
                ..missed
            }],
        );
        assert!(later.len() > 5 && later.len() < failed.len());
        let gone = field.sprites(
            0,
            &[LaneFeedback::default(); 4],
            [FieldNote {
                head: -500.,
                tail: Some(-200.),
                ..missed
            }],
        );
        assert_eq!(
            gone.len(),
            5,
            "Cull only once the missed hold has scrolled offscreen"
        );
        let awaiting_release = field.sprites(
            0,
            &[LaneFeedback::default(); 4],
            [FieldNote {
                head: -500.,
                tail: Some(-100.),
                holding: true,
                missed: false,
                ..missed
            }],
        );
        let head = awaiting_release
            .iter()
            .find(|sprite| sprite.texture == TextureId(4))
            .unwrap();
        assert_eq!(head.rect[1] + head.rect[3] / 2., RECEPTOR_Y);
    }
}
