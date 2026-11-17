//! Shared navigation typography and spacing for project libraries.

use crate::{
    theme,
    tool_icons::{self, Icon},
};
use egui::{Align2, Color32, CornerRadius, FontId, Pos2, RichText, Sense, Ui, Vec2};

/// Draws a muted library heading with matching section margins.
pub fn section_label(ui: &mut Ui, text: &str) {
    ui.add_space(11.0);
    ui.label(RichText::new(text).size(12.0).color(theme::muted()));
    ui.add_space(5.0);
}

/// Draws a library filter or tag and returns whether it was clicked.
pub fn nav_row(
    ui: &mut Ui,
    icon: Icon,
    label: &str,
    count: Option<usize>,
    selected: bool,
    icon_color: Color32,
) -> bool {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 28.0), Sense::click());
    if selected || response.hovered() {
        ui.painter().rect_filled(
            rect.shrink2(Vec2::new(2.0, 1.0)),
            CornerRadius::same(6),
            if selected {
                theme::accent()
            } else {
                theme::hover()
            },
        );
    }
    let center = Pos2::new(rect.left() + 16.0, rect.center().y);
    tool_icons::paint(ui.painter(), center, icon, icon_color);
    ui.painter().text(
        Pos2::new(rect.left() + 32.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(13.5),
        if selected {
            theme::on_selected()
        } else {
            theme::text()
        },
    );
    if let Some(count) = count {
        ui.painter().text(
            Pos2::new(rect.right() - 8.0, rect.center().y),
            Align2::RIGHT_CENTER,
            count.to_string(),
            FontId::proportional(12.0),
            theme::muted(),
        );
    }
    response.clicked()
}
