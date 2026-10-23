//! Virtualized Rope editor. Only visible lines are converted to strings/galleys.
use crate::theme;
use egui::{Event, FontId, Key, Sense, Stroke, Ui, text::CCursor};
use nozhik_buffer::{BufferError, Cursor, EditTransaction, TextBuffer, TextEdit};

/// Stateless view; caret, selections and history live in the buffer.
pub struct TextEditorView;

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
        let id = ui.id().with(("editor", buffer.snapshot().buffer_id));
        let response = ui.interact(ui.available_rect_before_wrap(), id, Sense::click());
        if response.clicked() && enabled {
            response.request_focus();
        }
        if enabled && response.has_focus() {
            for event in ui.input(|input| input.events.clone()) {
                Self::input(buffer, event, tab_width)?;
            }
        }
        ui.spacing_mut().item_spacing.y = 0.0;
        let mut click = None;
        let cursors = buffer.cursors().as_slice().to_vec();
        egui::ScrollArea::both()
            .id_salt(id)
            .auto_shrink([false, false])
            .show_rows(
                ui,
                font_size + 7.0,
                buffer.rope().len_lines(),
                |ui, rows| {
                    for line in rows {
                        let source = buffer.rope().line(line).to_string();
                        let text = source.trim_end_matches('\n').trim_end_matches('\r');
                        let galley = ui.painter().layout_no_wrap(
                            text.into(),
                            FontId::monospace(font_size),
                            theme::TEXT,
                        );
                        let width = ui.available_width().max(galley.size().x + 90.0);
                        let (rect, row) = ui.allocate_exact_size(
                            egui::vec2(width, font_size + 7.0),
                            Sense::click(),
                        );
                        let origin = rect.min + egui::vec2(66.0, 3.0);
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
                                let caret = galley
                                    .pos_from_cursor(CCursor::new(text[..local].chars().count()));
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
                                click = Some((start + byte, ui.input(|i| i.modifiers.shift)));
                            }
                        }
                        ui.painter().galley(origin, galley, theme::TEXT);
                    }
                },
            );
        if let Some((byte, extend)) = click {
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

#[cfg(test)]
mod tests {
    use super::*;
    use nozhik_core::BufferId;
    use nozhik_event_bus::{BufferEvent, Event as BusEvent, EventBus};

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
