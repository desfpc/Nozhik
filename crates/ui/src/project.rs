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
                    self.project = Some(tree);
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
                    self.status = format!("Saved {}", path.display());
                    self.vcs.invalidate();
                }
                Reply::Matches { token, files } => self.search.accept(token, files),
                Reply::Error(error) => {
                    if !self.integration.saving.is_empty() {
                        self.integration.save_error = Some(error.clone());
                        self.integration.saving.clear();
                    }
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
    pub(crate) fn project_panel(&mut self, ui: &mut egui::Ui) {
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
        let mut loads = Vec::new();
        let mut open = None;
        egui::ScrollArea::both().show(ui, |ui| {
            tree_row(
                ui,
                tree,
                &tree.root_path,
                &mut self.expanded,
                &mut self.selected_file,
                &mut loads,
                &mut open,
            );
        });
        for path in loads {
            if self.loading.insert(path.clone()) {
                self.queue(Command::LoadDir(path));
            }
        }
        if let Some(path) = open {
            self.open_file(path);
        }
    }
}
fn tree_row(
    ui: &mut egui::Ui,
    tree: &ProjectTree,
    path: &Path,
    expanded: &mut std::collections::HashSet<PathBuf>,
    selected: &mut Option<PathBuf>,
    loads: &mut Vec<PathBuf>,
    open: &mut Option<PathBuf>,
) {
    let is_open = expanded.contains(path);
    let name = path
        .file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy();
    let row = super::project_icons::row(ui, tree, path, &name, Some(is_open), false);
    if row.clicked() || (row.has_focus() && ui.input(|i| i.key_pressed(Key::Enter))) {
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
        loads.push(path.to_path_buf());
        ui.weak("Loading…");
        return;
    };
    ui.indent(path, |ui| {
        for child in children {
            match child {
                Node::Dir(path) => tree_row(ui, tree, path, expanded, selected, loads, open),
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
                    if row.clicked() {
                        *selected = Some(file.path.clone());
                        row.request_focus();
                    }
                    if row.double_clicked()
                        || (row.has_focus() && ui.input(|i| i.key_pressed(Key::Enter)))
                    {
                        *open = Some(file.path.clone());
                    }
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use nozhik_config::{Config, ConfigManager};
    use nozhik_event_bus::EventBus;
    use std::time::{Duration, Instant};

    fn until(
        shell: &mut NozhikUi,
        done: impl Fn(&NozhikUi) -> bool,
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
