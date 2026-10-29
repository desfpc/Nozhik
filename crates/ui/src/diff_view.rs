//! Virtualized unified diff with two gutters; only visible rows are laid out.
use crate::theme;
use egui::{Color32, FontId};
use nozhik_vcs::model::{DiffLineKind, FileDiff};

enum Row {
    Hunk(usize),
    Line(usize, usize),
}
#[derive(Default)]
pub struct DiffView {
    file: Option<FileDiff>,
    rows: Vec<Row>,
    selection: Option<((usize, usize), (usize, usize))>,
    dragging: bool,
    measured_width: Option<(f32, f32)>,
}
impl DiffView {
    pub(crate) fn clear(&mut self) {
        self.file = None;
        self.rows.clear();
        self.selection = None;
        self.dragging = false;
        self.measured_width = None;
    }
    pub(crate) fn set(&mut self, file: FileDiff) {
        self.rows.clear();
        self.selection = None;
        self.dragging = false;
        self.measured_width = None;
        for (h, hunk) in file.hunks.iter().enumerate() {
            self.rows.push(Row::Hunk(h));
            for l in 0..hunk.lines.len() {
                self.rows.push(Row::Line(h, l));
            }
        }
        self.file = Some(file);
    }
    fn text(&self, row: usize) -> &str {
        let Some(file) = &self.file else {
            return "";
        };
        match self.rows.get(row) {
            Some(Row::Hunk(h)) => file.hunks[*h].header.trim_end_matches(['\n', '\r']),
            Some(Row::Line(h, l)) => file.hunks[*h].lines[*l].text.trim_end_matches(['\n', '\r']),
            None => "",
        }
    }
    fn selected_text(&self) -> Option<String> {
        let (a, b) = self.selection?;
        let (start, end) = (a.min(b), a.max(b));
        if start == end {
            return None;
        }
        Some(
            (start.0..=end.0)
                .map(|row| {
                    let from = if row == start.0 { start.1 } else { 0 };
                    let to = if row == end.0 {
                        end.1
                    } else {
                        self.text(row).chars().count()
                    };
                    self.text(row)
                        .chars()
                        .skip(from)
                        .take(to.saturating_sub(from))
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }
    pub(crate) fn copy(&self, ctx: &egui::Context) -> bool {
        if !ctx.memory(|m| m.has_focus(egui::Id::new("git_diff_selection"))) {
            return false;
        }
        if let Some(text) = self.selected_text() {
            ctx.copy_text(text);
        }
        true
    }
    fn selection_input(&mut self, ui: &egui::Ui, origin: egui::Pos2, height: f32, font: &FontId) {
        let id = egui::Id::new("git_diff_selection");
        if !ui.is_enabled() {
            self.dragging = false;
            return;
        }
        let response = ui.interact(ui.clip_rect(), id, egui::Sense::click_and_drag());
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
        }
        // Replay events in order: a complete native drag can arrive in one frame.
        for event in ui.input(|i| i.events.clone()) {
            let pos = match event {
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers,
                } if ui.is_enabled()
                    && ui.clip_rect().contains(pos)
                    && ui.ctx().layer_id_at(pos) == Some(ui.layer_id()) =>
                {
                    self.dragging = true;
                    ui.memory_mut(|m| m.request_focus(id));
                    if !modifiers.shift {
                        self.selection = None;
                    }
                    Some(pos)
                }
                egui::Event::PointerMoved(pos) if self.dragging => Some(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    ..
                } if self.dragging => {
                    self.dragging = false;
                    Some(pos)
                }
                egui::Event::Key {
                    key: egui::Key::A,
                    pressed: true,
                    modifiers,
                    ..
                } if modifiers.command && ui.memory(|m| m.has_focus(id)) => {
                    let last = self.rows.len().saturating_sub(1);
                    self.selection = Some(((0, 0), (last, self.text(last).chars().count())));
                    None
                }
                egui::Event::Copy if ui.is_enabled() => {
                    self.copy(ui.ctx());
                    None
                }
                _ => None,
            };
            if let Some(pos) = pos {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                // Clamped content coordinates.
                let row = (((pos.y - origin.y) / height).max(0.0) as usize)
                    .min(self.rows.len().saturating_sub(1));
                let galley =
                    ui.painter()
                        .layout_no_wrap(self.text(row).into(), font.clone(), theme::TEXT);
                let column = galley
                    .cursor_from_pos(egui::vec2(pos.x - origin.x - 126.0, 0.0))
                    .index;
                let point = (row, column);
                let anchor = self.selection.map_or(point, |s| s.0);
                self.selection = Some((anchor, point));
            }
        }
        response.context_menu(|ui| {
            if ui
                .add_enabled(self.selected_text().is_some(), egui::Button::new("Copy"))
                .clicked()
            {
                if let Some(text) = self.selected_text() {
                    ui.ctx().copy_text(text);
                }
                ui.close();
            }
        });
    }
    pub(crate) fn header(&self, ui: &mut egui::Ui) {
        if let Some(file) = &self.file {
            ui.add(
                egui::Label::new(egui::RichText::new(file.path.to_string_lossy()).strong())
                    .truncate(),
            );
        } else {
            ui.weak("Select a changed file");
        }
    }
    pub(crate) fn show(&mut self, ui: &mut egui::Ui, font_size: f32) {
        let Some(file) = &self.file else {
            ui.weak("Select a changed file to view its diff");
            return;
        };
        if file.binary {
            ui.weak("Binary or oversized file — text diff is unavailable");
            return;
        }
        if file.truncated {
            ui.weak("Diff display limit reached; showing a partial patch");
        }
        if self.rows.is_empty() {
            ui.weak("No textual changes (contents unchanged or file metadata only)");
            return;
        }
        let font = FontId::monospace(font_size);
        let height = font_size + 6.0;
        if self
            .measured_width
            .is_none_or(|(size, _)| size.to_bits() != font_size.to_bits())
        {
            let width = (0..self.rows.len())
                .map(|row| {
                    ui.painter()
                        .layout_no_wrap(self.text(row).into(), font.clone(), theme::TEXT)
                        .size()
                        .x
                })
                .fold(0.0_f32, f32::max);
            self.measured_width = Some((font_size, width + 140.0));
        }
        let path = file.path.clone();
        let width = ui
            .available_width()
            .max(self.measured_width.map_or(0.0, |(_, w)| w));
        ui.spacing_mut().item_spacing.y = 0.0;
        egui::ScrollArea::both()
            .id_salt(("git_diff", &path))
            .scroll_source(
                egui::scroll_area::ScrollSource::SCROLL_BAR
                    | egui::scroll_area::ScrollSource::MOUSE_WHEEL,
            )
            .auto_shrink([false, false])
            .show_rows(ui, height, self.rows.len(), |ui, visible| {
                #[allow(clippy::cast_precision_loss)] // Bounded patch row count, in UI points.
                let origin = ui.cursor().min - egui::vec2(0.0, visible.start as f32 * height);
                self.selection_input(ui, origin, height, &font);
                for index in visible {
                    self.paint_row(ui, index, &font, egui::vec2(width, height));
                }
            });
    }
    fn paint_row(&self, ui: &mut egui::Ui, index: usize, font: &FontId, size: egui::Vec2) {
        let Some(file) = &self.file else {
            return;
        };
        let row = &self.rows[index];
        let (text, old, new, marker, background) = match *row {
            Row::Hunk(h) => (
                file.hunks[h].header.trim_end_matches(['\n', '\r']),
                None,
                None,
                ' ',
                Color32::from_rgb(29, 34, 38),
            ),
            Row::Line(h, l) => {
                let line = &file.hunks[h].lines[l];
                let (marker, bg) = match line.kind {
                    DiffLineKind::Context => (' ', Color32::from_rgb(35, 42, 48)),
                    DiffLineKind::Addition => ('+', crate::theme::DIFF_ADD),
                    DiffLineKind::Deletion => ('−', crate::theme::DIFF_DELETE),
                };
                (
                    line.text.trim_end_matches(['\n', '\r']),
                    line.old_line,
                    line.new_line,
                    marker,
                    bg,
                )
            }
        };
        let galley = ui
            .painter()
            .layout_no_wrap(text.into(), font.clone(), theme::TEXT);

        let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
        ui.painter().rect_filled(rect, 0.0, background);
        if matches!(row, Row::Hunk(_)) || background == Color32::from_rgb(35, 42, 48) {
            ui.painter().rect_filled(
                egui::Rect::from_min_max(
                    rect.left_top(),
                    egui::pos2(rect.left() + 100.0, rect.bottom()),
                ),
                0,
                Color32::from_rgb(29, 34, 38),
            );
        }
        for x in [50.0, 100.0] {
            ui.painter().line_segment(
                [
                    rect.left_top() + egui::vec2(x, 0.0),
                    rect.left_bottom() + egui::vec2(x, 0.0),
                ],
                egui::Stroke::new(1.0_f32, theme::BORDER),
            );
        }
        let y = rect.top() + 4.0;
        for (number, x) in [(old, 44.0), (new, 92.0)] {
            if let Some(number) = number {
                ui.painter().text(
                    egui::pos2(rect.left() + x, y),
                    egui::Align2::RIGHT_TOP,
                    number.to_string(),
                    font.clone(),
                    theme::MUTED,
                );
            }
        }
        ui.painter().text(
            egui::pos2(rect.left() + 108.0, y),
            egui::Align2::LEFT_TOP,
            marker,
            font.clone(),
            theme::TEXT,
        );
        self.paint_selection(ui, index, text, &galley, rect);
        ui.painter()
            .galley(egui::pos2(rect.left() + 126.0, y), galley, theme::TEXT);
    }
    fn paint_selection(
        &self,
        ui: &egui::Ui,
        index: usize,
        text: &str,
        galley: &egui::Galley,
        rect: egui::Rect,
    ) {
        if let Some((a, b)) = self.selection {
            let (start, end) = (a.min(b), a.max(b));
            if index >= start.0 && index <= end.0 && start != end {
                let from = if index == start.0 { start.1 } else { 0 };
                let to = if index == end.0 {
                    end.1
                } else {
                    text.chars().count()
                };
                let x1 = galley
                    .pos_from_cursor(egui::text::CCursor::new(from))
                    .left();
                let x2 = galley.pos_from_cursor(egui::text::CCursor::new(to)).left()
                    + if index < end.0 { 6.0 } else { 0.0 };
                ui.painter().rect_filled(
                    egui::Rect::from_min_max(
                        egui::pos2(rect.left() + 126.0 + x1, rect.top()),
                        egui::pos2(rect.left() + 126.0 + x2, rect.bottom()),
                    ),
                    0,
                    theme::ACCENT,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, Modifiers, PointerButton, Pos2};
    use nozhik_vcs::model::{DiffHunk, DiffLine};

    fn fixture() -> DiffView {
        let mut view = DiffView::default();
        view.set(FileDiff {
            path: "unicode.rs".into(),
            added: 2,
            deleted: 0,
            binary: false,
            truncated: false,
            hunks: vec![DiffHunk {
                header: "@@ patch @@".into(),
                old_start: 1,
                old_len: 1,
                new_start: 1,
                new_len: 100,
                lines: (0..100)
                    .map(|n| DiffLine {
                        kind: DiffLineKind::Addition,
                        old_line: None,
                        new_line: Some(n + 1),
                        text: if n == 0 {
                            "Привет 🦀\n".into()
                        } else {
                            format!("строка {n} {}\n", "x".repeat(150))
                        },
                    })
                    .collect(),
            }],
        });
        view
    }
    fn frame(
        view: &mut DiffView,
        ctx: &egui::Context,
        events: Vec<Event>,
    ) -> (egui::FullOutput, Pos2, egui::Vec2) {
        let mut origin = Pos2::ZERO;
        let mut offset = egui::Vec2::ZERO;
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    Pos2::ZERO,
                    egui::vec2(600.0, 300.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    origin = ui.cursor().min;
                    let id = ui.make_persistent_id(egui::Id::new((
                        "git_diff",
                        std::path::PathBuf::from("unicode.rs"),
                    )));
                    view.show(ui, 14.0);
                    offset = egui::scroll_area::State::load(ctx, id)
                        .map_or(egui::Vec2::ZERO, |s| s.offset);
                });
            },
        );
        (output, origin, offset)
    }
    fn button(pos: Pos2, pressed: bool) -> Event {
        Event::PointerButton {
            pos,
            pressed,
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
        }
    }
    #[test]
    fn drag_selects_unicode_across_rows_without_panning_and_copies() {
        let ctx = egui::Context::default();
        let mut view = fixture();
        let (_, origin, before) = frame(&mut view, &ctx, vec![]);
        let start = origin + egui::vec2(126.0, 25.0);
        let end = origin + egui::vec2(126.0, 45.0);
        frame(
            &mut view,
            &ctx,
            vec![Event::PointerMoved(start), button(start, true)],
        );
        let (_, _, during) = frame(&mut view, &ctx, vec![Event::PointerMoved(end)]);
        let (output, _, after) = frame(&mut view, &ctx, vec![button(end, false), Event::Copy]);
        assert_eq!(view.selected_text().as_deref(), Some("Привет 🦀\n"));
        assert_eq!(before, during);
        assert_eq!(before, after);
        assert!(
            output
                .platform_output
                .commands
                .iter()
                .any(|c| matches!(c, egui::OutputCommand::CopyText(t) if t == "Привет 🦀\n"))
        );
        frame(
            &mut view,
            &ctx,
            vec![Event::Key {
                key: egui::Key::A,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::COMMAND,
            }],
        );
        assert!(view.selected_text().is_some_and(
            |text| text.starts_with("@@ patch @@\nПривет 🦀\n") && text.contains("строка 99")
        ));
        // Reverse, complete drag in one native frame.
        frame(
            &mut view,
            &ctx,
            vec![
                Event::PointerMoved(end),
                button(end, true),
                Event::PointerMoved(start),
                button(start, false),
            ],
        );
        assert_eq!(view.selected_text().as_deref(), Some("Привет 🦀\n"));
        let (_, _, scrolled) = frame(
            &mut view,
            &ctx,
            vec![Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(-80.0, -120.0),
                modifiers: Modifiers::NONE,
            }],
        );
        assert!(
            scrolled.x > 0.0 && scrolled.y > 0.0,
            "both axes: {scrolled:?}"
        );
        if let Some(file) = fixture().file {
            view.set(file);
        }
        assert!(view.selected_text().is_none());
    }
}
