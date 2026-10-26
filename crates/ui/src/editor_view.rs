//! Virtualized Rope editor. Only visible lines are converted to strings/galleys.
use crate::theme;
use egui::{Event, FontId, Key, Sense, Stroke, Ui, text::CCursor};
use nozhik_buffer::{BufferError, Cursor, EditTransaction, TextBuffer, TextEdit};

/// Stateless view; caret, selections and history live in the buffer.
#[derive(Default)]
pub struct TextEditorView {
    /// Pending navigation target; scrolled into view on the next frame.
    pub reveal_line: Option<usize>,
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
        let response = ui.interact(ui.available_rect_before_wrap(), id, Sense::click());
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
                                continue;
                            }
                            Key::ArrowUp => {
                                state.selected = state
                                    .selected
                                    .checked_sub(1)
                                    .unwrap_or(state.completions.len() - 1);
                                continue;
                            }
                            Key::Escape => {
                                state.completions.clear();
                                state.token = 0;
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
                let trigger = matches!(&event, Event::Text(s) if s.chars().last().is_some_and(|c| c.is_alphanumeric() || matches!(c,'_'|'$'|'>'|':'|'\\')));
                Self::input(buffer, event, tab_width)?;
                if let Some(state) = language.as_deref_mut() {
                    state.completions.clear();
                    state.request = None;
                    state.request_at = Some(std::time::Instant::now());
                    if trigger {
                        state.request = Some((
                            nozhik_language_support::RequestKind::Completion,
                            buffer.cursors().as_slice()[0].head.byte_offset,
                        ));
                    }
                }
            }
        }
        ui.spacing_mut().item_spacing.y = 0.0;
        let mut click = None;
        let cursors = buffer.cursors().as_slice().to_vec();
        let revision = buffer.snapshot().revision;
        if let Some(state) = language.as_deref_mut()
            && state.revision != revision
        {
            state.revision = revision;
            state.spans.clear();
            state.diagnostics.clear();
            state.hover = None;
        }
        let reveal = self.reveal_line.take();
        let mut popup_origin = None;
        let mut hovered_byte = None;
        let mut scroll = egui::ScrollArea::both();
        if let Some(line) = reveal {
            scroll = scroll.vertical_scroll_offset(line as f32 * (font_size + 7.0));
        }
        scroll.id_salt(id).auto_shrink([false, false]).show_rows(
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
                        let color = span.map_or(theme::TEXT, |s| theme::syntax_color(&s.token));
                        job.append(
                            &text[range[0]..range[1]],
                            0.0,
                            egui::TextFormat {
                                font_id: FontId::monospace(font_size),
                                color,
                                italics: span.is_some_and(|s| s.token == "comment"),
                                ..Default::default()
                            },
                        );
                    }
                    let galley = ui.fonts(|fonts| fonts.layout_job(job));
                    let width = ui.available_width().max(galley.size().x + 90.0);
                    let (rect, row) =
                        ui.allocate_exact_size(egui::vec2(width, font_size + 7.0), Sense::click());
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
                        for diagnostic in &state.diagnostics {
                            if line < diagnostic.range.start.line as usize
                                || line > diagnostic.range.end.line as usize
                            {
                                continue;
                            }
                            let color = if diagnostic.severity
                                == Some(
                                    nozhik_language_support::lsp_types::DiagnosticSeverity::WARNING,
                                ) {
                                egui::Color32::YELLOW
                            } else {
                                egui::Color32::LIGHT_RED
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
                    if enabled && row.clicked() {
                        row.request_focus();
                        ui.memory_mut(|memory| memory.request_focus(id));
                        if let Some(pos) = row.interact_pointer_pos() {
                            let index = galley.cursor_from_pos(pos - origin).index;
                            let byte = text
                                .char_indices()
                                .nth(index)
                                .map_or(text.len(), |(byte, _)| byte);
                            if ui.input(|i| i.modifiers.command) {
                                if let Some(state) = language.as_deref_mut() {
                                    state.request_at = None;
                                    state.request = Some((
                                        nozhik_language_support::RequestKind::Definition,
                                        start + byte,
                                    ));
                                }
                            } else {
                                click = Some((start + byte, ui.input(|i| i.modifiers.shift)));
                            }
                        }
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
                let mut accept = None;
                egui::Area::new(id.with("completion")).order(egui::Order::Foreground).fixed_pos(origin).show(ui.ctx(),|ui| {
                    egui::Frame::popup(ui.style()).show(ui,|ui| {
                        ui.set_max_width(650.0);
                        egui::ScrollArea::vertical().max_height(230.0).show(ui,|ui| {
                            for (i,item) in state.completions.iter().enumerate() {
                                let icon = match item.kind { Some(nozhik_language_support::lsp_types::CompletionItemKind::METHOD | nozhik_language_support::lsp_types::CompletionItemKind::FUNCTION) => "ƒ", Some(nozhik_language_support::lsp_types::CompletionItemKind::CLASS) => "C", Some(nozhik_language_support::lsp_types::CompletionItemKind::PROPERTY) => "P", _ => "K" };
                                ui.horizontal(|ui| {
                                    if ui.selectable_label(i==state.selected,format!("{icon}  {}",item.label)).clicked() { accept = Some(i); }
                                    ui.weak(item.detail.as_deref().unwrap_or(""));
                                });
                            }
                        });
                    });
                });
                if let Some(i) = accept {
                    state.selected = i;
                    Self::accept_completion(buffer, state)?;
                    response.request_focus();
                }
            }
        }
        if let Some((byte, extend)) = click {
            if let Some(state) = language {
                state.completions.clear();
                state.token = 0;
                state.request = None;
            }
            let head = buffer.position_at(byte)?;
            let anchor = if extend {
                buffer.cursors().as_slice()[0].anchor
            } else {
                head
            };
            buffer.set_cursors(vec![Cursor { anchor, head }])?;
        }
        Ok(())
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
                buffer.apply_transaction(EditTransaction {
                    edits: edits
                        .into_iter()
                        .map(|(range, text)| TextEdit { range, text })
                        .collect(),
                    group: None,
                })?;
            }
        }
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

    fn replace(
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
