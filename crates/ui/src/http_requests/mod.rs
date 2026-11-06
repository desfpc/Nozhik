//! Project HTTP calls stored in `<project>/http_requests.json`.
//!
//! The file is created when HTTP Requests is opened. A missing file is an empty library.
//! A malformed file is kept and editing stays disabled.
#![allow(clippy::redundant_pub_crate)]

mod curl;
mod export;
mod model;
mod send;

pub(crate) use model::{
    BodyKind, Incoming, MAX_TAGS, content_type_kind, normalize_tag, percent_decode,
};

use crate::theme;
use crate::tool_icons;
use egui::{
    Color32, Context, Frame, Id, Key, Pos2, Rect, RichText, ScrollArea, Sense, Stroke, TextEdit,
    Ui, Vec2,
};
use model::{
    FILE_NAME, Field, MAX_BODY_BYTES, MAX_REQUESTS, Request, Store, absorb_url_query,
    apply_body_kind, limit_chars, normalize_request, normalize_tags, note_content_type,
    percent_encode, tag_key,
};
use send::{Hub, Outcome, Outgoing};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const SAVE_DELAY: Duration = Duration::from_millis(250);
const METHODS: [&str; 8] = [
    "GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS", "TRACE",
];
const SIDEBAR_BG: Color32 = Color32::from_rgb(37, 38, 40);
const LIST_BG: Color32 = Color32::from_rgb(32, 33, 36);
const EDITOR_BG: Color32 = Color32::from_rgb(30, 31, 33);
const NAV_SELECTED: Color32 = Color32::from_rgb(58, 60, 66);
const NAV_HOVER: Color32 = Color32::from_rgb(48, 49, 54);
const ORANGE: Color32 = Color32::from_rgb(214, 140, 64);

/// One `OpenAPI` JSON file the export button can write.
#[derive(Clone, Debug)]
pub(crate) struct ExportTarget {
    /// Label shown on the button or in the menu.
    pub label: String,
    /// Absolute path of the document.
    pub path: PathBuf,
}

/// What the shell should do after a click in the HTTP Requests window.
pub(crate) enum PanelCommand {
    /// Add or replace this saved call in an `OpenAPI` JSON document.
    Export { id: String, path: PathBuf },
}

/// A file read or create that should receive a saved call when the worker answers.
pub(crate) struct PendingExport {
    path: PathBuf,
    request: Request,
    phase: ExportPhase,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ExportPhase {
    Create,
    Read,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Filter {
    All,
    Untagged,
    Tag,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Query,
    Headers,
    Body,
}

enum WorkerReply {
    Loaded(Store),
    Reloaded(Store),
    Saved,
    Corrupt(String),
    Io(String),
}

enum Task {
    Save(Store),
    Reload,
    Finish {
        file: Option<Store>,
        done: mpsc::Sender<()>,
    },
}

enum Slot {
    Search,
    Name,
    Url,
    Body,
    Curl,
    Tag,
    QueryName(usize),
    QueryValue(usize),
    HeaderName(usize),
    HeaderValue(usize),
    FormName(usize),
    FormValue(usize),
}

/// HTTP library for one project and the state of its tool window.
#[allow(
    clippy::struct_excessive_bools,
    reason = "Each flag is an independent session fact: load, write lock, curl refresh and popups"
)]
pub(crate) struct HttpRequests {
    root: Option<PathBuf>,
    file: Store,
    writable: bool,
    loaded: bool,
    error: Option<String>,
    notice: Option<String>,
    reported: Option<String>,
    dirty: bool,
    pending_save: Option<Store>,
    worker: Option<mpsc::Sender<Task>>,
    output: Option<mpsc::Receiver<WorkerReply>>,
    changed: Option<Instant>,
    filter: Filter,
    tag_filter: String,
    query: String,
    selected: Option<String>,
    list_width: f32,
    tab: Tab,
    curl_text: String,
    curl_error: Option<String>,
    curl_stale: bool,
    assign_tags: bool,
    tag_draft: String,
    outcomes: HashMap<String, Outcome>,
    inflight: HashMap<String, mpsc::Receiver<Outcome>>,
    hub: Hub,
    pending_imports: Vec<Incoming>,
    next_id: u64,
}

impl Default for HttpRequests {
    fn default() -> Self {
        Self {
            root: None,
            file: Store::default(),
            writable: true,
            loaded: false,
            error: None,
            notice: None,
            reported: None,
            dirty: false,
            pending_save: None,
            worker: None,
            output: None,
            changed: None,
            filter: Filter::All,
            tag_filter: String::new(),
            query: String::new(),
            selected: None,
            list_width: 300.0,
            tab: Tab::Query,
            curl_text: String::new(),
            curl_error: None,
            curl_stale: true,
            assign_tags: false,
            tag_draft: String::new(),
            outcomes: HashMap::new(),
            inflight: HashMap::new(),
            hub: Hub::default(),
            pending_imports: Vec::new(),
            next_id: 0,
        }
    }
}

impl Drop for HttpRequests {
    fn drop(&mut self) {
        self.finish();
    }
}

impl HttpRequests {
    /// Starts loading when the tool is shown for this project root.
    pub(crate) fn ensure(&mut self, root: &Path, ctx: &Context) {
        if self.root.as_deref() == Some(root) && self.worker.is_some() {
            return;
        }
        self.finish();
        self.reset(root);
        let (sender, input) = mpsc::channel();
        let (output, receiver) = mpsc::channel();
        let path = root.join(FILE_NAME);
        let wake_ctx = ctx.clone();
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(move || wake_ctx.request_repaint());
        match std::thread::Builder::new()
            .name("nozhik-http-requests".into())
            .spawn(move || worker(&path, &input, &output, wake.as_ref()))
        {
            Ok(_) => {
                self.worker = Some(sender);
                self.output = Some(receiver);
            }
            Err(error) => {
                self.error = Some(error.to_string());
                self.loaded = true;
                self.writable = false;
            }
        }
    }

    /// True when `path` is this project's request file.
    pub(crate) fn is_store_path(&self, path: &Path) -> bool {
        self.root
            .as_ref()
            .is_some_and(|root| path == root.join(FILE_NAME))
    }

    /// Asks the worker to reread the file. A save that matches memory is ignored.
    pub(crate) fn note_disk_change(&self) {
        if let Some(worker) = &self.worker {
            let _sent = worker.send(Task::Reload);
        }
    }

    /// Drains worker replies, finished HTTP calls and debounced edits.
    pub(crate) fn tick(&mut self, ctx: &Context) {
        self.poll();
        self.poll_http();
        self.drain_imports();
        if !self.loaded && self.worker.is_some() || !self.inflight.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        if let Some(changed) = self.changed {
            let remaining = SAVE_DELAY.saturating_sub(changed.elapsed());
            if remaining.is_zero() {
                self.flush();
            } else {
                ctx.request_repaint_after(remaining);
            }
        }
    }

    /// A new load or limit failure, once per distinct message.
    pub(crate) fn take_problem(&mut self) -> Option<(crate::problems::Severity, String)> {
        let current = self.error.as_ref().map_or_else(
            || {
                self.notice
                    .as_ref()
                    .map(|message| (crate::problems::Severity::Warning, message.clone()))
            },
            |message| Some((crate::problems::Severity::Error, message.clone())),
        );
        let text = current.as_ref().map(|(_, message)| message.clone());
        if self.reported == text {
            return None;
        }
        self.reported = text;
        current
    }

    /// Blocks until the last edit is written, or one second passes.
    pub(crate) fn finish(&mut self) {
        let Some(worker) = self.worker.take() else {
            return;
        };
        let (done, wait) = mpsc::channel();
        let file = self.dirty.then(|| self.file.clone());
        if worker.send(Task::Finish { file, done }).is_ok() {
            let _finished = wait.recv_timeout(Duration::from_secs(1));
        }
        self.output = None;
        self.dirty = false;
        self.changed = None;
        self.pending_save = None;
    }

    /// Copies the saved call. `None` when the id is gone.
    pub(crate) fn snapshot(&self, id: &str) -> Option<Request> {
        self.file
            .requests
            .iter()
            .find(|request| request.id == id)
            .cloned()
    }

    /// Queues an `OpenAPI` operation. It is inserted once the library is loaded.
    pub(crate) fn import(&mut self, root: &Path, ctx: &Context, incoming: Incoming) {
        self.pending_imports.push(incoming);
        self.ensure(root, ctx);
        self.drain_imports();
    }

    /// Tag column. `enabled` is false while a modal owns the pointer.
    pub(crate) fn show_nav(&mut self, ui: &mut Ui, root: Option<&Path>, enabled: bool) {
        if let Some(root) = root {
            self.ensure(root, ui.ctx());
            self.tick(ui.ctx());
        }
        let mut pick = None;
        ScrollArea::vertical()
            .id_salt("http_request_nav")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(8.0);
                section_label(ui, "Library");
                if nav_row(ui, "All requests", None, self.filter == Filter::All) {
                    pick = Some(Filter::All);
                }
                if nav_row(ui, "Untagged", None, self.filter == Filter::Untagged) {
                    pick = Some(Filter::Untagged);
                }
                section_label(ui, "Tags");
                if self.file.requests.is_empty() && self.loaded {
                    ui.add_space(4.0);
                    ui.weak("No tags yet");
                }
                for (tag, count) in tag_rows(&self.file.requests) {
                    let selected =
                        self.filter == Filter::Tag && tag_key(&self.tag_filter) == tag_key(&tag);
                    if nav_row(ui, &tag, Some(count), selected) {
                        self.tag_filter = tag;
                        pick = Some(Filter::Tag);
                    }
                }
                ui.add_space(12.0);
            });
        if enabled && let Some(pick) = pick {
            self.filter = pick;
            self.assign_tags = false;
            self.sync_selection();
            self.curl_stale = true;
        }
    }

    /// List, builder and `cURL` pane.
    pub(crate) fn show_main(
        &mut self,
        ui: &mut Ui,
        root: Option<&Path>,
        enabled: bool,
        targets: &[ExportTarget],
    ) -> Option<PanelCommand> {
        if let Some(root) = root {
            self.ensure(root, ui.ctx());
            self.tick(ui.ctx());
        }
        let full = ui.available_rect_before_wrap();
        if full.width() < 8.0 || full.height() < 8.0 {
            return None;
        }
        let _allocated = ui.allocate_rect(full, Sense::hover());
        if root.is_none() {
            paint_centered(ui, full, "Open a project to store HTTP requests.");
            return None;
        }
        if !self.writable {
            let message = self.error.clone().unwrap_or_else(|| {
                "http_requests.json could not be read. Editing is disabled.".into()
            });
            paint_centered(ui, full, &message);
            return None;
        }
        if enabled && ui.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, Key::Enter))
        {
            self.send_selected();
        }
        if enabled
            && !self.editor_focused(ui.ctx())
            && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Delete))
        {
            self.delete_selected();
        }
        let list_width = self
            .list_width
            .clamp(220.0, (full.width() - 360.0).max(220.0));
        let list_rect = Rect::from_min_size(full.min, Vec2::new(list_width, full.height()));
        let splitter_rect = Rect::from_min_size(
            list_rect.right_top(),
            Vec2::new(theme::SPLITTER, full.height()),
        );
        let editor_rect = Rect::from_min_max(splitter_rect.right_top(), full.max);
        ui.painter().rect_filled(list_rect, 0.0, LIST_BG);
        ui.painter().rect_filled(editor_rect, 0.0, EDITOR_BG);
        let mut list_ui =
            ui.new_child(egui::UiBuilder::new().max_rect(list_rect.shrink2(Vec2::new(8.0, 8.0))));
        self.show_list(&mut list_ui, enabled);
        let splitter = ui
            .interact(splitter_rect, ui.id().with("http_splitter"), Sense::drag())
            .on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
        if splitter.dragged() && enabled {
            self.list_width = (self.list_width + splitter.drag_delta().x).clamp(220.0, 480.0);
        }
        ui.painter().vline(
            splitter_rect.center().x,
            splitter_rect.y_range(),
            Stroke::new(1.0_f32, theme::BORDER),
        );
        let mut editor_ui = ui.new_child(egui::UiBuilder::new().max_rect(editor_rect));
        self.show_editor(&mut editor_ui, enabled, targets)
    }

    /// True while a text field in this tool owns the keyboard.
    pub(crate) fn editor_focused(&self, ctx: &Context) -> bool {
        self.focused_slot(ctx).is_some()
    }

    /// Routes Edit menu clipboard actions while HTTP Requests owns the workspace.
    pub(crate) fn handle_clipboard(&mut self, ctx: &Context, action: &str) -> bool {
        let Some(slot) = self.focused_slot(ctx) else {
            if action == "ui.copy" {
                ctx.copy_text(self.curl_text.clone());
            }
            return true;
        };
        match action {
            "ui.copy" => self.copy_slot(ctx, &slot),
            "ui.cut" => self.cut_slot(ctx, &slot),
            "ui.paste" => ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste),
            _ => {}
        }
        true
    }

    fn reset(&mut self, root: &Path) {
        let pending = std::mem::take(&mut self.pending_imports);
        let list_width = self.list_width;
        *self = Self::default();
        self.root = Some(root.to_path_buf());
        self.list_width = list_width;
        self.pending_imports = pending;
    }

    fn poll(&mut self) {
        let replies = {
            let Some(output) = &self.output else {
                return;
            };
            let mut replies = Vec::new();
            while let Ok(reply) = output.try_recv() {
                replies.push(reply);
            }
            replies
        };
        for reply in replies {
            match reply {
                WorkerReply::Loaded(file) | WorkerReply::Reloaded(file) => {
                    if !self.dirty && self.pending_save.as_ref() != Some(&file) && self.file != file
                    {
                        self.file = file;
                        self.notice = None;
                        self.sync_selection();
                        self.curl_stale = true;
                    }
                    self.loaded = true;
                    self.writable = true;
                    self.error = None;
                }
                WorkerReply::Saved => {
                    if self.pending_save.as_ref() == Some(&self.file) {
                        self.dirty = false;
                        self.pending_save = None;
                    } else if self.dirty {
                        self.changed = Some(Instant::now());
                    }
                }
                WorkerReply::Corrupt(message) => {
                    self.loaded = true;
                    self.writable = false;
                    self.error = Some(message);
                    self.changed = None;
                    self.pending_save = None;
                }
                WorkerReply::Io(message) => {
                    self.loaded = true;
                    self.error = Some(message);
                    self.pending_save = None;
                    if self.dirty {
                        self.changed = Some(Instant::now());
                    }
                }
            }
        }
    }

    fn poll_http(&mut self) {
        let keys: Vec<String> = self.inflight.keys().cloned().collect();
        let mut finished = Vec::new();
        for key in keys {
            let Some(receiver) = self.inflight.get(&key) else {
                continue;
            };
            match receiver.try_recv() {
                Ok(outcome) => finished.push((key, outcome)),
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    finished.push((key, Outcome::transport("HTTP worker stopped")));
                }
            }
        }
        for (key, outcome) in finished {
            self.inflight.remove(&key);
            self.outcomes.insert(key, outcome);
        }
    }

    fn flush(&mut self) {
        self.changed = None;
        if !self.dirty || !self.writable {
            return;
        }
        let Some(worker) = &self.worker else {
            return;
        };
        let file = self.file.clone();
        if worker.send(Task::Save(file.clone())).is_err() {
            self.error = Some("HTTP Requests worker stopped".into());
            self.writable = false;
            return;
        }
        self.pending_save = Some(file);
    }

    fn touch(&mut self) {
        self.dirty = true;
        self.changed = Some(Instant::now());
    }

    fn drain_imports(&mut self) {
        if !self.loaded || !self.can_edit() || self.pending_imports.is_empty() {
            return;
        }
        let queued = std::mem::take(&mut self.pending_imports);
        for incoming in queued {
            self.add_incoming(incoming);
        }
    }

    fn add_incoming(&mut self, incoming: Incoming) {
        if self.file.requests.len() >= MAX_REQUESTS {
            self.notice = Some(format!(
                "A project can store at most {MAX_REQUESTS} requests"
            ));
            return;
        }
        let now = now_ms();
        let id = self.allocate_id();
        match incoming.into_request(id.clone(), now) {
            Ok(mut request) => {
                absorb_url_query(&mut request);
                self.file.requests.push(request);
                self.filter = Filter::All;
                self.query.clear();
                self.selected = Some(id);
                self.curl_stale = true;
                self.assign_tags = false;
                self.touch();
            }
            Err(error) => self.notice = Some(error),
        }
    }

    const fn can_edit(&self) -> bool {
        self.loaded && self.writable && self.error.is_none()
    }

    fn allocate_id(&mut self) -> String {
        for _ in 0..1_000 {
            self.next_id = self.next_id.saturating_add(1);
            let id = format!("h{:x}-{:x}", now_ms(), self.next_id);
            if !self.file.requests.iter().any(|request| request.id == id) {
                return id;
            }
        }
        self.next_id = self.next_id.saturating_add(1);
        format!("h{:x}-{:x}-extra", now_ms(), self.next_id)
    }

    fn sync_selection(&mut self) {
        let visible = self.visible_indices();
        if let Some(id) = &self.selected
            && visible
                .iter()
                .any(|&index| &self.file.requests[index].id == id)
        {
            return;
        }
        self.selected = visible
            .first()
            .map(|&index| self.file.requests[index].id.clone());
        self.assign_tags = false;
        self.curl_stale = true;
    }

    fn visible_indices(&self) -> Vec<usize> {
        let mut indices: Vec<usize> = self
            .file
            .requests
            .iter()
            .enumerate()
            .filter(|(_, request)| self.matches(request))
            .map(|(index, _)| index)
            .collect();
        indices.sort_by(|&left, &right| {
            self.file.requests[right]
                .updated_ms
                .cmp(&self.file.requests[left].updated_ms)
                .then(left.cmp(&right))
        });
        indices
    }

    fn matches(&self, request: &Request) -> bool {
        let tagged = match self.filter {
            Filter::All => true,
            Filter::Untagged => request.tags.is_empty(),
            Filter::Tag => request
                .tags
                .iter()
                .any(|tag| tag_key(tag) == tag_key(&self.tag_filter)),
        };
        if !tagged {
            return false;
        }
        let query = self.query.trim().to_lowercase();
        query.is_empty()
            || request.name.to_lowercase().contains(&query)
            || request.url.to_lowercase().contains(&query)
            || request.method.to_lowercase().contains(&query)
            || request
                .tags
                .iter()
                .any(|tag| tag.to_lowercase().contains(&query))
    }

    fn show_list(&mut self, ui: &mut Ui, enabled: bool) {
        ui.horizontal(|ui| {
            let width = (ui.available_width() - 36.0).max(40.0);
            ui.add_sized(
                Vec2::new(width, 28.0),
                TextEdit::singleline(&mut self.query)
                    .id(search_id())
                    .hint_text("Search...")
                    .desired_width(width),
            );
            if ui
                .add_enabled(
                    enabled && self.can_edit(),
                    egui::Button::new("+").min_size(Vec2::splat(28.0)),
                )
                .on_hover_text("New request")
                .clicked()
            {
                self.create();
            }
        });
        ui.add_space(8.0);
        if !self.loaded {
            ui.weak("Loading requests…");
            return;
        }
        let indices = self.visible_indices();
        if indices.is_empty() {
            ui.weak(if self.file.requests.is_empty() {
                "No requests yet"
            } else {
                "No matching requests"
            });
            return;
        }
        let mut delete = None;
        ScrollArea::vertical()
            .id_salt("http_request_list")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for index in indices {
                    let id = self.file.requests[index].id.clone();
                    let selected = self.selected.as_deref() == Some(id.as_str());
                    let response = request_card(ui, &self.file.requests[index], selected);
                    if enabled && response.clicked() {
                        self.selected = Some(id.clone());
                        self.curl_stale = true;
                        self.assign_tags = false;
                    }
                    if enabled {
                        response.context_menu(|ui| {
                            if ui.button("Delete").clicked() {
                                delete = Some(id.clone());
                                ui.close();
                            }
                        });
                    }
                }
            });
        if let Some(id) = delete {
            self.delete_id(&id);
        }
    }

    fn create(&mut self) {
        if !self.can_edit() || self.file.requests.len() >= MAX_REQUESTS {
            if self.can_edit() {
                self.notice = Some(format!(
                    "A project can store at most {MAX_REQUESTS} requests"
                ));
            }
            return;
        }
        let now = now_ms();
        let id = self.allocate_id();
        self.file.requests.push(Request::blank(id.clone(), now));
        self.filter = Filter::All;
        self.query.clear();
        self.selected = Some(id);
        self.curl_stale = true;
        self.assign_tags = false;
        self.tab = Tab::Query;
        self.touch();
    }

    fn delete_selected(&mut self) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        self.delete_id(&id);
    }

    fn delete_id(&mut self, id: &str) {
        if !self.can_edit() {
            return;
        }
        self.file.requests.retain(|request| request.id != id);
        self.outcomes.remove(id);
        self.inflight.remove(id);
        self.sync_selection();
        self.touch();
    }

    #[allow(
        clippy::too_many_lines,
        reason = "The builder, cURL pane and response stay in one scroll so both editors update together"
    )]
    fn show_editor(
        &mut self,
        ui: &mut Ui,
        enabled: bool,
        targets: &[ExportTarget],
    ) -> Option<PanelCommand> {
        if self.curl_stale {
            self.refresh_curl();
        }
        let Some(index) = self.selected_index() else {
            paint_centered(
                ui,
                ui.available_rect_before_wrap(),
                "Create a request to build it here.",
            );
            return None;
        };
        let request_id = self.file.requests[index].id.clone();
        let mut command = None;
        let mut builder_changed = false;
        let mut header_edited = false;
        ScrollArea::vertical()
            .id_salt(("http_editor", request_id.clone()))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.add_enabled_ui(enabled && self.can_edit(), |ui| {
                        let name = TextEdit::singleline(&mut self.file.requests[index].name)
                            .id(name_id(&request_id))
                            .hint_text("Request name")
                            .desired_width((ui.available_width() - 220.0).max(120.0));
                        if ui.add(name).changed() {
                            limit_chars(&mut self.file.requests[index].name, model::MAX_NAME_CHARS);
                            builder_changed = true;
                        }
                    });
                    if ui.add_enabled(enabled, egui::Button::new("Tags")).clicked() {
                        self.assign_tags = !self.assign_tags;
                    }
                    command = export_control(ui, enabled, &request_id, targets);
                });
                if self.assign_tags {
                    self.tag_popup(ui, index, enabled);
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.add_enabled_ui(enabled && self.can_edit(), |ui| {
                        let mut method = self.file.requests[index].method.clone();
                        egui::ComboBox::from_id_salt(("http_method", request_id.clone()))
                            .selected_text(&method)
                            .width(96.0)
                            .show_ui(ui, |ui| {
                                for item in METHODS {
                                    ui.selectable_value(&mut method, item.to_owned(), item);
                                }
                            });
                        if method != self.file.requests[index].method {
                            self.file.requests[index].method = method;
                            builder_changed = true;
                        }
                        let url = TextEdit::singleline(&mut self.file.requests[index].url)
                            .id(url_id(&request_id))
                            .hint_text("https://example.com/path")
                            .desired_width((ui.available_width() - 88.0).max(160.0));
                        if ui.add(url).changed() {
                            absorb_url_query(&mut self.file.requests[index]);
                            builder_changed = true;
                        }
                    });
                    if ui
                        .add_enabled(enabled && self.can_edit(), egui::Button::new("Send"))
                        .clicked()
                    {
                        self.send_selected();
                    }
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    for (tab, label) in [
                        (Tab::Query, "Query"),
                        (Tab::Headers, "Headers"),
                        (Tab::Body, "Body"),
                    ] {
                        if ui.selectable_label(self.tab == tab, label).clicked() {
                            self.tab = tab;
                        }
                    }
                });
                ui.add_space(4.0);
                ui.add_enabled_ui(enabled && self.can_edit(), |ui| match self.tab {
                    Tab::Query => {
                        builder_changed |= edit_fields(
                            ui,
                            &request_id,
                            "query",
                            &mut self.file.requests[index].query,
                        );
                    }
                    Tab::Headers => {
                        let changed = edit_fields(
                            ui,
                            &request_id,
                            "header",
                            &mut self.file.requests[index].headers,
                        );
                        header_edited = changed;
                        builder_changed |= changed;
                    }
                    Tab::Body => builder_changed |= self.edit_body(ui, index, &request_id),
                });
                ui.add_space(10.0);
                ui.label(RichText::new("cURL").strong());
                if let Some(error) = &self.curl_error {
                    ui.colored_label(theme::ERROR, error);
                }
                if builder_changed {
                    if header_edited {
                        note_content_type(&mut self.file.requests[index]);
                    }
                    self.curl_text = curl::render(&self.file.requests[index]);
                    self.curl_error = None;
                    self.curl_stale = false;
                }
                let editable = enabled && self.can_edit();
                let curl = TextEdit::multiline(&mut self.curl_text)
                    .id(curl_id(&request_id))
                    .font(egui::FontId::monospace(13.0))
                    .desired_rows(8)
                    .desired_width(ui.available_width());
                let curl_changed = ui.add_enabled(editable, curl).changed();
                if builder_changed {
                    self.file.requests[index].updated_ms = now_ms();
                    self.touch();
                } else if curl_changed {
                    match curl::parse(&self.curl_text) {
                        Ok(parsed) => {
                            curl::apply(&mut self.file.requests[index], parsed);
                            if let Err(error) = normalize_request(&mut self.file.requests[index]) {
                                self.curl_error = Some(error);
                            } else {
                                self.file.requests[index].updated_ms = now_ms();
                                self.curl_error = None;
                                self.touch();
                            }
                        }
                        Err(error) => self.curl_error = Some(error),
                    }
                }
                self.show_response(ui, &request_id);
                ui.add_space(12.0);
            });
        command
    }

    fn edit_body(&mut self, ui: &mut Ui, index: usize, request_id: &str) -> bool {
        let mut changed = false;
        let current = self.file.requests[index].body_kind;
        let mut kind = current;
        egui::ComboBox::from_id_salt(("http_body", request_id))
            .selected_text(current.label())
            .show_ui(ui, |ui| {
                for item in [
                    BodyKind::None,
                    BodyKind::Json,
                    BodyKind::Text,
                    BodyKind::Form,
                    BodyKind::Multipart,
                ] {
                    ui.selectable_value(&mut kind, item, item.label());
                }
            });
        if kind != current {
            apply_body_kind(&mut self.file.requests[index], kind);
            changed = true;
        }
        match self.file.requests[index].body_kind {
            BodyKind::None => {
                ui.weak("This request has no body. Query parameters are still sent.");
            }
            BodyKind::Json | BodyKind::Text => {
                let editor = TextEdit::multiline(&mut self.file.requests[index].body)
                    .id(body_id(request_id))
                    .font(egui::FontId::monospace(13.0))
                    .desired_rows(8)
                    .desired_width(ui.available_width());
                if ui.add(editor).changed() {
                    limit_chars(&mut self.file.requests[index].body, MAX_BODY_BYTES);
                    changed = true;
                }
            }
            BodyKind::Form | BodyKind::Multipart => {
                changed |= edit_fields(ui, request_id, "form", &mut self.file.requests[index].form);
            }
        }
        changed
    }

    fn tag_popup(&mut self, ui: &mut Ui, index: usize, enabled: bool) {
        let request_id = self.file.requests[index].id.clone();
        let mut remove = None;
        let mut add = None;
        ui.group(|ui| {
            ui.label("Tags");
            ui.horizontal_wrapped(|ui| {
                for tag in &self.file.requests[index].tags {
                    if ui.button(format!("{tag} ×")).clicked() && enabled {
                        remove = Some(tag.clone());
                    }
                }
            });
            let editable = enabled && self.can_edit();
            let editor = TextEdit::singleline(&mut self.tag_draft)
                .id(tag_id(&request_id))
                .hint_text("Add a tag")
                .desired_width(180.0);
            let response = ui.add_enabled(editable, editor);
            if response.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter)) {
                add = Some(std::mem::take(&mut self.tag_draft));
            }
        });
        if let Some(tag) = remove {
            self.remove_tag(index, &tag);
        }
        if let Some(raw) = add {
            for part in raw.split(',') {
                self.try_add_tag(index, part);
            }
        }
    }

    fn try_add_tag(&mut self, index: usize, raw: &str) {
        let Some(tag) = normalize_tag(raw) else {
            if !raw.trim().is_empty() {
                let max_chars = model::MAX_TAG_CHARS;
                self.notice = Some(format!(
                    "A tag can be at most {max_chars} characters and cannot contain a comma"
                ));
            }
            return;
        };
        if self.file.requests[index]
            .tags
            .iter()
            .any(|existing| tag_key(existing) == tag_key(&tag))
        {
            return;
        }
        if self.file.requests[index].tags.len() >= model::MAX_TAGS {
            let max_tags = model::MAX_TAGS;
            self.notice = Some(format!("A request can have at most {max_tags} tags"));
            return;
        }
        self.file.requests[index].tags.push(tag);
        self.file.requests[index].updated_ms = now_ms();
        self.touch();
    }

    fn remove_tag(&mut self, index: usize, tag: &str) {
        let before = self.file.requests[index].tags.len();
        let key = tag_key(tag);
        self.file.requests[index]
            .tags
            .retain(|existing| tag_key(existing) != key);
        if self.file.requests[index].tags.len() != before {
            self.file.requests[index].updated_ms = now_ms();
            self.touch();
        }
    }

    fn show_response(&self, ui: &mut Ui, id: &str) {
        if self.inflight.contains_key(id) {
            ui.add_space(8.0);
            ui.weak("Sending…");
        }
        let Some(outcome) = self.outcomes.get(id) else {
            return;
        };
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let color = outcome.status_code.map_or(theme::ERROR, |code| {
                if (200..400).contains(&code) {
                    theme::SUCCESS
                } else {
                    theme::ERROR
                }
            });
            let label = if outcome.status_line.is_empty() {
                "No response".to_owned()
            } else {
                outcome.status_line.clone()
            };
            ui.colored_label(color, label);
            ui.weak(format!("{} ms", outcome.elapsed_ms));
        });
        if let Some(error) = &outcome.transport_error {
            ui.colored_label(theme::ERROR, error);
        }
        if !outcome.headers.is_empty() {
            ui.monospace(&outcome.headers);
        }
        if !outcome.body.is_empty() {
            egui::Frame::new()
                .fill(Color32::from_rgb(24, 25, 26))
                .inner_margin(8)
                .corner_radius(6)
                .show(ui, |ui| {
                    ui.monospace(&outcome.body);
                });
        }
    }

    fn refresh_curl(&mut self) {
        self.curl_text = self
            .selected_index()
            .map(|index| curl::render(&self.file.requests[index]))
            .unwrap_or_default();
        self.curl_error = None;
        self.curl_stale = false;
    }

    fn send_selected(&mut self) {
        let Some(index) = self.selected_index() else {
            return;
        };
        let request = self.file.requests[index].clone();
        let id = request.id.clone();
        match assemble(&request) {
            Ok(outgoing) => match self.hub.submit(outgoing) {
                Ok(receiver) => {
                    self.inflight.insert(id.clone(), receiver);
                    self.outcomes.remove(&id);
                    self.notice = None;
                }
                Err(error) => self.notice = Some(error),
            },
            Err(error) => self.notice = Some(error),
        }
    }

    fn selected_index(&self) -> Option<usize> {
        let id = self.selected.as_ref()?;
        self.file
            .requests
            .iter()
            .position(|request| &request.id == id)
    }

    fn focused_slot(&self, ctx: &Context) -> Option<Slot> {
        ctx.memory(|memory| {
            if memory.has_focus(search_id()) {
                return Some(Slot::Search);
            }
            let id = self.selected.as_ref()?;
            if memory.has_focus(name_id(id)) {
                return Some(Slot::Name);
            }
            if memory.has_focus(url_id(id)) {
                return Some(Slot::Url);
            }
            if memory.has_focus(body_id(id)) {
                return Some(Slot::Body);
            }
            if memory.has_focus(curl_id(id)) {
                return Some(Slot::Curl);
            }
            if memory.has_focus(tag_id(id)) {
                return Some(Slot::Tag);
            }
            let index = self.selected_index()?;
            let request = &self.file.requests[index];
            for (offset, _) in request.query.iter().enumerate() {
                if memory.has_focus(field_id(id, "query", offset, "name")) {
                    return Some(Slot::QueryName(offset));
                }
                if memory.has_focus(field_id(id, "query", offset, "value")) {
                    return Some(Slot::QueryValue(offset));
                }
            }
            for (offset, _) in request.headers.iter().enumerate() {
                if memory.has_focus(field_id(id, "header", offset, "name")) {
                    return Some(Slot::HeaderName(offset));
                }
                if memory.has_focus(field_id(id, "header", offset, "value")) {
                    return Some(Slot::HeaderValue(offset));
                }
            }
            for (offset, _) in request.form.iter().enumerate() {
                if memory.has_focus(field_id(id, "form", offset, "name")) {
                    return Some(Slot::FormName(offset));
                }
                if memory.has_focus(field_id(id, "form", offset, "value")) {
                    return Some(Slot::FormValue(offset));
                }
            }
            None
        })
    }

    fn copy_slot(&self, ctx: &Context, slot: &Slot) {
        let Some(text) = self.slot_text(slot) else {
            return;
        };
        let id = self.slot_id(slot);
        if let Some(state) = TextEdit::load_state(ctx, id)
            && let Some(range) = state.cursor.char_range()
        {
            let selected = range.slice_str(&text);
            if !selected.is_empty() {
                ctx.copy_text(selected.to_owned());
                return;
            }
        }
        ctx.copy_text(text);
    }

    fn cut_slot(&mut self, ctx: &Context, slot: &Slot) {
        if !self.can_edit() {
            return;
        }
        let Some(text) = self.slot_text(slot) else {
            return;
        };
        let id = self.slot_id(slot);
        let Some(mut state) = TextEdit::load_state(ctx, id) else {
            return;
        };
        let Some(range) = state.cursor.char_range() else {
            return;
        };
        let selected = range.slice_str(&text);
        if selected.is_empty() {
            return;
        }
        ctx.copy_text(selected.to_owned());
        let span = range.as_sorted_char_range();
        let mut next = text.chars().take(span.start).collect::<String>();
        next.extend(text.chars().skip(span.end));
        self.set_slot(slot, next);
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::one(
                egui::text::CCursor::new(span.start),
            )));
        state.store(ctx, id);
    }

    fn slot_text(&self, slot: &Slot) -> Option<String> {
        let request = self
            .selected_index()
            .map(|index| &self.file.requests[index]);
        match slot {
            Slot::Search => Some(self.query.clone()),
            Slot::Tag => Some(self.tag_draft.clone()),
            Slot::Curl => Some(self.curl_text.clone()),
            Slot::Name => request.map(|request| request.name.clone()),
            Slot::Url => request.map(|request| request.url.clone()),
            Slot::Body => request.map(|request| request.body.clone()),
            Slot::QueryName(index) => request
                .and_then(|request| request.query.get(*index))
                .map(|field| field.name.clone()),
            Slot::QueryValue(index) => request
                .and_then(|request| request.query.get(*index))
                .map(|field| field.value.clone()),
            Slot::HeaderName(index) => request
                .and_then(|request| request.headers.get(*index))
                .map(|field| field.name.clone()),
            Slot::HeaderValue(index) => request
                .and_then(|request| request.headers.get(*index))
                .map(|field| field.value.clone()),
            Slot::FormName(index) => request
                .and_then(|request| request.form.get(*index))
                .map(|field| field.name.clone()),
            Slot::FormValue(index) => request
                .and_then(|request| request.form.get(*index))
                .map(|field| field.value.clone()),
        }
    }

    fn set_slot(&mut self, slot: &Slot, text: String) {
        match slot {
            Slot::Search => self.query = text,
            Slot::Tag => self.tag_draft = text,
            Slot::Curl => {
                self.curl_text = text;
                if let Some(index) = self.selected_index() {
                    match curl::parse(&self.curl_text) {
                        Ok(parsed) => {
                            curl::apply(&mut self.file.requests[index], parsed);
                            self.curl_error =
                                normalize_request(&mut self.file.requests[index]).err();
                            if self.curl_error.is_none() {
                                self.file.requests[index].updated_ms = now_ms();
                                self.touch();
                            }
                        }
                        Err(error) => self.curl_error = Some(error),
                    }
                }
            }
            Slot::Name
            | Slot::Url
            | Slot::Body
            | Slot::QueryName(_)
            | Slot::QueryValue(_)
            | Slot::HeaderName(_)
            | Slot::HeaderValue(_)
            | Slot::FormName(_)
            | Slot::FormValue(_) => {
                let Some(index) = self.selected_index() else {
                    return;
                };
                match slot {
                    Slot::Name => {
                        self.file.requests[index].name = text;
                        limit_chars(&mut self.file.requests[index].name, model::MAX_NAME_CHARS);
                    }
                    Slot::Url => {
                        self.file.requests[index].url = text;
                        absorb_url_query(&mut self.file.requests[index]);
                    }
                    Slot::Body => {
                        self.file.requests[index].body = text;
                        limit_chars(&mut self.file.requests[index].body, MAX_BODY_BYTES);
                    }
                    Slot::QueryName(offset) => {
                        if let Some(field) = self.file.requests[index].query.get_mut(*offset) {
                            field.name = text;
                        }
                    }
                    Slot::QueryValue(offset) => {
                        if let Some(field) = self.file.requests[index].query.get_mut(*offset) {
                            field.value = text;
                        }
                    }
                    Slot::HeaderName(offset) => {
                        if let Some(field) = self.file.requests[index].headers.get_mut(*offset) {
                            field.name = text;
                        }
                    }
                    Slot::HeaderValue(offset) => {
                        if let Some(field) = self.file.requests[index].headers.get_mut(*offset) {
                            field.value = text;
                        }
                        note_content_type(&mut self.file.requests[index]);
                    }
                    Slot::FormName(offset) => {
                        if let Some(field) = self.file.requests[index].form.get_mut(*offset) {
                            field.name = text;
                        }
                    }
                    Slot::FormValue(offset) => {
                        if let Some(field) = self.file.requests[index].form.get_mut(*offset) {
                            field.value = text;
                        }
                    }
                    Slot::Search | Slot::Tag | Slot::Curl => {}
                }
                self.file.requests[index].updated_ms = now_ms();
                self.curl_stale = true;
                self.touch();
            }
        }
    }

    fn slot_id(&self, slot: &Slot) -> Id {
        let request = self.selected.as_deref().unwrap_or("");
        match slot {
            Slot::Search => search_id(),
            Slot::Name => name_id(request),
            Slot::Url => url_id(request),
            Slot::Body => body_id(request),
            Slot::Curl => curl_id(request),
            Slot::Tag => tag_id(request),
            Slot::QueryName(index) => field_id(request, "query", *index, "name"),
            Slot::QueryValue(index) => field_id(request, "query", *index, "value"),
            Slot::HeaderName(index) => field_id(request, "header", *index, "name"),
            Slot::HeaderValue(index) => field_id(request, "header", *index, "value"),
            Slot::FormName(index) => field_id(request, "form", *index, "name"),
            Slot::FormValue(index) => field_id(request, "form", *index, "value"),
        }
    }
}

/// Tag sidebar surface.
pub(crate) fn sidebar_frame() -> Frame {
    Frame::new().fill(SIDEBAR_BG).inner_margin(0)
}

/// Builder surface.
pub(crate) fn editor_frame() -> Frame {
    Frame::new().fill(EDITOR_BG).inner_margin(0)
}

fn assemble(request: &Request) -> Result<Outgoing, String> {
    let url = url_with_query(&request.url, &request.query);
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("URL must start with http:// or https://".into());
    }
    let mut headers = Vec::new();
    for field in request
        .headers
        .iter()
        .filter(|field| field.enabled && !field.name.is_empty())
    {
        if request.body_kind == BodyKind::Multipart
            && field.name.eq_ignore_ascii_case("content-type")
        {
            continue;
        }
        headers.push((field.name.clone(), field.value.clone()));
    }
    let body = match request.body_kind {
        BodyKind::None => None,
        BodyKind::Json | BodyKind::Text => {
            if let Some(content) = request.body_kind.default_content_type()
                && !headers
                    .iter()
                    .any(|(name, _)| name.eq_ignore_ascii_case("content-type"))
            {
                headers.push(("Content-Type".into(), content.into()));
            }
            Some(request.body.clone().into_bytes())
        }
        BodyKind::Form => {
            if !headers
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case("content-type"))
            {
                headers.push((
                    "Content-Type".into(),
                    "application/x-www-form-urlencoded".into(),
                ));
            }
            Some(
                request
                    .form
                    .iter()
                    .filter(|field| field.enabled && !field.name.is_empty())
                    .map(|field| {
                        format!(
                            "{}={}",
                            percent_encode(&field.name),
                            percent_encode(&field.value)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("&")
                    .into_bytes(),
            )
        }
        BodyKind::Multipart => {
            let (content, bytes) = multipart_body(&request.form)?;
            headers.push(("Content-Type".into(), content));
            Some(bytes)
        }
    };
    Ok(Outgoing {
        method: request.method.clone(),
        url,
        headers,
        body,
    })
}

fn multipart_body(fields: &[Field]) -> Result<(String, Vec<u8>), String> {
    let boundary = "nozhikHttpBoundary";
    let mut body = Vec::new();
    for field in fields
        .iter()
        .filter(|field| field.enabled && !field.name.is_empty())
    {
        if field.name.contains(['"', '\r', '\n']) {
            return Err("Multipart field names cannot contain quotes or line breaks".into());
        }
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!(
                "Content-Disposition: form-data; name=\"{}\"\r\n\r\n",
                field.name
            )
            .as_bytes(),
        );
        body.extend_from_slice(field.value.as_bytes());
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    Ok((format!("multipart/form-data; boundary={boundary}"), body))
}

fn url_with_query(url: &str, query: &[Field]) -> String {
    let rows: Vec<_> = query
        .iter()
        .filter(|field| field.enabled && !field.name.is_empty())
        .collect();
    if rows.is_empty() {
        return url.to_owned();
    }
    let (base, fragment) = match url.split_once('#') {
        Some((base, fragment)) => (base, Some(fragment)),
        None => (url, None),
    };
    let join = if base.contains('?') { '&' } else { '?' };
    let mut out = base.to_owned();
    out.push(join);
    for (index, field) in rows.iter().enumerate() {
        if index > 0 {
            out.push('&');
        }
        out.push_str(&percent_encode(&field.name));
        out.push('=');
        out.push_str(&percent_encode(&field.value));
    }
    if let Some(fragment) = fragment {
        out.push('#');
        out.push_str(fragment);
    }
    out
}

fn edit_fields(ui: &mut Ui, request_id: &str, kind: &str, fields: &mut Vec<Field>) -> bool {
    let mut changed = false;
    let mut remove = None;
    for (index, field) in fields.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            if ui.checkbox(&mut field.enabled, "").changed() {
                changed = true;
            }
            let name = TextEdit::singleline(&mut field.name)
                .id(field_id(request_id, kind, index, "name"))
                .hint_text("Name")
                .desired_width(160.0);
            if ui.add(name).changed() {
                changed = true;
            }
            let value = TextEdit::singleline(&mut field.value)
                .id(field_id(request_id, kind, index, "value"))
                .hint_text("Value")
                .desired_width((ui.available_width() - 36.0).max(80.0));
            if ui.add(value).changed() {
                changed = true;
            }
            if ui.button("×").clicked() {
                remove = Some(index);
            }
        });
    }
    if let Some(index) = remove {
        fields.remove(index);
        changed = true;
    }
    if ui.button("Add").clicked() && fields.len() < model::MAX_FIELDS {
        fields.push(Field::new("", ""));
        changed = true;
    }
    changed
}

fn export_control(
    ui: &mut Ui,
    enabled: bool,
    id: &str,
    targets: &[ExportTarget],
) -> Option<PanelCommand> {
    if !enabled || targets.is_empty() {
        ui.add_enabled(false, egui::Button::new("To OpenAPI"))
            .on_hover_text("Open a project to export into openapi.json");
        return None;
    }
    if targets.len() == 1 {
        let target = &targets[0];
        return ui
            .button("To OpenAPI")
            .on_hover_text(&target.label)
            .clicked()
            .then(|| PanelCommand::Export {
                id: id.to_owned(),
                path: target.path.clone(),
            });
    }
    let mut command = None;
    ui.menu_button("To OpenAPI", |ui| {
        for target in targets {
            if ui.button(&target.label).clicked() {
                command = Some(PanelCommand::Export {
                    id: id.to_owned(),
                    path: target.path.clone(),
                });
                ui.close();
            }
        }
    })
    .response
    .on_hover_text("Choose an OpenAPI JSON document");
    command
}

fn request_card(ui: &mut Ui, request: &Request, selected: bool) -> egui::Response {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 48.0), Sense::click());
    let fill = if selected {
        ORANGE.linear_multiply(0.35)
    } else if response.hovered() {
        NAV_HOVER
    } else {
        Color32::TRANSPARENT
    };
    if fill != Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, 4.0, fill);
    }
    let color = method_color(&request.method);
    ui.painter().text(
        Pos2::new(rect.left() + 8.0, rect.top() + 8.0),
        egui::Align2::LEFT_TOP,
        &request.method,
        egui::FontId::proportional(12.0),
        color,
    );
    ui.painter().text(
        Pos2::new(rect.left() + 72.0, rect.top() + 6.0),
        egui::Align2::LEFT_TOP,
        &request.name,
        egui::FontId::proportional(14.0),
        theme::TEXT,
    );
    ui.painter().text(
        Pos2::new(rect.left() + 8.0, rect.top() + 26.0),
        egui::Align2::LEFT_TOP,
        if request.url.is_empty() {
            "No URL"
        } else {
            request.url.as_str()
        },
        egui::FontId::monospace(12.0),
        theme::MUTED,
    );
    response
}

fn method_color(method: &str) -> Color32 {
    match method {
        "GET" => Color32::from_rgb(97, 175, 254),
        "POST" => Color32::from_rgb(73, 204, 144),
        "PUT" => Color32::from_rgb(252, 161, 48),
        "PATCH" => Color32::from_rgb(80, 227, 194),
        "DELETE" => Color32::from_rgb(249, 62, 62),
        _ => Color32::from_rgb(160, 160, 170),
    }
}

fn nav_row(ui: &mut Ui, label: &str, count: Option<usize>, selected: bool) -> bool {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 28.0), Sense::click());
    if selected || response.hovered() {
        ui.painter()
            .rect_filled(rect, 4.0, if selected { NAV_SELECTED } else { NAV_HOVER });
    }
    let icon = tool_icons::Icon::Http;
    tool_icons::paint(
        ui.painter(),
        rect.left_center() + Vec2::new(16.0, 0.0),
        icon,
        ORANGE,
    );
    ui.painter().text(
        Pos2::new(rect.left() + 32.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(13.0),
        theme::TEXT,
    );
    if let Some(count) = count {
        ui.painter().text(
            Pos2::new(rect.right() - 8.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            count.to_string(),
            egui::FontId::proportional(12.0),
            theme::MUTED,
        );
    }
    response.clicked()
}

fn section_label(ui: &mut Ui, text: &str) {
    ui.add_space(6.0);
    ui.label(RichText::new(text).small().color(theme::MUTED).strong());
}

fn paint_centered(ui: &Ui, rect: Rect, message: &str) {
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        message,
        egui::FontId::proportional(14.0),
        theme::MUTED,
    );
}

fn tag_rows(requests: &[Request]) -> Vec<(String, usize)> {
    let mut rows: Vec<(String, usize)> = Vec::new();
    for request in requests {
        for tag in &request.tags {
            if let Some((_, count)) = rows
                .iter_mut()
                .find(|(name, _)| tag_key(name) == tag_key(tag))
            {
                *count += 1;
            } else {
                rows.push((tag.clone(), 1));
            }
        }
    }
    rows.sort_by(|left, right| {
        right
            .1
            .cmp(&left.1)
            .then_with(|| tag_key(&left.0).cmp(&tag_key(&right.0)))
    });
    rows
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
        })
}

fn search_id() -> Id {
    Id::new("http_requests_search")
}
fn name_id(id: &str) -> Id {
    Id::new(("http_name", id.to_owned()))
}
fn url_id(id: &str) -> Id {
    Id::new(("http_url", id.to_owned()))
}
fn body_id(id: &str) -> Id {
    Id::new(("http_body_text", id.to_owned()))
}
fn curl_id(id: &str) -> Id {
    Id::new(("http_curl", id.to_owned()))
}
fn tag_id(id: &str) -> Id {
    Id::new(("http_tag", id.to_owned()))
}
fn field_id(id: &str, kind: &str, index: usize, column: &str) -> Id {
    Id::new((
        "http_field",
        id.to_owned(),
        kind.to_owned(),
        index,
        column.to_owned(),
    ))
}

fn worker(
    path: &Path,
    input: &mpsc::Receiver<Task>,
    output: &mpsc::Sender<WorkerReply>,
    wake: &(dyn Fn() + Send + Sync),
) {
    let mut writable = true;
    match read_store(path) {
        Ok(file) => {
            let _sent = output.send(WorkerReply::Loaded(file));
        }
        Err(StoreError::Missing) => match write_store(path, &Store::default()) {
            Ok(file) => {
                let _sent = output.send(WorkerReply::Loaded(file));
            }
            Err(error) => {
                writable = false;
                let _sent = output.send(WorkerReply::Io(error.to_string()));
            }
        },
        Err(StoreError::Invalid(message)) => {
            writable = false;
            let _sent = output.send(WorkerReply::Corrupt(message));
        }
    }
    wake();
    while let Ok(task) = input.recv() {
        match task {
            Task::Save(file) => {
                let reply = if writable {
                    match write_store(path, &file) {
                        Ok(_) => WorkerReply::Saved,
                        Err(StoreError::Missing) => WorkerReply::Io(
                            "http_requests.json disappeared before it could be saved".into(),
                        ),
                        Err(StoreError::Invalid(message)) => {
                            writable = false;
                            WorkerReply::Corrupt(message)
                        }
                    }
                } else {
                    WorkerReply::Corrupt("Malformed http_requests.json preserved".into())
                };
                let _sent = output.send(reply);
                wake();
            }
            Task::Reload => {
                let reply = match read_store(path) {
                    Ok(file) => {
                        writable = true;
                        WorkerReply::Reloaded(file)
                    }
                    Err(StoreError::Missing) => {
                        writable = true;
                        WorkerReply::Reloaded(Store::default())
                    }
                    Err(StoreError::Invalid(message)) => {
                        writable = false;
                        WorkerReply::Corrupt(message)
                    }
                };
                let _sent = output.send(reply);
                wake();
            }
            Task::Finish { file, done } => {
                if writable
                    && let Some(file) = file
                    && let Err(error) = write_store(path, &file)
                {
                    eprintln!("HTTP Requests save: {error}");
                }
                let _sent = done.send(());
                break;
            }
        }
    }
}

#[derive(Debug)]
enum StoreError {
    Missing,
    Invalid(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing => formatter.write_str("http_requests.json is missing"),
            Self::Invalid(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for StoreError {}

fn read_store(path: &Path) -> Result<Store, StoreError> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(StoreError::Missing);
        }
        Err(error) => return Err(StoreError::Invalid(error.to_string())),
    };
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Err(StoreError::Invalid("http_requests.json is empty".into()));
    }
    let file: Store = serde_json::from_slice(&bytes)
        .map_err(|error| StoreError::Invalid(format!("http_requests.json: {error}")))?;
    prepare(file).map_err(StoreError::Invalid)
}

fn write_store(path: &Path, file: &Store) -> Result<Store, StoreError> {
    let file = prepare(file.clone()).map_err(StoreError::Invalid)?;
    match read_store(path) {
        Ok(_) | Err(StoreError::Missing) => {}
        Err(error) => return Err(error),
    }
    let parent = path
        .parent()
        .ok_or_else(|| StoreError::Invalid("http_requests.json path has no parent".into()))?;
    std::fs::create_dir_all(parent).map_err(|error| StoreError::Invalid(error.to_string()))?;
    let mut bytes =
        serde_json::to_vec_pretty(&file).map_err(|error| StoreError::Invalid(error.to_string()))?;
    bytes.push(b'\n');
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .map_err(|error| StoreError::Invalid(error.to_string()))?;
    {
        use std::io::Write;
        temporary
            .write_all(&bytes)
            .map_err(|error| StoreError::Invalid(error.to_string()))?;
        temporary
            .as_file()
            .sync_all()
            .map_err(|error| StoreError::Invalid(error.to_string()))?;
    }
    temporary
        .persist(path)
        .map_err(|error| StoreError::Invalid(error.to_string()))?;
    Ok(file)
}

fn prepare(mut file: Store) -> Result<Store, String> {
    if file.requests.len() > MAX_REQUESTS {
        return Err(format!(
            "http_requests.json has more than {MAX_REQUESTS} requests"
        ));
    }
    let mut ids = std::collections::HashSet::new();
    for request in &mut file.requests {
        if request.id.is_empty()
            || request.id.chars().count() > 80
            || request
                .id
                .chars()
                .any(|symbol| symbol.is_control() || symbol.is_whitespace())
        {
            return Err("http_requests.json has an invalid request id".into());
        }
        if !ids.insert(request.id.clone()) {
            return Err(format!(
                "http_requests.json repeats request id {}",
                request.id
            ));
        }
        normalize_request(request)?;
        request.tags = normalize_tags(&request.tags)?;
    }
    Ok(file)
}

impl crate::NozhikUi {
    /// `OpenAPI` JSON documents currently open, plus `<project>/openapi.json`.
    pub(crate) fn openapi_export_targets(&self) -> Vec<ExportTarget> {
        let mut targets = Vec::new();
        for document in &self.documents {
            if document.binary || !document.open {
                continue;
            }
            let known = self.openapi_document_ready(&document.path);
            if known {
                targets.push(ExportTarget {
                    label: document.name.clone(),
                    path: document.path.clone(),
                });
            }
        }
        if let Some(project) = &self.project {
            let path = project.root_path.join("openapi.json");
            if !targets.iter().any(|target| target.path == path) {
                targets.push(ExportTarget {
                    label: "openapi.json".into(),
                    path,
                });
            }
        }
        targets
    }

    fn openapi_document_ready(&self, path: &Path) -> bool {
        self.openapi.contains_path(path)
            || (crate::project_icons::is_openapi_filename(path)
                && path
                    .extension()
                    .and_then(|value| value.to_str())
                    .is_some_and(|value| value.eq_ignore_ascii_case("json")))
    }

    /// Writes one saved call into an open buffer, or queues a read/create of `path`.
    pub(crate) fn export_saved_request(&mut self, path: &Path, id: &str) {
        let Some(request) = self.http_requests.snapshot(id) else {
            return;
        };
        if let Some(index) = self
            .documents
            .iter()
            .position(|document| document.path == path)
        {
            self.write_openapi_document(index, &request);
            return;
        }
        self.pending_openapi_export = Some(PendingExport {
            path: path.to_path_buf(),
            request,
            phase: ExportPhase::Create,
        });
        if !self
            .fs
            .send(nozhik_fs_model::Command::Create(path.to_path_buf()))
        {
            self.pending_openapi_export = None;
            self.note_problem(
                "http-requests",
                "HTTP Requests",
                crate::problems::Severity::Error,
                "Filesystem worker stopped before openapi.json could be written",
            );
        }
    }

    /// Applies a queued export when the filesystem worker returns the file.
    pub(crate) fn finish_openapi_export(&mut self, path: &Path) {
        let matches = self
            .pending_openapi_export
            .as_ref()
            .is_some_and(|pending| same_export_path(&pending.path, path));
        if !matches {
            return;
        }
        let Some(pending) = self.pending_openapi_export.take() else {
            return;
        };
        let Some(index) = self
            .documents
            .iter()
            .position(|document| document.path == path)
        else {
            return;
        };
        if self.documents[index].binary {
            self.note_problem(
                "http-requests",
                "HTTP Requests",
                crate::problems::Severity::Error,
                "OpenAPI export needs a text JSON document",
            );
            return;
        }
        self.write_openapi_document(index, &pending.request);
    }

    /// Create failed because the file exists, so read it. A second failure is reported.
    ///
    /// Returns whether this error belonged to a queued export.
    pub(crate) fn openapi_export_failed(&mut self, error: &str) -> bool {
        let phase = self
            .pending_openapi_export
            .as_ref()
            .map(|pending| pending.phase);
        match phase {
            Some(ExportPhase::Create) => {
                if let Some(pending) = self.pending_openapi_export.as_mut() {
                    pending.phase = ExportPhase::Read;
                    let path = pending.path.clone();
                    if !self.fs.send(nozhik_fs_model::Command::Read(path)) {
                        self.pending_openapi_export = None;
                        self.note_problem(
                            "http-requests",
                            "HTTP Requests",
                            crate::problems::Severity::Error,
                            "Filesystem worker stopped before openapi.json could be read",
                        );
                    }
                }
                true
            }
            Some(ExportPhase::Read) => {
                self.pending_openapi_export = None;
                self.note_problem(
                    "http-requests",
                    "HTTP Requests",
                    crate::problems::Severity::Error,
                    error,
                );
                true
            }
            None => false,
        }
    }

    /// Moves prepared `OpenAPI` operations into the project library and opens the tool.
    pub(crate) fn import_openapi_requests(
        &mut self,
        ctx: &egui::Context,
        items: Vec<Result<Incoming, String>>,
    ) {
        if items.is_empty() {
            return;
        }
        let Some(root) = self
            .project
            .as_ref()
            .map(|project| project.root_path.clone())
        else {
            self.note_problem(
                "http-requests",
                "HTTP Requests",
                crate::problems::Severity::Error,
                "Open a project before moving an OpenAPI operation into HTTP Requests",
            );
            return;
        };
        let mut added = 0_usize;
        for item in items {
            match item {
                Ok(incoming) => {
                    self.http_requests.import(&root, ctx, incoming);
                    added += 1;
                }
                Err(error) => self.note_problem(
                    "http-requests",
                    "HTTP Requests",
                    crate::problems::Severity::Error,
                    &error,
                ),
            }
        }
        if added > 0 {
            self.left = Some(crate::LeftTool::HttpRequests);
            self.status = if added == 1 {
                "Added 1 request to HTTP Requests".into()
            } else {
                format!("Added {added} requests to HTTP Requests")
            };
        }
    }

    fn write_openapi_document(&mut self, index: usize, request: &Request) {
        let source = self.documents[index].buffer.rope().to_string();
        let path = self.documents[index].path.clone();
        match export::upsert(&source, request) {
            Ok(next) => {
                let end = self.documents[index].buffer.rope().len_bytes();
                if let Err(error) =
                    self.documents[index]
                        .buffer
                        .apply_transaction(nozhik_buffer::EditTransaction {
                            edits: vec![nozhik_buffer::TextEdit {
                                range: 0..end,
                                text: next,
                            }],
                            group: None,
                        })
                {
                    let message = error.to_string();
                    self.note_problem(
                        "http-requests",
                        "HTTP Requests",
                        crate::problems::Severity::Error,
                        &message,
                    );
                    return;
                }
                self.save_document(index);
                let (_, operation_path) = export::origin_and_path(&request.url);
                self.status = format!(
                    "Exported {} {operation_path} to {}",
                    request.method,
                    path.display()
                );
            }
            Err(error) => self.note_problem(
                "http-requests",
                "HTTP Requests",
                crate::problems::Severity::Error,
                &error,
            ),
        }
    }
}

fn same_export_path(pending: &Path, loaded: &Path) -> bool {
    if pending == loaded {
        return true;
    }
    let Some(name) = pending.file_name() else {
        return false;
    };
    let Some(parent) = pending.parent() else {
        return false;
    };
    loaded.file_name() == Some(name) && loaded.starts_with(parent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_roundtrip_and_corrupt_file_is_kept() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join(FILE_NAME);
        let mut request = Request::blank("h1".into(), 10);
        request.name = "List".into();
        request.url = "https://example.com/items".into();
        request.tags = vec!["shop".into()];
        let file = Store {
            requests: vec![request],
        };
        let written = write_store(&path, &file)?;
        let text = std::fs::read_to_string(&path)?;
        assert!(text.ends_with('\n'));
        assert!(text.contains("\"requests\""));
        let loaded = read_store(&path)?;
        assert_eq!(loaded, written);
        std::fs::write(&path, "{not json")?;
        assert!(write_store(&path, &Store::default()).is_err());
        assert_eq!(std::fs::read_to_string(&path)?, "{not json");
        Ok(())
    }

    #[test]
    fn import_adds_a_request_and_curl_follows_the_builder() -> Result<(), String> {
        let mut library = HttpRequests::default();
        library.loaded = true;
        library.writable = true;
        library.add_incoming(Incoming {
            name: "Login".into(),
            method: "post".into(),
            url: "https://example.com/login".into(),
            query: vec![("next".into(), "/home".into())],
            headers: vec![("Accept".into(), "application/json".into())],
            body_kind: BodyKind::Json,
            body: "{\"n\":1}".into(),
            form: Vec::new(),
            tags: vec!["auth".into()],
        });
        assert_eq!(library.file.requests.len(), 1);
        assert_eq!(library.file.requests[0].method, "POST");
        assert_eq!(library.file.requests[0].tags, vec!["auth".to_owned()]);
        library.refresh_curl();
        assert!(
            library
                .curl_text
                .contains("https://example.com/login?next=%2Fhome")
        );
        assert!(library.curl_text.contains("--data-raw '{\"n\":1}'"));
        let parsed = curl::parse(&library.curl_text)?;
        assert_eq!(parsed.method, "POST");
        assert_eq!(parsed.body_kind, BodyKind::Json);
        Ok(())
    }

    #[test]
    fn opening_the_tool_creates_the_file_and_a_new_request_shows_curl()
    -> Result<(), Box<dyn std::error::Error>> {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let dir = tempfile::tempdir()?;
        let path = dir.path().join(FILE_NAME);
        let mut library = HttpRequests::default();
        let render = |library: &mut HttpRequests, events: Vec<egui::Event>| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1200.0, 800.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::SidePanel::left("http_nav_test")
                        .exact_width(220.0)
                        .show(ctx, |ui| library.show_nav(ui, Some(dir.path()), true));
                    egui::CentralPanel::default()
                        .frame(editor_frame())
                        .show(ctx, |ui| {
                            let _command = library.show_main(ui, Some(dir.path()), true, &[]);
                        });
                },
            )
        };
        let shows = |output: &egui::FullOutput, label: &str| {
            output.shapes.iter().any(|shape| {
                matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text() == label)
            })
        };
        let center = |output: &egui::FullOutput, label: &str| {
            output.shapes.iter().find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == label => {
                    Some(text.pos + text.galley.size() * 0.5)
                }
                _ => None,
            })
        };
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut output = render(&mut library, Vec::new());
        while Instant::now() < deadline && !library.loaded {
            std::thread::sleep(Duration::from_millis(20));
            output = render(&mut library, Vec::new());
        }
        assert!(library.loaded, "HTTP Requests did not load");
        assert!(shows(&output, "All requests"));
        assert!(shows(&output, "Untagged"));
        let empty = std::fs::read_to_string(&path)?;
        assert!(empty.contains("\"requests\""));
        assert!(empty.ends_with('\n'));
        let plus = center(&output, "+").ok_or("missing new-request button")?;
        let click = |pos: egui::Pos2| {
            let button = |pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            vec![egui::Event::PointerMoved(pos), button(true), button(false)]
        };
        render(&mut library, click(plus));
        assert_eq!(library.file.requests.len(), 1);
        assert_eq!(library.file.requests[0].name, "Untitled request");
        assert_eq!(library.file.requests[0].method, "GET");
        assert!(library.curl_text.contains("curl -X GET"));
        library.finish();
        Ok(())
    }
}
