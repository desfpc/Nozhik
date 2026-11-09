//! Search Everywhere modal, with keyboard navigation and typed result targets.
use crate::Document;
use egui::{Context, Key};
use nozhik_fs_model::TextHit;
use nozhik_fs_model::fuzzy::FileMatch;
use nozhik_plugin_host::{
    Result as HostResult,
    keymap::KeymapRegistry,
    registry::{ActionContext, ActionRegistry},
};
use std::path::{Path, PathBuf};

/// Tabs shown in the modal. Symbols is intentionally absent.
pub(crate) const FILTERS: &[(&str, SearchFilter)] = &[
    ("All", SearchFilter::All),
    ("Classes", SearchFilter::Classes),
    ("Files", SearchFilter::Files),
    ("Actions", SearchFilter::Actions),
    ("Text", SearchFilter::Text),
    ("Recent", SearchFilter::Recent),
];

/// Which result group the modal is showing.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum SearchFilter {
    /// Files, classes, text and actions.
    #[default]
    All,
    /// Class declarations from the language index.
    Classes,
    /// Project paths, including files the language index does not parse.
    Files,
    /// Registered commands.
    Actions,
    /// Literal text in indexed files and in the rest of the project.
    Text,
    /// Recently opened paths.
    Recent,
}

/// One class row collected from the PHP or CSS index.
#[derive(Clone)]
pub(crate) struct ClassMatch {
    /// Source name.
    pub name: String,
    /// File that declares it.
    pub path: PathBuf,
    /// UTF-8 start of the name.
    pub byte: usize,
    /// UTF-8 end of the name.
    pub end: usize,
}

/// Target selected by the modal.
#[derive(Clone)]
pub enum SearchTarget {
    /// Canonical file in the project index.
    File(PathBuf),
    /// Index in the UI's document collection.
    Document(usize),
    /// Registered action identifier.
    Action(String),
    /// Open a file and select `[byte, end)`.
    Location {
        /// Canonical file.
        path: PathBuf,
        /// UTF-8 start.
        byte: usize,
        /// UTF-8 end, exclusive.
        end: usize,
    },
}

/// Persistent modal state; filtering never mutates document text.
#[derive(Default)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "Open, focus, external-items and scan flags are independent"
)]
pub struct SearchEverywhere {
    /// Whether the modal is visible.
    pub open: bool,
    query: String,
    filter: SearchFilter,
    include_external: bool,
    selected: usize,
    focus: bool,
    files: Vec<FileMatch>,
    recent: Vec<PathBuf>,
    actions: Vec<(String, SearchTarget)>,
    classes: Vec<ClassMatch>,
    class_generation: u64,
    token: u64,
    requested: Option<String>,
    text_hits: Vec<TextHit>,
    text_token: u64,
    text_requested: Option<String>,
    text_scanning: bool,
}

impl SearchEverywhere {
    #[cfg(test)]
    pub(crate) const fn files_filter_selected(&self) -> bool {
        matches!(self.filter, SearchFilter::Files)
    }

    /// Generation last applied from the language index.
    #[must_use]
    pub(crate) const fn class_generation(&self) -> u64 {
        self.class_generation
    }

    /// Replaces class rows when the language index publishes a new snapshot.
    pub(crate) fn set_classes(&mut self, generation: u64, classes: Vec<ClassMatch>) {
        if self.class_generation == generation {
            return;
        }
        self.class_generation = generation;
        self.classes = classes;
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

    pub(crate) const fn open_filter(&mut self, filter: SearchFilter) {
        self.filter = filter;
        self.open();
    }

    pub(crate) fn invalidate(&mut self) {
        self.token = self.token.wrapping_add(1);
        self.requested = None;
        self.files.clear();
        self.text_token = self.text_token.wrapping_add(1);
        self.text_requested = None;
        self.text_hits.clear();
        self.text_scanning = false;
    }

    /// Drops text hits under `path` so the next query reads that file again.
    pub(crate) fn forget_path(&mut self, path: &Path) {
        self.text_hits
            .retain(|hit| hit.path != path && !hit.path.starts_with(path));
        self.text_requested = None;
        self.text_token = self.text_token.wrapping_add(1);
        self.text_scanning = self.open && self.wants_text();
    }

    pub(crate) fn accept(&mut self, token: u64, files: Vec<FileMatch>) {
        if token == self.token {
            self.files = files;
        }
    }

    /// Appends one text-search batch. A mismatched token is ignored.
    pub(crate) fn accept_text(
        &mut self,
        token: u64,
        hits: Vec<TextHit>,
        scanning: bool,
        done: bool,
    ) {
        if token != self.text_token {
            return;
        }
        self.text_hits.extend(hits);
        self.text_scanning = scanning && !done;
    }

    pub(crate) fn take_request(&mut self) -> Option<(u64, String)> {
        if !self.open || !self.wants_files() || self.requested.as_ref() == Some(&self.query) {
            return None;
        }
        self.token = self.token.wrapping_add(1);
        self.requested = Some(self.query.clone());
        self.files.clear();
        Some((self.token, self.query.clone()))
    }

    /// Test setup for a visible query on one tab.
    #[cfg(test)]
    pub(crate) fn stage_query(&mut self, filter: SearchFilter, query: &str) {
        self.filter = filter;
        self.query = query.to_owned();
        self.open = true;
        self.requested = None;
        self.text_requested = None;
        self.files.clear();
        self.text_hits.clear();
        self.text_scanning = false;
    }

    /// Whether a content scan is still reading project files.
    #[cfg(test)]
    pub(crate) const fn scanning(&self) -> bool {
        self.text_scanning
    }

    /// Labels the modal would show for the current query.
    #[cfg(test)]
    pub(crate) fn labels(&self, documents: &[Document]) -> Vec<String> {
        self.results(documents)
            .into_iter()
            .map(|(label, _)| label)
            .collect()
    }

    /// Starts a content scan for All and Text. Empty queries do not read files.
    pub(crate) fn take_text_request(&mut self) -> Option<(u64, String)> {
        if !self.open || !self.wants_text() || self.text_requested.as_ref() == Some(&self.query) {
            return None;
        }
        self.text_token = self.text_token.wrapping_add(1);
        self.text_requested = Some(self.query.clone());
        self.text_hits.clear();
        self.text_scanning = true;
        Some((self.text_token, self.query.clone()))
    }

    const fn wants_files(&self) -> bool {
        matches!(self.filter, SearchFilter::All | SearchFilter::Files)
    }

    const fn wants_text(&self) -> bool {
        !self.query.is_empty() && matches!(self.filter, SearchFilter::All | SearchFilter::Text)
    }

    fn results(&self, documents: &[Document]) -> Vec<(String, SearchTarget)> {
        let query = self.query.to_lowercase();
        let mut results = Vec::new();
        if self.filter == SearchFilter::Recent {
            for path in &self.recent {
                let label = path.display().to_string();
                if label.to_lowercase().contains(&query) {
                    results.push((label, SearchTarget::File(path.clone())));
                }
            }
        }
        if matches!(self.filter, SearchFilter::All | SearchFilter::Classes)
            && (self.filter == SearchFilter::Classes || !self.query.is_empty())
        {
            self.push_classes(&query, &mut results);
        }
        if self.wants_files() && !self.query.starts_with('/') {
            self.push_files(&mut results);
        }
        if self.wants_text() {
            self.push_text(documents, &mut results);
        }
        if self.filter == SearchFilter::All && !self.query.starts_with('/') {
            for (index, document) in documents.iter().enumerate() {
                if document.name.to_lowercase().contains(&query) {
                    results.push((
                        format!("{}    open document", document.name),
                        SearchTarget::Document(index),
                    ));
                }
            }
        }
        if matches!(self.filter, SearchFilter::All | SearchFilter::Actions) {
            for (label, target) in &self.actions {
                if label.to_lowercase().contains(query.trim_start_matches('/')) {
                    results.push((label.clone(), target.clone()));
                }
            }
        }
        results
    }

    fn push_classes(&self, query: &str, results: &mut Vec<(String, SearchTarget)>) {
        for class in &self.classes {
            if !class.name.to_lowercase().contains(query) {
                continue;
            }
            results.push((
                format!("{}    {}", class.name, class.path.display()),
                SearchTarget::Location {
                    path: class.path.clone(),
                    byte: class.byte,
                    end: class.end,
                },
            ));
        }
    }

    fn push_files(&self, results: &mut Vec<(String, SearchTarget)>) {
        let extensions = indexed_extensions();
        let mut files = self.files.clone();
        files.sort_by(|left, right| {
            extension_indexed(&right.path, &extensions)
                .cmp(&extension_indexed(&left.path, &extensions))
                .then(right.score.cmp(&left.score))
                .then(left.relative.cmp(&right.relative))
        });
        for file in files {
            results.push((file.relative.clone(), SearchTarget::File(file.path)));
        }
    }

    fn push_text(&self, documents: &[Document], results: &mut Vec<(String, SearchTarget)>) {
        let open: Vec<&Path> = documents
            .iter()
            .filter(|document| !document.binary)
            .map(|document| document.path.as_path())
            .collect();
        let mut rows = Vec::new();
        for hit in &self.text_hits {
            if open.iter().any(|path| *path == hit.path) {
                continue;
            }
            rows.push(text_row(
                &hit.relative,
                hit.line,
                &hit.preview,
                hit.indexed,
                SearchTarget::Location {
                    path: hit.path.clone(),
                    byte: hit.byte,
                    end: hit.end,
                },
            ));
        }
        let extensions = indexed_extensions();
        for document in documents.iter().filter(|document| !document.binary) {
            let indexed = extension_indexed(&document.path, &extensions);
            for (label, target) in buffer_matches(document, &self.query) {
                rows.push(TextRow {
                    label,
                    target,
                    indexed,
                    relative: document.name.clone(),
                });
            }
        }
        rows.sort_by(|left, right| {
            right
                .indexed
                .cmp(&left.indexed)
                .then(left.relative.cmp(&right.relative))
                .then(left.label.cmp(&right.label))
        });
        results.extend(rows.into_iter().map(|row| (row.label, row.target)));
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
                    .filter(|binding| binding.action == action.id && binding.when.matches(context))
                    .map(|binding| binding.chord.label())
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
        if self.text_scanning {
            ctx.request_repaint();
        }
        let mut activated = None;
        let width = (ctx.screen_rect().width() - 48.0).clamp(280.0, 960.0);
        let list_height = results_list_height(ctx.screen_rect().height());
        let modal = egui::Modal::new(egui::Id::new("search_everywhere"))
            .area(
                egui::Area::new(egui::Id::new("search_everywhere"))
                    .anchor(egui::Align2::CENTER_TOP, [0.0, 72.0])
                    .default_size(egui::vec2(width, list_height + 150.0)),
            )
            .show(ctx, |ui| {
                ui.set_width(width);
                ui.horizontal_wrapped(|ui| {
                    for (label, filter) in FILTERS {
                        if ui
                            .selectable_label(self.filter == *filter, *label)
                            .clicked()
                        {
                            self.filter = *filter;
                            self.selected = 0;
                        }
                    }
                    ui.checkbox(&mut self.include_external, "Include non-project items");
                });
                ui.separator();
                let input = ui.add(
                    crate::form::singleline(&mut self.query)
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
                    self.requested = None;
                    self.text_hits.clear();
                    self.text_token = self.text_token.wrapping_add(1);
                    self.text_requested = None;
                    self.text_scanning = self.wants_text();
                }
                if self.text_scanning && self.wants_text() {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Searching project files…");
                    });
                }
                let results = self.results(documents);
                if ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::ArrowDown))
                    && !results.is_empty()
                {
                    self.selected = (self.selected + 1) % results.len();
                }
                if ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::ArrowUp))
                    && !results.is_empty()
                {
                    self.selected = (self.selected + results.len() - 1) % results.len();
                }
                self.selected = self.selected.min(results.len().saturating_sub(1));
                egui::ScrollArea::vertical()
                    .max_height(list_height)
                    .min_scrolled_height(list_height)
                    .auto_shrink([false, false])
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
                        if results.is_empty() && !self.text_scanning {
                            ui.weak("No results");
                        }
                    });
                if ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Enter)) {
                    activated = results.get(self.selected).map(|(_, target)| target.clone());
                }
            });
        if modal.should_close() || activated.is_some() {
            self.open = false;
        }
        activated
    }
}

struct TextRow {
    label: String,
    target: SearchTarget,
    indexed: bool,
    relative: String,
}

fn text_row(
    relative: &str,
    line: u32,
    preview: &str,
    indexed: bool,
    target: SearchTarget,
) -> TextRow {
    TextRow {
        label: format!("{relative}:{}    {preview}", line.saturating_add(1)),
        relative: relative.to_owned(),
        target,
        indexed,
    }
}

fn buffer_matches(document: &Document, query: &str) -> Vec<(String, SearchTarget)> {
    let Ok(engine) = nozhik_buffer::SearchEngine::literal(query) else {
        return Vec::new();
    };
    let mut rows = Vec::new();
    for range in engine.find_all(&document.buffer).into_iter().take(20) {
        let Ok(position) = document.buffer.position_at(range.start) else {
            continue;
        };
        let Ok(line) = usize::try_from(position.line) else {
            continue;
        };
        let Ok(source_line) = document.buffer.line(line) else {
            continue;
        };
        let preview: String = source_line.to_string().trim().chars().take(160).collect();
        rows.push((
            format!(
                "{}:{}    {preview}",
                document.name,
                position.line.saturating_add(1)
            ),
            SearchTarget::Location {
                path: document.path.clone(),
                byte: range.start,
                end: range.end,
            },
        ));
    }
    rows
}

/// Extensions parsed by the supported-language index, without a leading dot.
#[must_use]
pub(crate) fn indexed_extensions() -> Vec<String> {
    nozhik_config::languages::LANGUAGES
        .iter()
        .filter(|spec| spec.available)
        .flat_map(|spec| spec.extensions.iter().copied().map(str::to_ascii_lowercase))
        .collect()
}

pub(crate) fn extension_indexed(path: &Path, extensions: &[String]) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            extensions
                .iter()
                .any(|known| known.eq_ignore_ascii_case(ext))
        })
}

/// Height of the result list, in logical pixels. It uses most of the window.
#[must_use]
pub(crate) fn results_list_height(screen_height: f32) -> f32 {
    let screen = if screen_height.is_finite() {
        screen_height
    } else {
        900.0
    };
    let room = (screen - 180.0).max(220.0);
    if screen >= 760.0 {
        room.clamp(520.0, 880.0)
    } else {
        room.min(880.0)
    }
}

/// Class rows from the PHP symbol index and CSS selector declarations.
#[must_use]
pub(crate) fn class_matches(
    symbols: Option<&nozhik_language_support::symbol_index::SymbolIndex>,
    classes: Option<&nozhik_language_support::css_symbols::ClassIndex>,
) -> Vec<ClassMatch> {
    let mut rows = Vec::new();
    if let Some(symbols) = symbols {
        for declaration in symbols.declarations() {
            if declaration.kind != nozhik_language_support::symbol_index::SymbolKind::Class {
                continue;
            }
            rows.push(ClassMatch {
                name: declaration.name.clone(),
                path: declaration.path.clone(),
                byte: declaration.range.start,
                end: declaration.range.end,
            });
        }
    }
    if let Some(classes) = classes {
        for occurrence in classes.declarations() {
            rows.push(ClassMatch {
                name: occurrence.name.clone(),
                path: occurrence.location.path.clone(),
                byte: occurrence.location.byte,
                end: occurrence.location.byte + occurrence.location.length,
            });
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SEARCH_ACTION;
    use nozhik_buffer::TextBuffer;
    use nozhik_core::BufferId;

    fn key_input(key: Key) -> egui::RawInput {
        egui::RawInput {
            events: vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        }
    }

    #[test]
    fn symbols_tab_is_gone_and_the_result_list_uses_most_of_the_window() {
        assert!(FILTERS.iter().all(|(label, _)| *label != "Symbols"));
        assert!(FILTERS.iter().any(|(label, _)| *label == "Text"));
        assert!(results_list_height(1000.0) >= 700.0);
        assert!(results_list_height(800.0) >= 520.0);
        assert!(results_list_height(1000.0) > 300.0);
    }

    #[test]
    fn files_tab_uses_background_results_and_rejects_stale_queries()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut search = SearchEverywhere {
            filter: SearchFilter::Files,
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
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            search.show(ctx, &[]);
        });
        let mut result = None;
        let _ = ctx.run(key_input(Key::Enter), |ctx| {
            result = search.show(ctx, &[]);
        });
        assert!(matches!(result, Some(SearchTarget::File(path)) if path.ends_with("src/main.rs")));
        Ok(())
    }

    #[test]
    fn every_filter_finds_indexed_and_unindexed_project_files()
    -> Result<(), Box<dyn std::error::Error>> {
        let (mut search, documents) = needle_project();
        search.filter = SearchFilter::Files;
        let files = search.results(&documents);
        assert!(
            files[0].0.contains("Widget.php"),
            "indexed paths stay above the rest"
        );
        assert!(files[1].0.contains("readme.txt"));
        assert!(
            files
                .iter()
                .all(|(_, target)| matches!(target, SearchTarget::File(_)))
        );
        search.filter = SearchFilter::Text;
        let text = search.results(&documents);
        assert!(text.iter().any(|(label, _)| label.contains("INDEX_NEEDLE")));
        assert!(text.iter().any(|(label, _)| label.contains("PLAIN_NEEDLE")));
        assert!(text.iter().any(|(label, _)| label.contains("LIVE_NEEDLE")));
        assert!(
            text.iter()
                .all(|(label, _)| !label.contains("STALE_NEEDLE"))
        );
        assert!(text[0].0.contains("INDEX_NEEDLE") || text[0].0.contains("LIVE_NEEDLE"));
        let plain_at = text
            .iter()
            .position(|(label, _)| label.contains("PLAIN_NEEDLE"))
            .ok_or("plain text hit missing")?;
        let indexed_at = text
            .iter()
            .position(|(label, _)| label.contains("INDEX_NEEDLE"))
            .ok_or("indexed text hit missing")?;
        assert!(indexed_at < plain_at);
        search.filter = SearchFilter::Classes;
        search.query.clear();
        let classes = search.results(&documents);
        assert_eq!(classes.len(), 1);
        assert!(classes[0].0.contains("Widget"));
        assert!(matches!(classes[0].1, SearchTarget::Location { .. }));
        search.filter = SearchFilter::Actions;
        search.query = "search".into();
        let actions = search.results(&documents);
        assert!(
            actions
                .iter()
                .all(|(_, target)| matches!(target, SearchTarget::Action(_)))
        );
        assert!(actions[0].0.to_lowercase().contains("search"));
        search.filter = SearchFilter::Recent;
        search.query = "readme".into();
        let recent = search.results(&documents);
        assert!(matches!(&recent[0].1, SearchTarget::File(path) if path.ends_with("readme.txt")));
        search.filter = SearchFilter::All;
        search.query = "NEEDLE".into();
        let all = search.results(&documents);
        assert!(
            all.iter()
                .any(|(label, _)| label.contains("Widget.php") && !label.contains(':'))
        );
        assert!(
            all.iter()
                .any(|(label, _)| label.contains("readme.txt") && !label.contains(':'))
        );
        assert!(all.iter().any(|(label, _)| label.contains("INDEX_NEEDLE")));
        assert!(all.iter().any(|(label, _)| label.contains("PLAIN_NEEDLE")));
        assert!(all.iter().any(|(label, _)| label.contains("LIVE_NEEDLE")));
        assert!(all.iter().all(|(label, _)| !label.contains("STALE_NEEDLE")));
        assert!(
            all.iter()
                .all(|(_, target)| !matches!(target, SearchTarget::Action(_)))
        );
        Ok(())
    }

    #[test]
    fn stale_text_batches_are_ignored_and_a_new_query_scans_again()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut search = SearchEverywhere {
            filter: SearchFilter::Text,
            query: "NEEDLE".into(),
            ..SearchEverywhere::default()
        };
        search.open();
        let (token, _) = search.take_text_request().ok_or("missing text request")?;
        assert!(search.text_scanning);
        search.accept_text(
            token.wrapping_sub(1),
            vec![TextHit {
                path: PathBuf::from("/project/old.txt"),
                relative: "old.txt".into(),
                line: 0,
                preview: "OLD".into(),
                byte: 0,
                end: 3,
                indexed: false,
            }],
            true,
            false,
        );
        assert!(search.results(&[]).is_empty());
        search.accept_text(
            token,
            vec![TextHit {
                path: PathBuf::from("/project/notes/readme.txt"),
                relative: "notes/readme.txt".into(),
                line: 0,
                preview: "NEEDLE".into(),
                byte: 0,
                end: 6,
                indexed: false,
            }],
            false,
            true,
        );
        assert!(!search.text_scanning);
        assert_eq!(search.results(&[]).len(), 1);
        assert!(search.take_text_request().is_none());
        search.forget_path(Path::new("/project/notes/readme.txt"));
        assert!(search.results(&[]).is_empty());
        assert!(search.take_text_request().is_some());
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
            filter: SearchFilter::Actions,
            ..SearchEverywhere::default()
        };
        search.set_actions(&actions, &keymap, context)?;
        assert!(search.results(&[])[0].0.contains("Ctrl/Cmd+R"));
        search.open();
        let ctx = Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            search.show(ctx, &[]);
        });
        let mut target = None;
        let _ = ctx.run(key_input(Key::Enter), |ctx| {
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

    #[test]
    fn class_matches_keep_indexed_declarations_only() {
        use nozhik_language_support::{
            css_symbols::{ClassIndex, ClassOccurrence},
            symbol_index::{FileSymbols, LanguageId, SymbolIndex, SymbolKind},
        };
        let mut symbols = SymbolIndex::new(LanguageId::Php);
        let path = PathBuf::from("/project/src/Widget.php");
        symbols.replace(
            path.clone(),
            FileSymbols {
                declarations: vec![
                    declaration(&path, "Widget", SymbolKind::Class, 6..12),
                    declaration(&path, "run", SymbolKind::Method, 20..23),
                ],
                ..FileSymbols::default()
            },
        );
        let css_path = PathBuf::from("/project/theme.css");
        let mut css = SymbolIndex::new(LanguageId::Css);
        css.replace(
            css_path.clone(),
            FileSymbols {
                class_occurrences: vec![
                    ClassOccurrence {
                        name: "button".into(),
                        declaration: true,
                        location: location(&css_path, 1, 6),
                    },
                    ClassOccurrence {
                        name: "markup".into(),
                        declaration: false,
                        location: location(&css_path, 40, 6),
                    },
                ],
                ..FileSymbols::default()
            },
        );
        let classes = ClassIndex::from_partitions(std::iter::once(&css));
        let rows = class_matches(Some(&symbols), Some(&classes));
        let names: Vec<_> = rows.iter().map(|row| row.name.as_str()).collect();
        assert!(names.contains(&"Widget"));
        assert!(names.contains(&"button"));
        assert!(!names.contains(&"run"));
        assert!(!names.contains(&"markup"));
    }

    fn needle_project() -> (SearchEverywhere, Vec<Document>) {
        let indexed = PathBuf::from("/project/src/Widget.php");
        let plain = PathBuf::from("/project/notes/readme.txt");
        let open_path = PathBuf::from("/project/src/open.php");
        let buffer =
            TextBuffer::load_from_str(BufferId(1), "class Open { const LIVE_NEEDLE = 1; }\n");
        let documents = vec![Document {
            name: "open.php".into(),
            path: open_path.clone(),
            binary: false,
            buffer,
            open: true,
        }];
        let search = SearchEverywhere {
            query: "NEEDLE".into(),
            files: vec![
                FileMatch {
                    path: plain.clone(),
                    relative: "notes/readme.txt".into(),
                    score: 100,
                },
                FileMatch {
                    path: indexed.clone(),
                    relative: "src/Widget.php".into(),
                    score: 1,
                },
            ],
            classes: vec![ClassMatch {
                name: "Widget".into(),
                path: indexed.clone(),
                byte: 6,
                end: 12,
            }],
            text_hits: vec![
                TextHit {
                    path: indexed,
                    relative: "src/Widget.php".into(),
                    line: 1,
                    preview: "const INDEX_NEEDLE".into(),
                    byte: 20,
                    end: 32,
                    indexed: true,
                },
                TextHit {
                    path: plain.clone(),
                    relative: "notes/readme.txt".into(),
                    line: 0,
                    preview: "plain PLAIN_NEEDLE".into(),
                    byte: 6,
                    end: 17,
                    indexed: false,
                },
                TextHit {
                    path: open_path,
                    relative: "src/open.php".into(),
                    line: 0,
                    preview: "STALE_NEEDLE".into(),
                    byte: 0,
                    end: 5,
                    indexed: true,
                },
            ],
            actions: vec![(
                "Search Everywhere".into(),
                SearchTarget::Action(SEARCH_ACTION.into()),
            )],
            recent: vec![plain],
            ..SearchEverywhere::default()
        };
        (search, documents)
    }

    fn declaration(
        path: &Path,
        name: &str,
        kind: nozhik_language_support::symbol_index::SymbolKind,
        range: std::ops::Range<usize>,
    ) -> nozhik_language_support::symbol_index::SymbolDeclaration {
        nozhik_language_support::symbol_index::SymbolDeclaration {
            fqn: name.into(),
            name: name.into(),
            kind,
            path: path.to_path_buf(),
            range: range.clone(),
            line: 1,
            column: u32::try_from(range.start).unwrap_or(0),
            owner: None,
            base_types: Vec::new(),
            trait_types: Vec::new(),
            scope: 0..40,
            visibility: nozhik_language_support::symbol_index::Visibility::Public,
            is_static: false,
            type_name: None,
            type_source: None,
            signature: name.into(),
            documentation: String::new(),
            usages_count: 0,
        }
    }

    fn location(
        path: &Path,
        byte: usize,
        length: usize,
    ) -> nozhik_language_support::symbol_index::ReferenceLocation {
        nozhik_language_support::symbol_index::ReferenceLocation {
            path: path.to_path_buf(),
            byte,
            length,
            line: 0,
            column: 0,
            preview: std::sync::Arc::from("button"),
            preview_offset: 0,
        }
    }
}
