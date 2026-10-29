//! Bottom terminal tabs, session ownership and nonblocking screen presentation.
use crate::terminal_view::TerminalView;
use nozhik_terminal::{
    SessionSnapshot, SessionState, TerminalEvent, TerminalManager, TerminalSessionId,
    pty::SessionOptions,
};
use std::{collections::HashMap, path::Path};

pub const TERMINAL_LABEL: &str = "⌘ Terminal";
pub struct TerminalPanel {
    manager: TerminalManager,
    active: Option<TerminalSessionId>,
    snapshots: HashMap<TerminalSessionId, SessionSnapshot>,
    view: TerminalView,
    was_visible: bool,
    shown_active: Option<TerminalSessionId>,
    labels: HashMap<TerminalSessionId, String>,
    pub focus: bool,
    #[cfg(test)]
    actions_rect: Option<egui::Rect>,
}
impl TerminalPanel {
    pub fn new(ctx: &egui::Context) -> Self {
        let ctx = ctx.clone();
        Self {
            manager: TerminalManager::new(std::sync::Arc::new(move || ctx.request_repaint())),
            active: None,
            snapshots: HashMap::new(),
            view: TerminalView::default(),
            was_visible: false,
            shown_active: None,
            labels: HashMap::new(),
            focus: true,
            #[cfg(test)]
            actions_rect: None,
        }
    }
    pub fn tick(&mut self) -> Option<String> {
        let mut message = None;
        let events: Vec<_> = self.manager.drain().collect();
        for event in events {
            match event {
                TerminalEvent::Exited { id, code } => {
                    self.remove(id);
                    message = Some(format!("Terminal {} exited with code {code}", id.0));
                }
                TerminalEvent::Error { id, message: error } => {
                    eprintln!("Terminal {}: {error}", id.0);
                    message = Some(error);
                }
                TerminalEvent::Started { .. } => {}
            }
        }
        for session in self.manager.sessions() {
            if let Some(snapshot) = session.snapshot() {
                self.snapshots.insert(session.id(), snapshot);
            }
        }
        message
    }
    pub fn create_remote(
        &mut self,
        name: &str,
        transport: Box<dyn nozhik_terminal::external::ExternalTransport>,
    ) -> nozhik_terminal::Result<()> {
        let id = self
            .manager
            .create_external(SessionOptions::new(std::path::PathBuf::new()), transport)?;
        self.labels.insert(id, format!("[SSH: {name}]"));
        self.active = Some(id);
        self.focus = true;
        Ok(())
    }
    pub(crate) fn create(&mut self, root: &Path) -> Option<String> {
        match self.manager.create(SessionOptions::new(root.into())) {
            Ok(id) => {
                self.active = Some(id);
                self.focus = true;
                None
            }
            Err(error) => Some(error.to_string()),
        }
    }
    fn remove(&mut self, id: TerminalSessionId) {
        let _closed = self.manager.remove(id);
        self.snapshots.remove(&id);
        self.labels.remove(&id);
        self.view.forget(id);
        if self.active == Some(id) {
            self.active = self
                .manager
                .sessions()
                .next()
                .map(nozhik_terminal::TerminalSession::id);
            self.focus = true;
        }
    }
    pub const fn hidden(&mut self) {
        self.was_visible = false;
    }
    pub fn clipboard_action(&mut self, ctx: &egui::Context, action: &str) -> bool {
        let Some(id) = self.active else {
            return false;
        };
        let Some(session) = self.manager.session(id) else {
            return false;
        };
        self.view.clipboard_action(
            ctx,
            session,
            self.snapshots.get(&id).and_then(|s| s.grid.as_deref()),
            action,
        )
    }
    pub fn shutdown(&mut self) {
        self.manager.shutdown();
    }
    #[allow(
        clippy::too_many_lines,
        reason = "Terminal header actions and active-session presentation share one frame"
    )]
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        root: Option<&Path>,
        font_size: f32,
        enabled: bool,
    ) -> (bool, Option<String>) {
        let mut error = if !self.was_visible && self.active.is_none() {
            self.create(root.unwrap_or_else(|| Path::new(".")))
        } else {
            None
        };
        self.was_visible = true;
        let mut close = None;
        let mut close_all = false;
        let mut new_terminal = false;
        let mut collapse = false;
        let reveal = self.shown_active != self.active;
        self.shown_active = self.active;
        ui.add_enabled_ui(enabled, |ui| {
            ui.scope(|ui| {
                ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
                ui.visuals_mut().clip_rect_margin = 0.0;
                ui.horizontal(|ui| {
                    ui.set_height(34.0);
                    ui.label(egui::RichText::new("Terminal").strong());
                    ui.add_space(12.0);
                    let width = (ui.available_width() - 64.0).max(1.0);
                    egui::ScrollArea::horizontal()
                        .id_salt("terminal_tabs")
                        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                        .auto_shrink([false, false])
                        .max_width(width)
                        .max_height(34.0)
                        .show(ui, |ui| {
                            for session in self.manager.sessions() {
                                let id = session.id();
                                let label = self.labels.get(&id).cloned().unwrap_or_else(|| {
                                    self.snapshots
                                        .get(&id)
                                        .and_then(|s| s.grid.as_ref())
                                        .filter(|g| !g.title.is_empty())
                                        .map_or_else(
                                            || format!("Local {}", id.0),
                                            |g| g.title.chars().take(30).collect(),
                                        )
                                });
                                let font = egui::FontId::proportional(13.0);
                                let width = ui
                                    .painter()
                                    .layout_no_wrap(label.clone(), font.clone(), crate::theme::TEXT)
                                    .size()
                                    .x
                                    + 48.0;
                                let (rect, response) = ui.allocate_exact_size(
                                    egui::vec2(width, 34.0),
                                    egui::Sense::click(),
                                );
                                if reveal && self.active == Some(id) {
                                    ui.scroll_to_rect_animation(
                                        rect,
                                        None,
                                        egui::style::ScrollAnimation::none(),
                                    );
                                }
                                let cross = ui.interact(
                                    egui::Rect::from_center_size(
                                        egui::pos2(rect.right() - 15.0, rect.center().y),
                                        egui::vec2(20.0, 22.0),
                                    ),
                                    response.id.with("close"),
                                    egui::Sense::click(),
                                );
                                crate::editor_tabs::paint_tab(
                                    ui,
                                    &label,
                                    self.active == Some(id),
                                    false,
                                    rect,
                                    response.hovered(),
                                    cross.hovered(),
                                    &font,
                                );
                                if cross
                                    .on_hover_text("Close session and terminate shell")
                                    .clicked()
                                {
                                    close = Some(id);
                                } else if response.clicked() {
                                    self.active = Some(id);
                                    self.focus = true;
                                }
                                response.on_hover_text(session.options().cwd.display().to_string());
                            }
                            new_terminal = ui
                                .add_sized([28.0, 28.0], egui::Button::new("+").frame(false))
                                .on_hover_text("New terminal")
                                .clicked();
                        });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_sized([28.0, 28.0], egui::Button::new("−").frame(false))
                            .on_hover_text("Hide terminal; sessions keep running")
                            .clicked()
                        {
                            collapse = true;
                        }
                        let (rect, menu) =
                            ui.allocate_exact_size(egui::vec2(28.0, 28.0), egui::Sense::click());
                        if menu.hovered() {
                            ui.painter().rect_filled(rect, 5.0, crate::theme::HOVER);
                        }
                        for y in [-5.0, 0.0, 5.0] {
                            ui.painter().circle_filled(
                                rect.center() + egui::vec2(0.0, y),
                                1.3,
                                crate::theme::TEXT,
                            );
                        }
                        menu.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::WidgetType::Button,
                                true,
                                "Terminal actions",
                            )
                        });
                        #[cfg(test)]
                        {
                            self.actions_rect = Some(rect);
                        }
                        let menu = menu.on_hover_text("Terminal actions");
                        egui::Popup::menu(&menu).show(|ui| {
                            if ui
                                .add_enabled(self.active.is_some(), egui::Button::new("Close All"))
                                .clicked()
                            {
                                close_all = true;
                                ui.close();
                            }
                        });
                    });
                });
            });
        });
        if new_terminal {
            error = self.create(root.unwrap_or_else(|| Path::new(".")));
        }
        if close_all {
            let ids: Vec<_> = self
                .manager
                .sessions()
                .map(nozhik_terminal::TerminalSession::id)
                .collect();
            for id in ids {
                self.remove(id);
            }
        }
        if let Some(id) = close {
            self.remove(id);
        }
        ui.painter().hline(
            ui.min_rect().x_range(),
            ui.cursor().top(),
            egui::Stroke::new(1.0_f32, crate::theme::BORDER),
        );
        ui.add_space(6.0);
        if let Some(id) = self.active
            && let Some(session) = self.manager.session(id)
        {
            let snapshot = self.snapshots.get(&id);
            if let Some(SessionState::Failed(message)) = snapshot.map(|s| &s.state) {
                ui.colored_label(crate::theme::ERROR, message);
            } else {
                let grid = snapshot.and_then(|s| s.grid.as_deref());
                if let Some(message) = self.view.show(
                    ui,
                    session,
                    grid,
                    font_size,
                    enabled,
                    std::mem::take(&mut self.focus),
                ) {
                    error = Some(message);
                }
            }
        } else {
            ui.weak("No terminal sessions. Click + to start a shell.");
        }
        (collapse, error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, Pos2, Rect, vec2};

    fn frame(
        panel: &mut TerminalPanel,
        ctx: &egui::Context,
        root: &Path,
        events: Vec<Event>,
    ) -> (egui::FullOutput, bool) {
        let mut hidden = false;
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(700.0, 300.0))),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let (collapse, error) = panel.show(ui, Some(root), 14.0, true);
                    assert!(error.is_none());
                    hidden = collapse;
                });
            },
        );
        (output, hidden)
    }
    fn text_position(output: &egui::FullOutput, label: &str) -> Option<Pos2> {
        output.shapes.iter().find_map(|s| match &s.shape {
            egui::Shape::Text(t) if t.galley.text() == label => Some(t.pos + t.galley.size() * 0.5),
            _ => None,
        })
    }
    fn click(point: Pos2) -> Vec<Event> {
        let button = |pressed| Event::PointerButton {
            pos: point,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        vec![Event::PointerMoved(point), button(true), button(false)]
    }
    #[test]
    fn tabs_hide_reopen_close_all_and_empty_activation() -> Result<(), Box<dyn std::error::Error>> {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let dir = tempfile::tempdir()?;
        let mut panel = TerminalPanel::new(&ctx);
        let (output, _) = frame(&mut panel, &ctx, dir.path(), vec![]);
        let first = panel.active.ok_or("initial shell not created")?;
        assert_eq!(panel.manager.sessions().count(), 1);
        let minus = text_position(&output, "−").ok_or("missing hide control")?;
        let (_, hidden) = frame(&mut panel, &ctx, dir.path(), click(minus));
        assert!(hidden);
        panel.hidden();
        let (output, _) = frame(&mut panel, &ctx, dir.path(), vec![]);
        assert_eq!(panel.active, Some(first));
        let plus = text_position(&output, "+").ok_or("missing new control")?;
        frame(&mut panel, &ctx, dir.path(), click(plus));
        assert_eq!(panel.manager.sessions().count(), 2);
        let (_output, _) = frame(&mut panel, &ctx, dir.path(), vec![]);
        let dots = panel
            .actions_rect
            .ok_or("missing actions control")?
            .center();
        frame(&mut panel, &ctx, dir.path(), click(dots));
        let (output, _) = frame(&mut panel, &ctx, dir.path(), vec![]);
        let close = text_position(&output, "Close All").ok_or("missing Close All")?;
        frame(&mut panel, &ctx, dir.path(), click(close));
        assert_eq!(panel.manager.sessions().count(), 0);
        frame(&mut panel, &ctx, dir.path(), vec![]);
        assert!(panel.active.is_none());
        panel.hidden();
        frame(&mut panel, &ctx, dir.path(), vec![]);
        assert!(panel.active.is_some_and(|id| id != first));
        assert_eq!(panel.manager.sessions().count(), 1);
        panel.shutdown();
        Ok(())
    }
}
