//! Search Everywhere modal, with keyboard navigation and typed result targets.
use crate::{Document, SAVE_ACTION, SEARCH_ACTION, SETTINGS_ACTION};
use egui::{Context, Key};
use nozhik_fs_model::fuzzy::FileMatch;
use std::path::PathBuf;

/// Target selected by the modal.
#[derive(Clone)]
pub enum SearchTarget {
    /// Canonical file in the project index.
    File(PathBuf),
    /// Index in the UI's document collection.
    Document(usize),
    /// Registered action identifier.
    Action(&'static str),
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
    token: u64,
    requested: Option<String>,
}

impl SearchEverywhere {
    /// Opens the modal and requests input focus on its first frame.
    pub const fn open(&mut self) {
        self.open = true;
        self.focus = true;
        self.selected = 0;
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
            for (label, action) in [
                ("Search Everywhere", SEARCH_ACTION),
                ("Settings", SETTINGS_ACTION),
                ("Toggle Project Window", "ui.project"),
                ("Save File", SAVE_ACTION),
            ] {
                if label.to_lowercase().contains(query.trim_start_matches('/')) {
                    results.push((label.into(), SearchTarget::Action(action)));
                }
            }
        }
        results
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
                    for (index, label) in ["All", "Classes", "Files", "Symbols", "Actions", "Text"]
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
        assert!(matches!(result, Some(SearchTarget::Action(SEARCH_ACTION))));
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
