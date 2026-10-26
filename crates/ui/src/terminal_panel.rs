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
    initialized: bool,
    labels: HashMap<TerminalSessionId, String>,
    pub focus: bool,
}
impl TerminalPanel {
    pub fn new(ctx: &egui::Context) -> Self {
        let ctx = ctx.clone();
        Self {
            manager: TerminalManager::new(std::sync::Arc::new(move || ctx.request_repaint())),
            active: None,
            snapshots: HashMap::new(),
            view: TerminalView::default(),
            initialized: false,
            labels: HashMap::new(),
            focus: true,
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
        self.initialized = true;
        Ok(())
    }
    fn create(&mut self, root: &Path) -> Option<String> {
        match self.manager.create(SessionOptions::new(root.into())) {
            Ok(id) => {
                self.active = Some(id);
                self.focus = true;
                self.initialized = true;
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
    pub fn shutdown(&mut self) {
        self.manager.shutdown();
    }
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        root: Option<&Path>,
        font_size: f32,
        enabled: bool,
    ) -> (bool, Option<String>) {
        let mut error = if !self.initialized
            && let Some(root) = root
        {
            self.create(root)
        } else {
            None
        };
        let mut close = None;
        let mut collapse = false;
        ui.horizontal(|ui| {
            egui::ScrollArea::horizontal()
                .max_width((ui.available_width() - 145.0).max(80.0))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        for session in self.manager.sessions() {
                            let id = session.id();
                            let label = self
                                .snapshots
                                .get(&id)
                                .and_then(|s| s.grid.as_ref())
                                .filter(|g| !g.title.is_empty())
                                .map_or_else(
                                    || format!("Terminal {}", id.0),
                                    |g| g.title.chars().take(30).collect(),
                                );
                            if ui
                                .selectable_label(
                                    self.active == Some(id),
                                    self.labels.get(&id).cloned().unwrap_or(label),
                                )
                                .on_hover_text(session.options().cwd.display().to_string())
                                .clicked()
                            {
                                self.active = Some(id);
                                self.focus = true;
                            }
                            if ui
                                .small_button("×")
                                .on_hover_text("Close session and terminate shell")
                                .clicked()
                            {
                                close = Some(id);
                            }
                        }
                    });
                });
            if ui
                .add_enabled(root.is_some() && enabled, egui::Button::new("+"))
                .on_hover_text("New terminal in current project")
                .clicked()
                && let Some(root) = root
            {
                error = self.create(root);
            }
            if ui
                .add_enabled(self.active.is_some(), egui::Button::new("Close"))
                .clicked()
            {
                close = self.active;
            }
            if ui
                .button("—")
                .on_hover_text("Hide terminal; sessions keep running")
                .clicked()
            {
                collapse = true;
            }
        });
        if let Some(id) = close {
            self.remove(id);
        }
        ui.separator();
        if let Some(id) = self.active
            && let Some(session) = self.manager.session(id)
        {
            let snapshot = self.snapshots.get(&id);
            if let Some(SessionState::Failed(message)) = snapshot.map(|s| &s.state) {
                ui.colored_label(egui::Color32::LIGHT_RED, message);
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
            ui.weak(if root.is_some() {
                "No terminal sessions. Click + to start a shell."
            } else {
                "Open a project folder to start a terminal."
            });
        }
        (collapse, error)
    }
}
