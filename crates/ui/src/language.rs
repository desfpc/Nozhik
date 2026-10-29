//! Main-thread language state and snapshot synchronization.
use nozhik_language_support::{
    Command, Event, LanguageService, RequestKind, Response,
    lsp_types::{self, CompletionItem, Diagnostic, GotoDefinitionResponse, HoverContents, Url},
};
use std::{collections::HashMap, path::PathBuf};

pub struct DocumentationSource {
    pub language_id: String,
    pub label: String,
    pub provider: std::sync::Arc<
        dyn nozhik_language_support::doc_provider::LanguageDocProvider + Send + Sync,
    >,
}
impl From<std::sync::Arc<nozhik_language_support::php_stubs::PhpVersionStubs>>
    for DocumentationSource
{
    fn from(catalog: std::sync::Arc<nozhik_language_support::php_stubs::PhpVersionStubs>) -> Self {
        Self {
            language_id: catalog.doc_rules.language_id.clone(),
            label: catalog.doc_rules.label.clone(),
            provider: catalog,
        }
    }
}
#[derive(Default)]
pub struct EditorLanguage {
    pub documentation: Option<DocumentationSource>,
    pub triggers: nozhik_language_support::triggers::LanguageTriggerConfig,
    pub project_index: Option<std::sync::Arc<nozhik_language_support::symbol_index::SymbolIndex>>,
    pub project_path: Option<PathBuf>,
    // Presentation survives worker revision gates; navigation still uses project_index.
    pub(crate) painted_symbols: Vec<nozhik_language_support::symbol_index::SymbolDeclaration>,
    pub(crate) presentation_text: Option<ropey::Rope>,
    pub(crate) completion_text: Option<ropey::Rope>,
    pub(crate) completion_index:
        Option<std::sync::Arc<nozhik_language_support::symbol_index::SymbolIndex>>,
    pub usages_byte: Option<usize>,
    pub usages_hover_byte: Option<usize>,
    pub local_completion_pending: Option<usize>,
    pub revision: u64,
    pub spans: Vec<nozhik_language_support::highlighter::HighlightSpan>,
    pub diagnostics: Vec<Diagnostic>,
    pub syntax: Vec<Diagnostic>,
    pub syntax_theme: String,
    pub completions: Vec<CompletionItem>,
    pub selected: usize,
    pub completion_scroll: bool,
    pub hover: Option<String>,
    pub request: Option<(RequestKind, usize)>,
    pub request_at: Option<std::time::Instant>,
    pub token: u64,
    pub hover_token: u64,
    pub definition_token: u64,
    pub hover_byte: Option<usize>,
    pub completion_caret: usize,
    pub completion_prefix: String,
    pub stubs: Option<std::sync::Arc<nozhik_language_support::php_stubs::PhpVersionStubs>>,
}
#[derive(Default)]
pub struct LanguageUi {
    pub(crate) indexer: Option<nozhik_language_support::project_indexer::ProjectIndexer>,
    index: Option<std::sync::Arc<nozhik_language_support::symbol_index::SymbolIndex>>,
    indexed_revisions: HashMap<PathBuf, u64>,
    overlay_sent: HashMap<PathBuf, u64>,
    pub(crate) invalidate_dialog: bool,
    pub(crate) indexing: Option<(usize, usize)>,
    pub(crate) usages: Option<crate::usages_popup::UsagesPopup>,
    root: Option<PathBuf>,
    enabled_languages: Option<std::collections::BTreeSet<nozhik_config::languages::LanguageId>>,
    service: Option<LanguageService>,
    sent: HashMap<Url, u64>,
    pub views: HashMap<PathBuf, EditorLanguage>,
    token: u64,
    formats: HashMap<u64, (nozhik_core::BufferId, nozhik_config::code_style::CodeStyle)>,
    pub(crate) local_navigation: Option<(PathBuf, u32, u32)>,
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
        if !nozhik_config::languages::enabled_for_path(
            &self.config.enabled_languages,
            &document.path,
        ) {
            self.status = "Language support is disabled or unavailable for this file".into();
            return;
        }
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
        let policy_changed = self
            .language
            .enabled_languages
            .as_ref()
            .is_some_and(|previous| previous != &self.config.enabled_languages);
        let php_enabled = nozhik_config::languages::enabled(
            &self.config.enabled_languages,
            nozhik_config::languages::LanguageId::Php,
        );
        if root != self.language.root || policy_changed {
            self.language = LanguageUi {
                root: root.clone(),
                enabled_languages: Some(self.config.enabled_languages.clone()),
                ..LanguageUi::default()
            };
            if let Some(root) = root
                && php_enabled
            {
                let ctx = ctx.clone();
                let index_ctx = ctx.clone();
                self.language.indexer = Some(
                    nozhik_language_support::project_indexer::ProjectIndexer::with_languages(
                        root.clone(),
                        self.config.enabled_languages.clone(),
                        move || index_ctx.request_repaint(),
                    ),
                );
                self.language.service =
                    Some(LanguageService::start(root, move || ctx.request_repaint()));
            }
        }
        self.language.enabled_languages = Some(self.config.enabled_languages.clone());
        if !php_enabled {
            return;
        }
        self.project_index_tick();
        let Some(service) = &self.language.service else {
            return;
        };
        let mut open = std::collections::HashSet::new();
        for document in &self.documents {
            if !document.open
                || document.binary
                || !nozhik_config::languages::enabled_for_path(
                    &self.config.enabled_languages,
                    &document.path,
                )
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
            view.rebase(&document.buffer);
            view.project_path = Some(document.path.clone());
            view.project_index = self.language.index.clone().filter(|_| {
                self.language.indexed_revisions.get(&document.path) == Some(&snapshot.revision)
            });
            view.capture_index();
            if self
                .language
                .root
                .as_ref()
                .is_some_and(|root| document.path.starts_with(root))
                && self.language.overlay_sent.get(&document.path) != Some(&snapshot.revision)
                && self.language.indexer.as_ref().is_some_and(|indexer| {
                    indexer.send(
                        nozhik_language_support::project_indexer::IndexCommand::Overlay {
                            path: document.path.clone(),
                            revision: snapshot.revision,
                            text: snapshot.text.clone(),
                        },
                    )
                })
            {
                self.language
                    .overlay_sent
                    .insert(document.path.clone(), snapshot.revision);
            }
            if let Some(byte) = view.usages_byte.take()
                && let Some(index) = &view.project_index
                && let Some(symbol) = index.at(&document.path, byte)
            {
                self.language.usages = Some(crate::usages_popup::UsagesPopup::new(index, symbol));
            }
            view.stubs.clone_from(&self.settings_dialog.php_stubs);
            if let Some(byte) = view.local_completion_pending
                && view.project_index.is_some()
                && byte == document.buffer.cursors().as_slice()[0].head.byte_offset
            {
                view.completions =
                    view.local_completions(&document.buffer.rope().to_string(), byte);
                view.local_completion_pending = None;
            }
            view.documentation = view.stubs.clone().map(DocumentationSource::from);
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
                if let Some(index) = &view.project_index {
                    if kind == RequestKind::Completion {
                        view.completions = nozhik_language_support::type_inference::complete(
                            index,
                            &document.path,
                            &source,
                            byte,
                        );
                        append_stubs(view, &source, byte);
                    } else if kind == RequestKind::Definition
                        && let Some(symbol) = index.at(&document.path, byte)
                    {
                        self.language.local_navigation =
                            Some((symbol.path.clone(), symbol.line, symbol.column));
                        continue;
                    }
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
                    self.language.overlay_sent.remove(&path);
                    if let Some(indexer) = &self.language.indexer {
                        let _sent = indexer.send(
                            nozhik_language_support::project_indexer::IndexCommand::Close(path),
                        );
                    }
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
                            let prefix =
                                &text[nozhik_language_support::completion::prefix(&text, caret)];
                            let remote = nozhik_language_support::completion::filter(items, prefix);
                            let mut items = view.local_completions(&text, caret);
                            let access = view.triggers.access(&text[..caret]);
                            for item in remote {
                                use lsp_types::CompletionItemKind as K;
                                let allowed = access.is_none()
                                    || matches!(
                                        item.kind,
                                        Some(K::METHOD | K::PROPERTY | K::FIELD)
                                    )
                                    || (access.is_some_and(|(_, _, static_access)| static_access)
                                        && item.kind == Some(K::CONSTANT));
                                if allowed
                                    && !items
                                        .iter()
                                        .any(|other| other.label.eq_ignore_ascii_case(&item.label))
                                {
                                    items.push(item);
                                }
                            }
                            view.completions = items;
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
        if let Some((path, line, column)) = self.language.local_navigation.clone() {
            self.open_file(path.clone());
            if let Some(d) = self.documents.iter().find(|d| d.path == path) {
                let rope = d.buffer.rope();
                if (line as usize) < rope.len_lines() {
                    let byte = rope.line_to_byte(line as usize) + column as usize;
                    let source = rope.to_string();
                    if (column as usize) <= rope.line(line as usize).len_bytes()
                        && source.is_char_boundary(byte)
                    {
                        self.language.navigation =
                            Some((path, nozhik_language_support::position(&source, byte)));
                    } else {
                        self.status = "Symbol location changed; wait for indexing and retry".into();
                    }
                }
                self.language.local_navigation = None;
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

impl EditorLanguage {
    pub(crate) fn local_completions(&self, source: &str, byte: usize) -> Vec<CompletionItem> {
        self.local_fragment_completions(source, byte, byte)
    }
    pub(crate) fn local_fragment_completions(
        &self,
        source: &str,
        byte: usize,
        absolute_byte: usize,
    ) -> Vec<CompletionItem> {
        let index = self
            .project_index
            .as_ref()
            .or(self.completion_index.as_ref());
        let absolute_byte = self
            .completion_text
            .as_ref()
            .zip(self.presentation_text.as_ref())
            .map_or(absolute_byte, |(old, new)| {
                let (old_range, new_range) = changed_range(old, new);
                map_byte(absolute_byte, &new_range, &old_range)
            });
        let mut items =
            index
                .zip(self.project_path.as_ref())
                .map_or_else(Vec::new, |(index, path)| {
                    nozhik_language_support::type_inference::complete_fragment(
                        index,
                        path,
                        source,
                        byte,
                        absolute_byte,
                    )
                });
        let access = self.triggers.access(&source[..byte]);
        let project_receiver = access
            .and_then(|(left, _, _)| {
                index
                    .zip(self.project_path.as_ref())
                    .and_then(|(index, path)| {
                        nozhik_language_support::type_inference::receiver_type(
                            index,
                            path,
                            absolute_byte,
                            left,
                        )
                    })
            })
            .filter(|name| index.is_some_and(|i| i.declaration(name).is_some()));
        if project_receiver.is_none()
            && let Some(stubs) = &self.stubs
        {
            let query = nozhik_language_support::completion::query(source, byte);
            let mut catalog_items = stubs.search(query);
            if let Some((_, _, static_access)) = access {
                if !static_access {
                    catalog_items.clear();
                } else if let Some((class, _)) = query.rsplit_once("::")
                    && let Some(entity) = stubs
                        .entities
                        .iter()
                        .find(|s| s.name.eq_ignore_ascii_case(class.trim_start_matches('\\')))
                {
                    catalog_items.retain(|item| {
                        entity.constants.iter().any(|c| c.name == item.label)
                            || entity
                                .methods
                                .iter()
                                .any(|m| m.name == item.label && m.is_static)
                    });
                }
            }
            for item in catalog_items {
                if !items
                    .iter()
                    .any(|other| other.label.eq_ignore_ascii_case(&item.label))
                {
                    items.push(item);
                }
            }
        }
        items
    }
}
fn append_stubs(view: &mut EditorLanguage, source: &str, byte: usize) {
    view.completions = view.local_completions(source, byte);
}
impl crate::NozhikUi {
    fn project_index_tick(&mut self) {
        use nozhik_language_support::project_indexer::IndexEvent;
        if let Some(indexer) = &self.language.indexer {
            for event in indexer.drain() {
                match event {
                    IndexEvent::Snapshot(index, revisions) => {
                        self.language.index = Some(index);
                        self.language.indexed_revisions = revisions;
                    }
                    IndexEvent::Progress(done, total) => {
                        self.language.indexing =
                            (done != total || total == 0).then_some((done, total));
                    }
                    IndexEvent::Finished => self.language.indexing = None,
                    IndexEvent::Error(error) => {
                        self.language.indexing = None;
                        self.status = format!("Project index: {error}");
                    }
                }
            }
        }
    }
    pub(crate) fn index_changed(&self, path: PathBuf) {
        if let Some(indexer) = &self.language.indexer {
            let _sent =
                indexer.send(nozhik_language_support::project_indexer::IndexCommand::Changed(path));
        }
    }
    pub(crate) fn index_dialogs(&mut self, ctx: &egui::Context) {
        if self.language.invalidate_dialog {
            egui::Modal::new(egui::Id::new("invalidate_caches")).show(ctx, |ui| {
                ui.heading("Invalidate Caches / Delete Caches…");
                ui.label(
                    "Delete this project's .nozhik/cache directory and rebuild its symbol index?",
                );
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            self.language.indexer.is_some(),
                            egui::Button::new("Invalidate and Restart Indexing"),
                        )
                        .on_disabled_hover_text(
                            "Enable a supported language in Settings → Languages first",
                        )
                        .clicked()
                    {
                        if self.language.indexer.as_ref().is_some_and(|i| {
                            i.send(
                                nozhik_language_support::project_indexer::IndexCommand::Invalidate,
                            )
                        }) {
                            self.language.index = None;
                            self.language.indexing = Some((0, 0));
                            for view in self.language.views.values_mut() {
                                view.project_index = None;
                            }
                        }
                        self.language.invalidate_dialog = false;
                    }
                    if ui.button("Cancel").clicked() {
                        self.language.invalidate_dialog = false;
                    }
                });
            });
        }
        if let Some(popup) = &mut self.language.usages {
            if let Some(location) = popup.show(ctx) {
                // Reference columns are UTF-8; convert only after the target file has opened.
                self.language.local_navigation =
                    Some((location.path, location.line, location.column));
                ctx.request_repaint();
            }
            if !popup.open {
                self.language.usages = None;
            }
        }
    }
}
// Compare Rope iterators without allocating a full document String.
fn changed_range(
    old: &ropey::Rope,
    new: &ropey::Rope,
) -> (std::ops::Range<usize>, std::ops::Range<usize>) {
    // Most Rope chunks are shared after an edit. Skip them by identity before
    // comparing the small changed boundary chunks byte by byte.
    let shared_prefix: usize = old
        .chunks()
        .zip(new.chunks())
        .take_while(|(a, b)| a.len() == b.len() && (a.as_ptr() == b.as_ptr() || a == b))
        .map(|(chunk, _)| chunk.len())
        .sum();
    let mut start = shared_prefix
        + old
            .bytes_at(shared_prefix)
            .zip(new.bytes_at(shared_prefix))
            .take_while(|(a, b)| a == b)
            .count();
    // byte_to_char accepts interior bytes: use the leading-byte test instead.
    while start > 0
        && (old.get_byte(start).is_some_and(|b| b & 0xc0 == 0x80)
            || new.get_byte(start).is_some_and(|b| b & 0xc0 == 0x80))
    {
        start -= 1;
    }
    let limit = old.len_bytes().min(new.len_bytes()).saturating_sub(start);
    let mut shared_suffix = 0;
    for (a, b) in old
        .chunks_at_byte(old.len_bytes())
        .0
        .reversed()
        .zip(new.chunks_at_byte(new.len_bytes()).0.reversed())
    {
        if a.len() != b.len()
            || shared_suffix + a.len() > limit
            || (a.as_ptr() != b.as_ptr() && a != b)
        {
            break;
        }
        shared_suffix += a.len();
    }
    let mut suffix = shared_suffix
        + old
            .bytes_at(old.len_bytes() - shared_suffix)
            .reversed()
            .zip(new.bytes_at(new.len_bytes() - shared_suffix).reversed())
            .take(limit - shared_suffix)
            .take_while(|(a, b)| a == b)
            .count();
    while suffix > 0
        && (old
            .get_byte(old.len_bytes() - suffix)
            .is_some_and(|b| b & 0xc0 == 0x80)
            || new
                .get_byte(new.len_bytes() - suffix)
                .is_some_and(|b| b & 0xc0 == 0x80))
    {
        suffix -= 1;
    }
    (
        start..old.len_bytes() - suffix,
        start..new.len_bytes() - suffix,
    )
}
const fn map_byte(
    byte: usize,
    old: &std::ops::Range<usize>,
    new: &std::ops::Range<usize>,
) -> usize {
    if byte <= old.start {
        byte
    } else if byte >= old.end {
        new.end + byte - old.end
    } else {
        new.start
    }
}
impl EditorLanguage {
    pub(crate) fn capture_index(&mut self) {
        let Some(index) = &self.project_index else {
            return;
        };
        if self
            .completion_index
            .as_ref()
            .is_some_and(|previous| std::sync::Arc::ptr_eq(previous, index))
        {
            return;
        }
        self.painted_symbols = self
            .project_path
            .as_ref()
            .and_then(|path| index.declarations_by_file.get(path))
            .cloned()
            .unwrap_or_default();
        self.completion_index = Some(index.clone());
        self.completion_text = self.presentation_text.clone();
    }
    pub(crate) fn rebase(&mut self, buffer: &nozhik_buffer::TextBuffer) {
        let snapshot = buffer.snapshot();
        if self.revision != snapshot.revision {
            if let Some(old) = &self.presentation_text {
                let (removed, inserted) = changed_range(old, &snapshot.text);
                for span in &mut self.spans {
                    span.range = map_byte(span.range.start, &removed, &inserted)
                        ..map_byte(span.range.end, &removed, &inserted);
                }
                self.spans.retain(|span| !span.range.is_empty());
                for symbol in &mut self.painted_symbols {
                    symbol.range = map_byte(symbol.range.start, &removed, &inserted)
                        ..map_byte(symbol.range.end, &removed, &inserted);
                    let line = snapshot.text.byte_to_line(symbol.range.start);
                    symbol.line = u32::try_from(line).unwrap_or(u32::MAX);
                    symbol.column =
                        u32::try_from(symbol.range.start - snapshot.text.line_to_byte(line))
                            .unwrap_or(u32::MAX);
                }
            }
            self.revision = snapshot.revision;
            self.diagnostics.clear();
            self.syntax.clear();
            self.completions.clear();
            self.local_completion_pending = None;
            self.token = 0;
            self.request = None;
            self.hover = None;
            self.project_index = None;
        }
        self.presentation_text = Some(snapshot.text);
    }
}

#[cfg(test)]
mod tests {
    fn navigation_frame(
        shell: &mut crate::NozhikUi,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        modifiers: egui::Modifiers,
    ) -> egui::FullOutput {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1200.0, 1050.0),
                )),
                events,
                modifiers,
                ..Default::default()
            },
            |ctx| shell.update_frame(ctx),
        )
    }
    fn navigation_text(output: &egui::FullOutput, text: &str, column: usize) -> Option<egui::Pos2> {
        output.shapes.iter().find_map(|s| match &s.shape {
            egui::Shape::Text(t) if t.galley.job.text == text => Some(
                t.pos
                    + t.galley
                        .pos_from_cursor(egui::text::CCursor::new(column))
                        .min
                        .to_vec2()
                    + egui::vec2(2.0, 7.0),
            ),
            _ => None,
        })
    }
    fn navigation_click(
        shell: &mut crate::NozhikUi,
        ctx: &egui::Context,
        pos: egui::Pos2,
        modifiers: egui::Modifiers,
    ) {
        for pressed in [true, false] {
            navigation_frame(
                shell,
                ctx,
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers,
                    },
                ],
                modifiers,
            );
        }
    }
    fn navigation_wait(
        shell: &mut crate::NozhikUi,
        ctx: &egui::Context,
        path: &std::path::Path,
        byte: usize,
    ) -> Result<egui::FullOutput, Box<dyn std::error::Error>> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
        loop {
            let output = navigation_frame(
                shell,
                ctx,
                vec![egui::Event::PointerMoved(egui::pos2(1180.0, 1010.0))],
                egui::Modifiers::NONE,
            );
            if shell
                .active
                .and_then(|i| shell.documents.get(i))
                .is_some_and(|d| {
                    d.path == path && d.buffer.cursors().as_slice()[0].head.byte_offset == byte
                })
                && shell
                    .language
                    .views
                    .get(path)
                    .is_some_and(|v| v.project_index.is_some())
            {
                return Ok(output);
            }
            if std::time::Instant::now() > deadline {
                return Err(format!(
                    "navigation/index timeout for {}:{byte}; status={}",
                    path.display(),
                    shell.status
                )
                .into());
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
    #[test]
    fn presentation_diff_preserves_unicode_across_rope_chunks() {
        let text = "<?php // я🦀\r\n".repeat(2000);
        let old = ropey::Rope::from_str(&text);
        for offset in [0, 1, 500, 10000, old.len_chars()] {
            for removed in [0, 1, 20] {
                let end = (offset + removed).min(old.len_chars());
                let mut new = old.clone();
                new.remove(offset..end);
                new.insert(offset, "Ж🦀\n");
                let (before, after) = super::changed_range(&old, &new);
                assert!(text.is_char_boundary(before.start));
                assert!(text.is_char_boundary(before.end));
                let replacement = new.byte_slice(after).to_string();
                let mut reconstructed = text.clone();
                reconstructed.replace_range(before, &replacement);
                assert_eq!(reconstructed, new.to_string());
            }
        }
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "Runs real input through declaration/call navigation and asynchronous file opening for three symbol kinds"
    )]
    fn modifier_hover_and_row_click_cycle_between_declarations_and_usages()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let root = dir.path().canonicalize()?;
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/php-symbol-navigation");
        for name in [
            "Contracts/Vehicle.php",
            "Model/Vehicle.php",
            "Model/Car.php",
            "Model/Truck.php",
            "Model/ElectricCar.php",
            "Fleet.php",
            "main.php",
        ] {
            let path = root.join(name);
            std::fs::create_dir_all(path.parent().ok_or("fixture parent")?)?;
            std::fs::copy(fixture.join(name), path)?;
        }
        // Explicit Composer sources must participate even under ignored vendor/.
        let vendor = root.join("vendor/local/tools");
        std::fs::create_dir_all(&vendor)?;
        std::fs::write(root.join(".gitignore"), "vendor/\n")?;
        std::fs::write(
            root.join("composer.json"),
            r#"{"autoload":{"psr-4":{"Tools\\":"vendor/local/tools/"}}}"#,
        )?;
        std::fs::write(
            vendor.join("FlashVars.php"),
            "<?php\nnamespace Tools;\nclass FlashVars {}\n",
        )?;
        let fleet_path = root.join("Fleet.php");
        let fleet_source = std::fs::read_to_string(&fleet_path)?;
        std::fs::write(
            &fleet_path,
            fleet_source.replace(
                "use Demo\\Contracts",
                "use Tools\\FlashVars;\nuse Demo\\Contracts",
            ),
        )?;
        let bus = nozhik_event_bus::EventBus::new(128)?;
        let settings =
            nozhik_config::ConfigManager::from_path(root.join("settings.toml"), bus.clone())?;
        let mut shell = crate::NozhikUi::new(
            bus,
            settings,
            nozhik_config::Config {
                ai_enabled: false,
                ..Default::default()
            },
        );
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        ctx.style_mut(|style| style.animation_time = 0.0);
        shell.project = Some(nozhik_fs_model::tree::ProjectTree::new(&root)?);
        shell.language.root = Some(root.clone());
        shell.language.service = Some(nozhik_language_support::LanguageService::with_server(
            root.clone(),
            Some(nozhik_language_support::ServerCommand {
                program: root.join("no-lsp-for-test"),
                args: vec![],
            }),
            || {},
        ));
        shell.language.indexer = Some(
            nozhik_language_support::project_indexer::ProjectIndexer::start(root.clone(), || {}),
        );
        let fleet = root.join("Fleet.php");
        let source = std::fs::read_to_string(&fleet)?;
        shell.open_file(fleet.clone());
        navigation_wait(&mut shell, &ctx, &fleet, 0)?;
        let primary = egui::Modifiers {
            mac_cmd: cfg!(target_os = "macos"),
            ctrl: !cfg!(target_os = "macos"),
            command: true,
            ..Default::default()
        };
        for (call_line, call_name, declaration_file, declaration_line, declaration_name) in [
            (
                "use Tools\\FlashVars;",
                "FlashVars",
                "vendor/local/tools/FlashVars.php",
                "class FlashVars {}",
                "FlashVars",
            ),
            (
                "        $car->run();",
                "run",
                "Model/Vehicle.php",
                "    public function run(): string",
                "run",
            ),
            (
                "        $car = new CityCar();",
                "CityCar",
                "Model/Car.php",
                "class Car extends Vehicle",
                "Car",
            ),
            (
                "        $name = $car->name;",
                "name",
                "Model/Vehicle.php",
                "    public string $name = 'vehicle';",
                "name",
            ),
        ] {
            let output = navigation_frame(&mut shell, &ctx, vec![], egui::Modifiers::NONE);
            let position = navigation_text(
                &output,
                call_line,
                call_line.rfind(call_name).ok_or("call name")? + 1,
            )
            .ok_or_else(|| format!("call not visible: {call_line}"))?;
            navigation_click(&mut shell, &ctx, position, primary);
            let declaration = root.join(declaration_file);
            let declaration_source = std::fs::read_to_string(&declaration)?;
            let definition_byte = declaration_source
                .find(declaration_line)
                .ok_or("definition line")?
                + declaration_line
                    .find(declaration_name)
                    .ok_or("definition name")?;
            let definition_byte = definition_byte - usize::from(declaration_name == "name");
            navigation_wait(&mut shell, &ctx, &declaration, definition_byte)?;
            navigation_frame(&mut shell, &ctx, vec![], egui::Modifiers::NONE);
            let output = navigation_frame(&mut shell, &ctx, vec![], egui::Modifiers::NONE);
            let position = navigation_text(
                &output,
                declaration_line,
                declaration_line
                    .find(declaration_name)
                    .ok_or("definition glyph")?
                    + 1,
            )
            .ok_or("declaration not scrolled into view")?;
            let before = shell
                .documents
                .iter()
                .find(|d| d.path == declaration)
                .ok_or("opened declaration")?
                .buffer
                .cursors()
                .clone();
            navigation_frame(
                &mut shell,
                &ctx,
                vec![egui::Event::PointerMoved(position)],
                primary,
            );
            let mut output = navigation_frame(&mut shell, &ctx, vec![], primary);
            assert!(
                shell.language.usages.is_some(),
                "modifier hover must open the list for {declaration_name}"
            );
            // Release the modifier and move into the popup: it must remain clickable.
            for _ in 0..2 {
                output = navigation_frame(
                    &mut shell,
                    &ctx,
                    vec![egui::Event::PointerMoved(egui::pos2(1100.0, 990.0))],
                    egui::Modifiers::NONE,
                );
            }
            assert_eq!(
                shell
                    .documents
                    .iter()
                    .find(|d| d.path == declaration)
                    .ok_or("declaration")?
                    .buffer
                    .cursors(),
                &before
            );
            let row = navigation_text(
                &output,
                call_line,
                call_line.rfind(call_name).ok_or("preview name")? + 1,
            )
            .ok_or_else(|| format!("usage row not painted: {call_line}"))?;
            navigation_click(&mut shell, &ctx, row, egui::Modifiers::NONE);
            assert!(
                shell.language.usages.is_none(),
                "clicking a row must close the list"
            );
            let reference_byte = source.find(call_line).ok_or("reference line")?
                + if call_line.starts_with("use ") {
                    "use ".len()
                } else {
                    call_line.rfind(call_name).ok_or("reference name")?
                };
            navigation_wait(&mut shell, &ctx, &fleet, reference_byte)?;
            assert_eq!(
                shell
                    .documents
                    .iter()
                    .find(|d| d.path == fleet)
                    .ok_or("fleet")?
                    .buffer
                    .rope()
                    .to_string(),
                source
            );
        }
        Ok(())
    }

    #[test]
    fn invalidate_action_confirmation_deletes_cache_and_reindexes()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("sample.php");
        std::fs::write(&path, "<?php class Sample {}")?;
        let bus = nozhik_event_bus::EventBus::new(32)?;
        let settings =
            nozhik_config::ConfigManager::from_path(dir.path().join("settings.toml"), bus.clone())?;
        let mut shell = crate::NozhikUi::new(bus, settings, nozhik_config::Config::default());
        shell.documents.push(crate::Document {
            path,
            binary: false,
            name: "sample.php".into(),
            open: true,
            buffer: nozhik_buffer::TextBuffer::load_from_str(
                nozhik_core::BufferId(988),
                "<?php class Sample {}",
            ),
        });
        let ctx = egui::Context::default();
        let wait = |shell: &mut crate::NozhikUi| -> Result<(), Box<dyn std::error::Error>> {
            let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                shell.language_tick(&ctx);
                if shell
                    .language
                    .index
                    .as_ref()
                    .is_some_and(|i| i.declaration("\\sample").is_some())
                    && shell.language.indexing.is_none()
                {
                    return Ok(());
                }
                if std::time::Instant::now() > until {
                    return Err("indexing timeout".into());
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        };
        wait(&mut shell)?;
        let sentinel = dir.path().join(".nozhik/cache/obsolete.bin");
        std::fs::write(&sentinel, "old cache")?;
        shell
            .integration
            .actions
            .push("file.invalidate_caches".into());
        shell.integration_tick(&ctx);
        assert!(shell.language.invalidate_dialog);
        assert!(sentinel.exists(), "confirmation must precede deletion");
        let mut button = None;
        for _ in 0..2 {
            let frame = ctx.run(egui::RawInput::default(), |ctx| shell.index_dialogs(ctx));
            button = frame
                .shapes
                .iter()
                .find_map(|s| match &s.shape {
                    egui::Shape::Text(t)
                        if t.galley.job.text == "Invalidate and Restart Indexing" =>
                    {
                        Some(t.pos + t.galley.size() * 0.5)
                    }
                    _ => None,
                })
                .or(button);
        }
        let pos = button.ok_or("confirmation button missing")?;
        for pressed in [true, false] {
            let _frame = ctx.run(
                egui::RawInput {
                    events: vec![
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                    ..Default::default()
                },
                |ctx| shell.index_dialogs(ctx),
            );
        }
        assert!(!shell.language.invalidate_dialog);
        wait(&mut shell)?;
        assert!(!sentinel.exists());
        assert!(dir.path().join(".nozhik/cache/index_php.json").is_file());
        Ok(())
    }

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
