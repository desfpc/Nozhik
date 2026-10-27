//! Virtualized Rope editor. Only visible lines are converted to strings/galleys.
use crate::theme;
use egui::{Event, FontId, Key, Sense, Stroke, Ui, text::CCursor};
use nozhik_buffer::{BufferError, Cursor, EditTransaction, TextBuffer, TextEdit};

/// Virtualized view; text, caret, selections and history live in the buffer.
#[derive(Default)]
pub struct TextEditorView {
    pub(crate) ai: crate::ai_inline::Inline,
    pub(crate) focus_buffer: Option<nozhik_core::BufferId>,
    /// Pending navigation target; scrolled into view on the next frame.
    pub reveal_line: Option<usize>,
    /// Active declarative profile; absent means verbatim paste.
    pub code_style: Option<nozhik_config::code_style::CodeStyle>,
    /// Render editor text using the proportional built-in family.
    pub proportional_font: bool,
    pub(crate) documentation_target:
        Option<nozhik_language_support::doc_provider::DefinitionTarget>,
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
        if enabled && self.focus_buffer == Some(buffer.snapshot().buffer_id) {
            self.focus_buffer = None;
            response.request_focus();
        }
        if response.clicked() && enabled {
            response.request_focus();
        }
        self.ai
            .synchronize(buffer, ui.ctx(), enabled && response.has_focus());
        if ui.input(|i| i.pointer.any_pressed()) {
            self.ai.rearm();
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
                if matches!(&event, Event::Key { key: Key::Tab, pressed: true, modifiers, .. } if modifiers.is_none())
                    && let Some(text) = self.ai.pending_ghost_text.take()
                {
                    Self::replace(buffer, &text, None)?;
                    self.ai.synchronize(buffer, ui.ctx(), false);
                    if let Some(state) = language.as_deref_mut() {
                        state.completions.clear();
                        state.request = None;
                        state.token = 0;
                    }
                    continue;
                }
                self.ai.cancel();
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
                if let Event::Key {
                    key: key @ (Key::Backspace | Key::Delete),
                    modifiers,
                    ..
                } = &event
                    && modifiers.is_none()
                {
                    Self::replace_inner(
                        buffer,
                        "",
                        Some(*key),
                        Some((self.code_style.as_ref(), tab_width)),
                    )?;
                } else if let Event::Paste(text) = event {
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
        self.ai
            .synchronize(buffer, ui.ctx(), enabled && response.has_focus());
        let ghost = self.ai.pending_ghost_text.clone();
        let ghost_lines: Vec<_> = ghost
            .as_deref()
            .map_or_else(Vec::new, |s| s.split('\n').collect());
        let extra_rows = ghost_lines.len().saturating_sub(1);
        let ghost_line = buffer.cursors().as_slice()[0].head.line as usize;
        let ghost_column = buffer.cursors().as_slice()[0].head.column as usize;
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
        let mut inspected_doc = None;
        let inspect = enabled && ui.input(|i| primary_modifier(i.modifiers));
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
            buffer.rope().len_lines() + extra_rows,
            |ui, rows| {
                for virtual_line in rows {
                    let line = if virtual_line > ghost_line {
                        virtual_line.saturating_sub(extra_rows).max(ghost_line)
                    } else {
                        virtual_line
                    };
                    let synthetic =
                        virtual_line > ghost_line && virtual_line <= ghost_line + extra_rows;
                    let ghost_row =
                        (line == ghost_line && ghost.is_some()).then(|| virtual_line - line);
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
                    if let Some(part) = ghost_row {
                        job = ghost_job(&job, ghost_column, &ghost_lines, part, font_size);
                    }
                    let galley = ui.fonts(|fonts| fonts.layout_job(job));
                    let width = ui.available_width().max(galley.size().x + 90.0);
                    let (rect, row) =
                        ui.allocate_exact_size(egui::vec2(width, font_size + 7.0), Sense::hover());
                    if reveal == Some(line) {
                        ui.scroll_to_rect(rect, Some(egui::Align::Center));
                    }
                    let origin = rect.min + egui::vec2(66.0, 3.0);
                    if !synthetic
                        && ghost_row.is_none()
                        && row.contains_pointer()
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
                    if inspect
                        && !synthetic
                        && ghost_row.is_none()
                        && row.contains_pointer()
                        && let Some(pos) = ui.input(|i| i.pointer.hover_pos())
                        && let Some(range) = doc_token(text, &galley, origin, pos)
                        && let Some(state) = language.as_ref()
                        && !visible_spans.iter().any(|span| {
                            span.range.contains(&(start + range.start))
                                && (span.token.starts_with("comment")
                                    || span.token.starts_with("string")
                                    || span.token.starts_with("variable"))
                        })
                        && let Some(docs) = state.documentation.as_ref()
                        && let Some(info) = docs
                            .provider
                            .resolve_symbol_info(&docs.language_id, &text[range.clone()])
                        && info.target.is_some()
                    {
                        let x = |byte| {
                            origin.x
                                + galley
                                    .pos_from_cursor(CCursor::new(text[..byte].chars().count()))
                                    .min
                                    .x
                        };
                        let y = origin.y + galley.size().y;
                        ui.painter().line_segment(
                            [egui::pos2(x(range.start), y), egui::pos2(x(range.end), y)],
                            Stroke::new(1.0_f32, theme::INFO),
                        );
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        inspected_doc = Some((info, docs.label.clone(), pos));
                    }
                    if ghost_row.is_none()
                        && let Some(state) = &language
                    {
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
                        if synthetic {
                            String::new()
                        } else {
                            (line + 1).to_string()
                        },
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
                        if !synthetic && cursor.head.line as usize == line {
                            let local = cursor
                                .head
                                .byte_offset
                                .saturating_sub(start)
                                .min(text.len());
                            let caret =
                                galley.pos_from_cursor(CCursor::new(text[..local].chars().count()));
                            popup_origin = Some(
                                origin
                                    + egui::vec2(
                                        caret.min.x,
                                        (extra_rows as f32)
                                            .mul_add(font_size + 7.0, font_size + 6.0),
                                    ),
                            );
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
                    if ghost_row == Some(0) {
                        let hint = origin + egui::vec2(galley.size().x + 14.0, 0.0);
                        ui.painter().text(
                            hint,
                            egui::Align2::LEFT_TOP,
                            "⇥ to complete",
                            FontId::monospace(font_size),
                            theme::MUTED,
                        );
                    }
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
                && inspected_doc.is_none()
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
        if let Some((info, label, pos)) = &inspected_doc {
            doc_popup(ui, id, info, label, *pos, font_size);
            // Suppress both press-time selection and release-time caret movement.
            self.mouse_anchor = None;
            if response.clicked_by(egui::PointerButton::Primary)
                && let Some(nozhik_language_support::doc_provider::DefinitionTarget::ExternalUrl(
                    url,
                )) = &info.target
            {
                self.documentation_target = Some(
                    nozhik_language_support::doc_provider::DefinitionTarget::ExternalUrl(
                        url.clone(),
                    ),
                );
            }
            return Ok(());
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
            self.ai.cancel();
            self.ai.synchronize(buffer, ui.ctx(), false);
            if let Some(state) = language {
                state.completions.clear();
                state.token = 0;
                state.request = None;
            }
        } else if enabled
            && response.clicked_by(egui::PointerButton::Primary)
            && ui.input(|i| primary_modifier(i.modifiers))
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
        if (pointer.button_pressed(egui::PointerButton::Primary)
            && response.is_pointer_button_down_on()
            || response.clicked_by(egui::PointerButton::Primary))
            && !ui.input(|i| primary_modifier(i.modifiers))
            && let Some(pos) = pointer
                .press_origin()
                .or_else(|| response.interact_pointer_pos())
                .filter(|p| viewport.contains(*p))
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
        Self::replace_inner(buffer, text, deletion, None)
    }

    fn replace_inner(
        buffer: &mut TextBuffer,
        text: &str,
        deletion: Option<Key>,
        indentation: Option<(Option<&nozhik_config::code_style::CodeStyle>, u8)>,
    ) -> Result<(), BufferError> {
        let mut edits: Vec<TextEdit> = buffer
            .cursors()
            .as_slice()
            .iter()
            .map(|cursor| {
                let mut start = cursor.anchor.byte_offset.min(cursor.head.byte_offset);
                let mut end = cursor.anchor.byte_offset.max(cursor.head.byte_offset);
                if start == end {
                    if let Some((style, tab_width)) = indentation
                        && let Some(edit) = blank_line_deletion(buffer, start, style, tab_width)
                    {
                        return edit;
                    }
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
        if indentation.is_some() {
            // Several carets may delete the same blank line or adjacent line endings.
            edits.sort_by_key(|edit| edit.range.start);
            let mut merged: Vec<TextEdit> = Vec::with_capacity(edits.len());
            for edit in edits {
                if let Some(previous) = merged.last_mut()
                    && edit.range.start <= previous.range.end
                {
                    previous.range.end = previous.range.end.max(edit.range.end);
                } else {
                    merged.push(edit);
                }
            }
            edits = merged;
        }
        buffer.apply_transaction(EditTransaction { edits, group: None })
    }
}

// Only whitespace-only lines use smart deletion. Byte ranges stay in the original
// Rope, so all carets commit in one transaction and Undo restores their positions.
fn blank_line_deletion(
    buffer: &TextBuffer,
    caret: usize,
    style: Option<&nozhik_config::code_style::CodeStyle>,
    tab_width: u8,
) -> Option<TextEdit> {
    let rope = buffer.rope();
    let line = rope.byte_to_line(caret);
    let start = rope.line_to_byte(line);
    let source = rope.line(line).to_string();
    let content = source
        .strip_suffix("\r\n")
        .or_else(|| source.strip_suffix('\n'))
        .unwrap_or(&source);
    if !content.chars().all(|c| matches!(c, ' ' | '\t')) || caret > start + content.len() {
        return None;
    }
    let size = usize::from(style.map_or_else(|| tab_width.max(1), |s| s.indent.size.max(1)));
    let width = |text: &str| {
        text.chars()
            .take_while(|c| matches!(c, ' ' | '\t'))
            .fold(0, |col, c| {
                if c == '\t' {
                    col + size - col % size
                } else {
                    col + 1
                }
            })
    };
    let expected = (0..line)
        .rev()
        .find_map(|previous| {
            let previous = rope.line(previous).to_string();
            let previous = previous.trim_end_matches(['\r', '\n']);
            if previous.chars().all(|c| matches!(c, ' ' | '\t')) {
                return None;
            }
            Some(style.map_or_else(
                || width(previous),
                |profile| {
                    width(
                        nozhik_language_support::formatting::newline_text(
                            previous,
                            "\n",
                            Some(profile),
                        )
                        .trim_start_matches('\n'),
                    )
                },
            ))
        })
        .unwrap_or(0);
    if width(&content[..caret - start]) > expected {
        let mut column = 0;
        let mut keep = 0;
        for (byte, c) in content[..caret - start].char_indices() {
            let next = if c == '\t' {
                column + size - column % size
            } else {
                column + 1
            };
            if next > expected {
                break;
            }
            column = next;
            keep = byte + c.len_utf8();
        }
        return Some(TextEdit {
            range: start + keep..caret,
            text: " ".repeat(expected - column),
        });
    }
    // Remove the preceding line ending, placing the caret at the previous line's
    // end. The first line instead consumes its following line ending.
    let range = if line > 0 {
        let previous = rope.line(line - 1);
        let ending =
            if previous.len_bytes() >= 2 && previous.byte(previous.len_bytes() - 2) == b'\r' {
                2
            } else {
                1
            };
        start - ending..start + content.len()
    } else {
        start..start + source.len()
    };
    Some(TextEdit {
        range,
        text: String::new(),
    })
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
// Virtual ghost rows move the real suffix and following lines without modifying the Rope.
fn ghost_job(
    original: &egui::text::LayoutJob,
    column: usize,
    lines: &[&str],
    part: usize,
    font_size: f32,
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let column = column.min(original.text.len());
    let append_source = |job: &mut egui::text::LayoutJob, range: std::ops::Range<usize>| {
        for section in &original.sections {
            let start = section.byte_range.start.max(range.start);
            let end = section.byte_range.end.min(range.end);
            if start < end {
                job.append(&original.text[start..end], 0.0, section.format.clone());
            }
        }
    };
    if part == 0 {
        append_source(&mut job, 0..column);
    }
    if let Some(text) = lines.get(part) {
        job.append(
            text.trim_end_matches('\r'),
            0.0,
            egui::TextFormat {
                font_id: FontId::monospace(font_size),
                color: egui::Color32::from_rgba_unmultiplied(122, 126, 133, 120),
                ..Default::default()
            },
        );
    }
    if part + 1 == lines.len() {
        append_source(&mut job, column..original.text.len());
    }
    job
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

const fn primary_modifier(modifiers: egui::Modifiers) -> bool {
    if cfg!(target_os = "macos") {
        modifiers.mac_cmd
    } else {
        modifiers.ctrl
    }
}

// Unlike caret hit testing, documentation requires a glyph under the pointer:
// gutter, trailing whitespace and the right half of punctuation must not snap to a name.
fn doc_token(
    text: &str,
    galley: &egui::Galley,
    origin: egui::Pos2,
    pos: egui::Pos2,
) -> Option<std::ops::Range<usize>> {
    let x = pos.x - origin.x;
    if x < 0.0 || x >= galley.size().x {
        return None;
    }
    let mut index = galley.cursor_from_pos(pos - origin).index;
    if galley.pos_from_cursor(CCursor::new(index)).min.x > x {
        index = index.saturating_sub(1);
    }
    let byte = text.char_indices().nth(index)?.0;
    let identifier = |c: char| c.is_alphanumeric() || c == '_';
    if !text[byte..].chars().next().is_some_and(identifier) {
        return None;
    }
    let allowed = |c| identifier(c) || c == '\\' || c == ':';
    let start = text[..byte]
        .char_indices()
        .rev()
        .take_while(|(_, c)| allowed(*c))
        .last()
        .map_or(byte, |(i, _)| i);
    let end = byte
        + text[byte..]
            .char_indices()
            .take_while(|(_, c)| allowed(*c))
            .last()
            .map_or(0, |(i, c)| i + c.len_utf8());
    if start > 0 && (text[..start].ends_with('$') || text[..start].ends_with("->")) {
        return None;
    }
    let name = text[start..end].trim_start_matches('\\');
    if name.starts_with(|c: char| c.is_ascii_digit()) || name.ends_with(':') {
        return None;
    }
    Some(start..end)
}

fn doc_popup(
    ui: &Ui,
    id: egui::Id,
    info: &nozhik_language_support::doc_provider::SymbolDocInfo,
    label: &str,
    pos: egui::Pos2,
    font_size: f32,
) {
    use nozhik_language_support::doc_provider::EntityKind;
    let screen = ui.ctx().screen_rect();
    let width = 520.0_f32.min((screen.width() - 24.0).max(80.0));
    egui::Area::new(id.with("symbol_documentation"))
        .order(egui::Order::Tooltip)
        .interactable(false)
        .movable(false)
        .fixed_pos(egui::pos2(
            pos.x.min(screen.right() - width - 12.0).max(screen.left()),
            pos.y + 20.0,
        ))
        .constrain_to(screen.shrink(4.0))
        .show(ui.ctx(), |ui| {
            egui::Frame::new()
                .fill(egui::Color32::from_rgb(43, 45, 48))
                .stroke(Stroke::new(1.0_f32, theme::BORDER))
                .corner_radius(7)
                .inner_margin(10)
                .show(ui, |ui| {
                    ui.set_width(width - 20.0);
                    let doc_font =
                        FontId::new(font_size, egui::FontFamily::Name("JetBrains Mono".into()));
                    ui.style_mut().override_font_id = Some(doc_font.clone());
                    let icon = match info.kind {
                        EntityKind::Function | EntityKind::Method => "fx",
                        EntityKind::Class => "C",
                        EntityKind::Interface => "I",
                        EntityKind::Constant | EntityKind::Keyword => "k",
                    };
                    ui.label(
                        egui::RichText::new(format!("({icon}) {}", info.name))
                            .font(doc_font.clone())
                            .strong()
                            .color(theme::INFO),
                    );
                    egui::ScrollArea::vertical()
                        .max_height((screen.height() * 0.4).max(40.0))
                        .show(ui, |ui| {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(&info.signature)
                                        .font(doc_font)
                                        .color(theme::TEXT),
                                )
                                .wrap(),
                            );
                            if !info.description.is_empty() {
                                ui.separator();
                                ui.add(egui::Label::new(&info.description).wrap());
                            }
                        });
                    ui.separator();
                    let modifier = if cfg!(target_os = "macos") {
                        "Cmd"
                    } else {
                        "Ctrl"
                    };
                    ui.label(
                        egui::RichText::new(format!(
                            "{modifier}+Click to open {label} documentation"
                        ))
                        .small()
                        .color(theme::MUTED),
                    );
                });
        });
}
pub(crate) fn open_documentation(url: String) {
    if let Err(error) = std::thread::Builder::new()
        .name("documentation-browser".into())
        .spawn(move || {
            if let Err(error) = webbrowser::open(&url) {
                eprintln!("Open documentation: {error}");
            }
        })
    {
        eprintln!("Start documentation browser worker: {error}");
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
    fn fast_click_in_one_frame_places_caret_in_empty_indented_line()
    -> Result<(), Box<dyn std::error::Error>> {
        let ctx = egui::Context::default();
        let mut editor = TextEditorView::default();
        let mut buffer = TextBuffer::load_from_str(BufferId(31), "first\n    \nlast");
        let none = egui::Modifiers::NONE;
        mouse_frame(&mut editor, &mut buffer, &ctx, vec![], none, 0.0)?;
        let frame = mouse_frame(&mut editor, &mut buffer, &ctx, vec![], none, 0.1)?;
        let at = text_position(&frame, "    ", 4).ok_or("empty row")?;
        let mut events = pointer(at, true, none);
        events.extend(pointer(at, false, none));
        mouse_frame(&mut editor, &mut buffer, &ctx, events, none, 0.2)?;
        assert_eq!(buffer.cursors().as_slice()[0].head.line, 1);
        assert_eq!(buffer.cursors().as_slice()[0].head.column, 4);
        assert_eq!(buffer.snapshot().revision, 0);
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
    #[test]
    fn ghost_tab_is_one_undo_and_navigation_dismisses() -> Result<(), BufferError> {
        let ctx = egui::Context::default();
        let mut editor = TextEditorView::default();
        let mut buffer = TextBuffer::load_from_str(nozhik_core::BufferId(19), "");
        let mut language = crate::language::EditorLanguage::default();
        completion_frame(&ctx, &mut editor, &mut buffer, &mut language, vec![], 0.0)?;
        editor.ai.pending_ghost_text = Some("print('я');\n    next();".into());
        // The asynchronous reply gets its own paint frame before the user presses Tab.
        let painted =
            completion_frame(&ctx, &mut editor, &mut buffer, &mut language, vec![], 0.05)?;
        assert!(text_position(&painted, "print('я');", 0).is_some());
        assert!(text_position(&painted, "    next();", 0).is_some());
        completion_frame(
            &ctx,
            &mut editor,
            &mut buffer,
            &mut language,
            vec![completion_key(Key::Tab, false)],
            0.1,
        )?;
        assert_eq!(buffer.rope().to_string(), "print('я');\n    next();");
        assert_eq!(
            buffer.cursors().as_slice()[0].head.byte_offset,
            buffer.rope().len_bytes()
        );
        assert!(editor.ai.pending_ghost_text.is_none());
        buffer.undo()?;
        assert_eq!(buffer.rope().to_string(), "");
        completion_frame(&ctx, &mut editor, &mut buffer, &mut language, vec![], 0.2)?;
        for event in [
            completion_key(Key::Escape, false),
            completion_key(Key::ArrowRight, false),
            Event::Text("x".into()),
        ] {
            editor.ai.pending_ghost_text = Some("obsolete".into());
            completion_frame(
                &ctx,
                &mut editor,
                &mut buffer,
                &mut language,
                vec![event],
                0.3,
            )?;
            assert!(editor.ai.pending_ghost_text.is_none());
        }
        Ok(())
    }

    #[test]
    fn ghost_layout_preserves_suffix_and_indent() {
        let original = egui::text::LayoutJob::simple_singleline(
            "    beforeAfter".into(),
            FontId::monospace(14.0),
            theme::TEXT,
        );
        let lines = ["One", "    Two"];
        let first = ghost_job(&original, 10, &lines, 0, 14.0);
        let second = ghost_job(&original, 10, &lines, 1, 14.0);
        assert_eq!(first.text, "    beforeOne");
        assert_eq!(second.text, "    TwoAfter");
        assert_eq!(original.text, "    beforeAfter");
    }

    async fn wait_for_ghost(
        ctx: &egui::Context,
        editor: &mut TextEditorView,
        buffer: &mut TextBuffer,
        language: &mut crate::language::EditorLanguage,
    ) -> Result<egui::FullOutput, Box<dyn std::error::Error>> {
        let mut frame = 0.0;
        Ok(
            tokio::time::timeout(std::time::Duration::from_secs(65), async {
                loop {
                    frame += 0.02;
                    let output = completion_frame(ctx, editor, buffer, language, vec![], frame)?;
                    if editor.ai.pending_ghost_text.is_some() {
                        return Ok::<_, BufferError>(output);
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
            })
            .await??,
        )
    }

    #[tokio::test]
    async fn idle_completion_on_open_and_caret_move_survives_slow_backend()
    -> Result<(), Box<dyn std::error::Error>> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let endpoint = format!("http://{}", listener.local_addr()?);
        let server = tokio::spawn(async move {
            for _ in 0..2 {
                let (mut socket, _) = listener.accept().await?;
                let mut data = [0; 8192];
                assert!(socket.read(&mut data).await? > 0);
                tokio::time::sleep(std::time::Duration::from_millis(1600)).await;
                let body = r#"{"response":"return $a + $b;"}"#;
                socket.write_all(format!("HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}", body.len()).as_bytes()).await?;
            }
            Ok::<_, std::io::Error>(())
        });
        let ctx = egui::Context::default();
        let mut editor = TextEditorView {
            ai: crate::ai_inline::Inline::at_endpoint(&endpoint)?,
            ..Default::default()
        };
        editor.ai.runtime = Some(tokio::runtime::Handle::current());
        editor.ai.configure(&nozhik_config::Config {
            ai_debounce_ms: 50,
            ..Default::default()
        });
        let mut buffer = TextBuffer::load_from_str(BufferId(29), "<?php\n    ");
        let head = buffer.position_at(buffer.rope().len_bytes())?;
        buffer.set_cursors(vec![Cursor { anchor: head, head }])?;
        let revision = buffer.snapshot().revision;
        let mut language = crate::language::EditorLanguage::default();
        wait_for_ghost(&ctx, &mut editor, &mut buffer, &mut language).await?;
        assert_eq!(
            editor.ai.pending_ghost_text.as_deref(),
            Some("return $a + $b;")
        );
        assert_eq!(buffer.snapshot().revision, revision);
        completion_frame(
            &ctx,
            &mut editor,
            &mut buffer,
            &mut language,
            vec![completion_key(Key::ArrowLeft, false)],
            3.0,
        )?;
        assert!(editor.ai.pending_ghost_text.is_none());
        wait_for_ghost(&ctx, &mut editor, &mut buffer, &mut language).await?;
        assert_eq!(buffer.snapshot().revision, revision);
        completion_frame(
            &ctx,
            &mut editor,
            &mut buffer,
            &mut language,
            vec![completion_key(Key::Escape, false)],
            6.0,
        )?;
        completion_frame(&ctx, &mut editor, &mut buffer, &mut language, vec![], 6.1)?;
        assert!(editor.ai.pending_ghost_text.is_none());
        assert!(
            editor.ai.status.is_none(),
            "Esc must suppress idle regeneration at the same caret"
        );
        server.await??;
        Ok(())
    }

    #[tokio::test]
    #[ignore = "Requires a local installed Ollama with qwen2.5-coder:1.5b-base"]
    async fn live_ollama_idle_ghost_renders_and_tab_inserts()
    -> Result<(), Box<dyn std::error::Error>> {
        let ctx = egui::Context::default();
        let mut editor = TextEditorView::default();
        editor.ai.runtime = Some(tokio::runtime::Handle::current());
        editor.ai.configure(&nozhik_config::Config::default());
        let source = "<?php\nfunction add(int $a, int $b): int\n{\n    \n}\n";
        let mut buffer = TextBuffer::load_from_str(BufferId(30), source);
        let at = source.find("    ").ok_or("missing indentation")? + 4;
        let head = buffer.position_at(at)?;
        buffer.set_cursors(vec![Cursor { anchor: head, head }])?;
        let mut language = crate::language::EditorLanguage::default();
        let output = wait_for_ghost(&ctx, &mut editor, &mut buffer, &mut language).await?;
        let text = editor
            .ai
            .pending_ghost_text
            .clone()
            .ok_or("no live ghost")?;
        assert_ne!(text.trim(), "");
        assert!(output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(s) if s.galley.job.text.contains(text.lines().next().unwrap_or("")))));
        assert_eq!(buffer.rope().to_string(), source);
        eprintln!("LIVE OLLAMA IDLE GHOST: {text:?}");
        completion_frame(
            &ctx,
            &mut editor,
            &mut buffer,
            &mut language,
            vec![completion_key(Key::Tab, false)],
            64.0,
        )?;
        assert_eq!(
            buffer.rope().to_string(),
            format!("{}{}{}", &source[..at], text, &source[at..])
        );
        buffer.undo()?;
        assert_eq!(buffer.rope().to_string(), source);
        editor.ai.cancel();
        Ok(())
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
    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "Exercises real hover, press, release and ordinary selection frames together"
    )]
    fn documentation_hover_click_and_release() -> Result<(), Box<dyn std::error::Error>> {
        use nozhik_language_support::doc_provider::DefinitionTarget;
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let mut editor = TextEditorView::default();
        let source = "var_dump($x); unknown(); DateTime::format(); PHP_VERSION;";
        let mut buffer = TextBuffer::load_from_str(BufferId(101), source);
        let mut language = crate::language::EditorLanguage {
            stubs: Some(std::sync::Arc::new(
                nozhik_language_support::php_stubs::PhpVersionStubs::builtin()?,
            )),
            ..Default::default()
        };
        language.documentation = language
            .stubs
            .clone()
            .map(crate::language::DocumentationSource::from);
        let primary = egui::Modifiers {
            mac_cmd: cfg!(target_os = "macos"),
            command: true,
            ctrl: !cfg!(target_os = "macos"),
            ..Default::default()
        };
        let mut frame = |editor: &mut TextEditorView,
                         buffer: &mut TextBuffer,
                         events,
                         modifiers,
                         time|
         -> Result<egui::FullOutput, BufferError> {
            let mut result = Ok(());
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(900.0, 600.0),
                    )),
                    events,
                    modifiers,
                    time: Some(time),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        result =
                            editor.show_language(ui, buffer, 14.0, 4, true, Some(&mut language));
                    });
                },
            );
            result?;
            Ok(output)
        };
        let painted = frame(&mut editor, &mut buffer, vec![], egui::Modifiers::NONE, 0.0)?;
        let pos =
            text_position(&painted, source, 3).ok_or("symbol geometry")? + egui::vec2(2.0, 0.0);
        let before = buffer.cursors().clone();
        for time in [0.1, 0.2] {
            let output = frame(
                &mut editor,
                &mut buffer,
                vec![Event::PointerMoved(pos)],
                primary,
                time,
            )?;
            assert_eq!(
                output.platform_output.cursor_icon,
                egui::CursorIcon::PointingHand
            );
        }
        let popup = frame(&mut editor, &mut buffer, vec![], primary, 0.3)?;
        assert!(text_position(&popup, "(fx) var_dump", 0).is_some());
        frame(
            &mut editor,
            &mut buffer,
            pointer(pos, true, primary),
            primary,
            0.4,
        )?;
        assert!(editor.documentation_target.is_none());
        frame(
            &mut editor,
            &mut buffer,
            pointer(pos, false, primary),
            primary,
            0.5,
        )?;
        assert_eq!(
            editor.documentation_target.take(),
            Some(DefinitionTarget::ExternalUrl(
                "https://www.php.net/var_dump".into()
            ))
        );
        assert_eq!(buffer.cursors(), &before);
        assert_eq!(buffer.rope().to_string(), source);
        let released = frame(&mut editor, &mut buffer, vec![], egui::Modifiers::NONE, 0.6)?;
        assert_ne!(
            released.platform_output.cursor_icon,
            egui::CursorIcon::PointingHand
        );
        assert!(text_position(&released, "(fx) var_dump", 0).is_none());
        let unknown = text_position(&painted, source, 17).ok_or("unknown geometry")?;
        let output = frame(
            &mut editor,
            &mut buffer,
            vec![Event::PointerMoved(unknown)],
            primary,
            0.7,
        )?;
        assert_ne!(
            output.platform_output.cursor_icon,
            egui::CursorIcon::PointingHand
        );
        assert!(text_position(&output, "(fx) var_dump", 0).is_none());
        // A standard click still places the caret on an unknown name.
        frame(
            &mut editor,
            &mut buffer,
            pointer(unknown, true, egui::Modifiers::NONE),
            egui::Modifiers::NONE,
            0.8,
        )?;
        frame(
            &mut editor,
            &mut buffer,
            pointer(unknown, false, egui::Modifiers::NONE),
            egui::Modifiers::NONE,
            0.9,
        )?;
        assert!(buffer.cursors().as_slice()[0].head.byte_offset > 13);
        Ok(())
    }

    #[test]
    fn documentation_token_geometry_and_platform_modifier() -> Result<(), Box<dyn std::error::Error>>
    {
        assert_eq!(
            primary_modifier(egui::Modifiers {
                mac_cmd: true,
                ..Default::default()
            }),
            cfg!(target_os = "macos")
        );
        assert_eq!(
            primary_modifier(egui::Modifiers {
                ctrl: true,
                ..Default::default()
            }),
            !cfg!(target_os = "macos")
        );
        assert!(!primary_modifier(egui::Modifiers::NONE));
        let ctx = egui::Context::default();
        let source = "я \\DateTime::format(); $var_dump; unknown;";
        let mut galley = None;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            galley =
                Some(ctx.fonts(|f| {
                    f.layout_no_wrap(source.into(), FontId::monospace(14.0), theme::TEXT)
                }));
        });
        let galley = galley.ok_or("galley")?;
        let point =
            |column| galley.pos_from_cursor(CCursor::new(column)).min + egui::vec2(2.0, 3.0);
        let range =
            doc_token(source, &galley, egui::Pos2::ZERO, point(6)).ok_or("qualified symbol")?;
        assert_eq!(&source[range], "\\DateTime::format");
        assert!(doc_token(source, &galley, egui::Pos2::ZERO, egui::pos2(-10.0, 3.0)).is_none());
        assert!(
            doc_token(
                source,
                &galley,
                egui::Pos2::ZERO,
                egui::pos2(galley.size().x + 10.0, 3.0)
            )
            .is_none()
        );
        assert!(doc_token(source, &galley, egui::Pos2::ZERO, point(19)).is_none());
        assert!(doc_token(source, &galley, egui::Pos2::ZERO, point(25)).is_none());
        Ok(())
    }
    #[test]
    fn blank_line_delete_keys_snap_then_remove_with_undo() -> Result<(), Box<dyn std::error::Error>>
    {
        let styles = nozhik_config::code_style::CodeStyles::builtin()?;
        let style = styles
            .selected("php", &std::collections::BTreeMap::new())
            .ok_or("style")?;
        for key in [Key::Backspace, Key::Delete] {
            let ctx = egui::Context::default();
            let mut editor = TextEditorView {
                code_style: Some(style.clone()),
                ..Default::default()
            };
            let original = "<?php\nfunction f() {\n    if ($x) {\n            \n    }\n}\n";
            let start = original.find("            \n").ok_or("blank line")?;
            let mut buffer = TextBuffer::load_from_str(BufferId(201), original);
            let head = buffer.position_at(start + 12)?;
            buffer.set_cursors(vec![Cursor { anchor: head, head }])?;
            let mut language = crate::language::EditorLanguage::default();
            completion_frame(
                &ctx,
                &mut editor,
                &mut buffer,
                &mut language,
                vec![completion_key(key, false)],
                0.0,
            )?;
            assert_eq!(
                buffer.rope().to_string(),
                original.replacen("            \n", "        \n", 1)
            );
            assert_eq!(buffer.cursors().as_slice()[0].head.byte_offset, start + 8);
            completion_frame(
                &ctx,
                &mut editor,
                &mut buffer,
                &mut language,
                vec![completion_key(key, false)],
                0.1,
            )?;
            assert_eq!(
                buffer.rope().to_string(),
                original.replacen("            \n", "", 1)
            );
            assert_eq!(buffer.cursors().as_slice()[0].head.byte_offset, start - 1);
            buffer.undo()?;
            assert_eq!(buffer.cursors().as_slice()[0].head.byte_offset, start + 8);
            buffer.undo()?;
            assert_eq!(buffer.rope().to_string(), original);
            assert_eq!(buffer.cursors().as_slice()[0].head, head);
            assert!(!buffer.is_dirty());
        }
        Ok(())
    }

    #[test]
    fn blank_line_deletion_edges_tabs_crlf_and_selections() -> Result<(), Box<dyn std::error::Error>>
    {
        let styles = nozhik_config::code_style::CodeStyles::builtin()?;
        let mut style = styles
            .selected("php", &std::collections::BTreeMap::new())
            .ok_or("style")?
            .clone();
        style.indent.style = nozhik_config::code_style::IndentStyle::Tabs;
        let cases = [
            ("    if ($x) {\n\t\t\t", 3, "    if ($x) {\n\t\t", 2),
            ("    if ($x) {\n\t \t\t", 4, "    if ($x) {\n\t \t", 3),
            (
                "    if ($x) {\r\n   \r\nnext",
                2,
                "    if ($x) {\r\nnext",
                0,
            ),
            ("    if ($x) {\n        \nnext", 4, "    if ($x) {\nnext", 0),
            ("first\n    ", 4, "first\n", 0),
            ("first\n    ", 0, "first", 0),
            ("   \r\nnext", 0, "next", 0),
            ("", 0, "", 0),
        ];
        for (original, column, expected, retained) in cases {
            let mut buffer = TextBuffer::load_from_str(BufferId(202), original);
            let line = usize::from(original.contains("if ($x)") || original.starts_with("first"));
            let start = buffer.rope().line_to_byte(line);
            let head = buffer.position_at(start + column)?;
            buffer.set_cursors(vec![Cursor { anchor: head, head }])?;
            TextEditorView::replace_inner(
                &mut buffer,
                "",
                Some(Key::Backspace),
                Some((Some(&style), 4)),
            )?;
            assert_eq!(buffer.rope().to_string(), expected, "{original:?}");
            if retained > 0 {
                assert_eq!(
                    buffer.cursors().as_slice()[0].head.byte_offset,
                    start + retained
                );
            }
        }
        // An off-grid target inside a tab keeps exactly the needed visual columns.
        style.indent.size = 3;
        let mut buffer = TextBuffer::load_from_str(BufferId(203), " if ($x) {\n\t\t ");
        let end = buffer.rope().len_bytes();
        let head = buffer.position_at(end)?;
        buffer.set_cursors(vec![Cursor { anchor: head, head }])?;
        TextEditorView::replace_inner(&mut buffer, "", Some(Key::Delete), Some((Some(&style), 4)))?;
        assert_eq!(buffer.rope().to_string(), " if ($x) {\n\t ");
        // A selection bypasses blank-line inference, including a partial selection.
        let mut buffer = TextBuffer::load_from_str(BufferId(204), "x\n    \ny");
        buffer.set_cursors(vec![Cursor {
            anchor: buffer.position_at(3)?,
            head: buffer.position_at(5)?,
        }])?;
        TextEditorView::replace_inner(&mut buffer, "", Some(Key::Delete), Some((None, 4)))?;
        assert_eq!(buffer.rope().to_string(), "x\n  \ny");
        // Adjacent blank lines and several carets on one line form one Undo step.
        let original = "я\n    \n  \nz";
        let mut buffer = TextBuffer::load_from_str(BufferId(205), original);
        let cursors = [3, 5, 8]
            .into_iter()
            .map(|byte| {
                buffer
                    .position_at(byte)
                    .map(|head| Cursor { anchor: head, head })
            })
            .collect::<Result<Vec<_>, _>>()?;
        buffer.set_cursors(cursors)?;
        TextEditorView::replace_inner(&mut buffer, "", Some(Key::Backspace), Some((None, 4)))?;
        assert_eq!(buffer.rope().to_string(), "я\nz");
        buffer.undo()?;
        assert_eq!(buffer.rope().to_string(), original);
        Ok(())
    }
}
