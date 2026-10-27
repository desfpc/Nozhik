//! Virtualized Rope editor. Only visible lines are converted to strings/galleys.
use crate::theme;
use egui::{Event, FontId, Key, Sense, Stroke, Ui, text::CCursor};
use nozhik_buffer::{BufferError, Cursor, EditTransaction, TextBuffer, TextEdit};

/// Virtualized view; text, caret, selections and history live in the buffer.
#[derive(Default)]
pub struct TextEditorView {
    /// Pending navigation target; scrolled into view on the next frame.
    pub reveal_line: Option<usize>,
    /// Active declarative profile; absent means verbatim paste.
    pub code_style: Option<nozhik_config::code_style::CodeStyle>,
    /// Render editor text using the proportional built-in family.
    pub proportional_font: bool,
    mouse_anchor: Option<(nozhik_core::BufferId, u64, usize)>,
}

impl TextEditorView {
    /// Paints visible rows and routes focused input through buffer transactions.
    /// # Errors
    /// Returns invalid coordinate or transaction errors from the text engine.
    #[allow(
        clippy::cast_precision_loss,
        clippy::too_many_lines,
        reason = "Viewport coordinates are logical pixels; row painting and hit testing share one virtualized pass"
    )]
    pub fn show(
        &mut self,
        ui: &mut Ui,
        buffer: &mut TextBuffer,
        font_size: f32,
        tab_width: u8,
        enabled: bool,
    ) -> Result<(), BufferError> {
        self.show_language(ui, buffer, font_size, tab_width, enabled, None)
    }

    #[allow(
        clippy::too_many_arguments,
        clippy::too_many_lines,
        clippy::cast_precision_loss,
        reason = "Virtualized painting shares input and hit testing geometry"
    )]
    pub(crate) fn show_language(
        &mut self,
        ui: &mut Ui,
        buffer: &mut TextBuffer,
        font_size: f32,
        tab_width: u8,
        enabled: bool,
        mut language: Option<&mut crate::language::EditorLanguage>,
    ) -> Result<(), BufferError> {
        let id = ui.id().with(("editor", buffer.snapshot().buffer_id));
        let response = ui.interact(ui.available_rect_before_wrap(), id, Sense::click_and_drag());
        if response.clicked() && enabled {
            response.request_focus();
        }
        if enabled && response.has_focus() {
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(
                    id,
                    egui::EventFilter {
                        tab: true,
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        escape: true,
                    },
                );
            });
            for event in ui.input(|input| input.events.clone()) {
                if !matches!(
                    &event,
                    Event::Text(_)
                        | Event::Paste(_)
                        | Event::Copy
                        | Event::Cut
                        | Event::Key { pressed: true, .. }
                ) {
                    continue;
                }
                if let Some(state) = language.as_deref_mut()
                    && matches!(&event, Event::Key { key: Key::Space, pressed: true, modifiers, .. } if modifiers.ctrl)
                {
                    Self::refresh_completion(buffer, state, true);
                    continue;
                }
                if let Some(state) = language.as_deref_mut()
                    && let Event::Key {
                        key, pressed: true, ..
                    } = &event
                {
                    if *key == Key::F12 {
                        state.request_at = None;
                        state.request = Some((
                            nozhik_language_support::RequestKind::Definition,
                            buffer.cursors().as_slice()[0].head.byte_offset,
                        ));
                        continue;
                    }
                    if !state.completions.is_empty() {
                        match key {
                            Key::ArrowDown => {
                                state.selected = (state.selected + 1) % state.completions.len();
                                state.completion_scroll = true;
                                continue;
                            }
                            Key::ArrowUp => {
                                state.selected = state
                                    .selected
                                    .checked_sub(1)
                                    .unwrap_or(state.completions.len() - 1);
                                state.completion_scroll = true;
                                continue;
                            }
                            Key::Escape => {
                                state.completions.clear();
                                state.token = 0;
                                state.request = None;
                                continue;
                            }
                            Key::Enter | Key::Tab => {
                                Self::accept_completion(buffer, state)?;
                                continue;
                            }
                            _ => {}
                        }
                    }
                }
                if matches!(event, Event::Copy | Event::Cut) {
                    let text = buffer
                        .cursors()
                        .as_slice()
                        .iter()
                        .map(|c| {
                            buffer
                                .rope()
                                .byte_slice(
                                    c.anchor.byte_offset.min(c.head.byte_offset)
                                        ..c.anchor.byte_offset.max(c.head.byte_offset),
                                )
                                .to_string()
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    ui.ctx().copy_text(text);
                    if matches!(event, Event::Cut) {
                        Self::replace(buffer, "", None)?;
                    }
                    continue;
                }
                let trigger = matches!(&event, Event::Text(s) if s.chars().last().is_some_and(|c| c.is_alphanumeric() || matches!(c,'_'|'$'|'>'|':'|'\\')))
                    || matches!(
                        &event,
                        Event::Key {
                            key: Key::Backspace,
                            pressed: true,
                            ..
                        }
                    );
                if let Event::Paste(text) = event {
                    Self::paste(buffer, &text, self.code_style.as_ref())?;
                } else if matches!(
                    event,
                    Event::Key {
                        key: Key::Enter,
                        pressed: true,
                        ..
                    }
                ) {
                    Self::newline(buffer, self.code_style.as_ref())?;
                } else {
                    Self::input(buffer, event, tab_width)?;
                }
                if let Some(state) = language.as_deref_mut() {
                    state.completions.clear();
                    state.request = None;
                    state.request_at = Some(std::time::Instant::now());
                    if trigger {
                        Self::refresh_completion(buffer, state, false);
                    }
                }
            }
        }
        ui.spacing_mut().item_spacing.y = 0.0;
        let mut hit_rows = Vec::new();
        let cursors = buffer.cursors().as_slice().to_vec();
        let revision = buffer.snapshot().revision;
        if let Some(state) = language.as_deref_mut()
            && state.revision != revision
        {
            state.revision = revision;
            state.spans.clear();
            state.diagnostics.clear();
            state.syntax.clear();
            state.hover = None;
        }
        let reveal = self.reveal_line.take();
        let mut popup_origin = None;
        let mut hovered_byte = None;
        let mut scroll = egui::ScrollArea::both().scroll_source(egui::scroll_area::ScrollSource {
            drag: false,
            ..egui::scroll_area::ScrollSource::default()
        });
        if let Some(line) = reveal {
            scroll = scroll.vertical_scroll_offset(line as f32 * (font_size + 7.0));
        }
        let viewport = scroll.id_salt(id).auto_shrink([false, false]).show_rows(
            ui,
            font_size + 7.0,
            buffer.rope().len_lines(),
            |ui, rows| {
                for line in rows {
                    let source = buffer.rope().line(line).to_string();
                    let text = source.trim_end_matches('\n').trim_end_matches('\r');
                    let start = buffer.rope().line_to_byte(line);
                    let mut job = egui::text::LayoutJob::default();
                    let mut boundaries = vec![0, text.len()];
                    let visible_spans = language.as_ref().map_or(&[][..], |state| {
                        let first = state.spans.partition_point(|s| s.range.end <= start);
                        let tail = &state.spans[first..];
                        &tail[..tail.partition_point(|s| s.range.start < start + text.len())]
                    });
                    for span in visible_spans {
                        boundaries.push(span.range.start.saturating_sub(start).min(text.len()));
                        boundaries.push(span.range.end.saturating_sub(start).min(text.len()));
                    }
                    boundaries.sort_unstable();
                    boundaries.dedup();
                    for range in boundaries.windows(2) {
                        let span = visible_spans
                            .iter()
                            .find(|s| s.range.contains(&(start + range[0])));
                        let color = if language.as_ref().is_some_and(|s| s.syntax_theme == "plain")
                        {
                            theme::TEXT
                        } else {
                            let color = span.map_or(theme::TEXT, |s| theme::syntax_color(&s.token));
                            if language.as_ref().is_some_and(|s| s.syntax_theme == "muted") {
                                color.gamma_multiply(0.75)
                            } else {
                                color
                            }
                        };
                        job.append(
                            &text[range[0]..range[1]],
                            0.0,
                            egui::TextFormat {
                                font_id: if self.proportional_font {
                                    FontId::proportional(font_size)
                                } else {
                                    FontId::monospace(font_size)
                                },
                                color,
                                italics: span.is_some_and(|s| s.token == "comment"),
                                ..Default::default()
                            },
                        );
                    }
                    let galley = ui.fonts(|fonts| fonts.layout_job(job));
                    let width = ui.available_width().max(galley.size().x + 90.0);
                    let (rect, row) =
                        ui.allocate_exact_size(egui::vec2(width, font_size + 7.0), Sense::hover());
                    if reveal == Some(line) {
                        ui.scroll_to_rect(rect, Some(egui::Align::Center));
                    }
                    let origin = rect.min + egui::vec2(66.0, 3.0);
                    if row.hovered()
                        && let Some(pos) = ui.input(|i| i.pointer.hover_pos())
                    {
                        let index = galley.cursor_from_pos(pos - origin).index;
                        hovered_byte = Some(
                            start
                                + text
                                    .char_indices()
                                    .nth(index)
                                    .map_or(text.len(), |(i, _)| i),
                        );
                    }
                    if let Some(state) = &language {
                        for diagnostic in state.diagnostics.iter().chain(&state.syntax) {
                            if line < diagnostic.range.start.line as usize
                                || line > diagnostic.range.end.line as usize
                            {
                                continue;
                            }
                            let color = if diagnostic.severity
                                == Some(
                                    nozhik_language_support::lsp_types::DiagnosticSeverity::WARNING,
                                ) {
                                crate::theme::WARNING
                            } else {
                                crate::theme::ERROR
                            };
                            ui.painter().circle_filled(
                                rect.min + egui::vec2(12.0, 10.0),
                                3.0,
                                color,
                            );
                            let col = |p: nozhik_language_support::lsp_types::Position| -> usize {
                                if (p.line as usize) < line {
                                    return 0;
                                }
                                if p.line as usize > line {
                                    return text.chars().count();
                                }
                                let mut units = 0;
                                let mut chars = 0;
                                for c in text.chars() {
                                    if units >= p.character as usize {
                                        break;
                                    }
                                    units += c.len_utf16();
                                    chars += 1;
                                }
                                chars
                            };
                            let x1 = origin.x
                                + galley
                                    .pos_from_cursor(CCursor::new(col(diagnostic.range.start)))
                                    .min
                                    .x;
                            let x2 = (origin.x
                                + galley
                                    .pos_from_cursor(CCursor::new(col(diagnostic.range.end)))
                                    .min
                                    .x)
                                .max(x1 + 5.0);
                            let x1 = x1.max(ui.clip_rect().left());
                            let x2 = x2.min(ui.clip_rect().right());
                            if x1 >= x2 {
                                continue;
                            }
                            let y = rect.bottom() - 2.0;
                            let mut points = Vec::new();
                            let mut x = x1;
                            let mut down = false;
                            #[allow(
                                clippy::while_float,
                                reason = "Finite pixel coordinates advance by two pixels"
                            )]
                            while x <= x2 {
                                points.push(egui::pos2(x, y + if down { 2.0 } else { 0.0 }));
                                x += 2.0;
                                down = !down;
                            }
                            ui.painter()
                                .add(egui::Shape::line(points, Stroke::new(1.0_f32, color)));
                            let hit = egui::Rect::from_min_max(
                                egui::pos2(x1, rect.top()),
                                egui::pos2(x2, rect.bottom()),
                            );
                            ui.interact(
                                hit,
                                id.with(("diagnostic", line, diagnostic.message.as_str())),
                                Sense::hover(),
                            )
                            .on_hover_text(&diagnostic.message);
                        }
                    }
                    ui.painter().text(
                        rect.min + egui::vec2(48.0, 3.0),
                        egui::Align2::RIGHT_TOP,
                        (line + 1).to_string(),
                        FontId::monospace(font_size),
                        theme::MUTED,
                    );
                    let start = buffer.rope().line_to_byte(line);
                    for cursor in &cursors {
                        let low = cursor.anchor.byte_offset.min(cursor.head.byte_offset);
                        let high = cursor.anchor.byte_offset.max(cursor.head.byte_offset);
                        if low < start + source.len() && high > start {
                            let x = |byte: usize| {
                                let count = text[..byte.saturating_sub(start).min(text.len())]
                                    .chars()
                                    .count();
                                galley.pos_from_cursor(CCursor::new(count)).min.x
                            };
                            let selection = egui::Rect::from_min_max(
                                egui::pos2(origin.x + x(low), rect.top()),
                                egui::pos2(
                                    origin.x
                                        + x(high)
                                        + if high > start + text.len() { 7.0 } else { 0.0 },
                                    rect.bottom(),
                                ),
                            );
                            ui.painter().rect_filled(selection, 0.0, theme::ACCENT);
                        }
                        if cursor.head.line as usize == line {
                            let local = cursor
                                .head
                                .byte_offset
                                .saturating_sub(start)
                                .min(text.len());
                            let caret =
                                galley.pos_from_cursor(CCursor::new(text[..local].chars().count()));
                            popup_origin = Some(origin + egui::vec2(caret.min.x, font_size + 6.0));
                            if enabled && response.has_focus() {
                                let x = origin.x + caret.min.x;
                                ui.painter().line_segment(
                                    [
                                        egui::pos2(x, rect.top() + 2.0),
                                        egui::pos2(x, rect.bottom() - 2.0),
                                    ],
                                    Stroke::new(1.5_f32, theme::TEXT),
                                );
                            }
                        }
                    }
                    hit_rows.push(HitRow {
                        start,
                        text: text.into(),
                        rect,
                        origin,
                        galley: galley.clone(),
                    });
                    ui.painter().galley(origin, galley, theme::TEXT);
                }
            },
        );
        if let Some(state) = language.as_deref_mut() {
            if state.hover_byte != hovered_byte {
                state.hover_byte = hovered_byte;
                state.hover = None;
                state.hover_token = 0;
                if let Some(byte) = hovered_byte
                    && state.request.is_none()
                {
                    state.request_at = Some(std::time::Instant::now());
                    state.request = Some((nozhik_language_support::RequestKind::Hover, byte));
                }
            }
            if let Some(hover) = &state.hover
                && hovered_byte.is_some()
            {
                egui::Tooltip::for_widget(&response)
                    .at_pointer()
                    .show(|ui| {
                        ui.set_max_width(550.0);
                        markdown(ui, hover);
                    });
            }
            if enabled
                && !state.completions.is_empty()
                && let Some(origin) = popup_origin
            {
                if Self::completion_popup(ui, id, origin, state) {
                    Self::accept_completion(buffer, state)?;
                    response.request_focus();
                } else if ui.input(|i| i.pointer.any_pressed()) && !state.completions.is_empty() {
                    // A popup click selects without applying and keeps keyboard input in the editor.
                    response.request_focus();
                }
            }
        }
        let changed = self.pointer_input(
            ui,
            buffer,
            &response,
            viewport.inner_rect,
            &hit_rows,
            enabled,
        )?;
        if changed {
            if let Some(state) = language {
                state.completions.clear();
                state.token = 0;
                state.request = None;
            }
        } else if enabled
            && response.clicked_by(egui::PointerButton::Primary)
            && ui.input(|i| i.modifiers.command)
            && let Some(pos) = response.interact_pointer_pos()
            && let Some(byte) = hit_byte(&hit_rows, pos)
            && let Some(state) = language
        {
            state.request_at = None;
            state.request = Some((nozhik_language_support::RequestKind::Definition, byte));
        }
        Ok(())
    }

    fn pointer_input(
        &mut self,
        ui: &Ui,
        buffer: &mut TextBuffer,
        response: &egui::Response,
        viewport: egui::Rect,
        rows: &[HitRow],
        enabled: bool,
    ) -> Result<bool, BufferError> {
        let snapshot = buffer.snapshot();
        if !enabled
            || self.mouse_anchor.is_some_and(|(id, revision, _)| {
                id != snapshot.buffer_id || revision != snapshot.revision
            })
        {
            self.mouse_anchor = None;
        }
        if !enabled {
            return Ok(false);
        }
        let pointer = ui.input(|i| i.pointer.clone());
        let mut changed = false;
        if pointer.button_pressed(egui::PointerButton::Primary)
            && response.is_pointer_button_down_on()
            && !ui.input(|i| i.modifiers.command)
            && let Some(pos) = pointer.press_origin().filter(|p| viewport.contains(*p))
            && let Some(byte) = hit_byte(rows, pos)
        {
            let anchor = if ui.input(|i| i.modifiers.shift) {
                buffer.cursors().as_slice()[0].anchor.byte_offset
            } else {
                byte
            };
            self.mouse_anchor = Some((snapshot.buffer_id, snapshot.revision, anchor));
            response.request_focus();
        }
        if let Some((_, _, anchor)) = self.mouse_anchor
            && let Some(pos) = pointer.interact_pos()
            && let Some(byte) = hit_byte(rows, pos)
        {
            let mut range = anchor..byte;
            if response.triple_clicked_by(egui::PointerButton::Primary) {
                let line = buffer.rope().byte_to_line(byte);
                range = buffer.rope().line_to_byte(line)
                    ..(buffer.rope().line_to_byte(line) + buffer.rope().line(line).len_bytes());
            } else if response.double_clicked_by(egui::PointerButton::Primary) {
                range = word_range(buffer, byte);
            }
            let cursor = Cursor {
                anchor: buffer.position_at(range.start)?,
                head: buffer.position_at(range.end)?,
            };
            if buffer.cursors().as_slice() != [cursor] {
                buffer.set_cursors(vec![cursor])?;
                ui.ctx().request_repaint();
                changed = true;
            }
        }
        if pointer.button_released(egui::PointerButton::Primary) || !pointer.primary_down() {
            self.mouse_anchor = None;
        }
        Ok(changed)
    }

    fn newline(
        buffer: &mut TextBuffer,
        style: Option<&nozhik_config::code_style::CodeStyle>,
    ) -> Result<(), BufferError> {
        let edits = buffer
            .cursors()
            .as_slice()
            .iter()
            .map(|cursor| {
                let range = cursor.anchor.byte_offset.min(cursor.head.byte_offset)
                    ..cursor.anchor.byte_offset.max(cursor.head.byte_offset);
                let line = buffer.rope().byte_to_line(range.start);
                let start = buffer.rope().line_to_byte(line);
                let before = buffer.rope().byte_slice(start..range.start).to_string();
                let ending = if buffer.rope().line(line).to_string().ends_with("\r\n") {
                    "\r\n"
                } else {
                    "\n"
                };
                let text =
                    nozhik_language_support::formatting::newline_text(&before, ending, style);
                TextEdit { range, text }
            })
            .collect();
        buffer.apply_transaction(EditTransaction { edits, group: None })
    }

    pub(crate) fn paste(
        buffer: &mut TextBuffer,
        text: &str,
        style: Option<&nozhik_config::code_style::CodeStyle>,
    ) -> Result<(), BufferError> {
        let source = buffer.rope().to_string();
        let edits = buffer
            .cursors()
            .as_slice()
            .iter()
            .map(|c| {
                let range = c.anchor.byte_offset.min(c.head.byte_offset)
                    ..c.anchor.byte_offset.max(c.head.byte_offset);
                let (range, text) =
                    nozhik_language_support::formatting::paste_edit(&source, range, text, style);
                TextEdit { range, text }
            })
            .collect();
        buffer.apply_transaction(EditTransaction { edits, group: None })
    }

    pub(crate) fn apply_format(
        buffer: &mut TextBuffer,
        edits: Vec<(std::ops::Range<usize>, String)>,
    ) -> Result<(), BufferError> {
        buffer.apply_transaction(EditTransaction {
            edits: edits
                .into_iter()
                .map(|(range, text)| TextEdit { range, text })
                .collect(),
            group: None,
        })
    }

    fn refresh_completion(
        buffer: &TextBuffer,
        state: &mut crate::language::EditorLanguage,
        explicit: bool,
    ) {
        let head = buffer.cursors().as_slice()[0].head;
        let line = buffer.rope().line(head.line as usize).to_string();
        let local = head.byte_offset - buffer.rope().line_to_byte(head.line as usize);
        let query = nozhik_language_support::completion::query(&line, local);
        state.token = 0;
        state.selected = 0;
        state.completion_scroll = true;
        state.completion_caret = head.byte_offset;
        state.completion_prefix = query.into();
        state.completions = if explicit || !query.is_empty() {
            state
                .stubs
                .as_ref()
                .map_or_else(Vec::new, |stubs| stubs.search(query))
        } else {
            Vec::new()
        };
        if explicit || !query.is_empty() {
            state.request_at = if explicit {
                None
            } else {
                Some(std::time::Instant::now())
            };
            state.request = Some((
                nozhik_language_support::RequestKind::Completion,
                head.byte_offset,
            ));
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "One bounded popup pass owns row geometry, painting and interaction"
    )]
    fn completion_popup(
        ui: &Ui,
        id: egui::Id,
        origin: egui::Pos2,
        state: &mut crate::language::EditorLanguage,
    ) -> bool {
        use nozhik_language_support::lsp_types::CompletionItemKind as Kind;
        let screen = ui.ctx().screen_rect();
        let width = 500.0_f32.min((screen.width() - 16.0).max(0.0));
        let height = state
            .completions
            .iter()
            .take(9)
            .fold(12.0, |height, _| height + 28.0);
        let origin = egui::pos2(
            origin
                .x
                .min(screen.right() - width - 8.0)
                .max(screen.left() + 8.0),
            if origin.y + height > screen.bottom() {
                (origin.y - height - 24.0).max(screen.top() + 8.0)
            } else {
                origin.y
            },
        );
        let mut accept = false;
        let area = egui::Area::new(id.with("completion"))
            .order(egui::Order::Foreground)
            .movable(false)
            .fixed_pos(origin)
            .show(ui.ctx(), |ui| {
                egui::Frame::new()
                    .fill(egui::Color32::from_rgb(43, 45, 48))
                    .stroke(Stroke::new(1.0_f32, egui::Color32::from_rgb(57, 59, 64)))
                    .corner_radius(7)
                    .inner_margin(5)
                    .show(ui, |ui| {
                        ui.set_width((width - 10.0).max(0.0));
                        ui.spacing_mut().item_spacing.y = 0.0;
                        egui::ScrollArea::vertical()
                            .id_salt("completion_rows")
                            .max_height((screen.height() - 36.0).clamp(28.0, 252.0))
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                for (i, item) in state.completions.iter().enumerate() {
                                    let (rect, row) = ui.allocate_exact_size(
                                        egui::vec2(ui.available_width(), 28.0),
                                        Sense::click(),
                                    );
                                    if row.hovered()
                                        && ui.input(|input| {
                                            input.pointer.delta() != egui::Vec2::ZERO
                                        })
                                    {
                                        state.selected = i;
                                    }
                                    if row.clicked() {
                                        state.selected = i;
                                    }
                                    if row.double_clicked() {
                                        accept = true;
                                    }
                                    if i == state.selected {
                                        ui.painter().rect_filled(
                                            rect,
                                            4,
                                            egui::Color32::from_rgb(46, 67, 110),
                                        );
                                        if state.completion_scroll {
                                            row.scroll_to_me(Some(egui::Align::Center));
                                        }
                                    }
                                    let (icon, color) = match item.kind {
                                        Some(Kind::FUNCTION | Kind::METHOD) => {
                                            ("fx", egui::Color32::from_rgb(194, 116, 139))
                                        }
                                        Some(Kind::CLASS) => {
                                            ("C", egui::Color32::from_rgb(87, 147, 255))
                                        }
                                        Some(Kind::INTERFACE) => {
                                            ("I", egui::Color32::from_rgb(71, 186, 190))
                                        }
                                        Some(Kind::CONSTANT) => {
                                            ("c", egui::Color32::from_rgb(220, 171, 89))
                                        }
                                        _ => ("k", egui::Color32::from_rgb(220, 171, 89)),
                                    };
                                    let center = egui::pos2(rect.left() + 14.0, rect.center().y);
                                    ui.painter().circle_filled(center, 9.0, color);
                                    ui.painter().text(
                                        center,
                                        egui::Align2::CENTER_CENTER,
                                        icon,
                                        FontId::proportional(11.0),
                                        egui::Color32::from_rgb(43, 45, 48),
                                    );
                                    let details = item.label_details.as_ref();
                                    let result_type = details
                                        .and_then(|d| d.description.as_deref())
                                        .unwrap_or("");
                                    let font = FontId::monospace(13.0);
                                    let muted = egui::Color32::from_rgb(130, 133, 139);
                                    let right = ui.painter().layout_no_wrap(
                                        result_type.into(),
                                        font.clone(),
                                        muted,
                                    );
                                    let right_width = right.size().x.min(rect.width() * 0.35);
                                    let right_x = rect.right() - right_width - 8.0;
                                    let right_clip = egui::Rect::from_min_max(
                                        egui::pos2(right_x, rect.top()),
                                        rect.right_bottom(),
                                    );
                                    ui.painter()
                                        .with_clip_rect(ui.clip_rect().intersect(right_clip))
                                        .galley(
                                            egui::pos2(
                                                right_x,
                                                rect.center().y - right.size().y / 2.0,
                                            ),
                                            right,
                                            muted,
                                        );
                                    let mut job = egui::text::LayoutJob::default();
                                    // The active query is stored independently of LSP filterText and display metadata.
                                    let prefix = state
                                        .completion_prefix
                                        .trim_start_matches('\\')
                                        .rsplit("::")
                                        .next()
                                        .unwrap_or("");
                                    let split = prefix.len().min(item.label.len());
                                    let split = if item.label.is_char_boundary(split)
                                        && item.label[..split].eq_ignore_ascii_case(prefix)
                                    {
                                        split
                                    } else {
                                        0
                                    };
                                    job.append(
                                        &item.label[..split],
                                        0.0,
                                        egui::TextFormat::simple(
                                            font.clone(),
                                            egui::Color32::from_rgb(88, 157, 246),
                                        ),
                                    );
                                    job.append(
                                        &item.label[split..],
                                        0.0,
                                        egui::TextFormat::simple(font.clone(), theme::TEXT),
                                    );
                                    job.append(
                                        details
                                            .and_then(|d| d.detail.as_deref())
                                            .or(item.detail.as_deref())
                                            .unwrap_or(""),
                                        0.0,
                                        egui::TextFormat::simple(font, muted),
                                    );
                                    job.wrap.max_width = (right_x - rect.left() - 38.0).max(0.0);
                                    job.wrap.max_rows = 1;
                                    job.wrap.break_anywhere = true;
                                    let galley = ui.fonts(|fonts| fonts.layout_job(job));
                                    ui.painter().galley(
                                        egui::pos2(
                                            rect.left() + 29.0,
                                            rect.center().y - galley.size().y / 2.0,
                                        ),
                                        galley,
                                        theme::TEXT,
                                    );
                                    if let Some(
                                        nozhik_language_support::lsp_types::Documentation::String(
                                            doc,
                                        ),
                                    ) = &item.documentation
                                        && !doc.is_empty()
                                    {
                                        row.on_hover_text(doc);
                                    }
                                }
                            });
                    });
            });
        state.completion_scroll = false;
        if ui.input(|i| {
            i.pointer.any_pressed()
                && i.pointer
                    .interact_pos()
                    .is_some_and(|p| !area.response.rect.contains(p))
        }) {
            state.completions.clear();
            state.token = 0;
            state.request = None;
        }
        accept
    }

    fn accept_completion(
        buffer: &mut TextBuffer,
        state: &mut crate::language::EditorLanguage,
    ) -> Result<(), BufferError> {
        if state.revision == buffer.snapshot().revision
            && let Some(item) = state.completions.get(state.selected)
        {
            let source = buffer.rope().to_string();
            let caret = buffer.cursors().as_slice()[0].head.byte_offset;
            if caret == state.completion_caret
                && let Some(edits) =
                    nozhik_language_support::completion::edits(item, &source, caret)
            {
                let local_caret = item
                    .data
                    .as_ref()
                    .filter(|data| data["nozhik_php"] == true)
                    .and_then(|data| data["caret"].as_u64())
                    .and_then(|offset| usize::try_from(offset).ok())
                    .and_then(|offset| {
                        edits.first().map(|(range, text)| {
                            range.start + offset + usize::from(text.starts_with('\\'))
                        })
                    });
                buffer.apply_transaction(EditTransaction {
                    edits: edits
                        .into_iter()
                        .map(|(range, text)| TextEdit { range, text })
                        .collect(),
                    group: None,
                })?;
                if let Some(byte) = local_caret {
                    let head = buffer.position_at(byte)?;
                    buffer.set_cursors(vec![Cursor { anchor: head, head }])?;
                }
            }
        }
        state.request = None;
        state.completions.clear();
        state.token = 0;
        Ok(())
    }

    fn input(buffer: &mut TextBuffer, event: Event, tab_width: u8) -> Result<(), BufferError> {
        match event {
            Event::Text(text) | Event::Paste(text) => Self::replace(buffer, &text, None),
            Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => {
                if modifiers.command && key == Key::Z {
                    let result = if modifiers.shift {
                        buffer.redo()
                    } else {
                        buffer.undo()
                    };
                    return match result {
                        Err(BufferError::EmptyHistory) => Ok(()),
                        other => other,
                    };
                }
                if modifiers.command && key == Key::A {
                    return buffer.set_cursors(vec![Cursor {
                        anchor: buffer.position_at(0)?,
                        head: buffer.position_at(buffer.rope().len_bytes())?,
                    }]);
                }
                match key {
                    Key::Backspace | Key::Delete => Self::replace(buffer, "", Some(key)),
                    Key::Enter => Self::replace(buffer, "\n", None),
                    Key::Tab => Self::replace(buffer, &" ".repeat(usize::from(tab_width)), None),
                    Key::ArrowLeft
                    | Key::ArrowRight
                    | Key::ArrowUp
                    | Key::ArrowDown
                    | Key::Home
                    | Key::End => {
                        let mut cursors = Vec::new();
                        for cursor in buffer.cursors().as_slice() {
                            let index = buffer.rope().byte_to_char(cursor.head.byte_offset);
                            let line = cursor.head.line as usize;
                            let target = match key {
                                Key::ArrowLeft => index.saturating_sub(1),
                                Key::ArrowRight => (index + 1).min(buffer.rope().len_chars()),
                                Key::Home => buffer.rope().line_to_char(line),
                                Key::End => {
                                    buffer.rope().line_to_char(line)
                                        + buffer
                                            .rope()
                                            .line(line)
                                            .to_string()
                                            .trim_end_matches(['\r', '\n'])
                                            .chars()
                                            .count()
                                }
                                _ => {
                                    let next = if key == Key::ArrowUp {
                                        line.saturating_sub(1)
                                    } else {
                                        (line + 1).min(buffer.rope().len_lines() - 1)
                                    };
                                    let col = index - buffer.rope().line_to_char(line);
                                    buffer.rope().line_to_char(next)
                                        + col.min(
                                            buffer
                                                .rope()
                                                .line(next)
                                                .to_string()
                                                .trim_end_matches(['\r', '\n'])
                                                .chars()
                                                .count(),
                                        )
                                }
                            };
                            let head = buffer.char_to_position(target)?;
                            cursors.push(Cursor {
                                anchor: if modifiers.shift { cursor.anchor } else { head },
                                head,
                            });
                        }
                        buffer.set_cursors(cursors)
                    }
                    _ => Ok(()),
                }
            }
            _ => Ok(()),
        }
    }

    pub(crate) fn replace(
        buffer: &mut TextBuffer,
        text: &str,
        deletion: Option<Key>,
    ) -> Result<(), BufferError> {
        let edits = buffer
            .cursors()
            .as_slice()
            .iter()
            .map(|cursor| {
                let mut start = cursor.anchor.byte_offset.min(cursor.head.byte_offset);
                let mut end = cursor.anchor.byte_offset.max(cursor.head.byte_offset);
                if start == end {
                    let index = buffer.rope().byte_to_char(start);
                    match deletion {
                        Some(Key::Backspace) => {
                            start = buffer.rope().char_to_byte(index.saturating_sub(1));
                        }
                        Some(Key::Delete) => {
                            end = buffer
                                .rope()
                                .char_to_byte((index + 1).min(buffer.rope().len_chars()));
                        }
                        _ => {}
                    }
                    // Delete CRLF as one line break.
                    if start < end
                        && buffer.rope().byte_slice(start..end) == "\n"
                        && start > 0
                        && buffer.rope().byte(start - 1) == b'\r'
                    {
                        start -= 1;
                    }
                    if start < end
                        && buffer.rope().byte_slice(start..end) == "\r"
                        && end < buffer.rope().len_bytes()
                        && buffer.rope().byte(end) == b'\n'
                    {
                        end += 1;
                    }
                }
                TextEdit {
                    range: start..end,
                    text: text.into(),
                }
            })
            .collect();
        buffer.apply_transaction(EditTransaction { edits, group: None })
    }
}

struct HitRow {
    start: usize,
    text: String,
    rect: egui::Rect,
    origin: egui::Pos2,
    galley: std::sync::Arc<egui::Galley>,
}
fn hit_byte(rows: &[HitRow], pos: egui::Pos2) -> Option<usize> {
    let row = rows
        .iter()
        .find(|row| pos.y < row.rect.bottom())
        .or_else(|| rows.last())?;
    let index = row
        .galley
        .cursor_from_pos(egui::vec2((pos.x - row.origin.x).max(0.0), 0.0))
        .index;
    Some(
        row.start
            + row
                .text
                .char_indices()
                .nth(index)
                .map_or(row.text.len(), |(byte, _)| byte),
    )
}
fn word_range(buffer: &TextBuffer, byte: usize) -> std::ops::Range<usize> {
    let rope = buffer.rope();
    let mut start = rope.byte_to_char(byte);
    let mut end = start;
    let word = |c: char| c.is_alphanumeric() || c == '_';
    if start == rope.len_chars() || !word(rope.char(start)) {
        if start > 0 && word(rope.char(start - 1)) {
            start -= 1;
            end = start;
        } else {
            return byte..rope.char_to_byte((start + 1).min(rope.len_chars()));
        }
    }
    while start > 0 && word(rope.char(start - 1)) {
        start -= 1;
    }
    while end < rope.len_chars() && word(rope.char(end)) {
        end += 1;
    }
    rope.char_to_byte(start)..rope.char_to_byte(end)
}

// Basic Markdown presentation for documentation, without fetching external assets.
fn markdown(ui: &mut Ui, source: &str) {
    let mut code = false;
    for line in source.lines() {
        if line.starts_with("```") {
            code = !code;
            continue;
        }
        if code {
            ui.monospace(line);
            continue;
        }
        if line.starts_with('#') {
            ui.heading(line.trim_start_matches('#').trim_start());
            continue;
        }
        let line = line
            .strip_prefix("- ")
            .map_or_else(|| line.to_string(), |s| format!("• {s}"));
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            let mut rest = line.as_str();
            while !rest.is_empty() {
                let marker = if rest.starts_with("**") {
                    "**"
                } else if rest.starts_with('`') {
                    "`"
                } else {
                    ""
                };
                if !marker.is_empty()
                    && let Some(end) = rest[marker.len()..].find(marker)
                {
                    let value = &rest[marker.len()..marker.len() + end];
                    let text = egui::RichText::new(value);
                    ui.label(if marker == "`" {
                        text.monospace()
                    } else {
                        text.strong()
                    });
                    rest = &rest[end + marker.len() * 2..];
                } else {
                    let end = rest
                        .char_indices()
                        .skip(1)
                        .find(|(_, c)| matches!(c, '*' | '`'))
                        .map_or(rest.len(), |(i, _)| i);
                    ui.label(&rest[..end]);
                    rest = &rest[end..];
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nozhik_core::BufferId;
    use nozhik_event_bus::{BufferEvent, Event as BusEvent, EventBus};

    fn mouse_frame(
        editor: &mut TextEditorView,
        buffer: &mut TextBuffer,
        ctx: &egui::Context,
        events: Vec<Event>,
        modifiers: egui::Modifiers,
        time: f64,
    ) -> Result<egui::FullOutput, BufferError> {
        let mut result = Ok(());
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(500.0, 180.0),
                )),
                events,
                modifiers,
                time: Some(time),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    result = editor.show(ui, buffer, 14.0, 4, true);
                });
            },
        );
        result?;
        Ok(output)
    }
    fn pointer(pos: egui::Pos2, pressed: bool, modifiers: egui::Modifiers) -> Vec<Event> {
        vec![
            Event::PointerMoved(pos),
            Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers,
            },
        ]
    }
    fn text_position(output: &egui::FullOutput, text: &str, column: usize) -> Option<egui::Pos2> {
        output.shapes.iter().find_map(|shape| {
            if let egui::Shape::Text(shape) = &shape.shape
                && shape.galley.job.text == text
            {
                Some(
                    shape.pos
                        + shape
                            .galley
                            .pos_from_cursor(CCursor::new(column))
                            .center()
                            .to_vec2(),
                )
            } else {
                None
            }
        })
    }
    fn click_with(
        editor: &mut TextEditorView,
        buffer: &mut TextBuffer,
        ctx: &egui::Context,
        pos: egui::Pos2,
        modifiers: egui::Modifiers,
        time: f64,
    ) -> Result<(), BufferError> {
        mouse_frame(
            editor,
            buffer,
            ctx,
            pointer(pos, true, modifiers),
            modifiers,
            time,
        )?;
        mouse_frame(
            editor,
            buffer,
            ctx,
            pointer(pos, false, modifiers),
            modifiers,
            time + 0.02,
        )?;
        Ok(())
    }
    fn click_n(
        editor: &mut TextEditorView,
        buffer: &mut TextBuffer,
        ctx: &egui::Context,
        pos: egui::Pos2,
        count: u32,
        time: f64,
    ) -> Result<(), BufferError> {
        for click in 0..count {
            let time = f64::from(click).mul_add(0.1, time);
            mouse_frame(
                editor,
                buffer,
                ctx,
                pointer(pos, true, egui::Modifiers::NONE),
                egui::Modifiers::NONE,
                time,
            )?;
            mouse_frame(
                editor,
                buffer,
                ctx,
                pointer(pos, false, egui::Modifiers::NONE),
                egui::Modifiers::NONE,
                time + 0.02,
            )?;
        }
        Ok(())
    }
    #[test]
    fn keyboard_enter_inside_if_indents_and_undo_is_atomic()
    -> Result<(), Box<dyn std::error::Error>> {
        let styles = nozhik_config::code_style::CodeStyles::builtin()?;
        let style = styles
            .selected("php", &std::collections::BTreeMap::new())
            .ok_or("style")?;
        let source = "function check($condition, $message) {\n    if (!$condition) {";
        let mut buffer = TextBuffer::load_from_str(BufferId(23), source);
        let head = buffer.position_at(source.len())?;
        buffer.set_cursors(vec![Cursor { anchor: head, head }])?;
        let mut editor = TextEditorView {
            code_style: Some(style.clone()),
            ..TextEditorView::default()
        };
        let ctx = egui::Context::default();
        let mut result = Ok(());
        let _frame = ctx.run(
            egui::RawInput {
                events: vec![Event::Key {
                    key: Key::Enter,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.memory_mut(|m| {
                        m.request_focus(ui.id().with(("editor", buffer.snapshot().buffer_id)));
                    });
                    result = editor.show(ui, &mut buffer, 14.0, 4, true);
                });
            },
        );
        result?;
        assert_eq!(buffer.rope().to_string(), format!("{source}\n        "));
        assert_eq!(buffer.cursors().as_slice()[0].head.column, 8);
        buffer.undo()?;
        assert_eq!(buffer.rope().to_string(), source);
        assert_eq!(
            buffer.cursors().as_slice()[0].head.byte_offset,
            source.len()
        );
        TextEditorView::newline(&mut buffer, None)?;
        assert_eq!(buffer.rope().to_string(), format!("{source}\n"));
        assert_eq!(
            nozhik_language_support::formatting::newline_text(
                "    echo '{'; // {",
                "\n",
                Some(style)
            ),
            "\n    "
        );
        assert_eq!(
            nozhik_language_support::formatting::newline_text("    if ($x) {", "\r\n", Some(style)),
            "\r\n        "
        );
        Ok(())
    }

    #[test]
    fn mouse_click_drag_words_lines_shift_and_wheel() -> Result<(), Box<dyn std::error::Error>> {
        let source = format!("alpha βeta\nsecond_line\n{}", "scroll content\n".repeat(80));
        let mut buffer = TextBuffer::load_from_str(BufferId(18), &source);
        let mut editor = TextEditorView::default();
        let ctx = egui::Context::default();
        let none = egui::Modifiers::NONE;
        mouse_frame(&mut editor, &mut buffer, &ctx, vec![], none, 0.0)?;
        let frame = mouse_frame(&mut editor, &mut buffer, &ctx, vec![], none, 0.1)?;
        let start = text_position(&frame, "alpha βeta", 3).ok_or("first row")?;
        let end = text_position(&frame, "second_line", 6).ok_or("second row")?;
        mouse_frame(
            &mut editor,
            &mut buffer,
            &ctx,
            pointer(start, true, none),
            none,
            1.0,
        )?;
        assert_eq!(buffer.cursors().as_slice()[0].head.byte_offset, 3);
        mouse_frame(
            &mut editor,
            &mut buffer,
            &ctx,
            vec![Event::PointerMoved(end)],
            none,
            1.1,
        )?;
        let cursor = buffer.cursors().as_slice()[0];
        assert_eq!(cursor.anchor.byte_offset, 3);
        assert_eq!(cursor.head.line, 1);
        assert_eq!(cursor.head.column, 6);
        let frame = mouse_frame(
            &mut editor,
            &mut buffer,
            &ctx,
            pointer(end, false, none),
            none,
            1.2,
        )?;
        assert_eq!(
            text_position(&frame, "alpha βeta", 3),
            Some(start),
            "drag must not pan content"
        );
        let word = text_position(&frame, "alpha βeta", 8).ok_or("Unicode word")?;
        click_n(&mut editor, &mut buffer, &ctx, word, 2, 3.0)?;
        let cursor = buffer.cursors().as_slice()[0];
        assert_eq!(
            buffer
                .rope()
                .byte_slice(cursor.anchor.byte_offset..cursor.head.byte_offset)
                .to_string(),
            "βeta"
        );
        click_with(
            &mut editor,
            &mut buffer,
            &ctx,
            end,
            egui::Modifiers::SHIFT,
            4.0,
        )?;
        assert_eq!(
            buffer.cursors().as_slice()[0].anchor.byte_offset,
            cursor.anchor.byte_offset
        );
        assert_eq!(buffer.cursors().as_slice()[0].head.column, 6);
        click_n(&mut editor, &mut buffer, &ctx, end, 3, 6.0)?;
        let cursor = buffer.cursors().as_slice()[0];
        assert_eq!(
            buffer
                .rope()
                .byte_slice(cursor.anchor.byte_offset..cursor.head.byte_offset)
                .to_string(),
            "second_line\n"
        );
        mouse_frame(
            &mut editor,
            &mut buffer,
            &ctx,
            vec![
                Event::PointerMoved(end),
                Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, -140.0),
                    modifiers: none,
                },
            ],
            none,
            8.0,
        )?;
        assert_eq!(buffer.cursors().as_slice()[0], cursor);
        let frame = mouse_frame(&mut editor, &mut buffer, &ctx, vec![], none, 8.1)?;
        assert_ne!(
            text_position(&frame, "alpha βeta", 3),
            Some(start),
            "wheel must scroll viewport"
        );
        assert_eq!(buffer.rope().to_string(), source);
        Ok(())
    }

    #[test]
    fn horizontal_wheel_and_clipboard_paste_use_the_focused_editor()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut editor = TextEditorView::default();
        let ctx = egui::Context::default();
        let source = "long content ".repeat(100);
        let mut buffer = TextBuffer::load_from_str(BufferId(19), &source);
        let none = egui::Modifiers::NONE;
        mouse_frame(&mut editor, &mut buffer, &ctx, vec![], none, 0.0)?;
        let frame = mouse_frame(&mut editor, &mut buffer, &ctx, vec![], none, 0.1)?;
        let point = text_position(&frame, &source, 3).ok_or("long row")?;
        click_with(&mut editor, &mut buffer, &ctx, point, none, 1.0)?;
        let caret = buffer.cursors().as_slice()[0];
        mouse_frame(
            &mut editor,
            &mut buffer,
            &ctx,
            vec![Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(-120.0, 0.0),
                modifiers: none,
            }],
            none,
            2.0,
        )?;
        let frame = mouse_frame(&mut editor, &mut buffer, &ctx, vec![], none, 2.1)?;
        assert_ne!(text_position(&frame, &source, 3), Some(point));
        assert_eq!(buffer.cursors().as_slice()[0], caret);
        let styles = nozhik_config::code_style::CodeStyles::builtin()?;
        editor.code_style = styles
            .selected("php", &std::collections::BTreeMap::new())
            .cloned();
        let source = "<?php\nclass C\n{\n    function f()\n    {\n        \n    }\n}";
        let mut buffer = TextBuffer::load_from_str(BufferId(20), source);
        let line = source.find("        \n").ok_or("indentation")?;
        let head = buffer.position_at(line + 3)?;
        buffer.set_cursors(vec![Cursor { anchor: head, head }])?;
        let mut result = Ok(());
        let _frame = ctx.run(
            egui::RawInput {
                events: vec![Event::Paste("$user->save();".into())],
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.memory_mut(|m| {
                        m.request_focus(ui.id().with(("editor", buffer.snapshot().buffer_id)));
                    });
                    result = editor.show(ui, &mut buffer, 14.0, 4, true);
                });
            },
        );
        result?;
        assert!(
            buffer
                .rope()
                .to_string()
                .contains("\n        $user->save();\n")
        );
        buffer.undo()?;
        assert_eq!(buffer.rope().to_string(), source);
        TextEditorView::paste(
            &mut buffer,
            "if ($x) {\nrun();\n        if ($y) {\n x();\n }\n}",
            editor.code_style.as_ref(),
        )?;
        assert!(buffer.rope().to_string().contains("\n        if ($x) {\n            run();\n            if ($y) {\n                x();\n            }\n        }\n"));
        buffer.undo()?;
        assert_eq!(buffer.rope().to_string(), source);
        Ok(())
    }

    #[test]
    fn format_and_smart_paste_are_single_undo_transactions()
    -> Result<(), Box<dyn std::error::Error>> {
        let styles = nozhik_config::code_style::CodeStyles::builtin()?;
        let style = styles
            .selected("php", &std::collections::BTreeMap::new())
            .ok_or("profile")?;
        let original = "<?php class C{public function f(){return 1+2;}}";
        let mut buffer = TextBuffer::load_from_str(BufferId(2), original);
        let edits = nozhik_language_support::formatting::php_edits(original, style, None)?;
        TextEditorView::apply_format(&mut buffer, edits)?;
        assert!(buffer.rope().to_string().contains("return 1 + 2;"));
        buffer.undo()?;
        assert_eq!(buffer.rope().to_string(), original);
        assert!(!buffer.is_dirty());
        let original = "<?php\nclass C\n{\n    function f()\n    {\n        ";
        let mut buffer = TextBuffer::load_from_str(BufferId(3), original);
        let head = buffer.position_at(original.len())?;
        buffer.set_cursors(vec![Cursor { anchor: head, head }])?;
        TextEditorView::paste(
            &mut buffer,
            "    if ($x) {\n        run();\n    }",
            Some(style),
        )?;
        assert!(
            buffer
                .rope()
                .to_string()
                .ends_with("        if ($x) {\n            run();\n        }")
        );
        buffer.undo()?;
        assert_eq!(buffer.rope().to_string(), original);
        TextEditorView::paste(&mut buffer, "  raw\n text", None)?;
        assert_eq!(buffer.rope().to_string(), format!("{original}  raw\n text"));
        Ok(())
    }

    fn completion_frame(
        ctx: &egui::Context,
        editor: &mut TextEditorView,
        buffer: &mut TextBuffer,
        language: &mut crate::language::EditorLanguage,
        events: Vec<Event>,
        time: f64,
    ) -> Result<egui::FullOutput, BufferError> {
        let mut result = Ok(());
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 600.0),
                )),
                events,
                time: Some(time),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.memory_mut(|m| {
                        m.request_focus(ui.id().with(("editor", buffer.snapshot().buffer_id)));
                    });
                    result = editor.show_language(ui, buffer, 14.0, 4, true, Some(language));
                });
            },
        );
        result?;
        Ok(output)
    }
    fn completion_key(key: Key, ctrl: bool) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers {
                ctrl,
                ..Default::default()
            },
        }
    }
    #[test]
    fn offline_typing_navigation_accept_and_escape() -> Result<(), Box<dyn std::error::Error>> {
        let ctx = egui::Context::default();
        let mut editor = TextEditorView::default();
        let mut buffer = TextBuffer::load_from_str(BufferId(92), "");
        let mut state = crate::language::EditorLanguage {
            stubs: Some(std::sync::Arc::new(
                nozhik_language_support::php_stubs::PhpVersionStubs::builtin()?,
            )),
            ..Default::default()
        };
        completion_frame(
            &ctx,
            &mut editor,
            &mut buffer,
            &mut state,
            vec![Event::Text("p".into())],
            0.0,
        )?;
        assert_ne!(state.completions, []);
        completion_frame(
            &ctx,
            &mut editor,
            &mut buffer,
            &mut state,
            vec![Event::Text("r".into())],
            0.1,
        )?;
        assert_eq!(buffer.rope().to_string(), "pr");
        assert!(state.completions.iter().any(|s| s.label == "print_r"));
        completion_frame(
            &ctx,
            &mut editor,
            &mut buffer,
            &mut state,
            vec![completion_key(Key::ArrowUp, false)],
            0.2,
        )?;
        assert_eq!(state.selected, state.completions.len() - 1);
        completion_frame(
            &ctx,
            &mut editor,
            &mut buffer,
            &mut state,
            vec![completion_key(Key::ArrowDown, false)],
            0.3,
        )?;
        assert_eq!(state.selected, 0);
        let index = state
            .completions
            .iter()
            .position(|s| s.label == "print_r")
            .ok_or("print_r")?;
        let mut events: Vec<_> = (0..index)
            .map(|_| completion_key(Key::ArrowDown, false))
            .collect();
        events.push(completion_key(Key::Tab, false));
        completion_frame(&ctx, &mut editor, &mut buffer, &mut state, events, 0.4)?;
        assert_eq!(buffer.rope().to_string(), "print_r()");
        assert_eq!(buffer.cursors().as_slice()[0].head.byte_offset, 8);
        assert_eq!(state.completions, []);
        buffer.undo()?;
        assert_eq!(buffer.rope().to_string(), "pr");
        completion_frame(
            &ctx,
            &mut editor,
            &mut buffer,
            &mut state,
            vec![completion_key(Key::Space, true)],
            0.5,
        )?;
        assert_ne!(state.completions, []);
        completion_frame(
            &ctx,
            &mut editor,
            &mut buffer,
            &mut state,
            vec![completion_key(Key::Escape, false)],
            0.6,
        )?;
        assert_eq!(state.completions, []);
        assert!(state.request.is_none());
        assert_eq!(state.token, 0);
        assert_eq!(buffer.rope().to_string(), "pr");
        Ok(())
    }
    #[test]
    fn offline_constants_namespaces_and_members_insert_correctly()
    -> Result<(), Box<dyn std::error::Error>> {
        let stubs =
            std::sync::Arc::new(nozhik_language_support::php_stubs::PhpVersionStubs::builtin()?);
        for (source, name, expected, caret) in [
            ("PHP_V", "PHP_VERSION", "PHP_VERSION", 11),
            ("\\strl", "strlen", "\\strlen()", 8),
            ("DateTime::fo", "format", "DateTime::format()", 17),
        ] {
            let mut buffer = TextBuffer::load_from_str(BufferId(94), source);
            let head = buffer.position_at(source.len())?;
            buffer.set_cursors(vec![Cursor { anchor: head, head }])?;
            let mut state = crate::language::EditorLanguage {
                stubs: Some(stubs.clone()),
                ..Default::default()
            };
            TextEditorView::refresh_completion(&buffer, &mut state, true);
            state.selected = state
                .completions
                .iter()
                .position(|s| s.label == name)
                .ok_or("missing symbol")?;
            TextEditorView::accept_completion(&mut buffer, &mut state)?;
            assert_eq!(buffer.rope().to_string(), expected);
            assert_eq!(buffer.cursors().as_slice()[0].head.byte_offset, caret);
            buffer.undo()?;
            assert_eq!(buffer.rope().to_string(), source);
        }
        Ok(())
    }

    #[test]
    fn offline_popup_pointer_select_accept_and_dismiss() -> Result<(), Box<dyn std::error::Error>> {
        let ctx = egui::Context::default();
        let mut editor = TextEditorView::default();
        let mut buffer = TextBuffer::load_from_str(BufferId(93), "");
        let mut state = crate::language::EditorLanguage {
            stubs: Some(std::sync::Arc::new(
                nozhik_language_support::php_stubs::PhpVersionStubs::builtin()?,
            )),
            ..Default::default()
        };
        completion_frame(
            &ctx,
            &mut editor,
            &mut buffer,
            &mut state,
            vec![Event::Text("pri".into())],
            0.0,
        )?;
        let output = completion_frame(&ctx, &mut editor, &mut buffer, &mut state, vec![], 0.1)?;
        let pos = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.job.text.starts_with("printf(") => {
                    Some(text.pos + egui::vec2(10.0, 7.0))
                }
                _ => None,
            })
            .ok_or("printf row was painted")?;
        for (time, down) in [(1.0, true), (1.1, false)] {
            completion_frame(
                &ctx,
                &mut editor,
                &mut buffer,
                &mut state,
                pointer(pos, down, egui::Modifiers::NONE),
                time,
            )?;
        }
        assert_eq!(buffer.rope().to_string(), "pri", "one click only selects");
        assert_eq!(state.completions[state.selected].label, "printf");
        for (time, down) in [(1.2, true), (1.3, false)] {
            completion_frame(
                &ctx,
                &mut editor,
                &mut buffer,
                &mut state,
                pointer(pos, down, egui::Modifiers::NONE),
                time,
            )?;
        }
        assert_eq!(buffer.rope().to_string(), "printf()");
        assert_eq!(buffer.cursors().as_slice()[0].head.byte_offset, 7);
        buffer.undo()?;
        completion_frame(
            &ctx,
            &mut editor,
            &mut buffer,
            &mut state,
            vec![completion_key(Key::Space, true)],
            2.0,
        )?;
        completion_frame(
            &ctx,
            &mut editor,
            &mut buffer,
            &mut state,
            pointer(egui::pos2(700.0, 400.0), true, egui::Modifiers::NONE),
            3.0,
        )?;
        assert_eq!(state.completions, []);
        assert!(state.request.is_none());
        Ok(())
    }

    #[test]
    fn php_view_completion_keys_replace_prefix_without_newline()
    -> Result<(), Box<dyn std::error::Error>> {
        use nozhik_language_support::lsp_types::{
            CompletionItem, Diagnostic, DiagnosticSeverity, Position, Range,
        };
        let mut buffer = TextBuffer::load_from_str(BufferId(1), "<?php $fo");
        let head = buffer.position_at(buffer.rope().len_bytes())?;
        buffer.set_cursors(vec![Cursor { anchor: head, head }])?;
        let mut language = crate::language::EditorLanguage {
            spans: nozhik_language_support::highlighter::Highlighter::php()?
                .highlight("<?php $fo")?,
            diagnostics: vec![Diagnostic {
                range: Range::new(Position::new(0, 6), Position::new(0, 9)),
                severity: Some(DiagnosticSeverity::ERROR),
                message: "Unknown variable".into(),
                ..Default::default()
            }],
            completions: vec![
                CompletionItem::new_simple("$foo".into(), "first".into()),
                CompletionItem::new_simple("$format".into(), "second".into()),
            ],
            completion_caret: head.byte_offset,
            ..Default::default()
        };
        let context = egui::Context::default();
        let mut editor = TextEditorView::default();
        for key in [Key::ArrowDown, Key::Enter] {
            let input = egui::RawInput {
                events: vec![Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            };
            let mut result = Ok(());
            let _frame = context.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let id = ui.id().with(("editor", buffer.snapshot().buffer_id));
                    ui.memory_mut(|m| m.request_focus(id));
                    result =
                        editor.show_language(ui, &mut buffer, 14.0, 4, true, Some(&mut language));
                });
            });
            result?;
        }
        assert_eq!(buffer.rope().to_string(), "<?php $format");
        assert_eq!(language.completions, []);
        buffer.undo()?;
        assert_eq!(buffer.rope().to_string(), "<?php $fo");
        Ok(())
    }

    #[test]
    fn unicode_input_history_and_bus() -> Result<(), Box<dyn std::error::Error>> {
        let bus = EventBus::new(32)?;
        let mut receiver = bus.subscribe();
        let mut buffer = TextBuffer::load_from_str(BufferId(1), "");
        buffer.set_event_bus(bus);
        TextEditorView::input(&mut buffer, Event::Text("Ж🦀".into()), 4)?;
        assert_eq!(buffer.rope().to_string(), "Ж🦀");
        assert!(buffer.is_dirty());
        assert!(matches!(
            receiver.try_recv()?.as_ref(),
            BusEvent::Buffer(BufferEvent::Modified { .. })
        ));
        TextEditorView::replace(&mut buffer, "", Some(Key::Backspace))?;
        assert_eq!(buffer.rope().to_string(), "Ж");
        buffer.undo()?;
        assert_eq!(buffer.rope().to_string(), "Ж🦀");
        buffer.undo()?;
        assert!(!buffer.is_dirty());
        buffer.redo()?;
        assert_eq!(buffer.rope().to_string(), "Ж🦀");
        Ok(())
    }

    #[test]
    fn selected_text_and_crlf_delete() -> Result<(), BufferError> {
        let mut buffer = TextBuffer::load_from_str(BufferId(1), "a\r\nb");
        let head = buffer.position_at(3)?;
        buffer.set_cursors(vec![Cursor { anchor: head, head }])?;
        TextEditorView::replace(&mut buffer, "", Some(Key::Backspace))?;
        assert_eq!(buffer.rope().to_string(), "ab");
        buffer.set_cursors(vec![Cursor {
            anchor: buffer.position_at(0)?,
            head: buffer.position_at(2)?,
        }])?;
        TextEditorView::replace(&mut buffer, "Я", None)?;
        assert_eq!(buffer.rope().to_string(), "Я");
        assert_eq!(buffer.cursors().as_slice()[0].head.byte_offset, 2);
        Ok(())
    }
}
