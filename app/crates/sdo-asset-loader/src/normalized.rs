//! Storage-independent assembly. Identifiers are opaque outside the content boundary.
use anyhow::Result;
use sdo_assets::*;
use std::{path::PathBuf, sync::Arc};
pub(crate) fn scoped(prefix: &str, id: &str) -> String {
    if prefix.is_empty() {
        id.into()
    } else {
        format!("{prefix}::{id}")
    }
}
fn texture(t: &Texture) -> Texture {
    Texture {
        source: t.source.clone(),
        width: t.width,
        height: t.height,
        rgba: t.rgba.clone(),
    }
}
fn white() -> Texture {
    Texture {
        source: "<white>".into(),
        width: 1,
        height: 1,
        rgba: vec![255; 4],
    }
}
pub(crate) fn empty_ui() -> UiAssets {
    use font8x8::UnicodeFonts;
    let mut rgba = vec![0; 128 * 64 * 4];
    for code in 0..128u32 {
        if let Some(rows) = font8x8::BASIC_FONTS.get(char::from_u32(code).unwrap()) {
            for (y, row) in rows.into_iter().enumerate() {
                for x in 0..8 {
                    if row & (1 << x) != 0 {
                        let at =
                            ((code as usize / 16 * 8 + y) * 128 + code as usize % 16 * 8 + x) * 4;
                        rgba[at..at + 4].fill(255);
                    }
                }
            }
        }
    }
    UiAssets {
        root: PathBuf::new(),
        textures: Arc::new(vec![
            white(),
            Texture {
                source: "<utility-font>".into(),
                width: 128,
                height: 64,
                rgba,
            },
        ]),
        resources: vec![],
        skins: vec![],
        boards: vec![],
        scores: vec![],
        effects: vec![],
        health_bars: vec![],
        issues: vec![],
    }
}
pub(crate) fn shell_skin(ui: &UiAssets) -> FourKeySkin {
    let frames = || vec![TextureId(0)];
    FourKeySkin {
        definition: PathBuf::new(),
        resources: vec![],
        textures: ui.textures.clone(),
        board: TextureId(0),
        hit_effect: None,
        health: None,
        lanes: std::array::from_fn(|_| LaneFrames {
            normal: frames(),
            eighth: vec![],
            sixteenth: vec![],
            hold_head: frames(),
            hold_body: frames(),
            hold_tail: frames(),
            receptor: frames(),
            pressed: frames(),
        }),
        ui: GameplayUi {
            judgments: std::array::from_fn(|_| frames()),
            combo: frames(),
            combo_digits: vec![TextureId(0); 10],
            score_digits: vec![TextureId(0); 10],
            font: TextureId(1),
        },
    }
}
fn choice(prefix: &str, choice: &UiChoice) -> UiChoice {
    UiChoice {
        id: scoped(prefix, &choice.id),
        name: choice.name.clone(),
    }
}
pub(crate) fn merge_ui(parts: &[(&str, Arc<UiAssets>)]) -> UiAssets {
    let mut out = empty_ui();
    let mut textures = std::mem::take(Arc::get_mut(&mut out.textures).unwrap());
    for (prefix, ui) in parts {
        let offset = textures.len();
        textures.extend(ui.textures.iter().map(texture));
        let id = |t: TextureId| TextureId(t.0 + offset);
        let frames = |v: &Vec<TextureId>| v.iter().copied().map(id).collect();
        for skin in &ui.skins {
            let lanes = skin.lanes.each_ref().map(|l| LaneFrames {
                normal: frames(&l.normal),
                eighth: frames(&l.eighth),
                sixteenth: frames(&l.sixteenth),
                hold_head: frames(&l.hold_head),
                hold_body: frames(&l.hold_body),
                hold_tail: frames(&l.hold_tail),
                receptor: frames(&l.receptor),
                pressed: frames(&l.pressed),
            });
            out.skins.push(NoteSkin::new(
                choice(prefix, &skin.choice),
                scoped(prefix, skin.catalog_id()),
                skin.entry.clone(),
                lanes,
            ));
        }
        for board in &ui.boards {
            out.boards.push(NoteBoard {
                choice: choice(prefix, &board.choice),
                entry: board.entry.clone(),
                image: id(board.image),
            });
        }
        for score in &ui.scores {
            let s = &score.ui;
            out.scores.push(ScoreUi {
                choice: choice(prefix, &score.choice),
                entry: score.entry.clone(),
                ui: GameplayUi {
                    judgments: s.judgments.each_ref().map(frames),
                    combo: frames(&s.combo),
                    combo_digits: frames(&s.combo_digits),
                    score_digits: frames(&s.score_digits),
                    font: id(s.font),
                },
            });
        }
        for effect in &ui.effects {
            let mut animation = effect.animation.clone();
            animation.id = scoped(prefix, &animation.id);
            for frame in &mut animation.frames {
                frame.texture = id(frame.texture);
            }
            out.effects.push(NoteEffect {
                choice: choice(prefix, &effect.choice),
                entry: effect.entry.clone(),
                animation,
            });
        }
        for health in &ui.health_bars {
            let mut h = health.clone();
            h.choice = choice(prefix, &h.choice);
            h.frame = id(h.frame);
            h.fill = id(h.fill);
            h.full = id(h.full);
            out.health_bars.push(h);
        }
        out.resources.extend(ui.resources.clone());
        out.issues.extend(ui.issues.clone());
    }
    out.textures = Arc::new(textures);
    out
}
pub(crate) fn merge_characters(parts: &[(&str, Arc<CharacterAssets>)]) -> Result<CharacterAssets> {
    let mut presets = Vec::new();
    let mut textures = vec![white()];
    let mut items = Vec::new();
    let mut compatibility = Vec::new();
    let mut outfits = OutfitOverrides::new();
    let mut issues = Vec::new();
    for (prefix, assets) in parts {
        let offset = textures.len();
        textures.extend(assets.textures.iter().map(texture));
        for preset in &assets.presets {
            let mut p = preset.clone();
            p.choice = choice(prefix, &p.choice);
            p.parts = p
                .parts
                .iter()
                .map(|part| {
                    let mut p = (**part).clone();
                    for surface in &mut p.surfaces {
                        surface.texture.0 += offset;
                    }
                    Arc::new(p)
                })
                .collect();
            for animation in [&mut p.idle, &mut p.walk, &mut p.miss]
                .into_iter()
                .flatten()
            {
                animation.motion = scoped(prefix, &animation.motion);
            }
            for item in p.equipment.values_mut() {
                *item = scoped(prefix, item);
            }
            presets.push(p);
        }
        for item in &assets.wardrobe.items {
            let mut i = item.clone();
            i.id = scoped(prefix, &i.id);
            compatibility.push((
                i.id.clone(),
                assets
                    .presets
                    .iter()
                    .filter(|p| assets.wardrobe.is_compatible(&p.choice.id, p.body, item))
                    .map(|p| scoped(prefix, &p.choice.id))
                    .collect(),
            ));
            items.push(i);
        }
        for (preset, slots) in assets.outfit_overrides() {
            outfits.insert(
                scoped(prefix, &preset),
                slots
                    .into_iter()
                    .map(|(k, v)| (k, scoped(prefix, &v)))
                    .collect(),
            );
        }
        issues.extend(assets.issues.clone());
    }
    let mut wardrobe = WardrobeCatalog::from_items(items)?;
    for (item, presets) in compatibility {
        wardrobe.restrict_to_presets(&item, presets)?;
    }
    let mut out = CharacterAssets::new(presets, Arc::new(textures), Arc::new(wardrobe), &outfits)?;
    out.issues = issues;
    Ok(out)
}
