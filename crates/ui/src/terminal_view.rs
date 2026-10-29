//! Focused PTY input encoder and cell-aligned terminal painting.
use crate::theme;
use egui::{Color32, Event, FontId, Key, Modifiers, Sense, Stroke, Ui};
use nozhik_terminal::{
    TerminalSession, TerminalSessionId,
    grid::{TerminalColor, TerminalGrid},
};
use std::collections::HashMap;

#[derive(Default)]
pub struct TerminalView {
    sizes: HashMap<TerminalSessionId, (u16, u16)>,
    scroll: f32,
    selections: HashMap<TerminalSessionId, Selection>,
}
impl TerminalView {
    pub fn forget(&mut self, id: TerminalSessionId) {
        self.sizes.remove(&id);
        self.selections.remove(&id);
    }
    pub fn clipboard_action(
        &mut self,
        ctx: &egui::Context,
        session: &TerminalSession,
        grid: Option<&TerminalGrid>,
        action: &str,
    ) -> bool {
        let id = terminal_id(session.id());
        if !ctx.memory(|m| m.has_focus(id)) {
            return false;
        }
        let selection = self.selections.entry(session.id()).or_default();
        if action == "ui.paste" {
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste);
        } else if let Some(grid) = grid {
            let text = selection.text(grid);
            if !text.is_empty() {
                ctx.copy_text(text);
            }
            if action == "ui.cut"
                && let Some(bytes) = selection.cut_bytes(grid)
            {
                if let Err(error) = session.write(bytes) {
                    eprintln!("Terminal cut: {error}");
                }
                selection.end = selection.anchor;
            }
        }
        true
    }
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::too_many_lines,
        reason = "Clamped finite viewport pixels map onto bounded PTY rows and columns"
    )]
    pub fn show(
        &mut self,
        ui: &mut Ui,
        session: &TerminalSession,
        grid: Option<&TerminalGrid>,
        font_size: f32,
        enabled: bool,
        focus: bool,
    ) -> Option<String> {
        let font = FontId::monospace(font_size);
        let cell_width = ui.fonts(|f| f.glyph_width(&font, 'M')).max(1.0);
        let cell_height = font_size + 4.0;
        let (rect, _) = ui.allocate_exact_size(
            ui.available_size().max(egui::vec2(cell_width, cell_height)),
            Sense::hover(),
        );
        let response = ui.interact(rect, terminal_id(session.id()), Sense::click_and_drag());
        if enabled && (focus || response.clicked() || response.drag_started()) {
            response.request_focus();
        }
        let rows = (rect.height() / cell_height).floor().clamp(1.0, 200.0) as u16;
        let cols = (rect.width() / cell_width).floor().clamp(1.0, 500.0) as u16;
        let mut error = None;
        if self.sizes.get(&session.id()) != Some(&(rows, cols)) {
            match session.resize(rows, cols) {
                Ok(()) => {
                    self.sizes.insert(session.id(), (rows, cols));
                }
                Err(e) => error = Some(e.to_string()),
            }
        }
        ui.painter().rect_filled(rect, 0.0, theme::BACKGROUND);
        if response.hovered() && enabled {
            self.scroll += ui.input(|i| i.raw_scroll_delta.y) / cell_height;
            let delta = self.scroll.trunc() as i32;
            if delta != 0 {
                match session.scroll(delta) {
                    Ok(()) => self.scroll -= self.scroll.trunc(),
                    Err(e) => error = Some(e.to_string()),
                }
            }
        }
        let selection = self.selections.entry(session.id()).or_default();
        if let Some(grid) = grid {
            if selection.geometry != (grid.rows, grid.cols, grid.scrollback) {
                *selection = Selection {
                    geometry: (grid.rows, grid.cols, grid.scrollback),
                    ..Selection::default()
                };
            }
            let position = |point: egui::Pos2| {
                let row = ((point.y - rect.top()) / cell_height)
                    .floor()
                    .clamp(0.0, f32::from(grid.rows.saturating_sub(1)))
                    as u16;
                let col = ((point.x - rect.left()) / cell_width)
                    .round()
                    .clamp(0.0, f32::from(grid.cols)) as u16;
                let col = if grid.cell(row, col).is_some_and(|c| c.continuation) {
                    col.saturating_sub(1)
                } else {
                    col
                };
                (row, col)
            };
            // Native input can deliver a complete drag between two frames. Replay
            // pointer events in order instead of relying on the final button state.
            for event in ui.input(|i| i.events.clone()) {
                match event {
                    Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers,
                    } if enabled
                        && rect.contains(pos)
                        && ui.ctx().layer_id_at(pos) == Some(ui.layer_id()) =>
                    {
                        response.request_focus();
                        let point = position(pos);
                        if !modifiers.shift {
                            selection.anchor = point;
                        }
                        selection.end = point;
                        selection.dragging = true;
                    }
                    Event::PointerMoved(pos) if enabled && selection.dragging => {
                        selection.end = position(pos);
                    }
                    Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        ..
                    } if selection.dragging => {
                        if enabled {
                            selection.end = position(pos);
                        }
                        selection.dragging = false;
                    }
                    Event::PointerGone => selection.dragging = false,
                    _ => {}
                }
            }
            if !enabled {
                selection.dragging = false;
            }
        }
        response.clone().on_hover_cursor(egui::CursorIcon::Text);
        let mut clipboard = None;
        response.context_menu(|ui| {
            if ui
                .add_enabled(
                    enabled && selection.anchor != selection.end,
                    egui::Button::new("Copy"),
                )
                .clicked()
            {
                clipboard = Some("ui.copy");
                ui.close();
            }
            if ui
                .add_enabled(
                    enabled && grid.is_some_and(|g| selection.cut_bytes(g).is_some()),
                    egui::Button::new("Cut"),
                )
                .clicked()
            {
                clipboard = Some("ui.cut");
                ui.close();
            }
            if ui
                .add_enabled(enabled, egui::Button::new("Paste"))
                .clicked()
            {
                clipboard = Some("ui.paste");
                ui.close();
            }
        });
        if let Some(action) = clipboard {
            response.request_focus();
            self.clipboard_action(ui.ctx(), session, grid, action);
        }
        if enabled && response.has_focus() {
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(
                    response.id,
                    egui::EventFilter {
                        tab: true,
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        escape: true,
                    },
                );
            });
            let modifiers = ui.input(|i| i.modifiers);
            let events = ui.input(|i| i.events.clone());
            let clipboard_event = events
                .iter()
                .any(|e| matches!(e, Event::Copy | Event::Cut | Event::Paste(_)));
            for event in events {
                if matches!(&event, Event::Key { key: Key::V, modifiers, .. } if modifiers.command)
                    || (clipboard_event
                        && matches!(&event, Event::Key { key: Key::C | Key::X, modifiers, .. } if modifiers.command))
                {
                    continue;
                }
                let selection = self.selections.entry(session.id()).or_default();
                if matches!(event, Event::Copy | Event::Cut) && selection.anchor != selection.end {
                    ui.ctx()
                        .copy_text(grid.map_or_else(String::new, |g| selection.text(g)));
                    if matches!(event, Event::Cut)
                        && let Some(bytes) = grid.and_then(|g| selection.cut_bytes(g))
                    {
                        if let Err(e) = session.write(bytes) {
                            error = Some(e.to_string());
                        }
                        selection.end = selection.anchor;
                    }
                    continue;
                }
                if let Some(bytes) = encode(
                    &event,
                    modifiers,
                    grid.is_some_and(|g| g.application_cursor),
                    grid.is_some_and(|g| g.bracketed_paste),
                ) && let Err(e) = session.write(bytes)
                {
                    error = Some(e.to_string());
                } else if matches!(
                    event,
                    Event::Text(_) | Event::Paste(_) | Event::Key { pressed: true, .. }
                ) {
                    selection.end = selection.anchor;
                }
            }
        }
        let Some(grid) = grid else {
            ui.painter().text(
                rect.min,
                egui::Align2::LEFT_TOP,
                "Starting shell…",
                font,
                theme::MUTED,
            );
            return error;
        };
        let painter = ui.painter().with_clip_rect(rect);
        for row in 0..grid.rows.min(rows) {
            for col in 0..grid.cols.min(cols) {
                let Some(cell) = grid.cell(row, col) else {
                    continue;
                };
                if cell.continuation {
                    continue;
                }
                let origin = rect.min
                    + egui::vec2(f32::from(col) * cell_width, f32::from(row) * cell_height);
                let cell_rect = egui::Rect::from_min_size(
                    origin,
                    egui::vec2(cell_width * if cell.wide { 2.0 } else { 1.0 }, cell_height),
                );
                let mut fg = color(cell.foreground, theme::TEXT);
                let mut bg = color(cell.background, theme::BACKGROUND);
                if cell.style.inverse {
                    std::mem::swap(&mut fg, &mut bg);
                }
                if cell.style.dim {
                    fg = fg.gamma_multiply(0.65);
                }
                if bg != theme::BACKGROUND {
                    painter.rect_filled(cell_rect, 0.0, bg);
                }
                if self
                    .selections
                    .get(&session.id())
                    .is_some_and(|s| s.contains(row, col))
                {
                    painter.rect_filled(cell_rect, 0.0, theme::ACCENT);
                }
                if !cell.continuation && !cell.text.is_empty() {
                    let mut job = egui::text::LayoutJob::default();
                    job.append(
                        &cell.text,
                        0.0,
                        egui::TextFormat {
                            font_id: font.clone(),
                            color: fg,
                            italics: cell.style.italic,
                            ..Default::default()
                        },
                    );
                    let galley = ui.fonts(|f| f.layout_job(job));
                    painter.galley(origin, galley.clone(), fg);
                    if cell.style.bold {
                        painter.galley(origin + egui::vec2(0.5, 0.0), galley, fg);
                    }
                }
                if cell.style.underline {
                    painter.line_segment(
                        [
                            cell_rect.left_bottom() - egui::vec2(0.0, 2.0),
                            cell_rect.right_bottom() - egui::vec2(0.0, 2.0),
                        ],
                        Stroke::new(1.0_f32, fg),
                    );
                }
            }
        }
        if grid.cursor_visible && grid.cursor.0 < rows && grid.cursor.1 < cols {
            let origin = rect.min
                + egui::vec2(
                    f32::from(grid.cursor.1) * cell_width,
                    f32::from(grid.cursor.0) * cell_height,
                );
            let caret = egui::Rect::from_min_size(origin, egui::vec2(cell_width, cell_height));
            if enabled && response.has_focus() {
                painter.rect_filled(caret, 0.0, theme::CARET);
            } else {
                painter.rect_stroke(
                    caret,
                    0.0,
                    Stroke::new(1.0_f32, theme::MUTED),
                    egui::StrokeKind::Inside,
                );
            }
        }
        error
    }
}
fn terminal_id(id: TerminalSessionId) -> egui::Id {
    egui::Id::new(("terminal_input", id.0))
}

#[derive(Default)]
struct Selection {
    anchor: (u16, u16),
    end: (u16, u16),
    geometry: (u16, u16, usize),
    dragging: bool,
}
impl Selection {
    fn bounds(&self) -> ((u16, u16), (u16, u16)) {
        (self.anchor.min(self.end), self.anchor.max(self.end))
    }
    fn contains(&self, row: u16, col: u16) -> bool {
        let (start, end) = self.bounds();
        (row, col) >= start && (row, col) < end
    }
    fn text(&self, grid: &TerminalGrid) -> String {
        let (start, end) = self.bounds();
        if start == end {
            return String::new();
        }
        let mut lines = Vec::new();
        for row in start.0..=end.0.min(grid.rows.saturating_sub(1)) {
            let mut text = String::new();
            for col in 0..grid.cols {
                if self.contains(row, col)
                    && let Some(cell) = grid.cell(row, col)
                    && !cell.continuation
                {
                    if cell.text.is_empty() {
                        text.push(' ');
                    } else {
                        text.push_str(&cell.text);
                    }
                }
            }
            lines.push(text.trim_end().to_owned());
        }
        lines.join("\n")
    }
    // Only the live cursor row can be edited through a shell's line editor.
    // History and TUI output are immutable; Cut there remains Copy.
    fn cut_bytes(&self, grid: &TerminalGrid) -> Option<Vec<u8>> {
        let (start, end) = self.bounds();
        if start == end
            || grid.alternate
            || grid.scrollback != 0
            || start.0 != end.0
            || end.0 != grid.cursor.0
            || end.1 > grid.cursor.1
        {
            return None;
        }
        let glyphs = |from, to| {
            (from..to)
                .filter(|col| grid.cell(end.0, *col).is_some_and(|c| !c.continuation))
                .count()
        };
        let mut bytes = b"\x1b[D".repeat(glyphs(end.1, grid.cursor.1));
        bytes.extend(vec![127; glyphs(start.1, end.1)]);
        Some(bytes)
    }
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "The ANSI cube and grayscale formulas are bounded to 0..=255"
)]
fn color(color: TerminalColor, default: Color32) -> Color32 {
    let palette = theme::ANSI;
    match color {
        TerminalColor::Default => default,
        TerminalColor::Rgb(r, g, b) => Color32::from_rgb(r, g, b),
        TerminalColor::Indexed(i @ 0..=15) => {
            let (r, g, b) = palette[usize::from(i)];
            Color32::from_rgb(r, g, b)
        }
        TerminalColor::Indexed(i @ 16..=231) => {
            let i = u16::from(i) - 16;
            let channel = |c| if c == 0 { 0 } else { (55 + 40 * c) as u8 };
            Color32::from_rgb(channel(i / 36), channel((i / 6) % 6), channel(i % 6))
        }
        TerminalColor::Indexed(i) => {
            let c = 8 + (i - 232) * 10;
            Color32::from_gray(c)
        }
    }
}
fn encode(
    event: &Event,
    current: Modifiers,
    application: bool,
    bracketed: bool,
) -> Option<Vec<u8>> {
    match event {
        Event::Copy if !cfg!(target_os = "macos") || (current.ctrl && !current.mac_cmd) => {
            Some(vec![3])
        }
        Event::Cut if !cfg!(target_os = "macos") || (current.ctrl && !current.mac_cmd) => {
            Some(vec![24])
        }
        Event::Text(text) if !current.ctrl && !current.mac_cmd => Some(text.as_bytes().to_vec()),
        Event::Ime(egui::ImeEvent::Commit(text)) => Some(text.as_bytes().to_vec()),
        Event::Paste(text) => {
            let text = text.replace("\r\n", "\n");
            Some(if bracketed {
                format!("\x1b[200~{text}\x1b[201~").into_bytes()
            } else {
                text.replace('\n', "\r").into_bytes()
            })
        }
        Event::Key {
            key,
            pressed: true,
            modifiers,
            ..
        } => key_bytes(*key, *modifiers, application),
        _ => None,
    }
}
fn key_bytes(key: Key, m: Modifiers, application: bool) -> Option<Vec<u8>> {
    if m.mac_cmd {
        return None;
    }
    if m.ctrl {
        let name = key.name();
        if name.len() == 1 && name.as_bytes()[0].is_ascii_uppercase() {
            return Some(vec![name.as_bytes()[0] - b'A' + 1]);
        }
        let byte = match key {
            Key::Space | Key::Num2 => Some(0),
            Key::OpenBracket => Some(27),
            Key::Backslash => Some(28),
            Key::CloseBracket => Some(29),
            Key::Num6 => Some(30),
            Key::Minus => Some(31),
            _ => None,
        };
        if let Some(byte) = byte {
            return Some(vec![byte]);
        }
    }
    let modifier = 1 + u8::from(m.shift) + 2 * u8::from(m.alt) + 4 * u8::from(m.ctrl);
    let cursor = |letter| {
        if modifier > 1 {
            format!("\x1b[1;{modifier}{letter}")
        } else {
            format!("\x1b{}{letter}", if application { "O" } else { "[" })
        }
    };
    let tilde = |number| {
        if modifier > 1 {
            format!("\x1b[{number};{modifier}~")
        } else {
            format!("\x1b[{number}~")
        }
    };
    let mut text = match key {
        Key::Enter => "\r".into(),
        Key::Backspace => "\x7f".into(),
        Key::Escape => "\x1b".into(),
        Key::Tab => if m.shift { "\x1b[Z" } else { "\t" }.into(),
        Key::ArrowUp => cursor('A'),
        Key::ArrowDown => cursor('B'),
        Key::ArrowRight => cursor('C'),
        Key::ArrowLeft => cursor('D'),
        Key::Home => cursor('H'),
        Key::End => cursor('F'),
        Key::Insert => tilde(2),
        Key::Delete => tilde(3),
        Key::PageUp => tilde(5),
        Key::PageDown => tilde(6),
        Key::F1 | Key::F2 | Key::F3 | Key::F4 => {
            let letter = match key {
                Key::F1 => 'P',
                Key::F2 => 'Q',
                Key::F3 => 'R',
                _ => 'S',
            };
            if modifier > 1 {
                format!("\x1b[1;{modifier}{letter}")
            } else {
                format!("\x1bO{letter}")
            }
        }
        Key::F5 => tilde(15),
        Key::F6 => tilde(17),
        Key::F7 => tilde(18),
        Key::F8 => tilde(19),
        Key::F9 => tilde(20),
        Key::F10 => tilde(21),
        Key::F11 => tilde(23),
        Key::F12 => tilde(24),
        _ => return None,
    };
    if m.alt && matches!(key, Key::Enter | Key::Backspace) {
        text.insert(0, '\x1b');
    }
    Some(text.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unicode_selection_reverse_ranges_and_live_line_cut() {
        let mut emulator = nozhik_terminal::grid::Emulator::new(4, 30);
        emulator.process("old output\r\n$ Ж🦀abc".as_bytes());
        let grid = emulator.snapshot();
        let selection = Selection {
            anchor: (1, 6),
            end: (1, 2),
            ..Selection::default()
        };
        assert_eq!(selection.text(&grid), "Ж🦀a");
        assert_eq!(
            selection.cut_bytes(&grid),
            Some([b"\x1b[D".repeat(2), vec![127; 3]].concat())
        );
        let history = Selection {
            anchor: (0, 0),
            end: (0, 3),
            ..Selection::default()
        };
        assert_eq!(history.text(&grid), "old");
        assert!(history.cut_bytes(&grid).is_none());
        let multi = Selection {
            anchor: (1, 3),
            end: (0, 4),
            ..Selection::default()
        };
        assert_eq!(multi.text(&grid), "output\n$ Ж");
        let mut scrolled = grid;
        scrolled.scrollback = 1;
        assert!(selection.cut_bytes(&scrolled).is_none());
        scrolled.scrollback = 0;
        scrolled.alternate = true;
        assert!(selection.cut_bytes(&scrolled).is_none());
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "End-to-end pointer and clipboard regression with an isolated byte transport"
    )]
    fn pointer_selection_clipboard_focus_and_exact_paste() -> Result<(), Box<dyn std::error::Error>>
    {
        use nozhik_terminal::{
            TerminalManager,
            external::{ExternalOutput, ExternalTransport},
            pty::SessionOptions,
        };
        use std::{
            sync::{Arc, mpsc},
            time::Duration,
        };
        struct Transport(mpsc::Sender<Vec<u8>>);
        impl ExternalTransport for Transport {
            fn poll(&mut self) -> nozhik_terminal::Result<Option<ExternalOutput>> {
                Ok(None)
            }
            fn write(&mut self, bytes: Vec<u8>) -> nozhik_terminal::Result<()> {
                self.0
                    .send(bytes)
                    .map_err(|_| nozhik_terminal::TerminalError::Closed)
            }
            fn resize(&mut self, _: u16, _: u16) -> nozhik_terminal::Result<()> {
                Ok(())
            }
            fn close(&mut self) {}
        }
        let (sender, writes) = mpsc::channel();
        let mut manager = TerminalManager::new(Arc::new(|| {}));
        let id = manager.create_external(
            SessionOptions::new(std::path::PathBuf::new()),
            Box::new(Transport(sender)),
        )?;
        let session = manager.session(id).ok_or("missing session")?;
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let mut view = TerminalView::default();
        let mut emulator = nozhik_terminal::grid::Emulator::new(4, 30);
        emulator.process(b"hello world\r\n$ input\x1b[?2004h");
        let grid = emulator.snapshot();
        let mut rect = egui::Rect::NOTHING;
        let mut cell = 1.0;
        let mut frame = |view: &mut TerminalView, events: Vec<Event>, enabled| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(600.0, 250.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        rect = ui.available_rect_before_wrap();
                        cell = ui.fonts(|f| f.glyph_width(&FontId::monospace(14.0), 'M'));
                        assert!(
                            view.show(ui, session, Some(&grid), 14.0, enabled, false)
                                .is_none()
                        );
                    });
                },
            )
        };
        // Obtain geometry separately to keep the frame closure's mutable captures local.
        frame(&mut view, vec![], true);

        let start = rect.min + egui::vec2(0.1, 8.0);
        let end = start + egui::vec2(cell * 5.0, 0.0);
        let render = |view: &mut TerminalView, events: Vec<Event>, enabled| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(600.0, 250.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        assert!(
                            view.show(ui, session, Some(&grid), 14.0, enabled, false)
                                .is_none()
                        );
                    });
                },
            )
        };
        let pointer = |pos, pressed| Event::PointerButton {
            pos,
            pressed,
            button: egui::PointerButton::Primary,
            modifiers: Modifiers::NONE,
        };
        render(
            &mut view,
            vec![Event::PointerMoved(start), pointer(start, true)],
            true,
        );
        render(&mut view, vec![Event::PointerMoved(end)], true);
        render(&mut view, vec![pointer(end, false)], true);
        assert_eq!(
            view.selections
                .get(&id)
                .ok_or("missing selection")?
                .text(&grid),
            "hello"
        );
        render(
            &mut view,
            vec![
                Event::PointerMoved(start + egui::vec2(cell * 6.0, 0.0)),
                pointer(start + egui::vec2(cell * 6.0, 0.0), true),
                Event::PointerMoved(start + egui::vec2(cell * 11.0, 0.0)),
                pointer(start + egui::vec2(cell * 11.0, 0.0), false),
            ],
            true,
        );
        assert_eq!(
            view.selections
                .get(&id)
                .ok_or("missing fast selection")?
                .text(&grid),
            "world"
        );
        let copied = render(&mut view, vec![Event::Copy], true);
        assert!(
            copied
                .platform_output
                .commands
                .iter()
                .any(|c| matches!(c, egui::OutputCommand::CopyText(text) if text == "world"))
        );
        assert!(writes.try_recv().is_err());
        render(&mut view, vec![Event::Paste("Ж\r\n🦀".into())], true);
        assert_eq!(
            writes.recv_timeout(Duration::from_secs(2))?,
            "\x1b[200~Ж\n🦀\x1b[201~".as_bytes()
        );
        let input_start = start + egui::vec2(cell * 2.0, 18.0);
        let input_end = start + egui::vec2(cell * 7.0, 18.0);
        render(
            &mut view,
            vec![
                Event::PointerMoved(input_start),
                pointer(input_start, true),
                Event::PointerMoved(input_end),
                pointer(input_end, false),
            ],
            true,
        );
        let cut = render(&mut view, vec![Event::Cut], true);
        assert!(
            cut.platform_output
                .commands
                .iter()
                .any(|c| matches!(c, egui::OutputCommand::CopyText(text) if text == "input"))
        );
        assert_eq!(writes.recv_timeout(Duration::from_secs(2))?, vec![127; 5]);
        assert!(view.clipboard_action(&ctx, session, Some(&grid), "ui.paste"));
        render(&mut view, vec![Event::Paste("blocked".into())], false);
        ctx.memory_mut(|m| m.surrender_focus(terminal_id(id)));
        render(&mut view, vec![Event::Paste("unfocused".into())], true);
        assert!(writes.recv_timeout(Duration::from_millis(50)).is_err());
        manager.shutdown();
        Ok(())
    }

    #[test]
    fn control_keys_cursor_modes_function_keys_and_paste() {
        for (key, expected) in [
            (Key::Enter, b"\r".as_slice()),
            (Key::Backspace, b"\x7f"),
            (Key::Tab, b"\t"),
            (Key::Escape, b"\x1b"),
            (Key::ArrowUp, b"\x1b[A"),
            (Key::F12, b"\x1b[24~"),
        ] {
            assert_eq!(
                key_bytes(key, Modifiers::NONE, false).as_deref(),
                Some(expected)
            );
        }
        for (key, byte) in [(Key::C, 3), (Key::D, 4), (Key::Z, 26), (Key::L, 12)] {
            assert_eq!(key_bytes(key, Modifiers::CTRL, false), Some(vec![byte]));
        }
        assert_eq!(
            encode(&Event::Copy, Modifiers::CTRL, false, false),
            Some(vec![3])
        );
        assert_eq!(
            key_bytes(Key::ArrowUp, Modifiers::NONE, true),
            Some(b"\x1bOA".to_vec())
        );
        assert_eq!(
            key_bytes(Key::ArrowRight, Modifiers::CTRL, false),
            Some(b"\x1b[1;5C".to_vec())
        );
        assert_eq!(
            key_bytes(Key::Tab, Modifiers::SHIFT, false),
            Some(b"\x1b[Z".to_vec())
        );
        assert_eq!(
            encode(&Event::Text("Ж🦀".into()), Modifiers::NONE, false, false),
            Some("Ж🦀".as_bytes().to_vec())
        );
        assert_eq!(
            encode(
                &Event::Paste("one\r\ntwo".into()),
                Modifiers::NONE,
                false,
                false
            ),
            Some(b"one\rtwo".to_vec())
        );
        assert_eq!(
            encode(
                &Event::Paste("one\ntwo".into()),
                Modifiers::NONE,
                false,
                true
            ),
            Some(b"\x1b[200~one\ntwo\x1b[201~".to_vec())
        );
    }
}
