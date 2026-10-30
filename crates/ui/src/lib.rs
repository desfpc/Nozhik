//! Native UI shell. UI state and text buffers belong to the main thread.
mod app_ui;
mod workspace;
pub use workspace::Workspace;
mod data_grid;
mod repository_clone;
mod welcome;
pub use data_grid::DataGrid;
mod ai_inline;
mod database_panel;
mod diff_view;
mod editor_tabs;
pub mod editor_view;
mod integration;
mod language;
mod layout;
mod menu_bar;
mod plugins;
mod project;
mod project_dialog;
mod project_icons;
mod recent_projects;
mod remote_panel;
mod remote_shell;
mod remote_worker;
pub mod search_everywhere;
mod settings_dialog;
mod sql_console;
mod terminal_panel;
mod terminal_view;
pub mod theme;
mod tool_icons;
mod usages_popup;
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

/// One open document; guarded close saves or discards edits and releases its buffer.
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
    /// Remote SSH hosts and SFTP browser.
    Remote,
    /// Database explorer and connections.
    Database,
    /// Notifications placeholder.
    Notifications,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RefreshState {
    Idle,
    Pending,
    Again,
}

/// Application state for eframe, independent of the composition root.
pub struct NozhikUi {
    workspace: workspace::SessionLinks,
    welcome: welcome::Welcome,
    integration: integration::Integration,
    tabs: editor_tabs::TabStrip,
    recent_projects: recent_projects::RecentProjects,
    layout: layout::LayoutManager,
    wake_task: Option<tokio::task::JoinHandle<()>>,
    #[cfg(target_os = "macos")]
    native_menu: Option<menu_bar::native::NativeMenu>,
    database: database_panel::DatabaseUi,
    remote: remote_panel::RemoteUi,
    terminal: Option<terminal_panel::TerminalPanel>,
    language: language::LanguageUi,
    plugins: Option<PluginManager>,
    plugins_open: bool,
    plugin_tab: Option<String>,
    editor_focus: Option<egui::Id>,
    vcs: vcs_panel::VcsPanel,
    fs: FileSystem,
    project: Option<ProjectTree>,
    project_controls: project::ProjectControls,
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
    bottom: Option<&'static str>,
    search: search_everywhere::SearchEverywhere,
    settings_open: bool,
    settings_dialog: settings_dialog::SettingsDialog,
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
                for &(id, title, when) in menu_bar::MENUS.iter().flat_map(|(_, entries)| *entries) {
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
            workspace: workspace::SessionLinks::default(),
            welcome: welcome::Welcome::default(),
            settings_dialog: settings_dialog::SettingsDialog::start(
                settings.path().to_path_buf(),
                bus.clone(),
            ),
            recent_projects: recent_projects::RecentProjects::start(
                settings.path().with_file_name("recent_projects.json"),
            ),
            integration: integration::Integration::default(),
            tabs: editor_tabs::TabStrip::default(),
            layout: layout::LayoutManager::default(),
            wake_task: None,
            #[cfg(target_os = "macos")]
            native_menu: None,
            database: database_panel::DatabaseUi::default(),
            remote: remote_panel::RemoteUi::default(),
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
            project_controls: project::ProjectControls::default(),
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
                .database
                .active
                .and_then(|i| self.database.tabs.get(i))
                .is_some_and(|t| t.open)
                || self
                    .active
                    .and_then(|i| self.documents.get(i))
                    .is_some_and(|d| d.open && !d.binary),
            editor_focus: self.editor_focus.is_some() && !self.global_modal(),
            selected_file: self.selected_file.is_some(),
            project_open: self.project.is_some(),
        }
    }

    fn open_document(&mut self, index: usize) {
        self.database.active = None;
        if let Some(document) = self.documents.get_mut(index) {
            document.open = true;
            self.search.note_file(document.path.clone());
            self.active = Some(index);
            if let Some(view) = self.language.views.get_mut(&document.path) {
                view.usages_hover_byte = None;
            }
            self.editor.focus_buffer = Some(document.buffer.snapshot().buffer_id);
            self.bus.publish(Event::FileOpened(document.path.clone()));
        }
    }
}
