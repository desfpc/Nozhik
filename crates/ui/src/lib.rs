//! Native UI shell. UI state and text buffers belong to the main thread.
mod app_ui;
pub mod editor_view;
pub mod search_everywhere;
pub mod theme;

use nozhik_buffer::TextBuffer;
use nozhik_config::{Config, ConfigManager};
use nozhik_core::BufferId;
use nozhik_event_bus::{Event, EventBus};
use tokio::sync::broadcast;

/// Best-effort action opening Search Everywhere through the shared bus.
pub const SEARCH_ACTION: &str = "ui.search_everywhere";
/// Best-effort action opening the settings modal.
pub const SETTINGS_ACTION: &str = "ui.settings";

/// One in-memory document; closing its tab retains its unsaved buffer.
pub struct Document {
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
    /// Version control placeholder.
    Commit,
    /// Structure placeholder.
    Structure,
}

/// Application state for eframe, independent of the composition root.
pub struct NozhikUi {
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
    /// Creates the shell with explicitly labelled, editable sample documents.
    /// No project files are read or written.
    #[must_use]
    pub fn new(bus: EventBus, settings: ConfigManager, config: Config) -> Self {
        let events = bus.subscribe();
        let documents = [
            (
                "main.rs",
                "// Nozhik — in-memory sample\nfn main() {\n    println!(\"Hello, Nozhik!\");\n}\n",
            ),
            (
                "Cargo.toml",
                "# In-memory sample\n[package]\nname = \"nozhik\"\nversion = \"0.1.0\"\n",
            ),
        ]
        .into_iter()
        .zip(1_u64..)
        .map(|((name, text), id)| {
            let mut buffer = TextBuffer::load_from_str(BufferId(id), text);
            buffer.set_event_bus(bus.clone());
            Document {
                name: name.into(),
                buffer,
                open: id == 1,
            }
        })
        .collect();
        Self {
            bus,
            events,
            settings,
            config,
            documents,
            active: Some(0),
            left: Some(LeftTool::Project),
            right: None,
            bottom: None,
            search: search_everywhere::SearchEverywhere::default(),
            settings_open: false,
            shift_down: false,
            last_shift: None,
            status: "In-memory samples".into(),
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
