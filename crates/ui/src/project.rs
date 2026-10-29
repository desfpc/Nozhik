//! Main-thread integration for filesystem command completions and lazy tree rows.
use crate::{Document, NozhikUi, RefreshState};
use egui::Key;
use nozhik_buffer::TextBuffer;
use nozhik_core::BufferId;
use nozhik_event_bus::Event;
use nozhik_fs_model::{
    Command, Reply,
    tree::{Node, ProjectTree},
};
use std::path::{Path, PathBuf};

#[derive(Default)]
pub struct ProjectControls {
    recursive: Option<PathBuf>,
    reveal: Option<PathBuf>,
    #[cfg(test)]
    buttons: Vec<egui::Rect>,
    #[cfg(test)]
    rows: std::collections::HashMap<PathBuf, egui::Rect>,
}

impl NozhikUi {
    /// Opens a project asynchronously; existing documents and unsaved text are retained.
    pub fn open_project(&mut self, path: PathBuf) {
        self.status = format!("Opening {}…", path.display());
        self.queue(Command::OpenProject(path));
    }
    pub(crate) fn queue(&mut self, command: Command) {
        if !self.fs.send(command) {
            self.status = "Filesystem worker stopped".into();
        }
    }
    pub(crate) fn refresh_project(&mut self) {
        if self.project.is_none() {
            return;
        }
        if self.refresh == RefreshState::Idle {
            self.refresh = RefreshState::Pending;
            self.queue(Command::Refresh);
        } else {
            self.refresh = RefreshState::Again;
        }
    }

    pub(crate) fn open_file(&mut self, path: PathBuf) {
        if let Some(index) = self.documents.iter().position(|d| d.path == path) {
            self.open_document(index);
        } else {
            self.queue(Command::Read(path));
        }
    }
    pub(crate) fn save_file(&mut self) {
        if let Some(i) = self.database.active {
            self.save_sql(i);
        } else if let Some(i) = self.active {
            self.save_document(i);
        }
    }
    pub(crate) fn save_document(&mut self, index: usize) {
        let Some(document) = self.documents.get(index) else {
            return;
        };
        if document.binary {
            self.status = "Binary file not supported".into();
            return;
        }
        let path = document.path.clone();
        let snapshot = document.buffer.snapshot();
        if self.integration.saving.get(&path) == Some(&snapshot.revision) {
            return;
        }
        let queued = if self.remote.documents.contains_key(&path) {
            if self.remote.document_invalidated(&path) {
                let error = "SSH profile changed/deleted; reopen the file from SFTP before saving. Edits remain unsaved.";
                self.status = error.into();
                self.integration.save_error = Some(error.into());
                return;
            }
            self.remote.save(&path, snapshot.revision, snapshot.text)
        } else {
            self.fs.send(Command::Save {
                path: path.clone(),
                revision: snapshot.revision,
                text: snapshot.text,
            })
        };
        if queued {
            self.integration.saving.insert(path, snapshot.revision);
        } else {
            self.integration.save_error =
                Some("Save queue full/stopped; file remains unsaved".into());
            self.status = self.integration.save_error.clone().unwrap_or_default();
        }
    }
    pub(crate) fn receive_files(&mut self) {
        let replies: Vec<_> = self.fs.drain().collect();
        for reply in replies {
            match reply {
                Reply::Refreshed(tree) => {
                    self.accept_project_tree(tree);
                    let again = self.refresh == RefreshState::Again;
                    self.refresh = RefreshState::Idle;
                    self.search.invalidate();
                    if again {
                        self.refresh_project();
                    }
                }
                Reply::ProjectCreated(tree) => {
                    self.project_created();
                    self.recent_projects.note(tree.root_path.clone());
                    self.project_path = tree.root_path.display().to_string();
                    self.accept_project_tree(tree);
                }
                Reply::ProjectCreationError(error) => {
                    self.integration.creating_project = false;
                    self.integration.project_error = Some(error.clone());
                    self.status = error;
                }
                Reply::ProjectOpened(tree) => {
                    self.recent_projects.note(tree.root_path.clone());
                    self.project_path = tree.root_path.display().to_string();
                    self.accept_project_tree(tree);
                }
                Reply::Tree(tree) => self.accept_project_tree(tree),
                Reply::Read { path, text } => self.loaded_file(path, &text, false),
                Reply::Binary(path) => self.loaded_file(path, "", true),
                Reply::Saved { path, revision } => {
                    self.saved(&path, revision);
                    if let Some(document) = self.documents.iter_mut().find(|d| d.path == path)
                        && document.buffer.snapshot().revision == revision
                    {
                        document.buffer.mark_saved();
                    }
                    self.index_changed(path.clone());
                    self.status = format!("Saved {}", path.display());
                    self.vcs.invalidate();
                }
                Reply::Matches { token, files } => self.search.accept(token, files),
                Reply::Error(error) => {
                    if !self.integration.saving.is_empty() {
                        self.integration.save_error = Some(error.clone());
                        self.integration.saving.clear();
                    }
                    self.project_controls.recursive = None;
                    self.project_controls.reveal = None;
                    for path in self.loading.drain() {
                        self.expanded.remove(&path);
                    }
                    self.refresh = RefreshState::Idle;
                    self.status.clone_from(&error);
                    eprintln!("Filesystem: {error}");
                }
            }
        }
    }
    fn accept_project_tree(&mut self, tree: ProjectTree) {
        if self
            .project
            .as_ref()
            .is_none_or(|p| p.root_path != tree.root_path)
            && let Some(manager) = &self.plugins
            && let Err(error) = manager.scan(Some(tree.root_path.clone()))
        {
            self.status = error.to_string();
        }
        self.vcs.set_project(&tree.root_path);
        if self
            .project
            .as_ref()
            .is_none_or(|p| p.root_path != tree.root_path)
        {
            self.project_controls = ProjectControls::default();
            self.loading.clear();
            self.expanded.clear();
            self.selected_file = None;
            self.expanded.insert(tree.root_path.clone());
            self.status = format!("Project: {}", tree.root_path.display());
        }
        self.loading.retain(|path| {
            tree.directories
                .get(path)
                .is_some_and(|dir| dir.children.is_none())
        });
        self.project = Some(tree);
        self.search.invalidate();
    }
    pub(crate) fn loaded_file(&mut self, path: PathBuf, text: &str, binary: bool) {
        if let Some(index) = self.documents.iter().position(|d| d.path == path) {
            self.open_document(index);
            return;
        }
        let Some(next) = self.next_buffer_id.checked_add(1) else {
            self.status = "Buffer IDs exhausted".into();
            return;
        };
        let mut buffer = TextBuffer::load_from_str(BufferId(self.next_buffer_id), text);
        self.next_buffer_id = next;
        buffer.set_event_bus(self.bus.clone());
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        self.documents.push(Document {
            name,
            path: path.clone(),
            buffer,
            open: true,
            binary,
        });
        self.active = self.documents.len().checked_sub(1);
        self.editor.focus_buffer = self
            .documents
            .last()
            .map(|doc| doc.buffer.snapshot().buffer_id);
        self.database.active = None;
        self.search.note_file(path.clone());
        self.bus.publish(Event::FileOpened(path));
    }
    fn active_project_file(&self) -> Option<PathBuf> {
        let document = self.documents.get(self.active?)?;
        let tree = self.project.as_ref()?;
        (self.database.active.is_none()
            && !self.remote.documents.contains_key(&document.path)
            && document.path.starts_with(&tree.root_path))
        .then(|| document.path.clone())
    }

    fn select_opened_file(&mut self) {
        let Some(path) = self.active_project_file() else {
            return;
        };
        let Some(tree) = &self.project else { return };
        for parent in path.ancestors().skip(1) {
            if !parent.starts_with(&tree.root_path) {
                break;
            }
            self.expanded.insert(parent.to_path_buf());
        }
        self.selected_file = Some(path.clone());
        self.project_controls.reveal = Some(path);
    }

    fn collapse_project(&mut self) {
        self.project_controls.recursive = None;
        self.project_controls.reveal = None;
        self.expanded.clear();
    }

    fn project_header(&mut self, ui: &mut egui::Ui) {
        let active = self.active_project_file().is_some();
        let directory = self
            .selected_file
            .as_ref()
            .filter(|path| {
                self.project
                    .as_ref()
                    .is_some_and(|tree| tree.directories.contains_key(*path))
            })
            .cloned();
        #[cfg(test)]
        {
            self.project_controls.buttons = vec![egui::Rect::NOTHING; 4];
        }
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.label(egui::RichText::new("Project").strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                for (icon, label, enabled) in [
                    (3, "Hide", true),
                    (2, "Collapse All", self.project.is_some()),
                    (1, "Expand Selected", directory.is_some()),
                    (0, "Select Opened File", active),
                ] {
                    let response = project_button(ui, icon, label, enabled);
                    #[cfg(test)]
                    {
                        self.project_controls.buttons[icon] = response.rect;
                    }
                    if response.clicked() {
                        match icon {
                            0 => self.select_opened_file(),
                            1 => {
                                if let (Some(path), Some(tree)) = (&directory, &self.project) {
                                    self.expanded.extend(
                                        path.ancestors()
                                            .take_while(|parent| {
                                                parent.starts_with(&tree.root_path)
                                            })
                                            .map(Path::to_path_buf),
                                    );
                                }
                                self.project_controls.recursive.clone_from(&directory);
                            }
                            2 => self.collapse_project(),
                            _ => self.left = None,
                        }
                    }
                }
            });
        });
        ui.separator();
    }

    // Advance only through known children; all directory reads remain on the FS worker.
    fn expand_project_subtree(&mut self) {
        let (Some(root), Some(tree)) = (&self.project_controls.recursive, &self.project) else {
            return;
        };
        let mut pending = vec![root.clone()];
        let mut loads = Vec::new();
        let mut complete = true;
        while let Some(path) = pending.pop() {
            let Some(directory) = tree.directories.get(&path) else {
                continue;
            };
            self.expanded.insert(path.clone());
            if let Some(children) = &directory.children {
                pending.extend(children.iter().filter_map(|node| match node {
                    Node::Dir(path) => Some(path.clone()),
                    Node::File(_) => None,
                }));
            } else {
                complete = false;
                if self.loading.len() + loads.len() < 16 && !self.loading.contains(&path) {
                    loads.push(path);
                }
            }
        }
        if complete {
            self.project_controls.recursive = None;
        }
        for path in loads {
            self.loading.insert(path.clone());
            self.queue(Command::LoadDir(path));
        }
    }

    pub(crate) fn project_panel(&mut self, ui: &mut egui::Ui) {
        self.project_header(ui);
        self.expand_project_subtree();
        if self.project.is_none() {
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.project_path)
                        .hint_text("Project folder path")
                        .desired_width(150.0),
                );
                if ui.button("Open").clicked() {
                    self.open_project(PathBuf::from(self.project_path.trim()));
                }
            });
            if ui.button("Choose folder…").clicked() {
                self.choose_project_folder(ui.ctx());
            }
            ui.label("Recent Projects");
            let mut recent = None;
            egui::ScrollArea::vertical()
                .id_salt("project_history")
                .max_height(150.0)
                .show(ui, |ui| {
                    recent = self.recent_projects.list(ui);
                });
            if let Some(path) = recent {
                self.open_project(path);
            }
            ui.separator();
        }
        let Some(tree) = &self.project else {
            ui.weak("Choose a folder or enter its path");
            return;
        };
        let mut actions = TreeActions::default();
        #[cfg(test)]
        self.project_controls.rows.clear();
        egui::ScrollArea::both().show(ui, |ui| {
            tree_row(
                ui,
                tree,
                &tree.root_path,
                &mut self.expanded,
                &mut self.selected_file,
                &mut actions,
                &mut self.project_controls,
            );
        });
        for path in actions.loads {
            if self.loading.len() >= 16 {
                break;
            }
            if self.loading.insert(path.clone()) {
                self.queue(Command::LoadDir(path));
            }
        }
        if let Some(path) = actions.open {
            self.open_file(path);
        }
    }
}
#[derive(Default)]
struct TreeActions {
    loads: Vec<PathBuf>,
    open: Option<PathBuf>,
}

fn tree_row(
    ui: &mut egui::Ui,
    tree: &ProjectTree,
    path: &Path,
    expanded: &mut std::collections::HashSet<PathBuf>,
    selected: &mut Option<PathBuf>,
    actions: &mut TreeActions,
    controls: &mut ProjectControls,
) {
    let is_open = expanded.contains(path);
    let name = path
        .file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy();
    let row = super::project_icons::row(
        ui,
        tree,
        path,
        &name,
        Some(is_open),
        selected.as_deref() == Some(path),
    );
    #[cfg(test)]
    controls.rows.insert(path.to_path_buf(), row.rect);
    if row.clicked() || (row.has_focus() && ui.input(|i| i.key_pressed(Key::Enter))) {
        *selected = Some(path.to_path_buf());
        row.request_focus();
        controls.recursive = None;
        controls.reveal = None;
        if is_open {
            expanded.remove(path);
        } else {
            expanded.insert(path.to_path_buf());
        }
    }
    if !expanded.contains(path) {
        return;
    }
    let children = tree.directories.get(path).and_then(|d| d.children.as_ref());
    let Some(children) = children else {
        actions.loads.push(path.to_path_buf());
        ui.weak("Loading…");
        return;
    };
    ui.indent(path, |ui| {
        for child in children {
            match child {
                Node::Dir(path) => {
                    tree_row(ui, tree, path, expanded, selected, actions, controls);
                }
                Node::File(file) => {
                    let name = file.path.file_name().unwrap_or_default().to_string_lossy();
                    let row = super::project_icons::row(
                        ui,
                        tree,
                        &file.path,
                        &name,
                        None,
                        selected.as_ref() == Some(&file.path),
                    );
                    #[cfg(test)]
                    controls.rows.insert(file.path.clone(), row.rect);
                    if controls.reveal.as_ref() == Some(&file.path) {
                        row.scroll_to_me(Some(egui::Align::Center));
                        controls.reveal = None;
                    }
                    if row.clicked() {
                        controls.reveal = None;
                        *selected = Some(file.path.clone());
                        row.request_focus();
                    }
                    if row.double_clicked()
                        || (row.has_focus() && ui.input(|i| i.key_pressed(Key::Enter)))
                    {
                        actions.open = Some(file.path.clone());
                    }
                }
            }
        }
    });
}

fn project_button(ui: &mut egui::Ui, icon: usize, label: &str, enabled: bool) -> egui::Response {
    let response = ui
        .add_enabled(
            enabled,
            egui::Button::new("").min_size(egui::vec2(24.0, 24.0)),
        )
        .on_hover_text(label);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    let center = response.rect.center();
    let stroke = egui::Stroke::new(1.3_f32, ui.style().interact(&response).fg_stroke.color);
    let painter = ui.painter();
    let line = |a: (f32, f32), b: (f32, f32)| {
        painter.line_segment(
            [center + egui::vec2(a.0, a.1), center + egui::vec2(b.0, b.1)],
            stroke,
        );
    };
    match icon {
        0 => {
            painter.circle_stroke(center, 6.0, stroke);
            for (a, b) in [
                ((-9.0, 0.0), (-3.0, 0.0)),
                ((3.0, 0.0), (9.0, 0.0)),
                ((0.0, -9.0), (0.0, -3.0)),
                ((0.0, 3.0), (0.0, 9.0)),
            ] {
                line(a, b);
            }
        }
        1 | 2 => {
            let direction: f32 = if icon == 1 { -1.0 } else { 1.0 };
            for sign in [-1.0, 1.0] {
                let y = sign * 5.0;
                line(
                    (-4.0, (sign * direction).mul_add(2.0, y)),
                    (0.0, (sign * direction).mul_add(-2.0, y)),
                );
                line(
                    (0.0, (sign * direction).mul_add(-2.0, y)),
                    (4.0, (sign * direction).mul_add(2.0, y)),
                );
            }
        }
        _ => line((-7.0, 0.0), (7.0, 0.0)),
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use nozhik_config::{Config, ConfigManager};
    use nozhik_event_bus::EventBus;
    use std::time::{Duration, Instant};

    fn until(
        shell: &mut NozhikUi,
        done: impl Fn(&mut NozhikUi) -> bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done(shell) {
            shell.receive_files();
            if Instant::now() > deadline {
                return Err(format!("Timed out: {}", shell.status).into());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(())
    }
    fn frame(
        shell: &mut NozhikUi,
        context: &egui::Context,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(320.0, 180.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| shell.project_panel(ui));
            },
        )
    }

    fn click_control(shell: &mut NozhikUi, context: &egui::Context, index: usize) {
        let _ = frame(shell, context, vec![]);
        let position = shell.project_controls.buttons[index].center();
        for pressed in [true, false] {
            let _ = frame(
                shell,
                context,
                vec![
                    egui::Event::PointerMoved(position),
                    egui::Event::PointerButton {
                        pos: position,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
    }

    #[test]
    fn toolbar_reveals_expands_collapses_and_hides() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let root = dir.path().canonicalize()?;
        let branch = root.join("selected");
        let nested = branch.join("deep/empty");
        std::fs::create_dir_all(&nested)?;
        std::fs::create_dir_all(root.join("untouched/child"))?;
        for i in 0..30 {
            std::fs::write(branch.join(format!("a{i:02}.txt")), "a")?;
            std::fs::create_dir(branch.join(format!("a{i:02}")))?;
        }
        let file = branch.join("deep/z.php");
        std::fs::write(&file, "<?php")?;
        let bus = EventBus::new(128)?;
        let settings = ConfigManager::from_path(root.join("settings.toml"), bus.clone())?;
        let mut shell = NozhikUi::new(bus, settings, Config::default());
        let context = egui::Context::default();
        context.style_mut(|style| {
            style.animation_time = 0.0;
            style.scroll_animation = egui::style::ScrollAnimation::none();
        });
        shell.open_project(root.clone());
        until(&mut shell, |s| s.project.is_some())?;
        shell.open_file(file.clone());
        until(&mut shell, |s| s.active.is_some())?;
        click_control(&mut shell, &context, 1); // Disabled without a selected directory.
        assert!(shell.project_controls.recursive.is_none());
        click_control(&mut shell, &context, 2);
        assert!(shell.expanded.is_empty());
        click_control(&mut shell, &context, 0);
        assert_eq!(shell.selected_file.as_ref(), Some(&file));
        until(&mut shell, |s| {
            let _ = frame(s, &context, vec![]);
            s.project_controls.reveal.is_none()
        })?;
        let _ = frame(&mut shell, &context, vec![]);
        let _ = frame(&mut shell, &context, vec![]);
        assert!((40.0..180.0).contains(&shell.project_controls.rows[&file].center().y));
        assert!(shell.expanded.contains(&root));
        assert!(shell.expanded.contains(&branch.join("deep")));
        assert!(!shell.expanded.contains(&nested));
        // Select a directory using its real tree-row hit area.
        click_control(&mut shell, &context, 2);
        let _ = frame(&mut shell, &context, vec![]);
        let _ = frame(&mut shell, &context, vec![]);
        let root_position =
            shell.project_controls.rows[&root].left_center() + egui::vec2(50.0, 0.0);
        for pressed in [true, false] {
            let _ = frame(
                &mut shell,
                &context,
                vec![
                    egui::Event::PointerMoved(root_position),
                    egui::Event::PointerButton {
                        pos: root_position,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
        assert_eq!(shell.selected_file.as_ref(), Some(&root));
        // Keep the sibling lazy while expanding only this subtree.
        shell.selected_file = Some(branch.clone());
        click_control(&mut shell, &context, 1);
        until(&mut shell, |s| {
            let _ = frame(s, &context, vec![]);
            s.project_controls.recursive.is_none()
        })?;
        assert!(shell.expanded.contains(&nested));
        assert!(!shell.expanded.contains(&root.join("untouched")));
        assert!(
            shell
                .project
                .as_ref()
                .is_some_and(|tree| tree.directories[&root.join("untouched")].children.is_none())
        );
        shell.project_controls.recursive = Some(root.clone());
        click_control(&mut shell, &context, 2);
        until(&mut shell, |s| s.loading.is_empty())?;
        let _ = frame(&mut shell, &context, vec![]);
        assert!(shell.expanded.is_empty());
        assert!(shell.project_controls.recursive.is_none());
        click_control(&mut shell, &context, 3);
        assert!(shell.left.is_none());
        assert_eq!(shell.documents.len(), 1);
        shell.left = Some(crate::LeftTool::Project);
        assert_eq!(shell.selected_file.as_ref(), Some(&branch));
        // An active file outside this project cannot change the selection.
        let outside = root.parent().ok_or("No parent")?.join("outside.php");
        shell.loaded_file(outside, "", false);
        click_control(&mut shell, &context, 0);
        assert_eq!(shell.selected_file.as_ref(), Some(&branch));
        Ok(())
    }

    #[test]
    fn open_project_hides_start_actions_and_history() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let bus = EventBus::new(32)?;
        let settings = ConfigManager::from_path(dir.path().join("settings.toml"), bus.clone())?;
        let mut shell = NozhikUi::new(bus, settings, Config::default());
        let context = egui::Context::default();
        let labels = |shell: &mut NozhikUi| {
            let output = context.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| shell.project_panel(ui));
            });
            output
                .shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::Shape::Text(t) => Some(t.galley.text().to_owned()),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        let empty = labels(&mut shell);
        assert!(empty.iter().any(|s| s == "Choose folder…"));
        assert!(empty.iter().any(|s| s == "Recent Projects"));
        shell.open_project(dir.path().into());
        until(&mut shell, |s| s.project.is_some())?;
        let opened = labels(&mut shell);
        assert!(
            !opened
                .iter()
                .any(|s| matches!(s.as_str(), "Open" | "Choose folder…" | "Recent Projects"))
        );
        Ok(())
    }
    #[test]
    fn successful_opens_persist_history_and_reopen_without_losing_edits()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let first = dir.path().join("one");
        let second = dir.path().join("two");
        std::fs::create_dir(&first)?;
        std::fs::create_dir(&second)?;
        let file = first.join("main.rs");
        std::fs::write(&file, "a")?;
        let bus = EventBus::new(128)?;
        let settings = ConfigManager::from_path(dir.path().join("settings.toml"), bus.clone())?;
        let mut shell = NozhikUi::new(bus, settings, Config::default());
        shell.open_project(first.clone());
        until(&mut shell, |s| s.project.is_some())?;
        shell.open_file(file);
        until(&mut shell, |s| !s.documents.is_empty())?;
        shell.documents[0].buffer.insert(1, "edited")?;
        shell.open_project(second.clone());
        until(&mut shell, |s| {
            s.project
                .as_ref()
                .is_some_and(|p| p.root_path == second.canonicalize().unwrap_or_default())
        })?;
        shell
            .integration
            .actions
            .push(format!("ui.open_project:{}", first.display()));
        shell.global_actions(&egui::Context::default());
        until(&mut shell, |s| {
            s.project
                .as_ref()
                .is_some_and(|p| p.root_path == first.canonicalize().unwrap_or_default())
        })?;
        assert!(shell.documents[0].buffer.is_dirty());
        assert_eq!(shell.documents[0].buffer.rope().to_string(), "aedited");
        shell.open_project(dir.path().join("missing"));
        shell.status.clear();
        until(&mut shell, |s| !s.status.is_empty())?;
        // Explicit shutdown waits for all previously queued history writes.
        shell.recent_projects.finish();
        let entries: Vec<crate::recent_projects::RecentProject> =
            serde_json::from_slice(&std::fs::read(dir.path().join("recent_projects.json"))?)?;
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].path, first.canonicalize()?);
        assert_eq!(entries[1].path, second.canonicalize()?);
        assert!(entries[0].last_opened > entries[1].last_opened);
        Ok(())
    }
    #[test]
    fn project_open_deduplicate_save_race_binary_and_missing()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let bus = EventBus::new(128)?;
        let settings = ConfigManager::from_path(dir.path().join("settings.toml"), bus.clone())?;
        let mut shell = NozhikUi::new(bus, settings, Config::default());
        let path = dir.path().join("main.rs");
        std::fs::write(&path, "a")?;
        shell.open_project(dir.path().into());
        until(&mut shell, |s| s.project.is_some())?;
        shell.open_file(path);
        until(&mut shell, |s| s.documents.len() == 1)?;
        let path = shell.documents[0].path.clone();
        shell.open_file(path.clone());
        assert_eq!(shell.documents.len(), 1);
        shell.documents[0].buffer.insert(1, "b")?;
        shell.save_file();
        // This edit occurs before handling the save completion, even on a fast worker.
        shell.documents[0].buffer.insert(2, "c")?;
        until(&mut shell, |s| s.status.starts_with("Saved"))?;
        assert_eq!(std::fs::read_to_string(&path)?, "ab");
        assert!(shell.documents[0].buffer.is_dirty());
        shell.status.clear();
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            events: vec![egui::Event::Key {
                key: Key::S,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::COMMAND,
            }],
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| shell.shortcuts(ctx));
        shell.receive_events();
        until(&mut shell, |s| s.status.starts_with("Saved"))?;
        assert!(!shell.documents[0].buffer.is_dirty());
        assert_eq!(std::fs::read_to_string(&path)?, "abc");
        std::fs::remove_file(&path)?;
        shell.documents[0].buffer.insert(3, "d")?;
        shell.save_file();
        until(&mut shell, |s| !s.status.starts_with("Saved"))?;
        assert!(shell.documents[0].buffer.is_dirty());
        assert!(!path.exists());
        let binary = dir.path().join("binary.dat");
        std::fs::write(&binary, [0, 1, 2])?;
        shell.open_file(binary);
        until(&mut shell, |s| s.documents.len() == 2)?;
        assert!(shell.documents[1].binary);
        shell.save_file();
        assert_eq!(shell.status, "Binary file not supported");
        let _ = ctx.run(egui::RawInput::default(), |ctx| shell.editor_area(ctx));
        Ok(())
    }
}
