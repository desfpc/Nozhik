//! Bottom terminal tabs, session ownership and nonblocking screen presentation.
use crate::terminal_view::TerminalView;
use nozhik_terminal::{
    SessionSnapshot, SessionState, TerminalEvent, TerminalManager, TerminalSessionId,
    pty::SessionOptions,
};
use std::{
    collections::HashMap,
    path::Path,
    time::{Duration, Instant},
};

pub const TERMINAL_LABEL: &str = "⌘ Terminal";
pub struct TerminalPanel {
    manager: TerminalManager,
    context: egui::Context,
    llm: bool,
    active: Option<TerminalSessionId>,
    snapshots: HashMap<TerminalSessionId, SessionSnapshot>,
    view: TerminalView,
    was_visible: bool,
    shown_active: Option<TerminalSessionId>,
    labels: HashMap<TerminalSessionId, String>,
    docker: HashMap<TerminalSessionId, (String, String)>,
    pub focus: bool,
    pending_paste: Option<PendingPaste>,
    paste_notice: Option<String>,
    #[cfg(test)]
    actions_rect: Option<egui::Rect>,
}

struct PendingPaste {
    text: String,
    started: Instant,
}

/// How a snippet paste was handed to the terminal.
pub enum PasteDelivery {
    /// Bytes are already queued for the running session.
    Sent,
    /// Multiline text is held until the shell enables bracketed paste.
    Waiting,
}

/// One command line submitted to a PTY. Docker consoles carry the container identity.
pub struct SubmittedCommand {
    pub container_id: Option<String>,
    pub container_name: Option<String>,
    pub line: String,
}

const PASTE_LIMIT: usize = 64 * 1024;
const PASTE_WAIT: Duration = Duration::from_secs(2);
impl TerminalPanel {
    /// Failed terminal sessions for the project problem log.
    pub(crate) fn incidents(&self) -> Vec<crate::problems::Incident> {
        let mut incidents = Vec::new();
        for (id, snapshot) in &self.snapshots {
            if let SessionState::Failed(message) = &snapshot.state {
                incidents.push(crate::problems::incident(
                    format!("terminal-{}", id.0),
                    crate::problems::severity_of(message),
                    "Terminal",
                    message,
                ));
            }
        }
        incidents
    }
}
impl TerminalPanel {
    pub fn new(ctx: &egui::Context) -> Self {
        let ctx = ctx.clone();
        Self {
            context: ctx.clone(),
            llm: false,
            manager: TerminalManager::new(std::sync::Arc::new(move || ctx.request_repaint())),
            active: None,
            snapshots: HashMap::new(),
            view: TerminalView::default(),
            was_visible: false,
            shown_active: None,
            labels: HashMap::new(),
            docker: HashMap::new(),
            focus: true,
            pending_paste: None,
            paste_notice: None,
            #[cfg(test)]
            actions_rect: None,
        }
    }
    pub fn llm(ctx: &egui::Context) -> Self {
        let mut panel = Self::new(ctx);
        panel.llm = true;
        panel
    }
    pub fn active_agent(
        &self,
    ) -> Option<(TerminalSessionId, nozhik_terminal::agent::ForegroundAgent)> {
        let id = self.active?;
        Some((id, self.snapshots.get(&id)?.agent?))
    }
    pub fn paste_context(
        &mut self,
        id: TerminalSessionId,
        agent: nozhik_terminal::agent::ForegroundAgent,
        text: &str,
    ) -> Result<(), String> {
        if self.active != Some(id) || self.active_agent() != Some((id, agent)) {
            return Err("Active LLM session changed; add the context again".into());
        }
        self.manager
            .session(id)
            .ok_or("LLM session closed")?
            .paste_to_agent(agent, text)
            .map_err(|e| e.to_string())?;
        self.focus = true;
        Ok(())
    }
    pub fn tick(&mut self) -> Option<String> {
        let mut message = None;
        let events: Vec<_> = self.manager.drain().collect();
        for event in events {
            match event {
                TerminalEvent::Exited { id, code } => {
                    if self
                        .labels
                        .get(&id)
                        .is_none_or(|label| !label.starts_with("[Docker: "))
                    {
                        self.remove(id);
                    }
                    message = Some(format!("Terminal {} exited with code {code}", id.0));
                }
                TerminalEvent::Error { id, message: error } => {
                    eprintln!("Terminal {}: {error}", id.0);
                    message = Some(error);
                }
                TerminalEvent::Copied { text, .. } => self.context.copy_text(text),
                TerminalEvent::Started { .. } => {}
            }
        }
        for session in self.manager.sessions() {
            if let Some(snapshot) = session.snapshot() {
                self.snapshots.insert(session.id(), snapshot);
            }
        }
        let paste_notice = self.paste_notice.take().or_else(|| self.flush_paste());
        if let Some(notice) = paste_notice {
            if message.is_none() {
                message = Some(notice);
            } else {
                self.paste_notice = Some(notice);
            }
        }
        message
    }

    /// True when the active session can accept input now or as soon as it starts.
    pub(crate) fn has_running_session(&self) -> bool {
        self.active.is_some_and(|id| self.accepts_input(id))
    }

    /// Copies `text` into the active session.
    ///
    /// A single line is sent immediately and does not press Enter. A multiline
    /// snippet is sent with bracketed paste when the shell has enabled it, and
    /// otherwise waits up to two seconds. Raw newlines are never used as Enter.
    ///
    /// # Errors
    /// Returns when there is no running session, the text contains control
    /// characters other than tab and newline, or the encoded paste exceeds 64 KiB.
    pub(crate) fn queue_paste(&mut self, text: &str) -> Result<PasteDelivery, String> {
        let normalized = normalize_newlines(text);
        reject_paste(&normalized)?;
        let Some(id) = self.active else {
            return Err("No terminal session is open".into());
        };
        if !self.accepts_input(id) {
            return Err("The terminal is not running".into());
        }
        let bracketed = self.bracketed(id);
        if normalized.contains('\n') && !bracketed {
            self.pending_paste = Some(PendingPaste {
                text: normalized,
                started: Instant::now(),
            });
            self.context
                .request_repaint_after(Duration::from_millis(50));
            return Ok(PasteDelivery::Waiting);
        }
        self.pending_paste = None;
        let bytes = encode_paste(&normalized, bracketed)?;
        self.write_session(id, bytes)?;
        Ok(PasteDelivery::Sent)
    }

    fn accepts_input(&self, id: TerminalSessionId) -> bool {
        if self.manager.session(id).is_none() {
            return false;
        }
        match self.snapshots.get(&id).map(|snapshot| &snapshot.state) {
            None | Some(SessionState::Starting | SessionState::Running(_)) => true,
            Some(SessionState::Exited(_) | SessionState::Failed(_) | SessionState::Closed) => false,
        }
    }

    fn bracketed(&self, id: TerminalSessionId) -> bool {
        self.snapshots
            .get(&id)
            .and_then(|snapshot| snapshot.grid.as_ref())
            .is_some_and(|grid| grid.bracketed_paste)
    }

    fn write_session(&self, id: TerminalSessionId, bytes: Vec<u8>) -> Result<(), String> {
        self.manager
            .session(id)
            .ok_or_else(|| "The terminal session closed".to_owned())?
            .write(bytes)
            .map_err(|error| error.to_string())
    }

    fn flush_paste(&mut self) -> Option<String> {
        let (started, text) = {
            let pending = self.pending_paste.as_ref()?;
            (pending.started, pending.text.clone())
        };
        let Some(id) = self.active else {
            self.pending_paste = None;
            return Some(
                "snippet-paste:The terminal closed before the snippet could be pasted".into(),
            );
        };
        if !self.accepts_input(id) {
            self.pending_paste = None;
            return Some("snippet-paste:The terminal is not running".into());
        }
        if text.contains('\n') && !self.bracketed(id) {
            if started.elapsed() >= PASTE_WAIT {
                self.pending_paste = None;
                return Some(
                    "snippet-paste:The terminal did not enable paste mode. The snippet stays on the clipboard."
                        .into(),
                );
            }
            self.context
                .request_repaint_after(Duration::from_millis(50));
            return None;
        }
        self.pending_paste = None;
        match encode_paste(&text, self.bracketed(id)) {
            Ok(bytes) => match self.write_session(id, bytes) {
                Ok(()) => Some("snippet-pasted".into()),
                Err(error) => Some(format!("snippet-paste:{error}")),
            },
            Err(error) => Some(format!("snippet-paste:{error}")),
        }
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
    pub(crate) fn create_docker(
        &mut self,
        root: &Path,
        program: std::path::PathBuf,
        args: Vec<std::ffi::OsString>,
        container_id: &str,
        name: &str,
    ) -> nozhik_terminal::Result<()> {
        let mut options = SessionOptions::new(root.into());
        options.shell = Some(nozhik_terminal::pty::ShellCommand {
            program: program.into_os_string(),
            args,
        });
        let id = self.manager.create(options)?;
        self.labels.insert(id, format!("[Docker: {name}]"));
        self.docker
            .insert(id, (container_id.to_owned(), name.to_owned()));
        self.active = Some(id);
        self.focus = true;
        Ok(())
    }

    /// Commands submitted since the last frame, with the Docker session identity when known.
    pub(crate) fn take_commands(&mut self) -> Vec<SubmittedCommand> {
        self.view
            .take_submitted()
            .into_iter()
            .map(|(id, line)| {
                let docker = self.docker.get(&id);
                SubmittedCommand {
                    container_id: docker.map(|(container_id, _)| container_id.clone()),
                    container_name: docker.map(|(_, name)| name.clone()),
                    line,
                }
            })
            .collect()
    }
    pub(crate) fn create(&mut self, root: &Path) -> Option<String> {
        let mut options = SessionOptions::new(root.into());
        options.monitor_agent = self.llm;
        match self.manager.create(options) {
            Ok(id) => {
                self.active = Some(id);
                self.focus = true;
                None
            }
            Err(error) => Some(error.to_string()),
        }
    }
    fn remove(&mut self, id: TerminalSessionId) {
        if self.active == Some(id) && self.pending_paste.take().is_some() {
            self.paste_notice =
                Some("snippet-paste:The terminal closed before the snippet could be pasted".into());
        }
        let _closed = self.manager.remove(id);
        self.snapshots.remove(&id);
        self.labels.remove(&id);
        self.docker.remove(&id);
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
                    ui.label(
                        egui::RichText::new(if self.llm { "LLM" } else { "Terminal" }).strong(),
                    );
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
                                    .layout_no_wrap(
                                        label.clone(),
                                        font.clone(),
                                        crate::theme::text(),
                                    )
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
                            ui.painter().rect_filled(rect, 5.0, crate::theme::hover());
                        }
                        for y in [-5.0, 0.0, 5.0] {
                            ui.painter().circle_filled(
                                rect.center() + egui::vec2(0.0, y),
                                1.3,
                                crate::theme::text(),
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
            egui::Stroke::new(1.0_f32, crate::theme::border()),
        );
        ui.add_space(6.0);
        if let Some(id) = self.active
            && let Some(session) = self.manager.session(id)
        {
            let snapshot = self.snapshots.get(&id);
            if let Some(SessionState::Failed(_)) = snapshot.map(|s| &s.state) {
                ui.weak("Session failed. Open Problems for the details.");
            } else {
                let grid = snapshot.and_then(|s| s.grid.as_deref());
                if let Some(grid) = grid {
                    ui.add_enabled_ui(enabled, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            if ui.selectable_label(grid.reviewing, "Text selection").on_hover_text("Freeze visible terminal text for scrolling and copying; output continues in the background").clicked() {
                                self.focus = true;
                                if grid.reviewing { session.live(); } else if let Err(e) = session.review() { error = Some(e.to_string()); }
                            }
                            if ui.button("Copy all").on_hover_text("Copy available terminal history and screen; in a fullscreen TUI this includes the currently rendered screen").clicked()
                                && let Err(e) = session.copy_text(grid.selection_epoch, None) { error = Some(e.to_string()); }
                            if grid.reviewing {
                                if ui.button("Live · Esc").clicked() { session.live(); self.focus = true; }
                            } else if grid.mouse_mode != nozhik_terminal::grid::MouseProtocolMode::None {
                                ui.weak("Mouse → agent · Shift+drag to select text");
                            }
                        });
                    });
                }
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

/// Encodes a snippet for the terminal.
///
/// Bracketed mode wraps the text. Without it, only a single line is accepted,
/// and that line is not followed by Enter.
///
/// # Errors
/// Rejects control characters other than tab and newline, multiline text when
/// bracketed paste is off, and batches larger than 64 KiB.
pub fn encode_paste(text: &str, bracketed: bool) -> Result<Vec<u8>, String> {
    let normalized = normalize_newlines(text);
    reject_paste(&normalized)?;
    if normalized.contains('\n') && !bracketed {
        return Err("The terminal is not ready for a multiline paste".into());
    }
    let bytes = if bracketed {
        format!("\u{1b}[200~{normalized}\u{1b}[201~").into_bytes()
    } else {
        normalized.into_bytes()
    };
    if bytes.len() > PASTE_LIMIT {
        return Err("Snippet is larger than 64 KiB and was not sent to the terminal".into());
    }
    Ok(bytes)
}

fn normalize_newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

fn reject_paste(text: &str) -> Result<(), String> {
    if text
        .chars()
        .any(|symbol| symbol.is_control() && symbol != '\n' && symbol != '\t')
    {
        return Err("Snippet contains control characters and was not sent to the terminal".into());
    }
    if text.len() > PASTE_LIMIT {
        return Err("Snippet is larger than 64 KiB and was not sent to the terminal".into());
    }
    Ok(())
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

    #[test]
    fn encode_paste_keeps_single_lines_raw_and_brackets_multiline() -> Result<(), String> {
        let single = encode_paste("echo hi", false)?;
        assert_eq!(single, b"echo hi");
        assert!(!single.contains(&b'\r'));
        let lines = encode_paste("echo a\r\necho b", true)?;
        assert!(lines.starts_with(b"\x1b[200~"));
        assert!(lines.ends_with(b"\x1b[201~"));
        let text = String::from_utf8(lines).map_err(|error| error.to_string())?;
        assert!(text.contains("echo a\necho b"));
        assert!(!text.contains('\r'));
        assert!(encode_paste("echo hi\necho there", false).is_err());
        assert!(encode_paste("echo \u{1}hi", false).is_err());
        let huge = "x".repeat(PASTE_LIMIT + 1);
        assert!(encode_paste(&huge, true).is_err());
        Ok(())
    }

    #[test]
    fn queue_paste_without_a_session_reports_an_error_and_starts_nothing() -> Result<(), String> {
        let ctx = egui::Context::default();
        let mut panel = TerminalPanel::new(&ctx);
        match panel.queue_paste("echo hi") {
            Err(error) => assert!(error.contains("No terminal"), "{error}"),
            Ok(_) => return Err("paste was accepted without a session".into()),
        }
        assert!(panel.active.is_none());
        assert!(panel.pending_paste.is_none());
        panel.shutdown();
        Ok(())
    }
}
