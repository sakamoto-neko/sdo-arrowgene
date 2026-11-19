//! Original four-key skin layout and animation selection, emitting generic sprites.
use sdo_assets::FourKeySkin;
use sdo_gameplay::input::InputState;
use sdo_gameplay::{Judgment, Lane, NoteState, Practice};
use sdo_render::{Scene, Sprite};

mod field;
pub use field::{FieldNote, LaneFeedback, NOTE_TOP, NoteField, RECEPTOR_Y};

/// 69 pixels is the four-lane spacing constant traced in the 2014 client.
pub const LANE_PITCH: f32 = 69.0;
#[derive(Default)]
pub struct NoteDisplay {
    pub speed: sdo_config::ScrollSpeed,
}
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum ServerGrade {
    #[default]
    Unreported,
    Pending,
    Judged(Judgment),
}
impl ServerGrade {
    fn state(self, predicted: NoteState) -> NoteState {
        match self {
            Self::Unreported => predicted,
            Self::Pending if predicted == NoteState::Holding => predicted,
            Self::Pending => NoteState::Pending,
            Self::Judged(Judgment::Miss) => NoteState::Miss,
            Self::Judged(_) => NoteState::Hit,
        }
    }
}
#[derive(Default)]
pub struct RoundFeedback<'a> {
    pub grades: &'a [ServerGrade],
    pub correction: Option<sdo_gameplay::JudgmentEvent>,
    pub totals: Option<(u64, u32, i32)>,
}
impl NoteDisplay {
    /// Beat-space scrolling stays continuous across tempo changes. At 150 BPM,
    /// 1x travels 210 pixels/second, matching the original preview scale.
    pub fn note_distance(&self, practice: &Practice, note_time: f64) -> f32 {
        (practice.beat_at(note_time) - practice.beat_at(practice.position())) as f32
            * 84.0
            * self.speed.multiplier()
    }
    pub fn scene(&self, skin: &FourKeySkin, practice: &Practice, input: &InputState) -> Scene {
        self.scene_with_status(skin, practice, input, &[])
    }
    /// Dedicated online layout: a narrow field leaves the stage visible.
    pub fn multiplayer_scene(
        &self,
        skin: &FourKeySkin,
        practice: &Practice,
        input: &InputState,
        stage: sdo_assets::TextureId,
    ) -> Scene {
        self.round_scene(skin, practice, input, stage, RoundFeedback::default())
    }
    pub fn round_scene(
        &self,
        skin: &FourKeySkin,
        practice: &Practice,
        input: &InputState,
        stage: sdo_assets::TextureId,
        feedback: RoundFeedback<'_>,
    ) -> Scene {
        let (points, combo, hp) = feedback.totals.unwrap_or((
            practice.score().points,
            practice.score().combo,
            practice.health().value(),
        ));
        let mut scene = Scene::new(960., 720.);
        scene.push(Sprite::new(stage, [320., 0., 640., 720.]));
        let board = skin.texture(skin.board);
        let scale = (300. / board.width as f32).min(640. / board.height as f32);
        let origin = [10., 48.];
        scene.sprites.extend(
            self.field_sprites(skin, practice, input, feedback.grades)
                .into_iter()
                .map(|s| s.transformed(origin, scale)),
        );
        health_bar(
            &mut scene,
            skin,
            hp,
            [origin[0], origin[1], board.width as f32 * scale],
        );
        let center = origin[0] + board.width as f32 * scale * 0.5;
        digits(
            &mut scene,
            skin,
            &skin.ui.score_digits,
            points,
            [center, 12.],
            0.75,
            Some(8),
        );
        let predicted = practice.last_judgment().and_then(|mut e| {
            match e.note.and_then(|i| feedback.grades.get(i)) {
                Some(ServerGrade::Pending) => return None,
                Some(ServerGrade::Judged(grade)) => e.judgment = *grade,
                _ => {}
            }
            Some(e)
        });
        let event = feedback
            .correction
            .filter(|c| predicted.is_none_or(|p| c.at >= p.at))
            .or(predicted);
        if let Some(event) = event {
            let age = practice.position() - event.at;
            let frames = &skin.ui.judgments[event.judgment.index()];
            if (0.0..frames.len() as f64 / 30.).contains(&age) {
                let id = frames[(age * 30.) as usize];
                let image = skin.texture(id);
                scene.push(Sprite::new(
                    id,
                    [
                        center - image.width as f32 * 0.4,
                        248.,
                        image.width as f32 * 0.8,
                        image.height as f32 * 0.8,
                    ],
                ));
            }
        }
        if combo > 0 {
            digits(
                &mut scene,
                skin,
                &skin.ui.combo_digits,
                combo as u64,
                [center, 340.],
                0.85,
                None,
            );
            let id = skin.ui.combo[0];
            let image = skin.texture(id);
            scene.push(Sprite::new(
                id,
                [
                    center - image.width as f32 * 0.4,
                    395.,
                    image.width as f32 * 0.8,
                    image.height as f32 * 0.8,
                ],
            ));
        }
        text(
            &mut scene,
            skin,
            &format!("{}X  BPM {:.0}", self.speed.value(), practice.bpm()),
            22.,
            698.,
            1.,
            [0.55, 0.85, 0.9, 1.],
        );
        scene
    }
    /// Replace the control hints with caller-supplied status, without protocol dependencies.
    pub fn scene_with_status(
        &self,
        skin: &FourKeySkin,
        practice: &Practice,
        input: &InputState,
        status: &[String],
    ) -> Scene {
        let mut scene = Scene::new(960.0, 720.0);
        let board = skin.texture(skin.board);
        let board_x = (scene.width - board.width as f32) * 0.5;
        let board_y = 60.0;
        scene.sprites.extend(
            self.field_sprites(skin, practice, input, &[])
                .into_iter()
                .map(|sprite| sprite.transformed([board_x, board_y], 1.)),
        );
        health_bar(
            &mut scene,
            skin,
            practice.health().value(),
            [board_x, board_y, board.width as f32],
        );
        hud(&mut scene, skin, practice, status);
        text(
            &mut scene,
            skin,
            &format!("SPEED {}X   BPM {:.1}", self.speed.value(), practice.bpm()),
            686.,
            588.,
            1.25,
            [0.55, 0.85, 0.9, 1.],
        );
        scene
    }
    fn field_sprites(
        &self,
        skin: &FourKeySkin,
        practice: &Practice,
        input: &InputState,
        confirmed: &[ServerGrade],
    ) -> Vec<Sprite> {
        let time = practice.position();
        let frame = (time * 12.).floor() as usize;
        let held = input.held();
        let feedback = std::array::from_fn(|lane| LaneFeedback {
            held: held[lane],
            pressed_age: key_feedback_age(practice, input, Lane::ALL[lane], time),
            hit_age: practice.last_hit_at(Lane::ALL[lane]).map(|at| time - at),
        });
        let pixels_per_beat = 84. * self.speed.multiplier() as f64;
        let beat = practice.beat_at(time);
        let board_height = skin.texture(skin.board).height as f64;
        let notes = practice
            .visible_notes(
                beat - 56. / pixels_per_beat,
                beat + (board_height + 81.) / pixels_per_beat,
            )
            .filter_map(|(index, (note, state))| {
                let state = confirmed
                    .get(index)
                    .copied()
                    .unwrap_or_default()
                    .state(state);
                if matches!(state, NoteState::Hit | NoteState::Skipped)
                    || (state == NoteState::Miss && !note.is_hold())
                {
                    return None;
                }
                Some(FieldNote {
                    lane: note.lane.index(),
                    head: self.note_distance(practice, note.start),
                    tail: note
                        .is_hold()
                        .then(|| self.note_distance(practice, note.end)),
                    holding: state == NoteState::Holding,
                    missed: state == NoteState::Miss,
                    beat_phase: ((practice.beat_at(note.start) * 48.0).round() as usize / 12) % 4,
                })
            });
        NoteField::from(skin).sprites(frame, &feedback, notes)
    }
}
/// One hold layout for gameplay and the appearance preview. Caps are centered
/// on their note times; their authored transparent padding overlaps the body.
/// Clip the tail at the head's center so its built-in strip cannot travel ahead
/// of the starting arrow. The head draws last and stays at the receptor while
/// holding. Tile phase follows the moving tail instead of freezing at the head.
pub fn hold_sprites(
    textures: &[sdo_assets::Texture],
    lane: &sdo_assets::LaneFrames,
    frame: usize,
    [x, head_y, end_y]: [f32; 3],
    scale: f32,
    [top, bottom]: [f32; 2],
) -> Vec<Sprite> {
    let mut sprites = Vec::new();
    if scale <= 0. || !scale.is_finite() || end_y < head_y {
        return sprites;
    }
    let cap = |frames: &[sdo_assets::TextureId], y| {
        let id = frames[frame % frames.len()];
        let image = &textures[id.0];
        let (w, h) = (image.width as f32 * scale, image.height as f32 * scale);
        Sprite::new(id, [x - w * 0.5, y - h * 0.5, w, h])
    };
    let head = cap(&lane.hold_head, head_y);
    let tail = cap(&lane.hold_tail, end_y);
    let join = head_y;
    let body_end = end_y;
    let id = lane.hold_body[frame % lane.hold_body.len()];
    let body = &textures[id.0];
    let tile = body.height as f32 * scale;
    let body_top = join.max(top);
    let body_bottom = body_end.min(bottom);
    if tile > 0. && body_top < body_bottom {
        let width = body.width as f32 * scale;
        let mut y = body_end - ((body_end - body_top) / tile).ceil() * tile;
        while y < body_bottom {
            if let Some(sprite) = Sprite::new(id, [x - width * 0.5, y, width, tile])
                .clip_vertical(body_top, body_bottom)
            {
                sprites.push(sprite);
            }
            y += tile;
        }
    }
    if let Some(sprite) = tail.clip_vertical(join.max(top), bottom) {
        sprites.push(sprite);
    }
    if let Some(sprite) = head.clip_vertical(top, bottom) {
        sprites.push(sprite);
    }
    sprites
}

/// Compact choreography status beside the solo character view.
pub fn dance_status(scene: &mut Scene, skin: &FourKeySkin, label: &str, warning: bool) {
    text(
        scene,
        skin,
        label,
        24.,
        552.,
        0.9,
        if warning {
            [1., 0.65, 0.35, 1.]
        } else {
            [0.78, 0.83, 0.93, 1.]
        },
    );
}

/// Best completed manual result for this chart, difficulty and timing preset.
pub fn personal_best(scene: &mut Scene, skin: &FourKeySkin, points: Option<u64>) {
    text(
        scene,
        skin,
        "PERSONAL BEST",
        710.,
        100.,
        1.5,
        [0.55, 0.85, 0.9, 1.],
    );
    if let Some(points) = points {
        digits(
            scene,
            skin,
            &skin.ui.score_digits,
            points,
            [795., 124.],
            0.8,
            Some(8),
        );
    } else {
        text(
            scene,
            skin,
            "NO SCORE YET",
            710.,
            132.,
            1.25,
            [0.65, 0.7, 0.8, 1.],
        );
    }
}

/// Persistence feedback below the personal best, after the final judgment.
pub fn score_save_status(scene: &mut Scene, skin: &FourKeySkin, saved: bool) {
    text(
        scene,
        skin,
        if saved {
            "SCORE SAVED"
        } else {
            "SCORE SAVE FAILED"
        },
        710.,
        169.,
        1.1,
        if saved {
            [0.55, 0.9, 0.75, 1.]
        } else {
            [1., 0.65, 0.35, 1.]
        },
    );
}

fn key_feedback_age(practice: &Practice, input: &InputState, lane: Lane, now: f64) -> Option<f64> {
    input
        .last_pressed_at(lane)
        .into_iter()
        .chain(practice.last_hit_at(lane))
        .filter(|at| *at <= now)
        .max_by(f64::total_cmp)
        .map(|at| now - at)
}

fn digits(
    scene: &mut Scene,
    skin: &FourKeySkin,
    ids: &[sdo_assets::TextureId],
    value: u64,
    anchor: [f32; 2],
    scale: f32,
    width: Option<usize>,
) {
    let [center, y] = anchor;
    let text = if let Some(width) = width {
        format!("{value:0width$}")
    } else {
        value.to_string()
    };
    let advance = skin.texture(ids[0]).width as f32 * scale - 4.0 * scale;
    let x = center - advance * text.len() as f32 / 2.0;
    for (i, digit) in text.bytes().enumerate() {
        let id = ids[(digit - b'0') as usize];
        let image = skin.texture(id);
        scene.push(Sprite::new(
            id,
            [
                x + i as f32 * advance,
                y,
                image.width as f32 * scale,
                image.height as f32 * scale,
            ],
        ));
    }
}
fn text(
    scene: &mut Scene,
    skin: &FourKeySkin,
    label: &str,
    x: f32,
    y: f32,
    scale: f32,
    color: [f32; 4],
) {
    for (i, c) in label.chars().enumerate() {
        let code = if c.is_ascii() { c as u32 } else { b'?' as u32 };
        let mut sprite = Sprite::new(
            skin.ui.font,
            [x + i as f32 * 8.0 * scale, y, 8.0 * scale, 8.0 * scale],
        );
        sprite.uv = [
            (code % 16) as f32 / 16.0,
            (code / 16) as f32 / 8.0,
            (code % 16 + 1) as f32 / 16.0,
            (code / 16 + 1) as f32 / 8.0,
        ];
        sprite.color = color;
        scene.push(sprite);
    }
}
fn hud(scene: &mut Scene, skin: &FourKeySkin, practice: &Practice, status: &[String]) {
    let score = practice.score();
    let center = scene.width * 0.5;
    digits(
        scene,
        skin,
        &skin.ui.score_digits,
        score.points.min(99_999_999),
        [center, 12.0],
        1.0,
        Some(8),
    );
    let white = [0.78, 0.83, 0.93, 1.0];
    if status.is_empty() {
        text(scene, skin, "4 KEY", 36.0, 36.0, 2.0, white);
        text(scene, skin, "D F J K / ARROWS", 36.0, 72.0, 1.0, white);
        text(scene, skin, "SPACE PAUSE", 36.0, 92.0, 1.0, white);
        text(scene, skin, "R RESTART", 36.0, 112.0, 1.0, white);
        text(scene, skin, "F1 JUDGMENT PRESET", 36.0, 132.0, 1.0, white);
        text(scene, skin, "- / + SCROLL SPEED", 36.0, 152.0, 1.0, white);
    } else {
        status_lines(scene, skin, status);
    }
    text(scene, skin, "JUDGMENT PRESET", 36.0, 580.0, 1.0, white);
    text(
        scene,
        skin,
        practice.preset().id,
        36.0,
        602.0,
        1.0,
        [1.0, 0.8, 0.4, 1.0],
    );
    text(
        scene,
        skin,
        if practice.preset().verified_native_taps {
            "NATIVE TAP WINDOWS"
        } else {
            "PRACTICE RULES - ESTIMATED"
        },
        36.0,
        624.0,
        1.0,
        white,
    );
    text(scene, skin, "HOLDS: PRACTICE", 36.0, 638.0, 1.0, white);
    let w = practice.windows();
    text(
        scene,
        skin,
        &format!(
            "P {:.1} / C {:.1} / B {:.1} MS",
            w[0] * 1000.0,
            w[1] * 1000.0,
            w[2] * 1000.0
        ),
        36.0,
        656.0,
        1.0,
        white,
    );
    for (index, judgment) in sdo_gameplay::Judgment::ALL.into_iter().enumerate() {
        let id = skin.ui.judgments[index][0];
        let image = skin.texture(id);
        let y = 200.0 + index as f32 * 92.0;
        scene.push(Sprite::new(
            id,
            [
                710.0,
                y,
                image.width as f32 * 0.8,
                image.height as f32 * 0.8,
            ],
        ));
        digits(
            scene,
            skin,
            &skin.ui.score_digits,
            score.counts[judgment.index()] as u64,
            [785.0, y + 34.0],
            0.7,
            None,
        );
    }
    if let Some(event) = practice.last_judgment() {
        let age = practice.position() - event.at;
        let frames = &skin.ui.judgments[event.judgment.index()];
        if (0.0..frames.len() as f64 / 30.0).contains(&age) {
            let id = frames[((age * 30.0) as usize).min(frames.len() - 1)];
            let image = skin.texture(id);
            let mut sprite = Sprite::new(
                id,
                [
                    center - image.width as f32 / 2.0,
                    230.0,
                    image.width as f32,
                    image.height as f32,
                ],
            );
            sprite.color[3] = (1.0 - (age as f32 - 0.35).max(0.0) * 2.5).clamp(0.0, 1.0);
            scene.push(sprite);
            text(
                scene,
                skin,
                &format!("{:+.1} MS", event.error * 1000.0),
                center - 48.0,
                278.0,
                1.0,
                white,
            );
        }
    }
    if score.combo > 0 {
        digits(
            scene,
            skin,
            &skin.ui.combo_digits,
            score.combo as u64,
            [center, 315.0],
            1.0,
            None,
        );
        let id = skin.ui.combo[0];
        let image = skin.texture(id);
        scene.push(Sprite::new(
            id,
            [
                center - image.width as f32 / 2.0,
                378.0,
                image.width as f32,
                image.height as f32,
            ],
        ));
    }
    let progress = (practice.position() / practice.duration()).clamp(0.0, 1.0) as f32;
    let mut bar = Sprite::new(FourKeySkin::WHITE, [322.0, 684.0, 315.0 * progress, 3.0]);
    bar.color = [0.3, 0.8, 1.0, 1.0];
    scene.push(bar);
    text(
        scene,
        skin,
        &format!("{:.1} / {:.1} S", practice.position(), practice.duration()),
        414.0,
        698.0,
        1.0,
        white,
    );
    if practice.failed() {
        let mut panel = Sprite::new(FourKeySkin::WHITE, [337., 414., 286., 92.]);
        panel.color = [0.06, 0.07, 0.11, 0.94];
        scene.push(panel);
        text(
            scene,
            skin,
            "STAGE FAILED",
            center - 96.,
            434.,
            2.,
            [1., 0.45, 0.5, 1.],
        );
        text(
            scene,
            skin,
            "R RETRY  /  ESC SONGS",
            center - 80.,
            478.,
            1.,
            white,
        );
    } else if practice.paused() {
        text(
            scene,
            skin,
            "PAUSED",
            center - 48.0,
            440.0,
            2.0,
            [1.0, 0.8, 0.4, 1.0],
        );
    }
}

fn health_bar(
    scene: &mut Scene,
    skin: &FourKeySkin,
    hp: i32,
    [board_x, board_y, board_width]: [f32; 3],
) {
    let health = sdo_gameplay::Health::with_value(hp);
    let fraction = health.fraction();
    let (width, height) = skin.health.as_ref().map_or((246., 12.), |bar| {
        let frame = skin.texture(bar.frame);
        (frame.width as f32, frame.height as f32)
    });
    let scale = ((board_width - 28.).max(0.) / width).min(1.);
    let x = board_x + (board_width - width * scale) * 0.5;
    // The board's top strip sits above the four receptors.
    let y = board_y + 24. - height * scale * 0.5;
    let color = if health.low() {
        [1., 0.45, 0.5, 1.]
    } else {
        [0.78, 0.83, 0.93, 1.]
    };
    if let Some(bar) = &skin.health {
        scene.push(Sprite::new(
            bar.frame,
            [x, y, width * scale, height * scale],
        ));
        if fraction > 0.0 {
            let id = if health.value() == sdo_gameplay::Health::MAX {
                bar.full
            } else {
                bar.fill
            };
            let image = skin.texture(id);
            let mut fill = Sprite::new(
                id,
                [
                    x + bar.fill_offset[0] * scale,
                    y + bar.fill_offset[1] * scale,
                    image.width as f32 * scale * fraction,
                    image.height as f32 * scale,
                ],
            );
            fill.uv[2] = fraction;
            scene.push(fill);
        }
    } else {
        let mut track = Sprite::new(FourKeySkin::WHITE, [x, y, width * scale, height * scale]);
        track.color = [0.16, 0.18, 0.24, 1.];
        scene.push(track);
        if fraction > 0.0 {
            let mut fill = Sprite::new(
                FourKeySkin::WHITE,
                [
                    x + 2. * scale,
                    y + 2. * scale,
                    (width - 4.) * scale * fraction,
                    (height - 4.) * scale,
                ],
            );
            fill.color = color;
            scene.push(fill);
        }
    }
    if health.low() {
        let label = if health.failed() { "EMPTY" } else { "DANGER" };
        text(
            scene,
            skin,
            label,
            board_x + board_width * 0.5 - label.len() as f32 * 4. * scale,
            y + (height - 8.) * scale * 0.5,
            scale,
            [1., 0.9, 0.9, 1.],
        );
    }
}

#[cfg(test)]
mod key_feedback_tests {
    use super::*;
    use sdo_gameplay::input::Binding;
    #[test]
    fn released_taps_finish_their_animation_and_autoplay_provides_feedback_without_keys() {
        let mut practice = Practice::default();
        let mut input = InputState::default();
        assert_eq!(key_feedback_age(&practice, &input, Lane::Left, 0.0), None);
        input.update(Binding::D, true, 0.05);
        input.update(Binding::D, false, 0.06);
        assert!(!input.held()[0]);
        assert!(
            (key_feedback_age(&practice, &input, Lane::Left, 0.15).unwrap() - 0.1).abs() < 1e-9
        );
        input.clear();
        assert_eq!(key_feedback_age(&practice, &input, Lane::Left, 0.15), None);
        practice.advance_to(2.1, [false; 4], true);
        assert!((key_feedback_age(&practice, &input, Lane::Left, 2.1).unwrap() - 0.1).abs() < 1e-9);
    }
}

#[cfg(test)]
mod hold_layout_tests {
    use super::*;
    use sdo_assets::{LaneFrames, Texture, TextureId};
    fn fixture() -> ([Texture; 3], LaneFrames) {
        let textures = [(80, 80), (80, 64), (80, 85)].map(|(width, height)| Texture {
            source: "fixture".into(),
            width,
            height,
            rgba: vec![],
        });
        let lane = LaneFrames {
            normal: vec![TextureId(0)],
            eighth: vec![],
            sixteenth: vec![],
            hold_head: vec![TextureId(0)],
            hold_body: vec![TextureId(1)],
            hold_tail: vec![TextureId(2)],
            receptor: vec![TextureId(0)],
            pressed: vec![TextureId(0)],
        };
        (textures, lane)
    }
    #[test]
    fn shrinking_holds_keep_the_head_visible_and_never_draw_the_strip_through_it() {
        let (textures, lane) = fixture();
        // Includes the unequal 80/85-pixel caps used by Retro Rainbow,
        // the preview's short holds, and long gameplay holds.
        for scale in [0.5, 0.8, 1.] {
            for length in [0., 1., 10., 40., 80., 160., 400.] {
                let head_y = 100.;
                let sprites = hold_sprites(
                    &textures,
                    &lane,
                    0,
                    [50., head_y, head_y + length * scale],
                    scale,
                    [0., 800.],
                );
                let head = sprites.last().unwrap();
                assert_eq!(head.texture, TextureId(0));
                assert_eq!(head.uv, [0., 0., 1., 1.]);
                assert_eq!(head.rect[3], 80. * scale);
                let join = head_y;
                for segment in &sprites[..sprites.len() - 1] {
                    assert!(segment.rect[1] >= join - 0.001);
                    assert!(segment.rect[3] > 0.);
                    assert!(
                        (segment.rect[3]
                            - (segment.uv[3] - segment.uv[1])
                                * textures[segment.texture.0].height as f32
                                * scale)
                            .abs()
                            < 0.001
                    );
                }
            }
        }
    }
    #[test]
    fn hold_tiles_join_both_caps_and_scroll_with_the_tail_without_stretching() {
        let (textures, lane) = fixture();
        let draw = |end| hold_sprites(&textures, &lane, 0, [50., 100., end], 1., [0., 800.]);
        let first = draw(300.);
        let moved = draw(295.);
        let tiles: Vec<_> = first.iter().filter(|s| s.texture == TextureId(1)).collect();
        assert_eq!(tiles.first().unwrap().rect[1], 100.);
        for pair in tiles.windows(2) {
            assert_eq!(pair[0].rect[1] + pair[0].rect[3], pair[1].rect[1]);
        }
        let tail = first.iter().find(|s| s.texture == TextureId(2)).unwrap();
        assert_eq!(
            tiles.last().unwrap().rect[1] + tiles.last().unwrap().rect[3],
            tail.rect[1] + tail.rect[3] * 0.5
        );
        assert!((moved[0].uv[1] - first[0].uv[1] - 5. / 64.).abs() < 0.001);
        let clipped = hold_sprites(&textures, &lane, 0, [50., 100., 300.], 1., [175., 250.]);
        assert!(
            clipped
                .iter()
                .all(|s| s.rect[1] >= 175. && s.rect[1] + s.rect[3] <= 250.)
        );
    }
    #[test]
    fn transparent_cap_padding_cannot_open_gaps_in_the_hold_body() {
        let (mut textures, lane) = fixture();
        textures[2].height = 80;
        let sprites = hold_sprites(&textures, &lane, 0, [50., 100., 300.], 1., [0., 800.]);
        let tiles: Vec<_> = sprites
            .iter()
            .filter(|s| s.texture == TextureId(1))
            .collect();
        // Default/Pet tails contain an entirely transparent top half. Their
        // body must reach the tail center, not stop at the texture's top edge.
        assert_eq!(tiles.first().unwrap().rect[1], 100.);
        assert_eq!(
            tiles.last().unwrap().rect[1] + tiles.last().unwrap().rect[3],
            300.
        );
        let tail = sprites.iter().find(|s| s.texture == TextureId(2)).unwrap();
        assert_eq!(tail.rect[1] + tail.rect[3] * 0.5, 300.);
    }

    #[test]
    fn scroll_speed_and_tempo_change_visual_motion_without_changing_chart_time() {
        let mut practice = Practice::default().with_bpm(60.).with_tempo(vec![
            sdo_gameplay::TempoChange {
                beat: 0.,
                at: 0.,
                bpm: 60.,
            },
            sdo_gameplay::TempoChange {
                beat: 4.,
                at: 4.,
                bpm: 120.,
            },
        ]);
        let display = NoteDisplay::default();
        practice.seek_preview(3.);
        let before = display.note_distance(&practice, 6.);
        practice.seek_preview(4.);
        let boundary = display.note_distance(&practice, 6.);
        practice.seek_preview(5.);
        let after = display.note_distance(&practice, 6.);
        assert_eq!(before - boundary, 84.);
        assert_eq!(boundary - after, 168.);
        assert_eq!(
            NoteDisplay {
                speed: sdo_config::ScrollSpeed::new(3).unwrap()
            }
            .note_distance(&practice, 6.),
            after * 3.
        );
        assert_eq!(practice.position(), 5.);
        assert_eq!(practice.bpm(), 120.);
    }
}

/// Generic status overlay; callers provide text without coupling rendering to networking.
fn status_lines(scene: &mut Scene, skin: &FourKeySkin, lines: &[String]) {
    for (index, line) in lines.iter().take(11).enumerate() {
        text(
            scene,
            skin,
            &line.chars().take(34).collect::<String>(),
            36.,
            36. + index as f32 * 15.,
            1.,
            [1., 1., 1., 1.],
        );
    }
}
