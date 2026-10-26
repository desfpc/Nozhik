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
}
impl TerminalView {
    pub fn forget(&mut self, id: TerminalSessionId) {
        self.sizes.remove(&id);
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
        let (rect, response) = ui.allocate_exact_size(
            ui.available_size().max(egui::vec2(cell_width, cell_height)),
            Sense::click(),
        );
        if enabled && (focus || response.clicked()) {
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
            for event in ui.input(|i| i.events.clone()) {
                if let Some(bytes) = encode(
                    &event,
                    modifiers,
                    grid.is_some_and(|g| g.application_cursor),
                    grid.is_some_and(|g| g.bracketed_paste),
                ) && let Err(e) = session.write(bytes)
                {
                    error = Some(e.to_string());
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
