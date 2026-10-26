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

/// Success and added-content accent.
pub const SUCCESS: Color32 = Color32::from_rgb(90, 200, 120);
/// Error and deleted-content accent.
pub const ERROR: Color32 = Color32::from_rgb(240, 110, 110);
/// Warnings and modified-content accent.
pub const WARNING: Color32 = Color32::from_rgb(230, 190, 80);
/// Navigation and renamed-content accent.
pub const INFO: Color32 = Color32::from_rgb(120, 170, 240);
/// Destructive button background.
pub const DESTRUCTIVE: Color32 = Color32::from_rgb(150, 45, 50);
/// Diff hunk background.
pub const DIFF_HUNK: Color32 = Color32::from_rgb(43, 54, 72);
/// Diff addition background.
pub const DIFF_ADD: Color32 = Color32::from_rgb(27, 59, 40);
/// Diff deletion background.
pub const DIFF_DELETE: Color32 = Color32::from_rgb(68, 34, 38);
/// Logical corner radius shared by controls and windows.
pub const RADIUS: u8 = 4;
/// Width of visible draggable panel separators in logical pixels.
pub const SPLITTER: f32 = 5.0;
/// Base terminal palette; extended ANSI colors are protocol-defined.
pub const ANSI: [(u8, u8, u8); 16] = [
    (30, 31, 34),
    (229, 92, 94),
    (106, 171, 115),
    (229, 192, 123),
    (86, 168, 245),
    (199, 125, 187),
    (86, 182, 194),
    (188, 190, 196),
    (122, 126, 133),
    (255, 113, 113),
    (138, 201, 144),
    (255, 220, 150),
    (127, 186, 255),
    (223, 157, 224),
    (132, 214, 225),
    (255, 255, 255),
];
/// Terminal caret overlay.
pub const CARET: Color32 = Color32::from_rgba_premultiplied(85, 85, 85, 85);

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
    visuals.window_corner_radius = egui::CornerRadius::same(RADIUS);
    visuals.widgets.inactive.corner_radius = egui::CornerRadius::same(RADIUS);
    visuals.widgets.hovered.corner_radius = egui::CornerRadius::same(RADIUS);
    visuals.widgets.active.corner_radius = egui::CornerRadius::same(RADIUS);
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, BORDER);
    ctx.set_visuals(visuals);
    ctx.style_mut(|style| {
        style.interaction.resize_grab_radius_side = SPLITTER;
        style.spacing.button_padding = egui::vec2(9.0, 6.0);
        style.spacing.item_spacing = egui::vec2(6.0, 4.0);
    });
}

/// Maps Tree-sitter captures onto the New UI / Darcula syntax palette.
#[must_use]
pub fn syntax_color(token: &str) -> Color32 {
    match token.split('.').next().unwrap_or(token) {
        "keyword" | "tag" => Color32::from_rgb(207, 142, 109),
        "string" => Color32::from_rgb(106, 171, 115),
        "comment" => Color32::from_rgb(122, 126, 133),
        "function" | "constructor" => Color32::from_rgb(86, 168, 245),
        "type" | "module" => Color32::from_rgb(181, 182, 227),
        "number" | "constant" => Color32::from_rgb(42, 172, 184),
        "variable" | "property" => Color32::from_rgb(199, 125, 187),
        _ => TEXT,
    }
}
