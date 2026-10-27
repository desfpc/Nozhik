//! Background PHP syntax and LSP services. UI owns authoritative document text.
pub mod completion;
pub mod formatting;
pub mod highlighter;
pub mod lsp;
pub mod structural;
pub use lsp_types;
use lsp_types::{
    CompletionResponse, GotoDefinitionResponse, Hover, Position, PublishDiagnosticsParams, Url,
};
use ropey::Rope;
use serde_json::json;
use std::{collections::HashMap, ops::Range, path::PathBuf, sync::mpsc};

/// Language subsystem failure.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Process or transport I/O.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// JSON encoding/decoding.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// Protocol, parser or lifecycle failure.
    #[error("{0}")]
    Protocol(String),
}
/// Language subsystem result.
pub type Result<T> = std::result::Result<T, Error>;

/// Converts a valid UTF-8 byte boundary to LSP UTF-16 coordinates.
#[must_use]
pub fn position(source: &str, byte: usize) -> Position {
    let prefix = &source[..byte];
    let line = prefix.bytes().filter(|b| *b == b'\n').count();
    let start = prefix.rfind('\n').map_or(0, |i| i + 1);
    Position::new(
        u32::try_from(line).unwrap_or(u32::MAX),
        u32::try_from(prefix[start..].encode_utf16().count()).unwrap_or(u32::MAX),
    )
}
/// Converts UTF-16 positions strictly, rejecting split surrogate pairs and lines.
#[must_use]
pub fn byte_offset(source: &str, pos: Position) -> Option<usize> {
    let mut start = 0;
    for _ in 0..pos.line {
        start += source.get(start..)?.find('\n')? + 1;
    }
    let tail = source.get(start..)?;
    let raw = tail.split('\n').next()?;
    let line = if tail.contains('\n') {
        raw.strip_suffix('\r').unwrap_or(raw)
    } else {
        raw
    };
    let mut units = 0;
    for (i, c) in line.char_indices() {
        if units == pos.character {
            return Some(start + i);
        }
        units += u32::try_from(c.len_utf16()).ok()?;
    }
    (units == pos.character).then_some(start + line.len())
}
/// Converts a server range, validating ordering and UTF-8 boundaries.
#[must_use]
pub fn byte_range(source: &str, range: lsp_types::Range) -> Option<Range<usize>> {
    let start = byte_offset(source, range.start)?;
    let end = byte_offset(source, range.end)?;
    (start <= end).then_some(start..end)
}

/// Request or document lifecycle command. Snapshot versions use UI revisions.
#[derive(Clone)]
pub enum Command {
    /// Latest immutable document snapshot, also opens an unknown URI.
    Sync {
        /// File URI.
        uri: Url,
        /// Authoritative buffer revision.
        revision: u64,
        /// Immutable Rope snapshot.
        text: Rope,
    },
    /// Format an immutable snapshot, preferring the active LSP server.
    Format {
        /// File URI.
        uri: Url,
        /// Authoritative buffer revision.
        revision: u64,
        /// UI correlation token.
        token: u64,
        /// Original source snapshot.
        source: String,
        /// Selected profile.
        style: nozhik_config::code_style::CodeStyle,
        /// Optional original UTF-8 selection.
        range: Option<Range<usize>>,
    },
    /// Release document syntax/server state.
    Close(Url),
    /// Completion/hover/definition at a UTF-16 position, with UI request token.
    Request {
        /// File URI.
        uri: Url,
        /// Authoritative buffer revision.
        revision: u64,
        /// UI request correlation token.
        token: u64,
        /// LSP UTF-16 request position.
        position: Position,
        /// Requested feature.
        kind: RequestKind,
    },
}
/// Interactive request kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestKind {
    /// Suggestions.
    Completion,
    /// Markdown/type documentation.
    Hover,
    /// Definition locations.
    Definition,
}
/// Correlated replies; no UI mutation occurs in background workers.
pub enum Event {
    /// Byte capture ranges for one exact snapshot.
    Highlights {
        /// File URI.
        uri: Url,
        /// Authoritative buffer revision.
        revision: u64,
        /// Query capture byte ranges.
        spans: Vec<highlighter::HighlightSpan>,
        /// Local parser errors independent of LSP diagnostics.
        syntax: Vec<lsp_types::Diagnostic>,
    },
    /// Diagnostics for the server's synchronized snapshot.
    Diagnostics {
        /// File URI.
        uri: Url,
        /// Authoritative buffer revision.
        revision: u64,
        /// Server diagnostics.
        items: Vec<lsp_types::Diagnostic>,
    },
    /// Interactive response with original request token.
    Response {
        /// File URI.
        uri: Url,
        /// Authoritative buffer revision.
        revision: u64,
        /// UI request correlation token.
        token: u64,
        /// Typed response payload.
        data: Response,
    },
    /// Formatting edits for one original snapshot.
    Formatted {
        /// File URI.
        uri: Url,
        /// Original revision.
        revision: u64,
        /// Original UI correlation token.
        token: u64,
        /// UTF-8 replacements, or an actionable failure.
        edits: Result<Vec<(Range<usize>, String)>>,
    },
    /// Actionable server/parser failure, including installation fallback.
    Error(String),
}
/// Typed LSP response payloads.
pub enum Response {
    /// Completion items or list.
    Completion(Option<CompletionResponse>),
    /// Hover documentation.
    Hover(Option<Hover>),
    /// Definition locations or links.
    Definition(Option<GotoDefinitionResponse>),
}
/// Explicit server command; useful for custom installations and isolated tests.
#[derive(Clone)]
pub struct ServerCommand {
    /// Executable path, passed directly to Command without a shell.
    pub program: PathBuf,
    /// Server arguments (e.g. --stdio).
    pub args: Vec<String>,
}

/// Independent syntax worker and async LSP actor; dropping cancels pending work.
pub struct LanguageService {
    syntax: mpsc::SyncSender<Command>,
    server: tokio::sync::mpsc::Sender<Command>,
    events: mpsc::Receiver<Event>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    done: mpsc::Receiver<()>,
}
impl LanguageService {
    /// Starts background workers. No parsing, process I/O or PATH scanning on UI.
    #[must_use]
    pub fn start(root: PathBuf, wake: impl Fn() + Send + Sync + 'static) -> Self {
        Self::with_server(root, None, wake)
    }
    /// Starts with an explicit server command, or automatic PHP PATH discovery.
    #[must_use]
    pub fn with_server(
        root: PathBuf,
        configured: Option<ServerCommand>,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        let wake: std::sync::Arc<dyn Fn() + Send + Sync> = std::sync::Arc::new(wake);
        let (events, receiver) = mpsc::channel();
        let (stop, stopping) = tokio::sync::oneshot::channel();
        let (finished, done) = mpsc::channel();
        let (syntax, input) = mpsc::sync_channel(64);
        let (server, commands) = tokio::sync::mpsc::channel(64);
        let output = events.clone();
        let syntax_wake = wake.clone();
        std::thread::spawn(move || {
            let mut parsers = HashMap::new();
            while let Ok(command) = input.recv() {
                let result =
                    match command {
                        Command::Sync {
                            uri,
                            revision,
                            text,
                        } => {
                            let parser = parsers
                                .entry(uri.clone())
                                .or_insert_with(highlighter::Highlighter::php);
                            match parser {
                                Ok(parser) => parser.highlight(&text.to_string()).map(|spans| {
                                    Event::Highlights {
                                        uri,
                                        revision,
                                        spans,
                                        syntax: parser.diagnostics(),
                                    }
                                }),
                                Err(error) => Err(Error::Protocol(error.to_string())),
                            }
                        }
                        Command::Close(uri) => {
                            parsers.remove(&uri);
                            continue;
                        }
                        Command::Request { .. } | Command::Format { .. } => continue,
                    };
                if output
                    .send(result.unwrap_or_else(|e| Event::Error(e.to_string())))
                    .is_err()
                {
                    break;
                }
                syntax_wake();
            }
        });
        std::thread::spawn(move || {
            match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => {
                    runtime.block_on(server_actor(
                        root, configured, commands, stopping, &events, &wake,
                    ));
                }
                Err(error) => {
                    let _sent = events.send(Event::Error(error.to_string()));
                    wake();
                }
            }
            let _finished = finished.send(());
        });
        Self {
            stop: Some(stop),
            done,
            syntax,
            server,
            events: receiver,
        }
    }
    /// Enqueues a command without waiting. False means retry the snapshot later;
    /// duplicate Sync/Close commands are idempotent.
    #[must_use]
    pub fn send(&self, command: Command) -> bool {
        if !matches!(command, Command::Request { .. } | Command::Format { .. })
            && self.syntax.try_send(command.clone()).is_err()
        {
            return false;
        }
        self.server.try_send(command).is_ok()
    }
    /// Gracefully stops the server; intended for application exit, never a frame.
    /// Waits at most three seconds for bounded worker cleanup.
    pub fn shutdown(mut self) {
        if let Some(stop) = self.stop.take() {
            let _stopped = stop.send(());
        }
        let _finished = self.done.recv_timeout(std::time::Duration::from_secs(3));
    }
    /// Polls completed events without blocking.
    pub fn drain(&self) -> impl Iterator<Item = Event> + '_ {
        self.events.try_iter().take(128)
    }
}

impl Drop for LanguageService {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _stopped = stop.send(());
        }
    }
}

async fn server_actor(
    root: PathBuf,
    configured: Option<ServerCommand>,
    mut commands: tokio::sync::mpsc::Receiver<Command>,
    mut stop: tokio::sync::oneshot::Receiver<()>,
    output: &mpsc::Sender<Event>,
    wake: &std::sync::Arc<dyn Fn() + Send + Sync>,
) {
    let mut server = None;
    let mut notifications = None;
    let mut attempted = false;
    let mut documents: HashMap<Url, (u64, String, tokio::time::Instant)> = HashMap::new();
    let mut synchronized = HashMap::new();
    let mut timer = tokio::time::interval(std::time::Duration::from_millis(25));
    loop {
        let running = async {
            tokio::select! {
                command = commands.recv() => {
                    let Some(command) = command else { return false; };
                    if !attempted && matches!(command, Command::Sync { .. }) {
                        attempted = true;
                        if let Some((path,args)) = configured.as_ref().map(|c| (c.program.clone(),c.args.clone())).or_else(lsp::manager::discover) {
                            match lsp::manager::LspManager::start(tokio::process::Command::new(path).args(args), &root).await {
                                Ok((manager, receiver)) => { server = Some(manager); notifications = Some(receiver); }
                                Err(error) => { let _sent = output.send(Event::Error(error.to_string())); wake(); }
                            }
                        } else { let _sent = output.send(Event::Error("PHP LSP: install phpactor (language-server) or intelephense (--stdio) and add it to PATH; Tree-sitter remains available".into())); wake(); }
                    }
                    match command {
                        Command::Sync { uri, revision, text } => {
                            if documents.get(&uri).is_none_or(|d| d.0 != revision) {
                                let first = !documents.contains_key(&uri);
                                documents.insert(uri.clone(), (revision,text.to_string(),tokio::time::Instant::now()));
                                if first && let Some(manager) = &mut server { sync_one(manager,&uri,&documents,&mut synchronized,output).await; }
                            }
                        }
                        Command::Close(uri) => {
                            documents.remove(&uri); synchronized.remove(&uri);
                            if let Some(manager) = &mut server && let Err(error) = manager.close(&uri).await { let _sent = output.send(Event::Error(error.to_string())); }
                        }
                        Command::Format { uri, revision, token, source, style, range } => {
                            let edits = format_document(server.as_mut(), &uri, &source, &style, range).await;
                            let _sent = output.send(Event::Formatted { uri, revision, token, edits }); wake();
                        }
                        Command::Request { uri, revision, token, position, kind } => {
                            if documents.get(&uri).is_none_or(|d| d.0 != revision) { return true; }
                            if let Some(manager) = &mut server {
                                sync_one(manager,&uri,&documents,&mut synchronized,output).await;
                                let method = match kind { RequestKind::Completion => "textDocument/completion", RequestKind::Hover => "textDocument/hover", RequestKind::Definition => "textDocument/definition" };
                                let result = manager.client.request(method,json!({"textDocument":{"uri":uri},"position":position})).await.and_then(|value| {
                                    Ok(match kind {
                                        RequestKind::Completion => Response::Completion(serde_json::from_value(value)?),
                                        RequestKind::Hover => Response::Hover(serde_json::from_value(value)?),
                                        RequestKind::Definition => Response::Definition(serde_json::from_value(value)?),
                                    })
                                });
                                let event = result.map_or_else(|e| Event::Error(e.to_string()), |data| Event::Response { uri, revision, token, data });
                                let _sent = output.send(event); wake();
                            }
                        }
                    }
                }
                _ = timer.tick() => {
                    if let Some(manager) = &mut server {
                        for (uri,(revision,_,changed)) in &documents {
                            if changed.elapsed() >= std::time::Duration::from_millis(200) && synchronized.get(uri) != Some(revision) {
                                sync_one(manager,uri,&documents,&mut synchronized,output).await;
                            }
                        }
                        if let Some(receiver) = &mut notifications {
                            while let Ok(packet) = receiver.try_recv() {
                                if packet["method"] == "textDocument/publishDiagnostics"
                                    && let Ok(params) = serde_json::from_value::<PublishDiagnosticsParams>(packet["params"].clone())
                                    && let Some(revision) = synchronized.get(&params.uri)
                                    && documents.get(&params.uri).is_some_and(|d| d.0 == *revision)
                                    && params.version.is_none_or(|v| manager.version(&params.uri) == Some(v)) {
                                    let _sent = output.send(Event::Diagnostics { uri: params.uri, revision: *revision, items: params.diagnostics }); wake();
                                }
                            }
                        }
                    }
                }
            }
            true
        };
        tokio::select! {
            _ = &mut stop => break,
            keep_running = running => { if !keep_running { break; } }
        }
    }
    if let Some(mut manager) = server {
        let _shutdown =
            tokio::time::timeout(std::time::Duration::from_secs(2), manager.shutdown()).await;
    }
}
async fn sync_one(
    manager: &mut lsp::manager::LspManager,
    uri: &Url,
    documents: &HashMap<Url, (u64, String, tokio::time::Instant)>,
    synchronized: &mut HashMap<Url, u64>,
    output: &mpsc::Sender<Event>,
) {
    if let Some((revision, text, _)) = documents.get(uri) {
        match manager.sync(uri, text).await {
            Ok(()) => {
                synchronized.insert(uri.clone(), *revision);
            }
            Err(error) => {
                let _sent = output.send(Event::Error(error.to_string()));
            }
        }
    }
}

async fn format_document(
    manager: Option<&mut lsp::manager::LspManager>,
    uri: &Url,
    source: &str,
    style: &nozhik_config::code_style::CodeStyle,
    range: Option<Range<usize>>,
) -> Result<Vec<(Range<usize>, String)>> {
    if let Some(manager) = manager
        && manager.sync(uri, source).await.is_ok()
    {
        let capabilities = &manager.initialized.capabilities;
        let supported = if range.is_some() {
            capabilities
                .document_range_formatting_provider
                .as_ref()
                .is_some_and(|v| !matches!(v, lsp_types::OneOf::Left(false)))
        } else {
            capabilities
                .document_formatting_provider
                .as_ref()
                .is_some_and(|v| !matches!(v, lsp_types::OneOf::Left(false)))
        };
        if supported {
            let mut params = json!({"textDocument":{"uri":uri},"options":{"tabSize":style.indent.size,"insertSpaces":matches!(style.indent.style,nozhik_config::code_style::IndentStyle::Spaces)}});
            let method = if let Some(r) = &range {
                params["range"] = serde_json::to_value(lsp_types::Range::new(
                    position(source, r.start),
                    position(source, r.end),
                ))?;
                "textDocument/rangeFormatting"
            } else {
                "textDocument/formatting"
            };
            if let Ok(value) = manager.client.request(method, params).await
                && let Ok(Some(edits)) =
                    serde_json::from_value::<Option<Vec<lsp_types::TextEdit>>>(value)
            {
                let mut replacements = Vec::new();
                for edit in edits {
                    let bytes = byte_range(source, edit.range)
                        .ok_or_else(|| Error::Protocol("Invalid formatter range".into()))?;
                    if range
                        .as_ref()
                        .is_some_and(|r| bytes.start < r.start || bytes.end > r.end)
                    {
                        return Err(Error::Protocol(
                            "Formatter changed text outside selection".into(),
                        ));
                    }
                    replacements.push((bytes, edit.new_text));
                }
                if replacements
                    .iter()
                    .any(|(r, text)| source.get(r.clone()) != Some(text.as_str()))
                {
                    return formatting::normalize_lsp_indentation(
                        source,
                        style,
                        range.as_ref(),
                        replacements,
                    );
                }
            }
        }
    }
    formatting::php_edits(source, style, range.as_ref())
}
