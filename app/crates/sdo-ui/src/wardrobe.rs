use super::{Action, SongSelector, asset_choice};
use egui::{Color32, RichText};
use sdo_assets::{
    CharacterAssets, CharacterBody, CharacterSlot, ClothingCategory, ClothingItem,
    ClothingPreviewKey, WardrobeCatalog,
};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WardrobeChange {
    Set {
        slot: CharacterSlot,
        item: Option<String>,
    },
    Reset,
}
#[derive(Clone)]
struct OutfitView {
    body: Option<CharacterBody>,
    equipment: BTreeMap<CharacterSlot, String>,
}
#[derive(Clone)]
struct ClothingDrag {
    item: String,
    name: String,
    slot: CharacterSlot,
}
#[derive(Debug, Clone)]
pub enum ClothingThumbnail {
    Ready(egui::TextureId),
    Failed(String),
}
#[derive(Default)]
pub struct WardrobeView {
    catalog: Arc<WardrobeCatalog>,
    presets: BTreeMap<String, OutfitView>,
    category: ClothingCategory,
    query: String,
    visible: Vec<ClothingPreviewKey>,
    thumbnails: BTreeMap<ClothingPreviewKey, ClothingThumbnail>,
}
impl WardrobeView {
    pub fn category(&self) -> ClothingCategory {
        self.category
    }
    pub fn query(&self) -> &str {
        &self.query
    }
    pub fn browse(&mut self, category: ClothingCategory, query: &str) {
        self.category = category;
        self.query = query.into();
    }
    pub fn visible_previews(&self) -> &[ClothingPreviewKey] {
        &self.visible
    }
    pub fn thumbnail(&self, key: &ClothingPreviewKey) -> Option<&ClothingThumbnail> {
        self.thumbnails.get(key)
    }
    pub fn set_thumbnail(&mut self, key: ClothingPreviewKey, thumbnail: ClothingThumbnail) {
        self.thumbnails.insert(key, thumbnail);
    }
    pub fn remove_thumbnail(&mut self, key: &ClothingPreviewKey) {
        self.thumbnails.remove(key);
    }
    pub fn sync(&mut self, assets: &CharacterAssets) {
        self.catalog = assets.wardrobe.clone();
        self.presets = assets
            .presets
            .iter()
            .map(|p| {
                (
                    p.choice.id.clone(),
                    OutfitView {
                        body: p.body,
                        equipment: p.equipment.clone(),
                    },
                )
            })
            .collect();
    }
}

impl SongSelector {
    pub(super) fn show_character(&mut self, ui: &mut egui::Ui) -> Action {
        let available = ui.available_size();
        let left_width = (available.x * 0.34).clamp(210., 360.);
        let mut action = Action::None;
        let catalog = self.wardrobe.catalog.clone();
        self.wardrobe.visible.clear();
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(left_width, available.y),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(left_width);
                    asset_choice(
                        ui,
                        "Preset",
                        &mut self.character,
                        &self.character_choices,
                        false,
                    );
                    ui.add_space(8.);
                    let height = (available.y * 0.48).clamp(160., 360.);
                    let scale = (left_width / 640.).min(height / 720.);
                    let size = egui::vec2(640., 720.) * scale;
                    ui.vertical_centered(|ui| {
                        if let Some(texture) = self.character_texture {
                            let response = ui
                                .add(egui::Image::new((texture, size)).sense(egui::Sense::drag()));
                            if response.dragged() {
                                let delta = response.drag_motion();
                                self.character_yaw =
                                    (self.character_yaw + delta.x * 0.01 + std::f32::consts::PI)
                                        .rem_euclid(std::f32::consts::TAU)
                                        - std::f32::consts::PI;
                                self.character_elevation =
                                    (self.character_elevation + delta.y * 0.005).clamp(-0.6, 0.6);
                            }
                            if response.hovered() {
                                let scroll = ui.input(|i| i.smooth_scroll_delta.y);
                                self.character_zoom = (self.character_zoom
                                    * (-scroll * 0.002).exp())
                                .clamp(0.55, 2.5);
                            }
                            if let Some(payload) = response.dnd_release_payload::<ClothingDrag>() {
                                action = equip(&payload);
                            }
                        } else {
                            ui.allocate_ui(size, |ui| {
                                ui.centered_and_justified(|ui| {
                                    ui.label("Character preview unavailable");
                                });
                            });
                        }
                    });
                    ui.add_space(8.);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Equipped").strong());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("Reset outfit").clicked() {
                                action = Action::Equip(WardrobeChange::Reset);
                            }
                        });
                    });
                    let outfit = self.wardrobe.presets.get(&self.character).cloned();
                    egui::ScrollArea::vertical()
                        .id_salt("equipment-slots")
                        .max_height(ui.available_height().max(100.))
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            for slot in CharacterSlot::BASE {
                                let current = outfit.as_ref().and_then(|p| p.equipment.get(&slot));
                                if let Some(change) = equipment_slot(
                                    ui,
                                    &catalog,
                                    Some(slot),
                                    current,
                                    outfit.as_ref().is_some_and(|p| {
                                        p.equipment.contains_key(&CharacterSlot::Outfit)
                                            && matches!(
                                                slot,
                                                CharacterSlot::Top | CharacterSlot::Pants
                                            )
                                    }),
                                    &mut self.error,
                                ) {
                                    action = Action::Equip(change);
                                }
                            }
                            if let Some(change) =
                                equipment_slot(ui, &catalog, None, None, false, &mut self.error)
                            {
                                action = Action::Equip(change);
                            }
                            if let Some(outfit) = &outfit {
                                for (slot, item) in &outfit.equipment {
                                    if slot.category() == ClothingCategory::Accessories
                                        && let Some(change) = equipment_slot(
                                            ui,
                                            &catalog,
                                            Some(*slot),
                                            Some(item),
                                            false,
                                            &mut self.error,
                                        )
                                    {
                                        action = Action::Equip(change);
                                    }
                                }
                            }
                            for issue in &self.character_issues {
                                ui.colored_label(Color32::LIGHT_RED, issue);
                            }
                        });
                },
            );
            ui.add_space(16.);
            ui.vertical(|ui| {
                ui.set_width(ui.available_width().max(200.));
                ui.horizontal(|ui| {
                    ui.heading("Wardrobe");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("Clear").clicked() {
                            self.wardrobe.query.clear();
                        }
                    });
                });
                ui.add_sized(
                    [ui.available_width(), 32.],
                    egui::TextEdit::singleline(&mut self.wardrobe.query)
                        .id(egui::Id::unique("clothing-search"))
                        .hint_text("Search item ID or filename…"),
                );
                ui.add_space(8.);
                ui.horizontal_wrapped(|ui| {
                    for category in ClothingCategory::ALL {
                        ui.selectable_value(
                            &mut self.wardrobe.category,
                            category,
                            category.label(),
                        );
                    }
                });
                let outfit = self.wardrobe.presets.get(&self.character);
                let body = outfit.and_then(|p| p.body);
                let query = self.wardrobe.query.to_lowercase();
                let words = query.split_whitespace().collect::<Vec<_>>();
                let items = catalog
                    .items
                    .iter()
                    .filter(|item| {
                        catalog.is_compatible(&self.character, body, item)
                            && item.slot.category() == self.wardrobe.category
                            && words.iter().all(|word| item.search.contains(word))
                    })
                    .collect::<Vec<_>>();
                ui.add_space(8.);
                ui.label(
                    RichText::new(format!("{} items", items.len()))
                        .small()
                        .weak(),
                );
                ui.add_space(4.);
                if items.is_empty() {
                    ui.label("No matching items for this character.");
                } else {
                    egui::ScrollArea::vertical()
                        .id_salt((
                            "clothing-items",
                            &self.character,
                            self.wardrobe.category.label(),
                            &query,
                        ))
                        .auto_shrink([false, false])
                        .max_height(ui.available_height())
                        .show_rows(ui, 88., items.len(), |ui, range| {
                            for index in range {
                                let item = items[index];
                                let equipped = outfit
                                    .is_some_and(|p| p.equipment.get(&item.slot) == Some(&item.id));
                                let key = ClothingPreviewKey {
                                    preset: self.character.clone(),
                                    item: item.id.clone(),
                                };
                                self.wardrobe.visible.push(key.clone());
                                let thumbnail = self.wardrobe.thumbnails.get(&key);
                                let response = clothing_row(ui, item, equipped, thumbnail);
                                let preview_failed_click = response.clicked()
                                    && response
                                        .interact_pointer_pos()
                                        .is_some_and(|p| thumbnail_rect(response.rect).contains(p))
                                    && matches!(thumbnail, Some(ClothingThumbnail::Failed(_)));
                                if preview_failed_click
                                    && let Some(ClothingThumbnail::Failed(error)) = thumbnail
                                {
                                    self.error =
                                        Some(format!("Cannot preview {}: {error}", item.name));
                                }
                                if item.unavailable_reason.is_none() {
                                    response.dnd_set_drag_payload(ClothingDrag {
                                        item: item.id.clone(),
                                        name: item.name.clone(),
                                        slot: item.slot,
                                    });
                                }
                                if response.clicked()
                                    && !preview_failed_click
                                    && !equipped
                                    && item.unavailable_reason.is_none()
                                {
                                    action = Action::Equip(WardrobeChange::Set {
                                        slot: item.slot,
                                        item: Some(item.id.clone()),
                                    });
                                }
                            }
                        });
                }
            });
        });
        if let Some(payload) = egui::DragAndDrop::payload::<ClothingDrag>(ui.ctx())
            && let Some(pos) = ui.input(|i| i.pointer.interact_pos())
        {
            egui::Area::new(egui::Id::unique("dragged-clothing"))
                .order(egui::Order::Tooltip)
                .fixed_pos(pos + egui::vec2(14., 14.))
                .interactable(false)
                .show(ui.ctx(), |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.label(&payload.name);
                    });
                });
        }
        action
    }
}

fn equip(payload: &ClothingDrag) -> Action {
    Action::Equip(WardrobeChange::Set {
        slot: payload.slot,
        item: Some(payload.item.clone()),
    })
}
fn equipment_slot(
    ui: &mut egui::Ui,
    catalog: &WardrobeCatalog,
    slot: Option<CharacterSlot>,
    current: Option<&String>,
    covered_by_outfit: bool,
    error: &mut Option<String>,
) -> Option<WardrobeChange> {
    let mut change = None;
    let frame = egui::Frame::new()
        .fill(crate::theme::SURFACE)
        .corner_radius(6.)
        .inner_margin(6.);
    let response = frame
        .show(ui, |ui| {
            ui.set_width((ui.available_width() - 1.).max(100.));
            ui.horizontal(|ui| {
                ui.label(RichText::new(slot.map_or("Accessories", CharacterSlot::label)).strong());
                let name = current
                    .map(|id| {
                        catalog
                            .item(id)
                            .map(|item| item.name.as_str())
                            .unwrap_or(id.as_str())
                    })
                    .unwrap_or(if covered_by_outfit {
                        "Part of outfit"
                    } else if slot.is_some() {
                        "None"
                    } else {
                        "Drop here"
                    });
                let can_reset = current.is_some() || covered_by_outfit;
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(slot) = slot
                        && can_reset
                        && ui.small_button("Reset").clicked()
                    {
                        change = Some(WardrobeChange::Set { slot, item: None });
                    }
                    ui.add(
                        egui::Label::new(RichText::new(name).size(12.).color(crate::theme::MUTED))
                            .truncate(),
                    );
                });
            });
        })
        .response;
    let accepts = |payload: &ClothingDrag| {
        slot.map_or(
            payload.slot.category() == ClothingCategory::Accessories,
            |slot| slot == payload.slot,
        )
    };
    if let Some(payload) = response.dnd_hover_payload::<ClothingDrag>() {
        let color = if accepts(&payload) {
            crate::theme::ACCENT
        } else {
            Color32::LIGHT_RED
        };
        ui.painter()
            .rect_stroke(response.rect, 6., (2., color), egui::StrokeKind::Inside);
    }
    if let Some(payload) = response.dnd_release_payload::<ClothingDrag>() {
        if accepts(&payload) {
            change = Some(WardrobeChange::Set {
                slot: payload.slot,
                item: Some(payload.item.clone()),
            });
        } else {
            *error = Some(format!(
                "{} belongs in the {} slot.",
                payload.name,
                payload.slot.label()
            ));
        }
    }
    ui.add_space(4.);
    change
}

fn thumbnail_rect(rect: egui::Rect) -> egui::Rect {
    egui::Rect::from_min_size(rect.min + egui::vec2(8., 8.), egui::vec2(72., 72.))
}
fn clothing_row(
    ui: &mut egui::Ui,
    item: &ClothingItem,
    equipped: bool,
    thumbnail: Option<&ClothingThumbnail>,
) -> egui::Response {
    let available = item.unavailable_reason.is_none();
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), 88.),
        if available {
            egui::Sense::click_and_drag()
        } else {
            egui::Sense::hover()
        },
    );
    let accent = crate::theme::ACCENT;
    let title_color = if available {
        Color32::WHITE
    } else {
        Color32::from_gray(155)
    };
    let fill = if equipped {
        crate::theme::SELECTED
    } else if response.hovered() {
        Color32::from_rgb(27, 45, 62)
    } else {
        crate::theme::SURFACE
    };
    ui.painter().rect_filled(rect, 6., fill);
    let preview_rect = thumbnail_rect(rect);
    match thumbnail {
        Some(ClothingThumbnail::Ready(texture)) => {
            ui.painter().image(
                *texture,
                preview_rect,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.)),
                Color32::WHITE,
            );
        }
        _ => {
            ui.painter()
                .rect_filled(preview_rect, 4., Color32::from_rgb(24, 33, 46));
            ui.painter().text(
                preview_rect.center(),
                egui::Align2::CENTER_CENTER,
                if thumbnail.is_some() {
                    "Preview\nunavailable"
                } else {
                    "Loading…"
                },
                egui::FontId::proportional(10.),
                Color32::from_gray(165),
            );
        }
    }
    let width = (rect.width() - 190.).max(30.);
    let mut job = egui::text::LayoutJob::simple_singleline(
        item.name.clone(),
        egui::FontId::proportional(15.),
        title_color,
    );
    job.wrap.max_width = width;
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    let title = ui.fonts_mut(|fonts| fonts.layout_job(job));
    ui.painter()
        .galley(rect.min + egui::vec2(94., 20.), title, title_color);
    let mut job = egui::text::LayoutJob::simple_singleline(
        if available {
            item.id.clone()
        } else {
            "Complete mesh missing".into()
        },
        egui::FontId::proportional(11.),
        Color32::from_gray(155),
    );
    job.wrap.max_width = width;
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    let subtitle = ui.fonts_mut(|fonts| fonts.layout_job(job));
    ui.painter().galley(
        rect.min + egui::vec2(94., 48.),
        subtitle,
        Color32::from_gray(155),
    );
    ui.painter().text(
        rect.right_center() - egui::vec2(12., 0.),
        egui::Align2::RIGHT_CENTER,
        if !available {
            "Unavailable"
        } else if equipped {
            "Equipped"
        } else {
            "Equip"
        },
        egui::FontId::proportional(13.),
        if available {
            accent
        } else {
            Color32::from_gray(155)
        },
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, available, &item.name));
    if let Some(ClothingThumbnail::Failed(reason)) = thumbnail {
        response.on_hover_text(format!("Preview unavailable: {reason}"))
    } else if let Some(reason) = &item.unavailable_reason {
        response.on_hover_text(reason)
    } else {
        response
            .on_hover_text(&item.id)
            .on_hover_cursor(egui::CursorIcon::Grab)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{frame, frame_output, selector, text_center};
    fn browser() -> SongSelector {
        let mut s = selector();
        s.character = "male".into();
        s.character_view = true;
        s.character_texture = Some(egui::TextureId::User(99));
        s.character_choices = vec![
            sdo_assets::UiChoice {
                id: "male".into(),
                name: "Default male".into(),
            },
            sdo_assets::UiChoice {
                id: "female".into(),
                name: "Default female".into(),
            },
        ];
        let mut catalog = WardrobeCatalog::default();
        catalog.items = [
            "100001_MAN_HAIR.MSH",
            "100002_MAN_COAT.MSH",
            "200001_WOMAN_HAIR.MSH",
        ]
        .into_iter()
        .map(|name| ClothingItem::from_filename(name).unwrap())
        .collect();
        s.wardrobe.catalog = Arc::new(catalog);
        for (id, body) in [
            ("male", CharacterBody::Male),
            ("female", CharacterBody::Female),
        ] {
            s.wardrobe.presets.insert(
                id.into(),
                OutfitView {
                    body: Some(body),
                    equipment: BTreeMap::new(),
                },
            );
        }
        s
    }
    fn pointer(pos: egui::Pos2, pressed: bool) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            },
        ]
    }
    fn click(ctx: &egui::Context, s: &mut SongSelector, pos: egui::Pos2) -> Action {
        frame(ctx, s, pointer(pos, true));
        frame(ctx, s, pointer(pos, false))
    }
    #[test]
    fn categories_search_and_preset_filter_drive_equipment_requests() {
        let mut s = browser();
        let ctx = egui::Context::default();
        for _ in 0..3 {
            frame(&ctx, &mut s, vec![]);
        }
        let output = frame_output(&ctx, &mut s, vec![]).1;
        let hair = text_center(&output, "Hair 100001").unwrap();
        assert!(text_center(&output, "Hair 200001").is_none());
        for removed in [
            "Character preset",
            "Rotation",
            "Elevation",
            "Zoom",
            "Drag to rotate · Scroll to zoom",
        ] {
            assert!(text_center(&output, removed).is_none());
        }
        assert_eq!(
            click(&ctx, &mut s, hair),
            Action::Equip(WardrobeChange::Set {
                slot: CharacterSlot::Hair,
                item: Some("100001_MAN_HAIR.MSH".into()),
            })
        );
        let tops = text_center(&output, "Tops").unwrap();
        click(&ctx, &mut s, tops);
        let output = frame_output(&ctx, &mut s, vec![]).1;
        assert!(text_center(&output, "Top 100002").is_some());
        assert!(text_center(&output, "Hair 100001").is_none());
        s.wardrobe.query = "missing-id".into();
        assert!(
            text_center(
                &frame_output(&ctx, &mut s, vec![]).1,
                "No matching items for this character."
            )
            .is_some()
        );
        s.wardrobe.query.clear();
        s.wardrobe.category = ClothingCategory::Hair;
        s.character = "female".into();
        let output = frame_output(&ctx, &mut s, vec![]).1;
        assert!(text_center(&output, "Hair 200001").is_some());
        assert!(text_center(&output, "Hair 100001").is_none());
        assert_eq!(s.selected().unwrap().id, "00001");
    }
    #[test]
    fn unavailable_items_explain_missing_mesh_and_cannot_be_equipped_or_dragged() {
        let mut s = browser();
        Arc::get_mut(&mut s.wardrobe.catalog).unwrap().items[0].unavailable_reason =
            Some("Complete mesh 100001_MAN_HAIR.MSH is missing".into());
        let ctx = egui::Context::default();
        for _ in 0..3 {
            frame(&ctx, &mut s, vec![]);
        }
        let output = frame_output(&ctx, &mut s, vec![]).1;
        let row = text_center(&output, "Hair 100001").unwrap();
        assert!(text_center(&output, "Unavailable").is_some());
        assert!(text_center(&output, "Complete mesh missing").is_some());
        assert_eq!(click(&ctx, &mut s, row), Action::None);
        frame(&ctx, &mut s, pointer(row, true));
        assert_eq!(
            frame(
                &ctx,
                &mut s,
                vec![egui::Event::PointerMoved(row + egui::vec2(40., 40.))]
            ),
            Action::None
        );
        assert!(egui::DragAndDrop::payload::<ClothingDrag>(&ctx).is_none());
        assert_eq!(
            frame(&ctx, &mut s, pointer(row + egui::vec2(40., 40.), false)),
            Action::None
        );
    }
    #[test]
    fn preview_requests_follow_visible_preset_and_preview_errors_do_not_block_equipping() {
        let mut s = browser();
        let ctx = egui::Context::default();
        for _ in 0..3 {
            frame(&ctx, &mut s, vec![]);
        }
        let key = s.wardrobe.visible_previews()[0].clone();
        assert_eq!(key.preset, "male");
        assert_eq!(key.item, "100001_MAN_HAIR.MSH");
        s.wardrobe.set_thumbnail(
            key,
            ClothingThumbnail::Failed("Missing original texture".into()),
        );
        let output = frame_output(&ctx, &mut s, vec![]).1;
        let preview = text_center(&output, "Preview\nunavailable").unwrap();
        assert_eq!(click(&ctx, &mut s, preview), Action::None);
        assert!(
            s.error
                .as_ref()
                .unwrap()
                .contains("Missing original texture")
        );
        s.error = None;
        for _ in 0..2 {
            frame(&ctx, &mut s, vec![]);
        }
        let output = frame_output(&ctx, &mut s, vec![]).1;
        let title = text_center(&output, "Hair 100001").unwrap();
        assert!(matches!(click(&ctx, &mut s, title), Action::Equip(_)));
        s.character = "female".into();
        frame(&ctx, &mut s, vec![]);
        assert!(
            s.wardrobe
                .visible_previews()
                .iter()
                .all(|key| key.preset == "female" && key.item == "200001_WOMAN_HAIR.MSH")
        );
    }
    #[test]
    fn dragging_to_a_slot_equips_and_wrong_slots_show_an_error() {
        let mut s = browser();
        let ctx = egui::Context::default();
        for _ in 0..3 {
            frame(&ctx, &mut s, vec![]);
        }
        for (target, valid) in [("Hair", true), ("Face", false)] {
            let output = frame_output(&ctx, &mut s, vec![]).1;
            let source = text_center(&output, "Hair 100001").unwrap();
            // The first Hair label is the equipment slot, before the category tab.
            let dest = text_center(&output, target).unwrap();
            frame(&ctx, &mut s, pointer(source, true));
            frame(
                &ctx,
                &mut s,
                vec![egui::Event::PointerMoved(source + egui::vec2(20., 5.))],
            );
            frame(&ctx, &mut s, vec![egui::Event::PointerMoved(dest)]);
            let action = frame(&ctx, &mut s, pointer(dest, false));
            if valid {
                assert_eq!(
                    action,
                    Action::Equip(WardrobeChange::Set {
                        slot: CharacterSlot::Hair,
                        item: Some("100001_MAN_HAIR.MSH".into()),
                    })
                );
            } else {
                assert_eq!(action, Action::None);
                assert!(s.error.as_ref().unwrap().contains("Hair slot"));
            }
        }
        for _ in 0..2 {
            frame(&ctx, &mut s, vec![]);
        }
        assert!(text_center(&frame_output(&ctx, &mut s, vec![]).1, "Back to character").is_some());
    }
}
