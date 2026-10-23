//! `JetBrains` New UI dark palette, expressed in sRGB.
use egui::{Color32, Context, Stroke, Visuals};

/// Window and editor background: #1E1F22.
pub const BACKGROUND: Color32 = Color32::from_rgb(30, 31, 34);
/// Tool windows and status bar: #2B2D30.
pub const PANEL: Color32 = Color32::from_rgb(43, 45, 48);
/// Dividers: #393B40.
pub const BORDER: Color32 = Color32::from_rgb(57, 59, 64);
/// Active selection: #2E436E.
pub const ACCENT: Color32 = Color32::from_rgb(46, 67, 110);
/// Hover and active tabs: #35373C.
pub const HOVER: Color32 = Color32::from_rgb(53, 55, 60);
/// Primary text: #BCBEC4.
pub const TEXT: Color32 = Color32::from_rgb(188, 190, 196);
/// Secondary text: #7A7E85.
pub const MUTED: Color32 = Color32::from_rgb(122, 126, 133);

/// Applies palette and compact desktop spacing.
pub fn apply(ctx: &Context) {
    let mut visuals = Visuals::dark();
    visuals.override_text_color = Some(TEXT);
    visuals.panel_fill = BACKGROUND;
    visuals.window_fill = PANEL;
    visuals.extreme_bg_color = BACKGROUND;
    visuals.faint_bg_color = PANEL;
    visuals.selection.bg_fill = ACCENT;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, BORDER);
    visuals.widgets.inactive.weak_bg_fill = BACKGROUND;
    visuals.widgets.hovered.weak_bg_fill = HOVER;
    visuals.widgets.active.weak_bg_fill = ACCENT;
    ctx.set_visuals(visuals);
    ctx.style_mut(|style| {
        style.spacing.button_padding = egui::vec2(9.0, 6.0);
        style.spacing.item_spacing = egui::vec2(6.0, 4.0);
    });
}
