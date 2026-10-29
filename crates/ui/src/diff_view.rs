//! Virtualized unified diff with two gutters; only visible rows are laid out.
use crate::theme;
use egui::{Color32, FontId};
use nozhik_vcs::model::{DiffLineKind, FileDiff};

mod syntax;

#[derive(Clone, Copy)]
enum Row {
    Gap { h: usize, start: usize, end: usize },
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
    shown: Vec<Vec<bool>>,
    syntax: syntax::Syntax,
}
impl DiffView {
    pub(crate) fn clear(&mut self) {
        self.file = None;
        self.rows.clear();
        self.selection = None;
        self.dragging = false;
        self.measured_width = None;
        self.shown.clear();
        self.syntax = syntax::Syntax::default();
    }
    pub(crate) fn set(&mut self, mut file: FileDiff) {
        let complete = file.full_hunks.is_some();
        if let Some(hunks) = file.full_hunks.take() {
            file.hunks = hunks;
        }
        if self.file.as_ref() == Some(&file) {
            return;
        }
        self.clear();
        self.shown = file
            .hunks
            .iter()
            .map(|hunk| {
                let mut shown = vec![!complete; hunk.lines.len()];
                for (i, line) in hunk.lines.iter().enumerate() {
                    if line.kind != DiffLineKind::Context {
                        let end = (i + 4).min(shown.len());
                        shown[i.saturating_sub(3)..end].fill(true);
                    }
                }
                shown
            })
            .collect();
        self.file = Some(file);
        self.rebuild_rows();
    }
    fn rebuild_rows(&mut self) {
        self.rows.clear();
        for (h, shown) in self.shown.iter().enumerate() {
            self.rows.push(Row::Hunk(h));
            let mut l = 0;
            while l < shown.len() {
                if shown[l] {
                    self.rows.push(Row::Line(h, l));
                    l += 1;
                } else {
                    let start = l;
                    while l < shown.len() && !shown[l] {
                        l += 1;
                    }
                    self.rows.push(Row::Gap { h, start, end: l });
                }
            }
        }
        self.selection = None;
        self.dragging = false;
        self.measured_width = None;
    }
    fn expand(&mut self, h: usize, start: usize, end: usize, up: bool) {
        let range = if up {
            end.saturating_sub(20).max(start)..end
        } else {
            start..(start + 20).min(end)
        };
        self.shown[h][range].fill(true);
        self.rebuild_rows();
    }
    pub(crate) fn configure(&mut self, config: &nozhik_config::Config) {
        self.syntax.configure(config, self.file.as_ref());
    }
    fn text(&self, row: usize) -> &str {
        let Some(file) = &self.file else {
            return "";
        };
        match self.rows.get(row) {
            Some(Row::Hunk(h)) => file.hunks[*h].header.trim_end_matches(['\n', '\r']),
            Some(Row::Line(h, l)) => file.hunks[*h].lines[*l].text.trim_end_matches(['\n', '\r']),
            Some(Row::Gap { .. }) | None => "",
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
                    && pos.x >= origin.x + 100.0
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
        self.syntax.poll(ui.ctx(), self.file.as_ref());
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
        let mut expansion = None;
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
                    if let Row::Gap { h, start, end } = self.rows[index] {
                        expansion = self
                            .paint_gap(ui, h, start, end, &font, egui::vec2(width, height))
                            .or(expansion);
                    } else {
                        self.paint_row(ui, index, &font, egui::vec2(width, height));
                    }
                }
            });
        if let Some((h, start, end, up)) = expansion {
            self.expand(h, start, end, up);
            ui.ctx().request_repaint();
        }
    }
    fn paint_gap(
        &self,
        ui: &mut egui::Ui,
        h: usize,
        start: usize,
        end: usize,
        font: &FontId,
        size: egui::Vec2,
    ) -> Option<(usize, usize, usize, bool)> {
        let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
        ui.painter()
            .rect_filled(rect, 0.0, Color32::from_rgb(29, 34, 38));
        let height = size.y;
        let mut expansion = None;
        let count = self.shown[h].len();
        for (up, x, visible) in [(true, 0.0, end < count), (false, 50.0, start > 0)] {
            if !visible {
                continue;
            }
            let hit =
                egui::Rect::from_min_size(rect.min + egui::vec2(x, 0.0), egui::vec2(50.0, height));
            let response = ui
                .interact(
                    hit,
                    ui.id().with(("expand", h, start, up)),
                    egui::Sense::click(),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text(if up {
                    "Expand Up (20 lines)"
                } else {
                    "Expand Down (20 lines)"
                });
            if response.hovered() {
                ui.painter().rect_filled(hit, 0.0, theme::ACCENT);
            }
            let center = hit.center();
            let direction = if up { -1.0 } else { 1.0 };
            ui.painter().arrow(
                center - egui::vec2(0.0, direction * 4.0),
                egui::vec2(0.0, direction * 8.0),
                egui::Stroke::new(1.5_f32, theme::TEXT),
            );
            if response.clicked() {
                expansion = Some((h, start, end, up));
            }
        }
        ui.painter().text(
            rect.min + egui::vec2(126.0, 4.0),
            egui::Align2::LEFT_TOP,
            format!("… {} lines hidden", end - start),
            font.clone(),
            theme::MUTED,
        );
        expansion
    }
    fn paint_row(&self, ui: &mut egui::Ui, index: usize, font: &FontId, size: egui::Vec2) {
        let Some(file) = &self.file else {
            return;
        };
        let row = &self.rows[index];
        let (text, old, new, marker, background) = match *row {
            Row::Gap { .. } => return,
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
        let spans = match *row {
            Row::Line(h, l) => self.syntax.spans(h, l),
            _ => &[],
        };
        let galley = ui
            .painter()
            .layout_job(syntax::job(text, font, spans, self.syntax.palette()));

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
            full_hunks: None,
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
    fn context_fixture() -> DiffView {
        let mut view = fixture();
        let Some(mut file) = view.file.take() else {
            return view;
        };
        let mut hunk = file.hunks[0].clone();
        hunk.lines = (0..140)
            .map(|n| DiffLine {
                kind: if n == 70 {
                    DiffLineKind::Addition
                } else {
                    DiffLineKind::Context
                },
                old_line: (n != 70).then_some(n + 1),
                new_line: Some(n + 1),
                text: format!("line {n}\n"),
            })
            .collect();
        file.full_hunks = Some(vec![hunk]);
        view.set(file);
        view
    }
    #[test]
    fn arrows_expand_repeatedly_and_stop_at_file_boundaries() {
        let ctx = egui::Context::default();
        let mut view = context_fixture();
        let (_, origin, _) = frame(&mut view, &ctx, vec![]);
        assert_eq!(view.shown[0].iter().filter(|v| **v).count(), 7);
        let up = origin + egui::vec2(25.0, 30.0);
        frame(&mut view, &ctx, vec![Event::PointerMoved(up)]);
        frame(&mut view, &ctx, vec![button(up, true)]);
        frame(&mut view, &ctx, vec![button(up, false)]);
        assert_eq!(view.shown[0].iter().filter(|v| **v).count(), 27);
        if let Some(same) = context_fixture().file {
            view.set(same);
        }
        assert_eq!(
            view.shown[0].iter().filter(|v| **v).count(),
            27,
            "unchanged status polling retains expanded context"
        );
        assert!(view.selection.is_none(), "arrow clicks do not select text");
        while let Some(Row::Gap { h, start, end }) = view
            .rows
            .iter()
            .find(|r| matches!(r, Row::Gap { start: 0, .. }))
            .copied()
        {
            view.expand(h, start, end, true);
        }
        assert!(view.shown[0][0]);
        while let Some(Row::Gap { h, start, end }) = view
            .rows
            .iter()
            .find(|r| matches!(r, Row::Gap { .. }))
            .copied()
        {
            view.expand(h, start, end, false);
        }
        assert!(view.shown[0].iter().all(|v| *v));
        assert_eq!(view.rows.len(), 141);
        assert!(view.text(1).contains("line 0"));
        assert!(view.text(140).contains("line 139"));
    }
    #[test]
    fn bottom_arrow_reveals_following_context_with_real_clicks() {
        let ctx = egui::Context::default();
        let mut view = context_fixture();
        let Some(mut file) = view.file.take() else {
            return;
        };
        file.hunks[0].lines[70].kind = DiffLineKind::Context;
        file.hunks[0].lines[3].kind = DiffLineKind::Addition;
        file.full_hunks = Some(file.hunks.clone());
        view.set(file);
        let (_, origin, _) = frame(&mut view, &ctx, vec![]);
        let down = origin + egui::vec2(75.0, 170.0);
        frame(&mut view, &ctx, vec![Event::PointerMoved(down)]);
        frame(&mut view, &ctx, vec![button(down, true)]);
        frame(&mut view, &ctx, vec![button(down, false)]);
        assert!(view.shown[0][26]);
        assert!(!view.shown[0][27]);
        assert!(view.selection.is_none());
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
        if let Some(mut file) = fixture().file {
            file.path = "different.rs".into();
            view.set(file);
        }
        assert!(view.selected_text().is_none());
    }
}
