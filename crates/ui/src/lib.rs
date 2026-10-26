//! Native UI shell. UI state and text buffers belong to the main thread.
mod app_ui;
mod diff_view;
pub mod editor_view;
mod language;
mod plugins;
mod project;
pub mod search_everywhere;
mod terminal_panel;
mod terminal_view;
pub mod theme;
mod vcs_panel;

use nozhik_buffer::TextBuffer;
use nozhik_config::{Config, ConfigManager};
use nozhik_event_bus::{Event, EventBus};
use nozhik_fs_model::{FileSystem, tree::ProjectTree};
use nozhik_plugin_host::{
    PluginManager,
    registry::{Action, ActionContext, When},
};
use std::{collections::HashSet, path::PathBuf};
use tokio::sync::broadcast;

/// Best-effort action opening Search Everywhere through the shared bus.
pub const SEARCH_ACTION: &str = "ui.search_everywhere";
/// Best-effort action opening the settings modal.
pub const SETTINGS_ACTION: &str = "ui.settings";
/// Best-effort action saving the active file through the I/O worker.
pub const SAVE_ACTION: &str = "ui.save_file";

/// One in-memory document; closing its tab retains its unsaved buffer.
pub struct Document {
    /// Canonical native file identity.
    pub path: PathBuf,
    /// Binary placeholder; prevents editing and saving.
    pub binary: bool,
    /// Label in tabs and project tree.
    pub name: String,
    /// Authoritative text and history.
    pub buffer: TextBuffer,
    /// Whether its tab is open.
    pub open: bool,
}

/// Mutually exclusive tool windows on the left dock.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LeftTool {
    /// Project tree.
    Project,
    /// Git Changes and History.
    Commit,
    /// Structure placeholder.
    Structure,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RefreshState {
    Idle,
    Pending,
    Again,
}

/// Application state for eframe, independent of the composition root.
pub struct NozhikUi {
    terminal: Option<terminal_panel::TerminalPanel>,
    language: language::LanguageUi,
    plugins: Option<PluginManager>,
    plugins_open: bool,
    plugin_tab: Option<String>,
    editor_focus: Option<egui::Id>,
    vcs: vcs_panel::VcsPanel,
    fs: FileSystem,
    project: Option<ProjectTree>,
    expanded: HashSet<PathBuf>,
    loading: HashSet<PathBuf>,
    selected_file: Option<PathBuf>,
    project_path: String,
    next_buffer_id: u64,
    refresh: RefreshState,
    bus: EventBus,
    events: broadcast::Receiver<std::sync::Arc<Event>>,
    settings: ConfigManager,
    config: Config,
    documents: Vec<Document>,
    active: Option<usize>,
    left: Option<LeftTool>,
    right: Option<&'static str>,
    bottom: Option<&'static str>,
    search: search_everywhere::SearchEverywhere,
    settings_open: bool,
    shift_down: bool,
    last_shift: Option<f64>,
    status: String,
    editor: editor_view::TextEditorView,
}

impl NozhikUi {
    /// Creates an empty shell; all project I/O is queued to a dedicated worker.
    #[must_use]
    pub fn new(bus: EventBus, settings: ConfigManager, config: Config) -> Self {
        let events = bus.subscribe();
        let fs = FileSystem::start(bus.clone());
        #[cfg(not(test))]
        let plugins = PluginManager::start(bus.clone());
        #[cfg(test)]
        let plugins = PluginManager::with_paths(bus.clone(), Vec::new(), None);
        let plugins = match plugins {
            Ok(manager) => {
                for (id, title, when) in [
                    (SEARCH_ACTION, "Search Everywhere", ""),
                    (SETTINGS_ACTION, "Settings", ""),
                    (SAVE_ACTION, "Save File", "editorOpen"),
                    ("ui.project", "Toggle Project Window", ""),
                    ("ui.git", "Toggle Git Window", ""),
                    ("ui.terminal", "Toggle Terminal Window", ""),
                    ("ui.plugins", "Plugins", ""),
                ] {
                    if let Err(error) = manager.actions().register(Action {
                        id: id.into(),
                        title: title.into(),
                        when: When(when.into()),
                        owner: None,
                    }) {
                        eprintln!("Action registration: {error}");
                    }
                }
                Some(manager)
            }
            Err(error) => {
                eprintln!("Plugin host: {error}");
                None
            }
        };
        Self {
            terminal: None,
            language: language::LanguageUi::default(),
            plugins,
            plugins_open: false,
            plugin_tab: None,
            editor_focus: None,
            vcs: vcs_panel::VcsPanel::default(),
            fs,
            project: None,
            expanded: HashSet::new(),
            loading: HashSet::new(),
            selected_file: None,
            project_path: String::new(),
            next_buffer_id: 1,
            refresh: RefreshState::Idle,
            bus,
            events,
            settings,
            config,
            documents: Vec::new(),
            active: None,
            left: Some(LeftTool::Project),
            right: None,
            bottom: None,
            search: search_everywhere::SearchEverywhere::default(),
            settings_open: false,
            shift_down: false,
            last_shift: None,
            status: "Open a project folder".into(),
            editor: editor_view::TextEditorView::default(),
        }
    }

    fn action(&self, name: &str) {
        if let Some(manager) = &self.plugins {
            if let Err(error) = manager.actions().execute(name, self.action_context()) {
                eprintln!("Action: {error}");
            }
        } else {
            self.bus.publish(Event::ActionTriggered(name.into()));
        }
    }

    fn action_context(&self) -> ActionContext {
        ActionContext {
            editor_open: self
                .active
                .and_then(|i| self.documents.get(i))
                .is_some_and(|d| d.open && !d.binary),
            editor_focus: self.editor_focus.is_some()
                && !self.search.open
                && !self.settings_open
                && !self.plugins_open,
            selected_file: self.selected_file.is_some(),
            project_open: self.project.is_some(),
        }
    }

    fn open_document(&mut self, index: usize) {
        if let Some(document) = self.documents.get_mut(index) {
            document.open = true;
            self.active = Some(index);
            self.bus.publish(Event::FileOpened(document.path.clone()));
        }
    }
}
