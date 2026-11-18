use crate::notes::{FieldNote, LaneFeedback, NOTE_TOP, NoteField};
use egui::{Color32, Rect, TextureHandle, pos2, vec2};
use sdo_assets::{HitAnimation, LaneFrames, Texture, TextureId, UiAssets, UiSelection};
use std::{collections::HashMap, sync::Arc};

/// Paints a cropped view of the gameplay NoteField; no independent lane/arrow layout.
pub struct AppearancePreview {
    textures: Arc<Vec<Texture>>,
    skins: HashMap<String, [LaneFrames; 4]>,
    effects: HashMap<String, HitAnimation>,
    boards: HashMap<String, TextureId>,
    uploaded: HashMap<(TextureId, bool), TextureHandle>,
    selection: Option<UiSelection>,
    started: f64,
}
impl AppearancePreview {
    pub fn new(assets: &UiAssets) -> Self {
        Self {
            textures: assets.textures.clone(),
            skins: assets
                .skins
                .iter()
                .map(|s| (s.choice.id.clone(), s.lanes.clone()))
                .collect(),
            effects: assets
                .effects
                .iter()
                .map(|s| (s.choice.id.clone(), s.animation.clone()))
                .collect(),
            boards: assets
                .boards
                .iter()
                .map(|b| (b.choice.id.clone(), b.image))
                .collect(),
            uploaded: HashMap::new(),
            selection: None,
            started: 0.0,
        }
    }
    fn sprite(&mut self, ui: &egui::Ui, sprite: &sdo_render::Sprite) {
        let id = sprite.texture;
        let additive = sprite.blend == sdo_render::BlendMode::Additive;
        let image = &self.textures[id.0];
        let texture = self.uploaded.entry((id, additive)).or_insert_with(|| {
            let mut color = egui::ColorImage::from_rgba_unmultiplied(
                [image.width as usize, image.height as usize],
                &image.rgba,
            );
            if additive {
                // Egui adds premultiplied RGB when alpha is zero. Respect source alpha
                // before conversion so transparent PNG padding contributes no light.
                for (pixel, rgba) in color.pixels.iter_mut().zip(image.rgba.as_chunks::<4>().0) {
                    let premultiplied =
                        Color32::from_rgba_unmultiplied(rgba[0], rgba[1], rgba[2], rgba[3]);
                    *pixel = Color32::from_rgba_premultiplied(
                        premultiplied.r(),
                        premultiplied.g(),
                        premultiplied.b(),
                        0,
                    );
                }
            }
            ui.ctx().load_texture(
                format!("appearance-{}-{additive}", id.0),
                color,
                egui::TextureOptions::LINEAR,
            )
        });
        ui.painter().image(
            texture.id(),
            Rect::from_min_size(
                pos2(sprite.rect[0], sprite.rect[1]),
                vec2(sprite.rect[2], sprite.rect[3]),
            ),
            Rect::from_min_max(
                pos2(sprite.uv[0], sprite.uv[1]),
                pos2(sprite.uv[2], sprite.uv[3]),
            ),
            Color32::WHITE,
        );
    }
    pub fn show(&mut self, ui: &mut egui::Ui, selection: &UiSelection) {
        let now = ui.input(|i| i.time);
        if self.selection.as_ref() != Some(selection) {
            self.selection = Some(selection.clone());
            self.started = now;
        }
        let width = ui.available_width();
        let height = (width * 0.56)
            .clamp(200., 300.)
            .min(ui.available_height().max(160.));
        let (rect, _) = ui.allocate_exact_size(vec2(width, height), egui::Sense::hover());
        if !ui.is_rect_visible(rect) {
            return;
        }
        let painter = ui.painter().with_clip_rect(rect);
        painter.rect_filled(rect, 8., Color32::from_rgb(9, 15, 25));
        painter.text(
            rect.min + vec2(12., 10.),
            egui::Align2::LEFT_TOP,
            "LIVE PREVIEW",
            egui::FontId::proportional(10.),
            Color32::from_rgb(118, 156, 178),
        );
        let Some(lanes) = self.skins.get(&selection.skin).cloned() else {
            return;
        };
        let Some(&board) = self.boards.get(&selection.board) else {
            return;
        };
        let effect = self.effects.get(&selection.effect).cloned();
        let effect_duration = effect
            .as_ref()
            .map_or(0., |e| e.frames.len() as f64 / e.lanes.max(1) as f64 / 30.);
        let phase = (now - self.started + 1.08).rem_euclid((effect_duration + 1.4).max(2.5));
        let frame = (now * 12.) as usize;
        let content = Rect::from_min_max(rect.min + vec2(12., 30.), rect.max - vec2(12., 10.));
        if content.width() <= 0. || content.height() <= 0. {
            return;
        }

        let field = NoteField {
            textures: &self.textures,
            lanes: &lanes,
            board,
            effect: effect.as_ref(),
        };
        // Only demonstration state belongs here. Geometry, frame selection,
        // clipping and draw order all come from the gameplay note field.
        let travel = 72.;
        let notes = (0..4).filter_map(|lane| {
            let hold = lane == 1;
            if phase >= if hold { 1.8 } else { 1. } {
                return None;
            }
            Some(FieldNote {
                lane,
                head: (1. - phase as f32) * travel,
                tail: hold.then_some((1.8 - phase as f32) * travel),
                holding: hold && phase >= 1.,
                missed: false,
                beat_phase: lane,
            })
        });
        let feedback = std::array::from_fn(|lane| LaneFeedback {
            held: lane == 1 && (1.0..1.8).contains(&phase),
            pressed_age: (phase >= 1.).then_some(phase - 1.),
            hit_age: (phase >= 1.).then_some(phase - 1.),
        });
        // Fill the panel horizontally with a uniformly scaled gameplay board.
        // The viewport crops the scrolling field instead of shrinking every lane
        // to fit an entire incoming hold into a fixed-height thumbnail.
        let scale = content.width() / self.textures[board.0].width as f32;
        let origin = [content.left(), content.top() - NOTE_TOP * scale];
        let bottom = NOTE_TOP + content.height() / scale;
        let sprites = field.sprites(frame, &feedback, notes);
        let mut canvas = ui.new_child(egui::UiBuilder::new().max_rect(content));
        canvas.set_clip_rect(content.intersect(ui.clip_rect()));
        for sprite in sprites {
            if let Some(sprite) = sprite.clip_vertical(NOTE_TOP, bottom) {
                self.sprite(&canvas, &sprite.transformed(origin, scale));
            }
        }
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(33));
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    pub(crate) fn preview() -> AppearancePreview {
        let lanes = |id| {
            let f = LaneFrames {
                normal: vec![TextureId(id); 4],
                eighth: vec![],
                sixteenth: vec![],
                hold_head: vec![TextureId(id); 4],
                hold_body: vec![TextureId(id); 4],
                hold_tail: vec![TextureId(id); 4],
                receptor: vec![TextureId(id); 2],
                pressed: vec![TextureId(id); 2],
            };
            std::array::from_fn(|_| f.clone())
        };
        AppearancePreview {
            textures: Arc::new(
                (0..4)
                    .map(|id| Texture {
                        source: format!("{id}.PNG").into(),
                        width: 2,
                        height: 2,
                        rgba: [id as u8 * 60, 100, 200, 255].repeat(4),
                    })
                    .chain((4..6).map(|id| Texture {
                        source: format!("board-{id}.PNG").into(),
                        width: 315,
                        height: 600,
                        rgba: [20, 30, id as u8 * 30, 255].repeat(315 * 600),
                    }))
                    .collect(),
            ),
            skins: [("first".into(), lanes(0)), ("second".into(), lanes(1))].into(),
            effects: [(
                "glow".into(),
                HitAnimation {
                    id: "glow".into(),
                    label: "Glow".into(),
                    lanes: 1,
                    frames: (0..12)
                        .map(|i| sdo_assets::HitFrame {
                            texture: TextureId(2 + i % 2),
                            additive: true,
                        })
                        .collect(),
                },
            )]
            .into(),
            boards: [("".into(), TextureId(4)), ("second".into(), TextureId(5))].into(),
            uploaded: HashMap::new(),
            selection: None,
            started: 0.,
        }
    }
    fn frame(
        ctx: &egui::Context,
        preview: &mut AppearancePreview,
        selection: &UiSelection,
        time: f64,
    ) -> Vec<egui::TextureId> {
        frame_shapes(ctx, preview, selection, time, 350.)
            .into_iter()
            .map(|(id, _)| id)
            .collect()
    }
    fn frame_shapes(
        ctx: &egui::Context,
        preview: &mut AppearancePreview,
        selection: &UiSelection,
        time: f64,
        width: f32,
    ) -> Vec<(egui::TextureId, Rect)> {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0., 0.), vec2(width, 400.))),
                time: Some(time),
                ..Default::default()
            },
            |ui| {
                preview.show(ui, selection);
                let after = ui.label("After preview");
                assert!(
                    after.rect.top() >= 200.,
                    "Preview must reserve its layout space"
                );
            },
        );
        // This CPU-only test inspects shapes without a GPU texture uploader.
        output.textures_delta.clear();
        output
            .shapes
            .into_iter()
            .filter_map(|s| match s.shape {
                egui::Shape::Mesh(mesh) => {
                    let mut min = pos2(f32::INFINITY, f32::INFINITY);
                    let mut max = pos2(f32::NEG_INFINITY, f32::NEG_INFINITY);
                    for vertex in &mesh.vertices {
                        min = min.min(vertex.pos);
                        max = max.max(vertex.pos);
                    }
                    Some((mesh.texture_id, Rect::from_min_max(min, max)))
                }
                egui::Shape::LineSegment { .. } => {
                    panic!("Preview must use the board's lane dividers")
                }
                _ => None,
            })
            .collect()
    }
    #[test]
    fn preview_preserves_gameplay_lane_positions_and_aspect_ratio_when_resized() {
        let selected = UiSelection {
            skin: "first".into(),
            effect: "none".into(),
            ..Default::default()
        };
        for width in [180., 350., 600.] {
            let ctx = egui::Context::default();
            let mut p = preview();
            let shapes = frame_shapes(&ctx, &mut p, &selected, 1., width);
            let board_id = p.uploaded[&(TextureId(4), false)].id();
            let board = shapes.iter().find(|(id, _)| *id == board_id).unwrap().1;
            assert!((board.width() - (width - 24.)).abs() < 0.001);
            let scale = board.width() / 315.;
            let receptor_id = p.uploaded[&(TextureId(0), false)].id();
            let receptors = shapes
                .iter()
                .filter(|(id, _)| *id == receptor_id)
                .take(4)
                .map(|(_, rect)| *rect)
                .collect::<Vec<_>>();
            assert_eq!(receptors.len(), 4);
            let borders = [14., 83., 152., 221., 290.];
            for (lane, receptor) in receptors.iter().enumerate() {
                let center = receptor.center();
                let expected_x = board.left() + NoteField::lane_center(lane) * scale;
                assert!((center.x - expected_x).abs() < 0.001);
                assert!(
                    (center.y - (board.top() + (crate::notes::RECEPTOR_Y - NOTE_TOP) * scale))
                        .abs()
                        < 0.001
                );
                assert!(center.x > board.left() + borders[lane] * scale);
                assert!(center.x < board.left() + borders[lane + 1] * scale);
                assert!((receptor.width() - 2. * scale).abs() < 0.001);
                assert!((receptor.height() - 2. * scale).abs() < 0.001);
            }
        }
    }

    #[test]
    fn held_preview_finishes_pressed_animation_instead_of_restarting_it() {
        let ctx = egui::Context::default();
        let mut p = preview();
        for lane in p.skins.get_mut("first").unwrap() {
            lane.pressed = vec![TextureId(2), TextureId(3)];
        }
        let selected = UiSelection {
            skin: "first".into(),
            effect: "none".into(),
            ..Default::default()
        };
        frame(&ctx, &mut p, &selected, 1.);
        let shapes = frame(&ctx, &mut p, &selected, 1.14);
        assert!(shapes.contains(&p.uploaded[&(TextureId(3), false)].id()));
        assert!(!p.uploaded.contains_key(&(TextureId(2), false)));
    }

    #[test]
    fn switching_skin_effect_and_none_changes_the_rendered_frames_immediately() {
        let ctx = egui::Context::default();
        let mut p = preview();
        let mut selected = UiSelection {
            skin: "first".into(),
            effect: "glow".into(),
            ..Default::default()
        };
        let a = frame(&ctx, &mut p, &selected, 1.);
        let first = p.uploaded[&(TextureId(0), false)].id();
        let effect_a = p.uploaded[&(TextureId(2), true)].id();
        assert!(a.contains(&first) && a.contains(&effect_a));
        let b = frame(&ctx, &mut p, &selected, 1.1);
        let effect_b = p.uploaded[&(TextureId(3), true)].id();
        assert!(b.contains(&effect_b) && !b.contains(&effect_a));
        selected.skin = "second".into();
        selected.effect = "none".into();
        let c = frame(&ctx, &mut p, &selected, 1.2);
        let second = p.uploaded[&(TextureId(1), false)].id();
        assert!(c.contains(&second));
        assert!(!c.contains(&first) && !c.contains(&effect_a) && !c.contains(&effect_b));
        let previous_board = p.uploaded[&(TextureId(4), false)].id();
        assert!(c.contains(&previous_board));
        selected.board = "second".into();
        let changed_board = frame(&ctx, &mut p, &selected, 1.3);
        assert!(changed_board.contains(&p.uploaded[&(TextureId(5), false)].id()));
        assert!(!changed_board.contains(&previous_board));
    }
}
