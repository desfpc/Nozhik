//! Shared single-line fields for every Nozhik form.
//!
//! A field is the same height as a button, and its text is vertically centered.
//! [`begin`] adds two logical pixels between the rows of that form only.
use egui::{Margin, TextBuffer, Ui};

/// Extra space between labels, fields and buttons, in logical pixels.
pub const GAP: f32 = 2.0;

/// Horizontal inset of field text, in logical pixels.
const PAD_X: i8 = 8;

/// Loosens the current form by [`GAP`] on both axes. Child rows inherit it.
pub fn begin(ui: &mut Ui) {
    let spacing = ui.spacing().item_spacing;
    ui.spacing_mut().item_spacing = egui::vec2(spacing.x + GAP, spacing.y + GAP);
}

/// Standard single-line field. Further builder calls (hint, password, width) still apply.
#[must_use = "add the field to the form or the input is not shown"]
pub fn singleline(text: &mut dyn TextBuffer) -> egui::TextEdit<'_> {
    egui::TextEdit::singleline(text)
        .margin(Margin::symmetric(PAD_X, crate::theme::CONTROL_PAD_Y))
        .vertical_align(egui::Align::Center)
}

/// Text width that makes a [`singleline`] frame `outer` logical pixels wide.
///
/// [`egui::TextEdit::desired_width`] is the text area. The frame adds [`PAD_X`] on both sides.
#[must_use]
pub fn frame_text_width(outer: f32) -> f32 {
    f32::mul_add(f32::from(PAD_X), -2.0, outer).max(24.0)
}

/// Search field with a square add button on the right. The button stays inside the row.
#[must_use = "handle the add button click"]
pub fn search_add_row(
    ui: &mut Ui,
    text: &mut dyn TextBuffer,
    id: egui::Id,
    enabled: bool,
    hover: &str,
) -> egui::Response {
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let height = ui.spacing().interact_size.y;
            let button = ui
                .add_enabled(
                    enabled,
                    egui::Button::new("+").min_size(egui::Vec2::splat(height)),
                )
                .on_hover_text(hover);
            ui.add(
                singleline(text)
                    .id(id)
                    .hint_text("Search...")
                    .desired_width(frame_text_width(ui.available_width())),
            );
            button
        })
        .inner
    })
    .inner
}

#[cfg(test)]
mod tests {
    use super::{begin, singleline};

    #[test]
    fn field_matches_button_height_and_combo_shares_its_center_line() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let mut text = "Search...".to_owned();
        let _output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 600.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let before = ui.spacing().item_spacing;
                    begin(ui);
                    let after = ui.spacing().item_spacing;
                    assert!((after.x - before.x - super::GAP).abs() < f32::EPSILON);
                    assert!((after.y - before.y - super::GAP).abs() < f32::EPSILON);
                    ui.horizontal(|ui| {
                        let field = singleline(&mut text).hint_text("Search...").show(ui);
                        let button = ui.button("Save");
                        let combo = egui::ComboBox::from_id_salt("page_rows")
                            .width(120.0)
                            .selected_text("500 rows")
                            .show_ui(ui, |_| {});
                        let field_h = field.response.rect.height();
                        let button_h = button.rect.height();
                        let text_center =
                            f32::mul_add(field.galley.rect.height(), 0.5, field.galley_pos.y);
                        assert!(
                            (field_h - button_h).abs() < 1.0,
                            "field height {field_h} button height {button_h}"
                        );
                        assert!(
                            (text_center - field.response.rect.center().y).abs() < 1.0,
                            "text center {text_center} field center {}",
                            field.response.rect.center().y
                        );
                        assert!(
                            (field.response.rect.center().y - button.rect.center().y).abs() < 1.0,
                            "field center {} button center {}",
                            field.response.rect.center().y,
                            button.rect.center().y
                        );
                        assert!(
                            (combo.response.rect.center().y - button.rect.center().y).abs() < 1.0,
                            "combo center {} height {} button center {} height {}",
                            combo.response.rect.center().y,
                            combo.response.rect.height(),
                            button.rect.center().y,
                            button_h
                        );
                    });
                });
            },
        );
    }

    #[test]
    fn search_add_button_stays_inside_the_row() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let mut text = String::new();
        let width = 280.0;
        let _output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 400.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let left = ui.cursor().left();
                    let row = ui.allocate_ui(egui::vec2(width, 48.0), |ui| {
                        super::search_add_row(ui, &mut text, ui.id().with("search"), true, "New")
                    });
                    let button = row.inner;
                    let limit = left + width;
                    assert!(
                        button.rect.right() <= limit + 1.0,
                        "button right {} exceeds row {limit}",
                        button.rect.right()
                    );
                    assert!(
                        (limit - button.rect.right()).abs() < 1.0,
                        "button right {} should sit on the row edge {limit}",
                        button.rect.right()
                    );
                });
            },
        );
    }
}
