//! Search Everywhere modal, with keyboard navigation and typed result targets.
use crate::Document;
use egui::{Context, Key};
use nozhik_fs_model::fuzzy::FileMatch;
use nozhik_plugin_host::{
    Result as HostResult,
    keymap::KeymapRegistry,
    registry::{ActionContext, ActionRegistry},
};
use std::path::PathBuf;

/// Target selected by the modal.
#[derive(Clone)]
pub enum SearchTarget {
    /// Canonical file in the project index.
    File(PathBuf),
    /// Index in the UI's document collection.
    Document(usize),
    /// Registered action identifier.
    Action(String),
}

/// Persistent modal state; filtering never mutates document text.
#[derive(Default)]
pub struct SearchEverywhere {
    /// Whether the modal is visible.
    pub open: bool,
    query: String,
    filter: usize,
    include_external: bool,
    selected: usize,
    focus: bool,
    files: Vec<FileMatch>,
    recent: Vec<PathBuf>,
    actions: Vec<(String, SearchTarget)>,
    token: u64,
    requested: Option<String>,
}

impl SearchEverywhere {
    #[cfg(test)]
    pub(crate) const fn files_filter_selected(&self) -> bool {
        self.filter == 2
    }

    /// Opens the modal and requests input focus on its first frame.
    pub const fn open(&mut self) {
        self.open = true;
        self.focus = true;
        self.selected = 0;
    }

    pub(crate) fn note_file(&mut self, path: PathBuf) {
        self.recent.retain(|p| p != &path);
        self.recent.insert(0, path);
        self.recent.truncate(30);
    }
    pub(crate) const fn open_filter(&mut self, filter: usize) {
        self.filter = filter;
        self.open();
    }
    pub(crate) fn invalidate(&mut self) {
        self.token = self.token.wrapping_add(1);
        self.requested = None;
        self.files.clear();
    }
    pub(crate) fn accept(&mut self, token: u64, files: Vec<FileMatch>) {
        if token == self.token {
            self.files = files;
        }
    }
    pub(crate) fn take_request(&mut self) -> Option<(u64, String)> {
        if !self.open || self.requested.as_ref() == Some(&self.query) {
            return None;
        }
        self.token = self.token.wrapping_add(1);
        self.requested = Some(self.query.clone());
        self.files.clear();
        Some((self.token, self.query.clone()))
    }
    fn results(&self, documents: &[Document]) -> Vec<(String, SearchTarget)> {
        let query = self.query.to_lowercase();
        let mut results = Vec::new();
        if self.filter == 6 {
            for path in &self.recent {
                let label = path.display().to_string();
                if label.to_lowercase().contains(&query) {
                    results.push((label, SearchTarget::File(path.clone())));
                }
            }
        }
        if matches!(self.filter, 0 | 2) && !query.starts_with('/') {
            for file in &self.files {
                results.push((file.relative.clone(), SearchTarget::File(file.path.clone())));
            }
        }
        if matches!(self.filter, 0 | 5) && !query.starts_with('/') {
            for (index, document) in documents.iter().enumerate() {
                let matched = if self.filter == 5 {
                    nozhik_buffer::SearchEngine::literal(&self.query)
                        .is_ok_and(|search| !search.find_all(&document.buffer).is_empty())
                } else {
                    document.name.to_lowercase().contains(&query)
                };
                if matched {
                    results.push((
                        format!("{}    open document", document.name),
                        SearchTarget::Document(index),
                    ));
                }
            }
        }
        if matches!(self.filter, 0 | 4) {
            for (label, target) in &self.actions {
                if label.to_lowercase().contains(query.trim_start_matches('/')) {
                    results.push((label.clone(), target.clone()));
                }
            }
        }
        results
    }
    /// Refreshes command results and their current shortcut labels from shared registries.
    /// # Errors
    /// Returns registry lock errors.
    pub fn set_actions(
        &mut self,
        actions: &ActionRegistry,
        keymap: &KeymapRegistry,
        context: ActionContext,
    ) -> HostResult<()> {
        let bindings = keymap.bindings()?;
        self.actions = actions
            .available(context)?
            .into_iter()
            .map(|action| {
                let shortcuts = bindings
                    .iter()
                    .filter(|b| b.action == action.id && b.when.matches(context))
                    .map(|b| b.chord.label())
                    .collect::<Vec<_>>()
                    .join(", ");
                (
                    format!("{}    {}", action.title, shortcuts),
                    SearchTarget::Action(action.id),
                )
            })
            .collect();
        Ok(())
    }

    /// Renders the modal and returns the target activated by Enter or click.
    pub fn show(&mut self, ctx: &Context, documents: &[Document]) -> Option<SearchTarget> {
        if !self.open {
            return None;
        }
        let mut activated = None;
        let modal = egui::Modal::new(egui::Id::new("search_everywhere"))
            .area(
                egui::Area::new(egui::Id::new("search_everywhere"))
                    .anchor(egui::Align2::CENTER_TOP, [0.0, 90.0]),
            )
            .show(ctx, |ui| {
                ui.set_width((ctx.screen_rect().width() - 48.0).clamp(240.0, 720.0));
                ui.horizontal_wrapped(|ui| {
                    for (index, label) in [
                        "All", "Classes", "Files", "Symbols", "Actions", "Text", "Recent",
                    ]
                    .iter()
                    .enumerate()
                    {
                        if ui.selectable_label(self.filter == index, *label).clicked() {
                            self.filter = index;
                            self.selected = 0;
                        }
                    }
                    ui.checkbox(&mut self.include_external, "Include non-project items");
                });
                ui.separator();
                let input = ui.add(
                    egui::TextEdit::singleline(&mut self.query)
                        .hint_text("Type / to see commands")
                        .desired_width(f32::INFINITY),
                );
                if self.focus {
                    input.request_focus();
                    self.focus = false;
                }
                if input.changed() {
                    self.selected = 0;
                    self.files.clear();
                    self.token = self.token.wrapping_add(1);
                }
                let results = self.results(documents);
                if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::ArrowDown))
                    && !results.is_empty()
                {
                    self.selected = (self.selected + 1) % results.len();
                }
                if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::ArrowUp))
                    && !results.is_empty()
                {
                    self.selected = (self.selected + results.len() - 1) % results.len();
                }
                self.selected = self.selected.min(results.len().saturating_sub(1));
                egui::ScrollArea::vertical()
                    .max_height(300.0)
                    .show(ui, |ui| {
                        for (index, (label, target)) in results.iter().enumerate() {
                            let row = ui.selectable_label(index == self.selected, label);
                            if row.clicked() {
                                activated = Some(target.clone());
                            }
                            if index == self.selected {
                                row.scroll_to_me(Some(egui::Align::Center));
                            }
                        }
                        if results.is_empty() {
                            ui.weak("No results");
                        }
                    });
                if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter)) {
                    activated = results.get(self.selected).map(|(_, target)| target.clone());
                }
            });
        if modal.should_close() || activated.is_some() {
            self.open = false;
        }
        activated
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SEARCH_ACTION;
    use nozhik_buffer::TextBuffer;
    use nozhik_core::BufferId;

    #[test]
    fn files_tab_uses_background_results_and_rejects_stale_queries()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut search = SearchEverywhere {
            filter: 2,
            ..Default::default()
        };
        search.open();
        let (old, _) = search.take_request().ok_or("missing first query")?;
        search.query = "mnrs".into();
        let (current, _) = search.take_request().ok_or("missing changed query")?;
        let files = vec![FileMatch {
            path: PathBuf::from("/project/src/main.rs"),
            relative: "src/main.rs".into(),
            score: 10,
        }];
        search.accept(old, files.clone());
        assert!(search.results(&[]).is_empty());
        search.accept(current, files);
        let ctx = Context::default();
        let mut result = None;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            search.show(ctx, &[]);
        });
        let input = egui::RawInput {
            events: vec![egui::Event::Key {
                key: Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            result = search.show(ctx, &[]);
        });
        assert!(matches!(result, Some(SearchTarget::File(path)) if path.ends_with("src/main.rs")));
        Ok(())
    }
    #[test]
    fn dynamic_actions_show_shortcuts_execute_and_disappear()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        use nozhik_event_bus::{Event, EventBus};
        use nozhik_plugin_host::{
            PluginManager,
            keymap::{KeyBinding, KeyChord, KeymapConfig, KeymapProfile},
            registry::{Action, When},
        };
        let bus = EventBus::new(16)?;
        let mut events = bus.subscribe();
        let host = PluginManager::with_paths(bus, Vec::new(), None)?;
        let actions = host.actions();
        actions.register(Action {
            id: "demo.run".into(),
            title: "Demo: Run".into(),
            when: When("editorOpen".into()),
            owner: Some("demo".into()),
        })?;
        let keymap = host.keymap();
        keymap.replace(KeymapConfig {
            profile: KeymapProfile::Custom,
            bindings: vec![KeyBinding {
                chord: KeyChord {
                    key: "R".into(),
                    primary: true,
                    ..KeyChord::default()
                },
                action: "demo.run".into(),
                when: When::default(),
            }],
        })?;
        let context = ActionContext {
            editor_open: true,
            ..ActionContext::default()
        };
        let mut search = SearchEverywhere {
            filter: 4,
            ..SearchEverywhere::default()
        };
        search.set_actions(&actions, &keymap, context)?;
        assert!(search.results(&[])[0].0.contains("Ctrl/Cmd+R"));
        search.open();
        let ctx = Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            search.show(ctx, &[]);
        });
        let input = egui::RawInput {
            events: vec![egui::Event::Key {
                key: Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..egui::RawInput::default()
        };
        let mut target = None;
        let _ = ctx.run(input, |ctx| {
            target = search.show(ctx, &[]);
        });
        let Some(SearchTarget::Action(id)) = target else {
            return Err("Enter did not select the dynamic action".into());
        };
        actions.execute(&id, context)?;
        assert_eq!(
            *events.try_recv()?,
            Event::ActionTriggered("demo.run".into())
        );
        actions.unregister_owner("demo")?;
        search.set_actions(&actions, &keymap, context)?;
        assert!(search.results(&[]).is_empty());
        Ok(())
    }

    #[test]
    fn keyboard_navigation_activation_and_escape() {
        let ctx = Context::default();
        let documents = vec![Document {
            name: "sample.rs".into(),
            path: PathBuf::from("sample.rs"),
            binary: false,
            buffer: TextBuffer::load_from_str(BufferId(1), ""),
            open: true,
        }];
        let mut search = SearchEverywhere::default();
        search.actions.push((
            "Search Everywhere".into(),
            SearchTarget::Action(SEARCH_ACTION.into()),
        ));
        search.open();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            search.show(ctx, &documents);
        });
        let key_input = |key| egui::RawInput {
            events: vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        };
        let _ = ctx.run(key_input(Key::ArrowDown), |ctx| {
            search.show(ctx, &documents);
        });
        assert_eq!(search.selected, 1);
        let mut result = None;
        let _ = ctx.run(key_input(Key::Enter), |ctx| {
            result = search.show(ctx, &documents);
        });
        assert!(matches!(result, Some(SearchTarget::Action(id)) if id == SEARCH_ACTION));
        assert!(!search.open);
        search.open();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            search.show(ctx, &documents);
        });
        let _ = ctx.run(key_input(Key::Escape), |ctx| {
            search.show(ctx, &documents);
        });
        assert!(!search.open);
    }
}
