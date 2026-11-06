//! Native mouse capture and local history scrollbar for terminal cells.
use egui::{Event, Modifiers, PointerButton, Rect, Ui};
use nozhik_terminal::{
    TerminalSession,
    grid::{MouseProtocolMode, TerminalGrid},
    mouse::{self, MouseAction, MouseModifiers},
};

#[derive(Default)]
pub(super) struct MouseState {
    pub pressed: Option<PointerButton>,
    pub last_cell: Option<(u16, u16)>,
}
const fn button_code(button: PointerButton) -> Option<u8> {
    match button {
        PointerButton::Primary => Some(0),
        PointerButton::Middle => Some(1),
        PointerButton::Secondary => Some(2),
        _ => None,
    }
}
pub(super) fn mouse_bytes(
    grid: &TerminalGrid,
    action: MouseAction,
    row: u16,
    col: u16,
    modifiers: Modifiers,
) -> Option<Vec<u8>> {
    mouse::encode(
        grid.mouse_mode,
        grid.mouse_encoding,
        action,
        row,
        col,
        MouseModifiers {
            shift: modifiers.shift,
            alt: modifiers.alt,
            ctrl: modifiers.ctrl,
        },
    )
}
impl MouseState {
    #[allow(
        clippy::too_many_arguments,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "Ordered native pointer events use clamped cell coordinates and terminal geometry"
    )]
    pub fn route(
        &mut self,
        ui: &Ui,
        session: &TerminalSession,
        grid: &TerminalGrid,
        rect: Rect,
        width: f32,
        height: f32,
        enabled: bool,
        events: &[Event],
    ) -> Option<String> {
        if !enabled || grid.mouse_mode == MouseProtocolMode::None {
            self.pressed = None;
            self.last_cell = None;
            return None;
        }
        let position = |pos: egui::Pos2| {
            (
                ((pos.y - rect.top()) / height)
                    .floor()
                    .clamp(0.0, f32::from(grid.rows.saturating_sub(1))) as u16,
                ((pos.x - rect.left()) / width)
                    .floor()
                    .clamp(0.0, f32::from(grid.cols.saturating_sub(1))) as u16,
            )
        };
        let inside = |pos| rect.contains(pos) && ui.ctx().layer_id_at(pos) == Some(ui.layer_id());
        let mut bytes = Vec::new();
        for event in events {
            let routed = match event {
                Event::PointerButton {
                    pos,
                    button,
                    pressed: true,
                    modifiers,
                } if inside(*pos) && !modifiers.shift => {
                    let action = button_code(*button).map(MouseAction::Press);
                    if action.is_some() {
                        self.pressed = Some(*button);
                    }
                    ui.memory_mut(|m| m.request_focus(super::terminal_id(session.id())));
                    action.map(|a| (a, position(*pos), *modifiers))
                }
                Event::PointerButton {
                    pos,
                    button,
                    pressed: false,
                    modifiers,
                } if self.pressed == Some(*button) => {
                    self.pressed = None;
                    button_code(*button)
                        .map(|b| (MouseAction::Release(b), position(*pos), *modifiers))
                }
                Event::PointerMoved(pos)
                    if (inside(*pos) || self.pressed.is_some())
                        && !ui.input(|i| i.modifiers.shift) =>
                {
                    let cell = position(*pos);
                    let moved = self.last_cell != Some(cell);
                    self.last_cell = Some(cell);
                    if moved {
                        Some((
                            MouseAction::Motion(self.pressed.and_then(button_code)),
                            cell,
                            ui.input(|i| i.modifiers),
                        ))
                    } else {
                        None
                    }
                }
                Event::PointerGone => {
                    let action = self
                        .pressed
                        .take()
                        .and_then(button_code)
                        .map(MouseAction::Release);
                    action.map(|a| {
                        (
                            a,
                            self.last_cell.unwrap_or((0, 0)),
                            ui.input(|i| i.modifiers),
                        )
                    })
                }
                _ => None,
            };
            if let Some((action, (row, col), modifiers)) = routed {
                self.last_cell = Some((row, col));
                if let Some(encoded) = mouse_bytes(grid, action, row, col, modifiers) {
                    bytes.extend(encoded);
                }
            }
        }
        if bytes.is_empty() {
            None
        } else {
            session.write_mouse(bytes).err().map(|e| e.to_string())
        }
    }
}

#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "History is bounded to 3000 rows and scrollbar ratios are clamped to 0..=1"
)]
pub(super) fn scrollbar(
    ui: &Ui,
    session: &TerminalSession,
    grid: &TerminalGrid,
    whole: Rect,
    enabled: bool,
) -> Option<String> {
    let track = Rect::from_min_max(egui::pos2(whole.right() - 10.0, whole.top()), whole.max);
    if grid.history_len == 0 {
        return None;
    }
    let response = ui.interact(
        track,
        super::terminal_id(session.id()).with("history_scroll"),
        egui::Sense::click_and_drag(),
    );
    ui.painter().rect_filled(track, 3.0, crate::theme::panel());
    let height = (track.height() * f32::from(grid.rows)
        / (grid.history_len as f32 + f32::from(grid.rows)))
    .clamp(18.0_f32.min(track.height()), track.height());
    let space = (track.height() - height).max(1.0);
    let top = space.mul_add(
        1.0 - grid.scrollback as f32 / grid.history_len as f32,
        track.top(),
    );
    ui.painter().rect_filled(
        Rect::from_min_size(
            egui::pos2(track.left(), top),
            egui::vec2(track.width(), height),
        ),
        3.0,
        if response.hovered() || response.dragged() {
            crate::theme::text()
        } else {
            crate::theme::muted()
        },
    );
    if enabled
        && (response.dragged() || response.clicked())
        && let Some(pos) = response.interact_pointer_pos()
    {
        let ratio = ((pos.y - track.top() - height / 2.0) / space).clamp(0.0, 1.0);
        let offset = ((1.0 - ratio) * grid.history_len as f32).round() as usize;
        return session.scroll_to(offset).err().map(|e| e.to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
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
    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "Real pointer events verify native capture, wheel, selection override and modal gating together"
    )]
    fn native_click_drag_wheel_shift_selection_and_disabled_input()
    -> Result<(), Box<dyn std::error::Error>> {
        let (sender, writes) = mpsc::channel();
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let mut manager = TerminalManager::new(Arc::new(|| {}));
        let id = manager.create_external(
            SessionOptions::new(std::path::PathBuf::new()),
            Box::new(Transport(sender)),
        )?;
        let session = manager.session(id).ok_or("missing session")?;
        let mut emulator = nozhik_terminal::grid::Emulator::new(20, 80);
        emulator.process(b"\x1b[?1002h\x1b[?1006hhello world");
        let grid = emulator.snapshot();
        let mut view = super::super::TerminalView::default();
        let origin = std::cell::Cell::new(egui::Pos2::ZERO);
        let width = std::cell::Cell::new(1.0);
        let frame = |view: &mut super::super::TerminalView, events: Vec<Event>, enabled| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(700.0, 400.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        origin.set(ui.available_rect_before_wrap().min);
                        width.set(
                            ui.fonts(|f| f.glyph_width(&super::super::terminal_font(14.0), 'M')),
                        );
                        assert!(
                            view.show(ui, session, Some(&grid), 14.0, enabled, false)
                                .is_none()
                        );
                    });
                },
            )
        };
        frame(&mut view, vec![], true);
        let point = origin.get() + egui::vec2(width.get() * 9.5, 27.0);
        let pointer = |pos, pressed, modifiers| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers,
        };
        frame(
            &mut view,
            vec![
                Event::PointerMoved(point),
                pointer(point, true, Modifiers::NONE),
                pointer(point, false, Modifiers::NONE),
            ],
            true,
        );
        assert_eq!(
            writes.recv_timeout(Duration::from_secs(2))?,
            b"\x1b[<0;10;2M\x1b[<0;10;2m"
        );
        frame(
            &mut view,
            vec![Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, 18.0),
                modifiers: Modifiers::NONE,
            }],
            true,
        );
        assert_eq!(
            writes.recv_timeout(Duration::from_secs(2))?,
            b"\x1b[<64;10;2M"
        );
        frame(
            &mut view,
            vec![
                pointer(point, true, Modifiers::NONE),
                Event::PointerMoved(point + egui::vec2(width.get(), 0.0)),
                pointer(point + egui::vec2(width.get(), 0.0), false, Modifiers::NONE),
            ],
            true,
        );
        assert_eq!(
            writes.recv_timeout(Duration::from_secs(2))?,
            b"\x1b[<0;10;2M\x1b[<32;11;2M\x1b[<0;11;2m"
        );
        frame(
            &mut view,
            vec![
                pointer(point, true, Modifiers::NONE),
                pointer(point, false, Modifiers::NONE),
            ],
            false,
        );
        assert!(writes.recv_timeout(Duration::from_millis(50)).is_err());
        let first = point - egui::vec2(width.get() * 9.4, 18.0);
        let last = first + egui::vec2(width.get() * 5.0, 0.0);
        frame(
            &mut view,
            vec![
                Event::PointerMoved(first),
                pointer(first, true, Modifiers::SHIFT),
                Event::PointerMoved(last),
                pointer(last, false, Modifiers::SHIFT),
            ],
            true,
        );
        assert!(writes.recv_timeout(Duration::from_millis(50)).is_err());
        let output = frame(&mut view, vec![Event::Copy], true);
        assert!(
            output
                .platform_output
                .commands
                .iter()
                .any(|c| matches!(c,egui::OutputCommand::CopyText(text) if text=="hello"))
        );
        manager.shutdown();
        Ok(())
    }
}
