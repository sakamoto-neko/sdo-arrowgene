//! Shared colors and spacing for the desktop menus.
use egui::{Color32, CornerRadius, Frame, RichText, Stroke, TextStyle};

pub const CANVAS: Color32 = Color32::from_rgb(15, 22, 33);
pub const SURFACE: Color32 = Color32::from_rgb(24, 34, 48);
pub const EDGE: Color32 = Color32::from_rgb(53, 69, 86);
pub const MUTED: Color32 = Color32::from_rgb(167, 184, 202);
pub const TEXT: Color32 = Color32::from_rgb(234, 241, 248);
pub const ACCENT: Color32 = Color32::from_rgb(123, 229, 210);
pub const SELECTED: Color32 = Color32::from_rgb(32, 71, 77);

pub fn panel(padding: i8) -> Frame {
    Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1., EDGE))
        .corner_radius(10)
        .inner_margin(padding)
}

pub fn primary(
    ui: &mut egui::Ui,
    label: impl Into<RichText>,
    size: [f32; 2],
    enabled: bool,
) -> egui::Response {
    let label = label.into().strong();
    let button = if enabled {
        egui::Button::new(label.color(Color32::from_rgb(12, 36, 39))).fill(ACCENT)
    } else {
        egui::Button::new(label)
    };
    ui.add_enabled_ui(enabled, |ui| ui.add_sized(size, button.corner_radius(7)))
        .inner
}

pub fn apply(ctx: &egui::Context) {
    let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
    let visuals = &mut style.visuals;
    visuals.panel_fill = CANVAS;
    visuals.window_fill = SURFACE;
    visuals.window_stroke = Stroke::new(1., EDGE);
    visuals.window_corner_radius = CornerRadius::same(10);
    visuals.extreme_bg_color = Color32::from_rgb(12, 19, 29);
    visuals.faint_bg_color = Color32::from_rgb(29, 41, 56);
    visuals.weak_text_color = Some(MUTED);
    visuals.selection.bg_fill = SELECTED;
    visuals.selection.stroke = Stroke::new(1., ACCENT);
    visuals.widgets.noninteractive.fg_stroke.color = TEXT;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1., EDGE);
    let widgets = &mut visuals.widgets;
    for (state, fill, border) in [
        (&mut widgets.inactive, Color32::from_rgb(36, 49, 66), EDGE),
        (
            &mut widgets.hovered,
            Color32::from_rgb(48, 66, 81),
            Color32::from_rgb(111, 145, 154),
        ),
        (&mut widgets.active, SELECTED, ACCENT),
        (&mut widgets.open, SELECTED, ACCENT),
    ] {
        state.weak_bg_fill = fill;
        state.bg_fill = fill;
        state.bg_stroke = Stroke::new(1., border);
        state.fg_stroke.color = TEXT;
        state.corner_radius = CornerRadius::same(6);
        state.expansion = 0.;
    }
    style.spacing.item_spacing = egui::vec2(8., 6.);
    style.spacing.button_padding = egui::vec2(9., 4.);
    style
        .text_styles
        .insert(TextStyle::Body, egui::FontId::proportional(14.));
    style
        .text_styles
        .insert(TextStyle::Button, egui::FontId::proportional(14.));
    style
        .text_styles
        .insert(TextStyle::Heading, egui::FontId::proportional(23.));
    style.animation_time = 0.12;
    ctx.set_theme(egui::Theme::Dark);
    ctx.set_style_of(egui::Theme::Dark, style);
}
