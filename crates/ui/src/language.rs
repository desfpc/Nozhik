//! Main-thread language state and snapshot synchronization.
use nozhik_language_support::{
    Command, Event, LanguageService, RequestKind, Response,
    lsp_types::{self, CompletionItem, Diagnostic, GotoDefinitionResponse, HoverContents, Url},
};
use std::{collections::HashMap, path::PathBuf};

#[derive(Default)]
pub struct EditorLanguage {
    pub revision: u64,
    pub spans: Vec<nozhik_language_support::highlighter::HighlightSpan>,
    pub diagnostics: Vec<Diagnostic>,
    pub syntax: Vec<Diagnostic>,
    pub syntax_theme: String,
    pub completions: Vec<CompletionItem>,
    pub selected: usize,
    pub hover: Option<String>,
    pub request: Option<(RequestKind, usize)>,
    pub request_at: Option<std::time::Instant>,
    pub token: u64,
    pub hover_token: u64,
    pub definition_token: u64,
    pub hover_byte: Option<usize>,
    pub completion_caret: usize,
}
#[derive(Default)]
pub struct LanguageUi {
    root: Option<PathBuf>,
    service: Option<LanguageService>,
    sent: HashMap<Url, u64>,
    pub views: HashMap<PathBuf, EditorLanguage>,
    token: u64,
    formats: HashMap<u64, (nozhik_core::BufferId, nozhik_config::code_style::CodeStyle)>,
    pub navigation: Option<(PathBuf, lsp_types::Position)>,
}
impl crate::NozhikUi {
    pub(crate) fn reformat_code(&mut self, ctx: &egui::Context) {
        self.language_tick(ctx);
        if self.database.active.is_some() {
            self.status = "No formatting standard for this buffer".into();
            return;
        }
        let Some(document) = self
            .active
            .and_then(|i| self.documents.get(i))
            .filter(|d| !d.binary && d.open)
        else {
            return;
        };
        let extension = document
            .path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        let Some(style) = self
            .settings_dialog
            .styles
            .selected(extension, &self.config.file_types)
            .cloned()
        else {
            self.status = "Formatting disabled: no code style selected".into();
            return;
        };
        let Ok(uri) = Url::from_file_path(&document.path) else {
            return;
        };
        let source = document.buffer.rope().to_string();
        let cursor = &document.buffer.cursors().as_slice()[0];
        let range = (cursor.anchor != cursor.head).then(|| {
            cursor.anchor.byte_offset.min(cursor.head.byte_offset)
                ..cursor.anchor.byte_offset.max(cursor.head.byte_offset)
        });
        self.language.token += 1;
        let token = self.language.token;
        let identity = document.buffer.snapshot().buffer_id;
        self.language
            .formats
            .insert(token, (identity, style.clone()));
        let command = Command::Format {
            token,
            uri,
            revision: document.buffer.snapshot().revision,
            source,
            style,
            range,
        };
        if !self
            .language
            .service
            .as_ref()
            .is_some_and(|service| service.send(command))
        {
            self.language.formats.remove(&token);
            self.status = "Language worker unavailable; retry Reformat Code".into();
        }
    }

    pub(crate) fn shutdown_language(&mut self) {
        if let Some(service) = self.language.service.take() {
            service.shutdown();
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "Routes typed language events into document and navigation state"
    )]
    pub(crate) fn language_tick(&mut self, ctx: &egui::Context) {
        let root = self
            .project
            .as_ref()
            .map(|p| p.root_path.clone())
            .or_else(|| {
                self.documents
                    .first()
                    .and_then(|d| d.path.parent().map(std::path::Path::to_path_buf))
            });
        if root != self.language.root {
            self.language = LanguageUi {
                root: root.clone(),
                ..LanguageUi::default()
            };
            if let Some(root) = root {
                let ctx = ctx.clone();
                self.language.service =
                    Some(LanguageService::start(root, move || ctx.request_repaint()));
            }
        }
        let Some(service) = &self.language.service else {
            return;
        };
        let mut open = std::collections::HashSet::new();
        for document in &self.documents {
            if !document.open
                || document.binary
                || !nozhik_language_support::lsp::manager::is_php(&document.path)
            {
                continue;
            }
            let Ok(uri) = Url::from_file_path(&document.path) else {
                continue;
            };
            open.insert(uri.clone());
            let snapshot = document.buffer.snapshot();
            let view = self
                .language
                .views
                .entry(document.path.clone())
                .or_default();
            if view.revision != snapshot.revision {
                view.revision = snapshot.revision;
                view.spans.clear();
                view.diagnostics.clear();
                view.syntax.clear();
                view.completions.clear();
                view.hover = None;
            }
            if self.language.sent.get(&uri) != Some(&snapshot.revision)
                && service.send(Command::Sync {
                    uri: uri.clone(),
                    revision: snapshot.revision,
                    text: snapshot.text,
                })
            {
                self.language.sent.insert(uri.clone(), snapshot.revision);
            }
            let ready = view
                .request_at
                .is_none_or(|time| time.elapsed() >= std::time::Duration::from_millis(200));
            if !ready && let Some(at) = view.request_at {
                ctx.request_repaint_after(
                    std::time::Duration::from_millis(200).saturating_sub(at.elapsed()),
                );
            }
            if ready && let Some((kind, byte)) = view.request.take() {
                let source = document.buffer.rope().to_string();
                if byte > source.len() || !source.is_char_boundary(byte) {
                    continue;
                }
                self.language.token += 1;
                let token = self.language.token;
                match kind {
                    RequestKind::Completion => {
                        view.token = token;
                        view.completion_caret = byte;
                    }
                    RequestKind::Hover => view.hover_token = token,
                    RequestKind::Definition => view.definition_token = token,
                }
                if !service.send(Command::Request {
                    uri,
                    revision: snapshot.revision,
                    token,
                    position: nozhik_language_support::position(&source, byte),
                    kind,
                }) {
                    view.request = Some((kind, byte));
                }
            }
        }
        #[allow(
            clippy::needless_collect,
            reason = "Closed URIs must be owned before removing from the source map"
        )]
        for uri in self
            .language
            .sent
            .keys()
            .filter(|u| !open.contains(*u))
            .cloned()
            .collect::<Vec<_>>()
        {
            if service.send(Command::Close(uri.clone())) {
                self.language.sent.remove(&uri);
                if let Ok(path) = uri.to_file_path() {
                    self.language.views.remove(&path);
                }
            }
        }
        for event in service.drain() {
            match event {
                Event::Formatted {
                    uri,
                    revision,
                    token,
                    edits,
                } => {
                    let Some((identity, style)) = self.language.formats.remove(&token) else {
                        continue;
                    };
                    if let Ok(path) = uri.to_file_path()
                        && let Some(document) =
                            self.documents.iter_mut().find(|d| d.path == path && d.open)
                        && document.buffer.snapshot().buffer_id == identity
                        && document.buffer.snapshot().revision == revision
                        && self.settings_dialog.styles.selected(
                            path.extension().and_then(|s| s.to_str()).unwrap_or(""),
                            &self.config.file_types,
                        ) == Some(&style)
                    {
                        match edits {
                            Ok(edits) => match crate::editor_view::TextEditorView::apply_format(
                                &mut document.buffer,
                                edits,
                            ) {
                                Ok(()) => self.status = "Reformat Code completed".into(),
                                Err(e) => self.status = e.to_string(),
                            },
                            Err(e) => self.status = e.to_string(),
                        }
                    } else {
                        self.status = "Reformat discarded: document or code style changed".into();
                    }
                }
                Event::Error(error) => {
                    eprintln!("Language support: {error}");
                    self.status = error;
                }
                Event::Highlights {
                    uri,
                    revision,
                    spans,
                    syntax,
                } => {
                    if let Ok(path) = uri.to_file_path()
                        && let Some(view) = self.language.views.get_mut(&path)
                        && view.revision == revision
                    {
                        view.spans = spans;
                        view.syntax = syntax;
                    }
                }
                Event::Diagnostics {
                    uri,
                    revision,
                    items,
                } => {
                    if let Ok(path) = uri.to_file_path()
                        && let Some(view) = self.language.views.get_mut(&path)
                        && view.revision == revision
                    {
                        view.diagnostics = items;
                    }
                }
                Event::Response {
                    uri,
                    revision,
                    token,
                    data,
                } => {
                    let Ok(path) = uri.to_file_path() else {
                        continue;
                    };
                    let Some(view) = self.language.views.get_mut(&path) else {
                        continue;
                    };
                    if view.revision != revision {
                        continue;
                    }
                    match data {
                        Response::Completion(items) if token == view.token => {
                            let items = match items {
                                Some(lsp_types::CompletionResponse::Array(v)) => v,
                                Some(lsp_types::CompletionResponse::List(v)) => v.items,
                                None => Vec::new(),
                            };
                            let Some(document) = self.documents.iter().find(|d| d.path == path)
                            else {
                                continue;
                            };
                            let text = document.buffer.rope().to_string();
                            let caret = document.buffer.cursors().as_slice()[0].head.byte_offset;
                            if caret != view.completion_caret {
                                continue;
                            }
                            view.completions = nozhik_language_support::completion::filter(
                                items,
                                &text[nozhik_language_support::completion::prefix(&text, caret)],
                            );
                            view.selected = 0;
                        }
                        Response::Hover(hover) if token == view.hover_token => {
                            view.hover = hover.map(|h| match h.contents {
                                HoverContents::Markup(v) => v.value,
                                HoverContents::Scalar(v) => marked(v),
                                HoverContents::Array(v) => {
                                    v.into_iter().map(marked).collect::<Vec<_>>().join("\n\n")
                                }
                            });
                        }
                        Response::Definition(result) if token == view.definition_token => {
                            let location = match result {
                                Some(GotoDefinitionResponse::Scalar(v)) => {
                                    Some((v.uri, v.range.start))
                                }
                                Some(GotoDefinitionResponse::Array(v)) => {
                                    v.into_iter().next().map(|v| (v.uri, v.range.start))
                                }
                                Some(GotoDefinitionResponse::Link(v)) => v
                                    .into_iter()
                                    .next()
                                    .map(|v| (v.target_uri, v.target_selection_range.start)),
                                None => None,
                            };
                            if let Some((uri, pos)) = location
                                && let Ok(path) = uri.to_file_path()
                            {
                                self.language.navigation = Some((path, pos));
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        if let Some((path, pos)) = self.language.navigation.clone() {
            self.open_file(path.clone());
            if let Some(document) = self.documents.iter_mut().find(|d| d.path == path) {
                if let Some(byte) =
                    nozhik_language_support::byte_offset(&document.buffer.rope().to_string(), pos)
                    && let Ok(head) = document.buffer.position_at(byte)
                {
                    if let Err(error) = document
                        .buffer
                        .set_cursors(vec![nozhik_buffer::Cursor { anchor: head, head }])
                    {
                        self.status = error.to_string();
                    }
                    self.editor.reveal_line = Some(pos.line as usize);
                }
                self.language.navigation = None;
            }
        }
    }
}
fn marked(value: lsp_types::MarkedString) -> String {
    match value {
        lsp_types::MarkedString::String(s) => s,
        lsp_types::MarkedString::LanguageString(s) => {
            format!("```{}\n{}\n```", s.language, s.value)
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn typed_dirty_buffer_reformats_nested_print_in_one_undo()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let bus = nozhik_event_bus::EventBus::new(32)?;
        let settings =
            nozhik_config::ConfigManager::from_path(dir.path().join("settings.toml"), bus.clone())?;
        let mut shell = crate::NozhikUi::new(bus, settings, nozhik_config::Config::default());
        shell.documents.push(crate::Document {
            path: dir.path().join("check.php"),
            binary: false,
            name: "check.php".into(),
            open: true,
            buffer: nozhik_buffer::TextBuffer::load_from_str(nozhik_core::BufferId(83), ""),
        });
        shell.active = Some(0);
        shell.language.root = Some(dir.path().into());
        shell.language.service = Some(nozhik_language_support::LanguageService::with_server(
            dir.path().into(),
            Some(nozhik_language_support::ServerCommand {
                program: dir.path().join("missing-lsp"),
                args: vec![],
            }),
            || {},
        ));
        let source = "function check($condition, $message) {\n    if (!$condition) {\nprint('test');\n        throw new RuntimeException($message);\n    }\n}";
        let expected = "function check($condition, $message)\n{\n    if (!$condition) {\n        print('test');\n        throw new RuntimeException($message);\n    }\n}\n";
        let ctx = egui::Context::default();
        let mut result = Ok(());
        let _frame = ctx.run(
            egui::RawInput {
                events: source
                    .chars()
                    .map(|c| egui::Event::Text(c.to_string()))
                    .collect(),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let buffer = &mut shell.documents[0].buffer;
                    ui.memory_mut(|m| {
                        m.request_focus(ui.id().with(("editor", buffer.snapshot().buffer_id)));
                    });
                    result = shell.editor.show(ui, buffer, 14.0, 4, true);
                });
            },
        );
        result?;
        assert_eq!(shell.documents[0].buffer.rope().to_string(), source);
        assert!(shell.documents[0].buffer.is_dirty());
        shell.action("code.reformat");
        shell.receive_events();
        shell.global_actions(&ctx);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while shell.documents[0].buffer.rope() != expected && std::time::Instant::now() < deadline {
            shell.language_tick(&ctx);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let buffer = &mut shell.documents[0].buffer;
        assert_eq!(buffer.rope().to_string(), expected);
        buffer.undo()?;
        assert_eq!(buffer.rope().to_string(), source);
        assert!(buffer.is_dirty());
        Ok(())
    }

    #[test]
    fn reformat_action_updates_active_buffer_and_undo_restores_source()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let bus = nozhik_event_bus::EventBus::new(32)?;
        let settings =
            nozhik_config::ConfigManager::from_path(dir.path().join("settings.toml"), bus.clone())?;
        let mut shell = crate::NozhikUi::new(bus, settings, nozhik_config::Config::default());
        let source = "class Sample {\npublic function test() {\n$this->doSomething();\n}\n}";
        shell.documents.push(crate::Document {
            path: dir.path().join("sample.php"),
            binary: false,
            name: "sample.php".into(),
            open: true,
            buffer: nozhik_buffer::TextBuffer::load_from_str(nozhik_core::BufferId(82), source),
        });
        shell.active = Some(0);
        shell.language.root = Some(dir.path().into());
        shell.language.service = Some(nozhik_language_support::LanguageService::with_server(
            dir.path().into(),
            Some(nozhik_language_support::ServerCommand {
                program: dir.path().join("missing-lsp"),
                args: vec![],
            }),
            || {},
        ));
        let ctx = egui::Context::default();
        shell.action("code.reformat");
        shell.receive_events();
        shell.global_actions(&ctx);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while !shell.documents[0].buffer.is_dirty() && std::time::Instant::now() < deadline {
            shell.language_tick(&ctx);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let buffer = &mut shell.documents[0].buffer;
        assert_eq!(
            buffer.rope().to_string(),
            "class Sample\n{\n    public function test()\n    {\n        $this->doSomething();\n    }\n}\n"
        );
        buffer.undo()?;
        assert_eq!(buffer.rope().to_string(), source);
        assert!(!buffer.is_dirty());
        Ok(())
    }
}
