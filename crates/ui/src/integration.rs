//! Global action routing, path/search dialogs and unsaved-document shutdown gate.
use crate::{LeftTool, NozhikUi, editor_view::TextEditorView};
use nozhik_buffer::{Cursor, EditTransaction, SearchEngine, TextEdit};
use std::{collections::HashMap, path::PathBuf, sync::Arc};

#[derive(Clone, Copy)]
pub enum CloseTarget {
    Window,
    File(usize),
    Sql(usize),
}
#[derive(Clone, Copy)]
enum PathAction {
    Open,
    New,
    SaveSql(usize),
}
#[derive(Default)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "Independent modal, window, shutdown and presentation states"
)]
pub struct Integration {
    pub actions: Vec<String>,
    pub last_editor: Option<egui::Id>,
    path_action: Option<PathAction>,
    path: String,
    path_focus: bool,
    pub close: Option<CloseTarget>,
    waiting: bool,
    pub confirmed: bool,
    pub stopped: bool,
    pub saving: HashMap<PathBuf, u64>,
    pub save_error: Option<String>,
    find: bool,
    query: String,
    replacement: String,
    about: bool,
    fullscreen: bool,
}
impl Integration {
    pub(crate) const fn blocking(&self) -> bool {
        self.close.is_some() || self.path_action.is_some()
    }
}
impl NozhikUi {
    /// Connect event-driven UI wakeup to the composition root's runtime.
    pub fn attach_runtime(&mut self, ctx: &egui::Context, runtime: &tokio::runtime::Handle) {
        if self.wake_task.is_some() {
            return;
        }
        let mut events = self.bus.subscribe();
        let mut updates = self
            .plugins
            .as_ref()
            .map(nozhik_plugin_host::PluginManager::subscribe_updates);
        let wake = ctx.clone();
        self.wake_task=Some(runtime.spawn(async move {
            loop {tokio::select! {
                event=events.recv()=>match event {Ok(_)|Err(tokio::sync::broadcast::error::RecvError::Lagged(_))=>wake.request_repaint(),Err(tokio::sync::broadcast::error::RecvError::Closed)=>break},
                notice=async {if let Some(updates)=&mut updates {updates.changed().await}else{std::future::pending().await}}=>{if notice.is_err(){updates=None;}wake.request_repaint();}
            }}
        }));
        let wake = ctx.clone();
        self.layout.start(
            self.settings.path().with_file_name("layout.json"),
            Arc::new(move || wake.request_repaint()),
        );
        #[cfg(target_os = "macos")]
        match crate::menu_bar::native::NativeMenu::new(ctx) {
            Ok(menu) => self.native_menu = Some(menu),
            Err(e) => self.status = format!("Native menu: {e}"),
        }
    }
    pub(crate) const fn global_modal(&self) -> bool {
        self.integration.close.is_some()
            || self.integration.path_action.is_some()
            || self.search.open
            || self.settings_open
            || self.plugins_open
            || self.remote.modal_open()
            || self.database.modal_open()
            || self.integration.about
    }
    pub(crate) fn active_buffer(&mut self) -> Option<&mut nozhik_buffer::TextBuffer> {
        if let Some(index) = self.database.active {
            self.database.tabs.get_mut(index).map(|t| &mut t.buffer)
        } else {
            self.active
                .and_then(|i| self.documents.get_mut(i))
                .filter(|d| d.open && !d.binary)
                .map(|d| &mut d.buffer)
        }
    }
    pub(crate) fn close_target_dirty(&self, target: CloseTarget) -> bool {
        match target {
            CloseTarget::Window => {
                self.documents.iter().any(|d| d.buffer.is_dirty())
                    || self.database.tabs.iter().any(|t| t.buffer.is_dirty())
            }
            CloseTarget::File(i) => self.documents.get(i).is_some_and(|d| d.buffer.is_dirty()),
            CloseTarget::Sql(i) => self
                .database
                .tabs
                .get(i)
                .is_some_and(|t| t.buffer.is_dirty()),
        }
    }
    pub(crate) fn request_close(&mut self, target: CloseTarget, ctx: &egui::Context) {
        if self.close_target_dirty(target) {
            self.integration.close = Some(target);
            self.integration.waiting = false;
            self.integration.save_error = None;
        } else {
            self.finish_close(target, ctx);
        }
    }
    fn finish_close(&mut self, target: CloseTarget, ctx: &egui::Context) {
        self.integration.close = None;
        match target {
            CloseTarget::Window => {
                self.integration.confirmed = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            CloseTarget::File(i) => {
                if i < self.documents.len() {
                    self.documents.remove(i);
                }
                if let Some(active) = self.active {
                    self.active = if active == i {
                        self.documents.iter().position(|d| d.open)
                    } else {
                        Some(if active > i { active - 1 } else { active })
                    };
                }
            }
            CloseTarget::Sql(i) => {
                if i < self.database.tabs.len() {
                    self.database.tabs.remove(i);
                }
                if let Some(active) = self.database.active {
                    self.database.active = if active == i {
                        None
                    } else {
                        Some(if active > i { active - 1 } else { active })
                    };
                }
            }
        }
    }
    fn discard_close(&mut self, target: CloseTarget, ctx: &egui::Context) {
        self.integration.waiting = false;
        self.finish_close(target, ctx);
    }

    pub(crate) fn save_all(&mut self) {
        self.integration.save_error = None;
        for i in 0..self.documents.len() {
            if self.documents[i].buffer.is_dirty() {
                self.save_document(i);
            }
        }
        for i in 0..self.database.tabs.len() {
            if self.database.tabs[i].buffer.is_dirty() {
                self.save_sql(i);
            }
        }
    }
    pub(crate) fn save_sql(&mut self, index: usize) {
        let Some(tab) = self.database.tabs.get(index) else {
            return;
        };
        let Some(path) = tab.path.clone() else {
            if self.integration.path_action.is_none() {
                self.integration.path_action = Some(PathAction::SaveSql(index));
                self.integration.path_focus = true;
                self.integration.path.clear();
            }
            return;
        };
        let snapshot = tab.buffer.snapshot();
        if self.integration.saving.get(&path) == Some(&snapshot.revision) {
            return;
        }
        if self.fs.send(nozhik_fs_model::Command::Save {
            path: path.clone(),
            revision: snapshot.revision,
            text: snapshot.text,
        }) {
            self.integration.saving.insert(path, snapshot.revision);
        } else {
            self.integration.save_error = Some("File worker stopped".into());
        }
    }
    pub(crate) fn saved(&mut self, path: &std::path::Path, revision: u64) {
        if self.integration.saving.get(path) == Some(&revision) {
            self.integration.saving.remove(path);
        }
        for tab in &mut self.database.tabs {
            if tab.path.as_deref() == Some(path) && tab.buffer.snapshot().revision == revision {
                tab.buffer.mark_saved();
            }
        }
    }
    #[allow(
        clippy::too_many_lines,
        reason = "One dispatcher connects registered actions to existing subsystem entry points"
    )]
    pub(crate) fn global_actions(&mut self, ctx: &egui::Context) {
        for action in std::mem::take(&mut self.integration.actions) {
            match action.as_str() {
                "ui.new_file" | "ui.open" => {
                    self.integration.path_action = Some(if action == "ui.open" {
                        PathAction::Open
                    } else {
                        PathAction::New
                    });
                    self.integration.path_focus = true;
                    self.integration.path = self
                        .project
                        .as_ref()
                        .map_or_else(String::new, |p| p.root_path.display().to_string());
                }
                "ui.save_all" => self.save_all(),
                "ui.close_tab" => {
                    if let Some(i) = self.database.active {
                        self.request_close(CloseTarget::Sql(i), ctx);
                    } else if let Some(i) = self.active {
                        self.request_close(CloseTarget::File(i), ctx);
                    }
                }
                "ui.exit" => self.request_close(CloseTarget::Window, ctx),
                "ui.undo" | "ui.redo" => {
                    if let Some(buffer) = self.active_buffer() {
                        let result = if action == "ui.undo" {
                            buffer.undo()
                        } else {
                            buffer.redo()
                        };
                        if let Err(e) = result
                            && !matches!(e, nozhik_buffer::BufferError::EmptyHistory)
                        {
                            self.status = e.to_string();
                        }
                    }
                }
                "ui.cut" | "ui.copy" | "ui.paste" => {
                    if action == "ui.copy"
                        && let Some(text) = self
                            .database
                            .active
                            .and_then(|i| self.database.tabs.get(i))
                            .and_then(|t| t.results.get(t.selected_result))
                            .and_then(crate::DataGrid::selected_copy)
                    {
                        ctx.copy_text(text);
                        continue;
                    }
                    // RequestPaste reads the system clipboard through egui-winit; native menu clicks restore editor focus.
                    if let Some(buffer) = self.active_buffer() {
                        let text = selected_text(buffer);
                        if action != "ui.paste" {
                            ctx.copy_text(text);
                        }
                        if action == "ui.cut"
                            && let Err(e) = TextEditorView::replace(buffer, "", None)
                        {
                            self.status = e.to_string();
                        }
                    }
                    if action == "ui.paste" {
                        self.integration.actions.push("ui.paste_next_frame".into());
                        ctx.request_repaint();
                    }
                }
                "ui.paste_next_frame" => {
                    if let Some(id) = self.integration.last_editor {
                        ctx.memory_mut(|m| m.request_focus(id));
                    }
                    ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste);
                }
                "ui.find" | "ui.replace" => self.integration.find = true,
                "ui.database" => {
                    self.right = if self.right == Some("Database") {
                        None
                    } else {
                        Some("Database")
                    }
                }
                "ui.remote" => {
                    self.left = if self.left == Some(LeftTool::Remote) {
                        None
                    } else {
                        Some(LeftTool::Remote)
                    }
                }
                "ui.fullscreen" => {
                    self.integration.fullscreen = !self.integration.fullscreen;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(
                        self.integration.fullscreen,
                    ));
                }
                "ui.go_file" => self.search.open_filter(2),
                "ui.recent_files" => self.search.open_filter(6),
                "ui.definition" => {
                    if let Some(i) = self.active
                        && let Some(d) = self.documents.get(i)
                        && let Some(view) = self.language.views.get_mut(&d.path)
                    {
                        view.request = Some((
                            nozhik_language_support::RequestKind::Definition,
                            d.buffer.cursors().as_slice()[0].head.byte_offset,
                        ));
                        ctx.request_repaint();
                    }
                }
                "ui.commit" | "ui.branches" => {
                    self.left = Some(LeftTool::Commit);
                    self.vcs.open_branches = action == "ui.branches";
                }
                "ui.fetch" => self.vcs.run_global(nozhik_vcs::Task::Fetch),
                "ui.pull" => self.vcs.run_global(nozhik_vcs::Task::Pull),
                "ui.push" => self.vcs.run_global(nozhik_vcs::Task::Push),
                "ui.new_connection" => {
                    self.right = Some("Database");
                    self.database.new_connection();
                }
                "ui.sql_console" => {
                    self.right = Some("Database");
                    self.open_database_console();
                }
                "ui.new_terminal" => {
                    self.bottom = Some(crate::terminal_panel::TERMINAL_LABEL);
                    let root = self.project.as_ref().map(|p| p.root_path.clone());
                    let panel = self
                        .terminal
                        .get_or_insert_with(|| crate::terminal_panel::TerminalPanel::new(ctx));
                    if let Some(root) = root
                        && let Some(e) = panel.create(&root)
                    {
                        self.status = e;
                    }
                }
                "ui.about" => self.integration.about = true,
                _ => {}
            }
        }
    }
    pub(crate) fn integration_tick(&mut self, ctx: &egui::Context) {
        self.layout.tick(ctx);
        if ctx.input(|i| i.viewport().close_requested()) && !self.integration.confirmed {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if self.integration.close.is_none() {
                self.request_close(CloseTarget::Window, ctx);
            }
        }
        self.global_actions(ctx);
        if self.integration.waiting && self.integration.path_action.is_none() {
            // Resolve further unsaved SQL destinations one at a time.
            if self
                .database
                .tabs
                .iter()
                .any(|t| t.buffer.is_dirty() && t.path.is_none())
            {
                self.save_all();
            }
            if self.integration.save_error.is_some() {
                self.integration.waiting = false;
            } else if self.integration.saving.is_empty()
                && let Some(target) = self.integration.close
                && !self.close_target_dirty(target)
            {
                self.finish_close(target, ctx);
            }
        }
    }
    pub(crate) fn integration_dialogs(&mut self, ctx: &egui::Context) {
        self.path_dialog(ctx);
        if let Some(target) = self.integration.close
            && self.integration.path_action.is_none()
        {
            let mut save = false;
            let mut discard = false;
            let mut cancel = false;
            egui::Modal::new(egui::Id::new("unsaved_close")).show(ctx, |ui| {
                ui.heading("Unsaved changes");
                ui.label("Save modified files before closing?");
                if let Some(error) = &self.integration.save_error {
                    ui.colored_label(crate::theme::ERROR, error);
                }
                ui.horizontal(|ui| {
                    save = ui
                        .add_enabled(!self.integration.waiting, egui::Button::new("Save All"))
                        .clicked();
                    discard = ui
                        .add_enabled(!self.integration.waiting, egui::Button::new("Discard"))
                        .clicked();
                    cancel = ui.button("Cancel").clicked();
                });
                if self.integration.waiting {
                    ui.spinner();
                    ui.label("Waiting for saved revisions…");
                }
            });
            if save {
                self.save_all();
                self.integration.waiting = true;
            }
            if discard {
                self.discard_close(target, ctx);
            }
            if cancel {
                self.integration.close = None;
                self.integration.waiting = false;
            }
        }
        if self.integration.about {
            let mut open = true;
            egui::Window::new("About Nozhik")
                .open(&mut open)
                .show(ctx, |ui| {
                    ui.heading("Nozhik v0.1.0-RC1");
                    ui.label(format!(
                        "Base commit {} · {}",
                        env!("NOZHIK_COMMIT"),
                        env!("NOZHIK_RUSTC")
                    ));
                    ui.label(format!(
                        "{} / {} · egui 0.32 / Glow · Tokio workers",
                        std::env::consts::OS,
                        std::env::consts::ARCH
                    ));
                    ui.label("Core Editor · Git · PHP LSP · Terminal · SSH/SFTP · SQL");
                });
            self.integration.about = open;
        }
    }
    fn path_dialog(&mut self, ctx: &egui::Context) {
        let Some(action) = self.integration.path_action else {
            return;
        };
        let mut accept = false;
        let mut cancel = false;
        let modal = egui::Modal::new(egui::Id::new("path_dialog")).show(ctx, |ui| {
            ui.heading(match action {
                PathAction::Open => "Open file or folder",
                PathAction::New => "New file",
                PathAction::SaveSql(_) => "Save SQL console",
            });
            ui.label("Absolute path");
            let input = ui.text_edit_singleline(&mut self.integration.path);
            accept = ui.button("Open / Save").clicked()
                || (input.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
            cancel = ui.button("Cancel").clicked();
        });
        if accept {
            let path = PathBuf::from(&self.integration.path);
            if !path.is_absolute() {
                self.status = "Enter an absolute path".into();
                return;
            }
            match action {
                PathAction::Open => self.queue(nozhik_fs_model::Command::OpenPath(path)),
                PathAction::New => self.queue(nozhik_fs_model::Command::Create(path)),
                PathAction::SaveSql(i) => {
                    if let Some(tab) = self.database.tabs.get_mut(i) {
                        tab.path = Some(path);
                    }
                    self.save_sql(i);
                }
            }
            self.integration.path_action = None;
        }
        if cancel || modal.should_close() {
            self.integration.path_action = None;
            self.integration.waiting = false;
        }
    }
    pub(crate) fn find_bar(&mut self, ui: &mut egui::Ui) {
        if !self.integration.find {
            return;
        }
        let mut find = false;
        let mut replace = false;
        let mut all = false;
        ui.horizontal(|ui| {
            ui.label("Find");
            ui.text_edit_singleline(&mut self.integration.query);
            ui.label("Replace");
            ui.text_edit_singleline(&mut self.integration.replacement);
            find = ui.button("Next").clicked();
            replace = ui.button("Replace").clicked();
            all = ui.button("Replace All").clicked();
            if ui.button("×").clicked() {
                self.integration.find = false;
            }
        });
        if !find && !replace && !all {
            return;
        }
        if self.integration.query.is_empty() {
            return;
        }
        let query = self.integration.query.clone();
        let replacement = self.integration.replacement.clone();
        let Some(buffer) = self.active_buffer() else {
            return;
        };
        let result = (|| -> Result<Option<usize>, Box<dyn std::error::Error>> {
            let mut matches = SearchEngine::literal(&query)?.find_all(buffer);
            if all {
                let edits = matches
                    .into_iter()
                    .map(|range| TextEdit {
                        range,
                        text: replacement.clone(),
                    })
                    .collect();
                buffer.apply_transaction(EditTransaction { edits, group: None })?;
                return Ok(None);
            }
            let mut caret = buffer.cursors().as_slice()[0].head.byte_offset;
            if replace {
                let c = buffer.cursors().as_slice()[0];
                let range = c.anchor.byte_offset.min(c.head.byte_offset)
                    ..c.anchor.byte_offset.max(c.head.byte_offset);
                if buffer.rope().byte_slice(range) == query {
                    TextEditorView::replace(buffer, &replacement, None)?;
                    matches = SearchEngine::literal(&query)?.find_all(buffer);
                    caret = buffer.cursors().as_slice()[0].head.byte_offset;
                }
            }
            if let Some(range) = matches
                .iter()
                .find(|r| r.start >= caret)
                .or_else(|| matches.first())
            {
                let anchor = buffer.position_at(range.start)?;
                let head = buffer.position_at(range.end)?;
                buffer.set_cursors(vec![Cursor { anchor, head }])?;
                return Ok(Some(usize::try_from(head.line)?));
            }
            Ok(None)
        })();
        match result {
            Ok(line) => {
                if let Some(index) = self.database.active
                    && let Some(tab) = self.database.tabs.get_mut(index)
                {
                    tab.reveal(line);
                } else {
                    self.editor.reveal_line = line;
                }
            }
            Err(e) => self.status = e.to_string(),
        }
    }
    /// Idempotent bounded teardown; called by eframe before the app event runtime stops.
    pub fn shutdown_systems(&mut self) {
        if self.integration.stopped {
            return;
        }
        self.integration.stopped = true;
        if let Some(task) = self.wake_task.take() {
            task.abort();
        }
        self.layout.finish();
        self.shutdown_language();
        if let Some(panel) = &mut self.terminal {
            panel.shutdown();
        }
        self.database.shutdown();
        self.remote.shutdown();
        self.plugins = None;
    }
}
fn selected_text(buffer: &nozhik_buffer::TextBuffer) -> String {
    buffer
        .cursors()
        .as_slice()
        .iter()
        .map(|c| {
            buffer
                .rope()
                .byte_slice(
                    c.anchor.byte_offset.min(c.head.byte_offset)
                        ..c.anchor.byte_offset.max(c.head.byte_offset),
                )
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}
impl Drop for NozhikUi {
    fn drop(&mut self) {
        self.shutdown_systems();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nozhik_config::{Config, ConfigManager};
    use nozhik_event_bus::EventBus;
    use std::time::{Duration, Instant};
    fn shell(path: &std::path::Path) -> Result<NozhikUi, Box<dyn std::error::Error>> {
        let bus = EventBus::new(128)?;
        let settings = ConfigManager::from_path(path.join("settings.toml"), bus.clone())?;
        Ok(NozhikUi::new(bus, settings, Config::default()))
    }
    #[test]
    fn menu_catalog_dispatches_and_save_all_waits_for_current_revision()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let mut shell = shell(dir.path())?;
        let ctx = egui::Context::default();
        let manager = shell.plugins.as_ref().ok_or("plugin host missing")?;
        let context = nozhik_plugin_host::registry::ActionContext {
            editor_open: true,
            editor_focus: true,
            project_open: true,
            selected_file: true,
        };
        let available = manager.actions().available(context)?;
        for (id, _, _) in crate::menu_bar::MENUS
            .iter()
            .flat_map(|(_, entries)| *entries)
        {
            assert!(available.iter().any(|a| a.id == *id), "missing {id}");
        }
        let path = dir.path().join("save.txt");
        std::fs::write(&path, "original")?;
        shell.loaded_file(path.clone(), "original", false);
        shell.documents[0].buffer.insert(0, "changed ")?;
        shell.request_close(CloseTarget::Window, &ctx);
        assert!(shell.integration.close.is_some());
        assert!(!shell.integration.confirmed);
        shell.action("ui.save_all");
        shell.receive_events();
        shell.global_actions(&ctx);
        shell.integration.waiting = true;
        // A newer revision must not be marked saved by an older acknowledgement.
        shell.documents[0].buffer.insert(0, "newer ")?;
        let deadline = Instant::now() + Duration::from_secs(3);
        while !shell.integration.saving.is_empty() && Instant::now() < deadline {
            shell.receive_files();
            std::thread::sleep(Duration::from_millis(5));
        }
        shell.integration_tick(&ctx);
        assert!(shell.documents[0].buffer.is_dirty());
        assert!(!shell.integration.confirmed);
        shell.save_all();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !shell.integration.saving.is_empty() && Instant::now() < deadline {
            shell.receive_files();
            std::thread::sleep(Duration::from_millis(5));
        }
        shell.integration_tick(&ctx);
        assert!(shell.integration.confirmed);
        assert_eq!(std::fs::read_to_string(path)?, "newer changed original");
        Ok(())
    }
    #[test]
    fn failed_save_blocks_exit_and_discard_tab_releases_dirty_buffer()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let mut shell = shell(dir.path())?;
        let ctx = egui::Context::default();
        let parent = dir.path().join("not-a-directory");
        std::fs::write(&parent, "file")?;
        shell.loaded_file(parent.join("blocked.txt"), "original", false);
        shell.documents[0].buffer.insert(0, "changed")?;
        shell.request_close(CloseTarget::Window, &ctx);
        shell.save_all();
        shell.integration.waiting = true;
        let deadline = Instant::now() + Duration::from_secs(3);
        while shell.integration.save_error.is_none() && Instant::now() < deadline {
            shell.receive_files();
            std::thread::sleep(Duration::from_millis(5));
        }
        shell.integration_tick(&ctx);
        assert!(shell.integration.save_error.is_some());
        assert!(!shell.integration.confirmed);
        assert!(shell.documents[0].buffer.is_dirty());
        shell.discard_close(CloseTarget::File(0), &ctx);
        assert!(shell.documents.is_empty());
        Ok(())
    }
    #[test]
    fn settled_empty_workspace_does_not_schedule_periodic_frames()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let mut shell = shell(dir.path())?;
        let ctx = egui::Context::default();
        let mut delay = Duration::ZERO;
        for i in 0..30 {
            let frame = ctx.run(
                egui::RawInput {
                    time: Some(f64::from(i)),
                    ..egui::RawInput::default()
                },
                |ctx| shell.update_frame(ctx),
            );
            delay = frame
                .viewport_output
                .get(&egui::ViewportId::ROOT)
                .map_or(Duration::MAX, |v| v.repaint_delay);
            if i == 0 {
                std::thread::sleep(Duration::from_millis(30));
            }
        }
        assert!(
            delay > Duration::from_secs(1),
            "Idle frames must sleep until input/worker wake, got {delay:?}"
        );
        Ok(())
    }
}

#[cfg(test)]
mod splitter_tests {
    use super::*;
    use nozhik_config::{Config, ConfigManager};
    use nozhik_event_bus::EventBus;
    #[test]
    fn dragging_left_splitter_persists_proportion_and_clean_close_releases_buffer()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let bus = EventBus::new(64)?;
        let settings = ConfigManager::from_path(dir.path().join("settings.toml"), bus.clone())?;
        let mut shell = NozhikUi::new(bus, settings, Config::default());
        let ctx = egui::Context::default();
        shell
            .layout
            .start(dir.path().join("layout.json"), Arc::new(|| {}));
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
        let _frame = ctx.run(
            egui::RawInput {
                screen_rect: Some(screen),
                ..egui::RawInput::default()
            },
            |ctx| shell.update_frame(ctx),
        );
        let rect = egui::containers::panel::PanelState::load(&ctx, egui::Id::new("left_dock"))
            .ok_or("no left dock")?
            .rect;
        let from = rect.right_center();
        let to = from + egui::vec2(80.0, 0.0);
        let before = shell.layout.value.left;
        for events in [
            vec![egui::Event::PointerMoved(from)],
            vec![egui::Event::PointerButton {
                pos: from,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
            vec![egui::Event::PointerMoved(to)],
            vec![egui::Event::PointerButton {
                pos: to,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        ] {
            let _frame = ctx.run(
                egui::RawInput {
                    screen_rect: Some(screen),
                    events,
                    ..egui::RawInput::default()
                },
                |ctx| shell.update_frame(ctx),
            );
        }
        assert!(
            shell.layout.value.left > before,
            "splitter failed: {} <= {before}",
            shell.layout.value.left
        );
        shell.layout.finish();
        let saved: crate::layout::Layout =
            serde_json::from_slice(&std::fs::read(dir.path().join("layout.json"))?)?;
        assert_eq!(saved, shell.layout.value);
        shell.loaded_file(dir.path().join("large.txt"), &"x".repeat(1_000_000), false);
        shell.request_close(CloseTarget::File(0), &ctx);
        assert!(shell.documents.is_empty());
        Ok(())
    }
}
