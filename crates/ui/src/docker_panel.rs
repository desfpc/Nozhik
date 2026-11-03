//! Docker tool window, project worker ownership and revision-bound Compose gutter actions.
use nozhik_docker::{
    Action, Command, Container, DockerWorker, Event, Response, RunMarker, Snapshot,
};
use std::{
    collections::HashMap,
    path::PathBuf,
    time::{Duration, Instant},
};

#[derive(Default)]
pub struct DockerUi {
    root: Option<PathBuf>,
    worker: Option<DockerWorker>,
    pub(crate) files: Vec<PathBuf>,
    selected: Option<PathBuf>,
    snapshot: Option<Snapshot>,
    token: u64,
    busy: Option<u64>,
    user_operation: bool,
    error: Option<String>,
    container: Option<String>,
    details: bool,
    logs_tab: bool,
    logs: String,
    next_refresh: Option<Instant>,
    sent: HashMap<PathBuf, (u64, u64)>,
    markers: HashMap<PathBuf, (u64, u64, Vec<RunMarker>)>,
}
impl DockerUi {
    pub(crate) fn shutdown(&mut self) {
        self.worker = None;
    }
    fn configuration(&self) -> Vec<PathBuf> {
        self.selected.as_ref().map_or_else(Vec::new, |file| {
            nozhik_docker::compose::configuration(file, &self.files)
        })
    }
    fn send(&mut self, action: Option<Action>, user_operation: bool) {
        if self.busy.is_some() {
            return;
        }
        let files = self.configuration();
        if files.is_empty() {
            return;
        }
        self.token += 1;
        let token = self.token;
        let command = match action {
            Some(action) => Command::Act {
                token,
                files,
                action,
            },
            None => Command::Refresh { token, files },
        };
        if self
            .worker
            .as_ref()
            .is_some_and(|worker| worker.send(command))
        {
            self.busy = Some(token);
            self.user_operation = user_operation;
            self.error = None;
        } else {
            self.error = Some("Docker worker busy; retry the action".into());
        }
    }
    fn target(&self) -> Option<&Container> {
        self.snapshot
            .as_ref()?
            .containers
            .iter()
            .find(|c| Some(&c.id) == self.container.as_ref())
    }
}
impl crate::NozhikUi {
    pub(crate) fn docker_tick(&mut self, ctx: &egui::Context) {
        let Some(root) = self.project.as_ref().map(|p| p.root_path.clone()) else {
            return;
        };
        if self.docker.root.as_ref() != Some(&root) {
            self.docker = DockerUi::default();
            let context = ctx.clone();
            self.docker.worker = Some(DockerWorker::start(root.clone(), move || {
                context.request_repaint();
            }));
            self.docker.root = Some(root);
        }
        let events: Vec<_> = self
            .docker
            .worker
            .as_ref()
            .map_or_else(Vec::new, |w| w.drain().collect());
        for event in events {
            self.docker_event(ctx, event);
        }
        for document in self
            .documents
            .iter()
            .filter(|d| d.open && !d.binary && nozhik_docker::is_compose_file(&d.path))
        {
            let snapshot = document.buffer.snapshot();
            let identity = (snapshot.buffer_id.0, snapshot.revision);
            if self.docker.sent.get(&document.path) != Some(&identity)
                && self.docker.worker.as_ref().is_some_and(|worker| {
                    worker.send(Command::Document {
                        path: document.path.clone(),
                        buffer: identity.0,
                        revision: identity.1,
                        source: snapshot.text.to_string(),
                    })
                })
            {
                self.docker.sent.insert(document.path.clone(), identity);
            }
        }
        self.editor.docker_gutter = self
            .active
            .and_then(|i| self.documents.get(i))
            .and_then(|document| self.docker.markers.get(&document.path))
            .cloned();
        if let Some((buffer, revision, service)) = self.editor.docker_run.take()
            && let Some(document) = self.documents.iter().find(|d| {
                d.buffer.snapshot().buffer_id.0 == buffer
                    && d.buffer.snapshot().revision == revision
            })
        {
            let path = document.path.clone();
            if document.buffer.is_dirty() {
                self.status = "Save the Compose file before starting services".into();
            } else if self.docker.busy.is_some() {
                self.status = "Wait for the current Docker operation".into();
            } else {
                self.docker.selected = Some(path);
                self.left = Some(crate::LeftTool::Docker);
                self.docker_request(Some(Action::Up(service)));
            }
        }
        let visible = self.left == Some(crate::LeftTool::Docker) || self.docker.details;
        if visible && self.docker.selected.is_some() {
            let now = Instant::now();
            if self.docker.busy.is_none() && self.docker.next_refresh.is_none_or(|next| now >= next)
            {
                let action = if self.docker.details && self.docker.logs_tab {
                    self.docker.container.clone().map(Action::Logs)
                } else {
                    None
                };
                self.docker.send(action, false);
                self.docker.next_refresh = Some(now + Duration::from_secs(3));
            }
            ctx.request_repaint_after(Duration::from_secs(3));
        }
    }
    fn docker_event(&mut self, ctx: &egui::Context, event: Event) {
        match event {
            Event::Files(Ok(files)) => {
                if self
                    .docker
                    .selected
                    .as_ref()
                    .is_none_or(|selected| !files.contains(selected))
                {
                    if let Some(worker) = &self.docker.worker {
                        worker.cancel();
                    }
                    self.docker.busy = None;
                    self.docker.user_operation = false;
                    self.docker.token += 1;
                    self.docker.selected = files
                        .iter()
                        .min_by_key(|path| {
                            let name = path
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .to_ascii_lowercase();
                            (
                                path.components().count(),
                                match name.as_str() {
                                    "compose.yaml" => 0,
                                    "compose.yml" => 1,
                                    "docker-compose.yaml" => 2,
                                    "docker-compose.yml" => 3,
                                    _ => 4,
                                },
                            )
                        })
                        .cloned();
                    self.docker.snapshot = None;
                    self.docker.next_refresh = None;
                }
                self.docker.files = files;
            }
            Event::Files(Err(error)) => {
                self.docker.error = Some(error.to_string());
            }
            Event::Markers {
                path,
                buffer,
                revision,
                markers,
            } => {
                if self.documents.iter().any(|d| {
                    d.open
                        && d.path == path
                        && d.buffer.snapshot().buffer_id.0 == buffer
                        && d.buffer.snapshot().revision == revision
                }) {
                    self.docker
                        .markers
                        .insert(path, (buffer, revision, markers));
                }
            }
            Event::Completed(token, result) if self.docker.busy == Some(token) => {
                self.docker.busy = None;
                self.docker.user_operation = false;
                match result {
                    Ok(Response::Snapshot(snapshot)) => {
                        self.docker.snapshot = Some(snapshot);
                    }
                    Ok(Response::Logs(logs)) => {
                        self.docker.logs = logs;
                    }
                    Ok(Response::Console {
                        program,
                        args,
                        name,
                    }) => {
                        let root = self.docker.root.clone().unwrap_or_default();
                        let panel = self
                            .terminal
                            .get_or_insert_with(|| crate::terminal_panel::TerminalPanel::new(ctx));
                        if let Err(error) = panel.create_docker(&root, program, args, &name) {
                            self.docker.error = Some(error.to_string());
                        } else {
                            self.bottom = Some(crate::terminal_panel::TERMINAL_LABEL);
                        }
                    }
                    Err(error) => {
                        self.docker.error = Some(error.to_string());
                    }
                }
            }
            Event::Completed(_, _) => {}
        }
    }
    fn docker_request(&mut self, action: Option<Action>) {
        let files = self.docker.configuration();
        if action.is_some()
            && self
                .documents
                .iter()
                .any(|d| d.buffer.is_dirty() && files.contains(&d.path))
        {
            self.docker.error = Some(
                "Save all files in this Compose configuration before running an action".into(),
            );
            return;
        }
        self.docker.send(action, true);
    }
    pub(crate) fn docker_panel(&mut self, ui: &mut egui::Ui) {
        let (mut action, refresh) = panel_header(ui, &mut self.docker);
        let busy = self.docker.busy.is_some();
        let files = self.docker.configuration();
        for file in &files {
            ui.weak(file.file_name().unwrap_or_default().to_string_lossy());
        }
        ui.separator();
        let mut selected = None;
        if let Some(snapshot) = &self.docker.snapshot {
            ui.label(egui::RichText::new(&snapshot.project).strong());
            egui::ScrollArea::vertical()
                .id_salt("docker_services")
                .show(ui, |ui| {
                    for service in &snapshot.services {
                        let containers: Vec<_> = snapshot
                            .containers
                            .iter()
                            .filter(|c| c.service == service.name)
                            .collect();
                        if containers.is_empty() {
                            ui.horizontal(|ui| {
                                if ui
                                    .add_enabled(!busy, egui::Button::new("▶"))
                                    .on_hover_text(format!("Create/start {}", service.name))
                                    .clicked()
                                {
                                    action = Some(Action::Up(Some(service.name.clone())));
                                }
                                ui.vertical(|ui| {
                                    ui.label(&service.name);
                                    ui.weak(if service.image.is_empty() {
                                        "Not created"
                                    } else {
                                        &service.image
                                    });
                                });
                            });
                        }
                        for container in containers {
                            if let Some(selected_action) = container_row(
                                ui,
                                container,
                                busy,
                                self.docker.container.as_deref() == Some(&container.id),
                                &mut selected,
                            ) {
                                action = Some(selected_action);
                            }
                        }
                        ui.separator();
                    }
                    for container in snapshot
                        .containers
                        .iter()
                        .filter(|c| !snapshot.services.iter().any(|s| s.name == c.service))
                    {
                        if let Some(selected_action) = container_row(
                            ui,
                            container,
                            busy,
                            self.docker.container.as_deref() == Some(&container.id),
                            &mut selected,
                        ) {
                            action = Some(selected_action);
                        }
                    }
                });
        } else {
            ui.weak("Select a Compose configuration to load its services and containers.");
        }
        if let Some(id) = selected {
            self.docker.container = Some(id);
            self.docker.details = true;
            self.docker.logs.clear();
            self.docker.logs_tab = false;
        }
        if refresh {
            self.docker_request(None);
        }
        if let Some(action) = action {
            self.docker_request(Some(action));
        }
    }
    pub(crate) fn docker_window(&mut self, ctx: &egui::Context) {
        if !self.docker.details {
            return;
        }
        let Some(container) = self.docker.target().cloned() else {
            self.docker.details = false;
            return;
        };
        let mut open = true;
        let mut action = None;
        egui::Window::new(format!("Docker · {}", container.name))
            .id(egui::Id::new("docker_container"))
            .open(&mut open)
            .default_size([720.0, 500.0])
            .resizable(true)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    operation_indicator(ui, &self.docker);
                    ui.selectable_value(&mut self.docker.logs_tab, false, "Info");
                    if ui
                        .selectable_value(&mut self.docker.logs_tab, true, "Logs")
                        .clicked()
                    {
                        action = Some(Action::Logs(container.id.clone()));
                    }
                    if ui
                        .add_enabled(
                            self.docker.busy.is_none() && container.state == "running",
                            egui::Button::new("Open console"),
                        )
                        .clicked()
                    {
                        action = Some(Action::Console(container.id.clone()));
                    }
                });
                if let Some(error) = &self.docker.error {
                    ui.colored_label(crate::theme::ERROR, error);
                }
                ui.separator();
                egui::ScrollArea::both()
                    .id_salt("docker_container_content")
                    .max_height(ui.available_height())
                    .show(ui, |ui| {
                        if self.docker.logs_tab {
                            if self.docker.logs.is_empty() {
                                ui.weak("No logs loaded. Logs refresh every three seconds.");
                            } else {
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(&self.docker.logs).monospace(),
                                    )
                                    .selectable(true),
                                );
                            }
                        } else {
                            container_info(ui, &container);
                        }
                    });
            });
        self.docker.details = open;
        if let Some(action) = action {
            self.docker_request(Some(action));
        }
    }
}
fn container_row(
    ui: &mut egui::Ui,
    container: &Container,
    busy: bool,
    selected: bool,
    clicked: &mut Option<String>,
) -> Option<Action> {
    let mut action = None;
    ui.horizontal(|ui| {
        let running = container.state == "running";
        ui.colored_label(
            if running {
                crate::theme::SUCCESS
            } else {
                crate::theme::ERROR
            },
            "●",
        );
        if ui
            .add_enabled(!busy, egui::Button::new(if running { "■" } else { "▶" }))
            .on_hover_text(if running {
                "Stop container"
            } else {
                "Start container"
            })
            .clicked()
        {
            action = Some(if running {
                Action::Stop(container.id.clone())
            } else {
                Action::Start(container.id.clone())
            });
        }
        ui.vertical(|ui| {
            if ui.selectable_label(selected, &container.name).clicked() {
                *clicked = Some(container.id.clone());
            }
            ui.weak(format!("{} · {}", container.state, container.image));
        });
    });
    action
}
fn container_info(ui: &mut egui::Ui, container: &Container) {
    egui::Grid::new("docker_info").striped(true).show(ui, |ui| {
        for (key, value) in [
            ("Name", container.name.clone()),
            ("ID", container.id.clone()),
            ("Image", container.image.clone()),
            ("Service", container.service.clone()),
            (
                "Status",
                format!(
                    "{} · exit {} · {}",
                    container.state, container.exit_code, container.health
                ),
            ),
        ] {
            ui.label(key);
            ui.add(egui::Label::new(value).selectable(true));
            ui.end_row();
        }
    });
    for (title, rows) in [("Mounts", &container.mounts), ("Labels", &container.labels)] {
        ui.separator();
        ui.strong(title);
        egui::Grid::new(title).striped(true).show(ui, |ui| {
            for (key, value) in rows {
                ui.add(egui::Label::new(key).selectable(true));
                ui.add(egui::Label::new(value).selectable(true));
                ui.end_row();
            }
        });
    }
}

fn operation_indicator(ui: &mut egui::Ui, state: &DockerUi) {
    // Reserve the same space for idle, background and user-initiated requests.
    let (rect, _) = ui.allocate_exact_size(egui::vec2(16.0, 18.0), egui::Sense::hover());
    if state.busy.is_some() && state.user_operation {
        ui.put(rect, egui::Spinner::new().size(16.0))
            .on_hover_text("Docker operation…");
    }
}

fn panel_header(ui: &mut egui::Ui, state: &mut DockerUi) -> (Option<Action>, bool) {
    let mut action = None;
    let mut refresh = false;
    let busy = state.busy.is_some();
    ui.add_enabled_ui(!busy, |ui| {
        let previous = state.selected.clone();
        egui::ComboBox::from_id_salt("docker_configuration")
            .selected_text(
                state
                    .selected
                    .as_ref()
                    .and_then(|p| p.file_name())
                    .map_or_else(
                        || "Compose file".into(),
                        |n| n.to_string_lossy().into_owned(),
                    ),
            )
            .width(ui.available_width())
            .show_ui(ui, |ui| {
                for path in &state.files {
                    let label = state
                        .root
                        .as_ref()
                        .and_then(|root| path.strip_prefix(root).ok())
                        .unwrap_or(path)
                        .display()
                        .to_string();
                    ui.selectable_value(&mut state.selected, Some(path.clone()), label);
                }
            });
        if previous != state.selected {
            state.snapshot = None;
            state.container = None;
            state.details = false;
            state.logs.clear();
            state.error = None;
            refresh = true;
        }
    });
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(!busy, egui::Button::new("▶ All"))
            .on_hover_text("Compose up: start all services")
            .clicked()
        {
            action = Some(Action::Up(None));
        }
        if ui
            .add_enabled(!busy, egui::Button::new("■ All"))
            .on_hover_text("Stop project containers, retaining data")
            .clicked()
        {
            action = Some(Action::StopAll);
        }
        if ui
            .add_enabled(!busy, egui::Button::new("Refresh"))
            .clicked()
        {
            refresh = true;
        }
        operation_indicator(ui, state);
        // Keep the toolbar geometry stable during periodic snapshot/log refreshes.
        if ui.add_enabled(busy, egui::Button::new("Cancel")).clicked()
            && let Some(worker) = &state.worker
        {
            worker.cancel();
        }
    });
    if let Some(error) = &state.error {
        ui.colored_label(crate::theme::ERROR, error);
    }
    (action, refresh)
}

#[cfg(test)]
mod tests;
