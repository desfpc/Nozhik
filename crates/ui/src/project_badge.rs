//! Shared project identity for the launcher, current project and project menus.
use egui::{Align2, Color32, FontId, Painter, Rect};

/// Stable bright palette selected by the first three case-insensitive characters.
fn color(name: &str) -> Color32 {
    const COLORS: [Color32; 12] = [
        Color32::from_rgb(208, 148, 40),
        Color32::from_rgb(92, 119, 224),
        Color32::from_rgb(222, 126, 85),
        Color32::from_rgb(39, 165, 178),
        Color32::from_rgb(173, 99, 211),
        Color32::from_rgb(66, 170, 125),
        Color32::from_rgb(210, 96, 144),
        Color32::from_rgb(65, 146, 216),
        Color32::from_rgb(184, 154, 44),
        Color32::from_rgb(138, 112, 222),
        Color32::from_rgb(204, 114, 65),
        Color32::from_rgb(51, 166, 151),
    ];
    let hash = name
        .chars()
        .take(3)
        .flat_map(char::to_lowercase)
        .fold(0_usize, |hash, character| {
            hash.wrapping_mul(31).wrapping_add(character as usize)
        });
    COLORS[hash % COLORS.len()]
}

/// Paints a project initial on its stable colored background.
pub fn paint(painter: &Painter, rect: Rect, name: &str) {
    painter.rect_filled(rect, rect.height() * 0.2, color(name));
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        name.chars()
            .next()
            .unwrap_or('N')
            .to_uppercase()
            .to_string(),
        FontId::proportional(rect.height() * 0.65),
        Color32::WHITE,
    );
}
