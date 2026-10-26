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
    pub navigation: Option<(PathBuf, lsp_types::Position)>,
}
impl crate::NozhikUi {
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
        let root = self.project.as_ref().map(|p| p.root_path.clone());
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
                Event::Error(error) => {
                    eprintln!("Language support: {error}");
                    self.status = error;
                }
                Event::Highlights {
                    uri,
                    revision,
                    spans,
                } => {
                    if let Ok(path) = uri.to_file_path()
                        && let Some(view) = self.language.views.get_mut(&path)
                        && view.revision == revision
                    {
                        view.spans = spans;
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
