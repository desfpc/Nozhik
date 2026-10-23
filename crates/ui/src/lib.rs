//! Native UI shell. UI state and text buffers belong to the main thread.
mod app_ui;
mod diff_view;
pub mod editor_view;
mod project;
pub mod search_everywhere;
pub mod theme;
mod vcs_panel;

use nozhik_buffer::TextBuffer;
use nozhik_config::{Config, ConfigManager};
use nozhik_event_bus::{Event, EventBus};
use nozhik_fs_model::{FileSystem, tree::ProjectTree};
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
        Self {
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
            editor: editor_view::TextEditorView,
        }
    }

    fn action(&self, name: &str) {
        self.bus.publish(Event::ActionTriggered(name.into()));
    }

    fn open_document(&mut self, index: usize) {
        if let Some(document) = self.documents.get_mut(index) {
            document.open = true;
            self.active = Some(index);
        }
    }
}
