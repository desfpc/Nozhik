//! Project notes, commands and code fragments stored in `<project>/snippets.json`.
//!
//! The file is created when Snippets is opened. A missing file is an empty library.
//! A malformed file is kept as-is and editing stays disabled. Saves are atomic and
//! run on one worker thread, off the UI thread.

use crate::theme;
use crate::tool_icons::{self, Icon};
use chrono::{Datelike, Timelike};
use egui::{
    Align2, Color32, Context, CornerRadius, FontFamily, FontId, Frame, Id, Key, Margin, Pos2, Rect,
    RichText, ScrollArea, Sense, Shape, Stroke, TextEdit, Ui, Vec2,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const FILE_NAME: &str = "snippets.json";
const MAX_SNIPPETS: usize = 5_000;
const MAX_BODY_BYTES: usize = 256 * 1024;
const MAX_TITLE_CHARS: usize = 500;
const MAX_TAG_CHARS: usize = 40;
const MAX_TAGS: usize = 64;
const SAVE_DELAY: Duration = Duration::from_millis(250);

/// What the shell should do with the snippet the user just acted on.
pub enum SnippetCommand {
    /// Copy is already done by the caller. Paste this text into the project terminal.
    OpenInConsole(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct SnippetFile {
    #[serde(default)]
    snippets: Vec<Snippet>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Snippet {
    id: String,
    title: String,
    body: String,
    #[serde(default)]
    tags: Vec<String>,
    created_ms: i64,
    updated_ms: i64,
}

#[derive(Debug)]
enum StoreError {
    Missing,
    Invalid(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing => f.write_str("snippets.json is missing"),
            Self::Invalid(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for StoreError {}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Filter {
    All,
    Untagged,
    Tag,
}

enum NavPick {
    All,
    Untagged,
    Tag(String),
}

enum WorkerReply {
    Loaded(SnippetFile),
    Reloaded(SnippetFile),
    Saved,
    Corrupt(String),
    Io(String),
}

enum Task {
    Save(SnippetFile),
    Reload,
    Finish {
        file: Option<SnippetFile>,
        done: mpsc::Sender<()>,
    },
}

#[derive(Clone, Copy)]
enum Field {
    Title,
    Body,
    Search,
    Tag,
}

/// Snippet library for one project, plus the state of its tool window.
#[allow(
    clippy::struct_excessive_bools,
    reason = "Each flag is an independent session fact: load, write lock, edit focus and popup"
)]
pub struct Snippets {
    root: Option<PathBuf>,
    file: SnippetFile,
    writable: bool,
    loaded: bool,
    error: Option<String>,
    /// User-limit notice. A successful save must not clear it.
    notice: Option<String>,
    reported: Option<String>,
    dirty: bool,
    pending_save: Option<SnippetFile>,
    worker: Option<mpsc::Sender<Task>>,
    output: Option<mpsc::Receiver<WorkerReply>>,
    changed: Option<Instant>,
    filter: Filter,
    /// Canonical spelling of [`Filter::Tag`].
    tag_filter: String,
    query: String,
    selected: Option<String>,
    list_width: f32,
    assign_tags: bool,
    /// Screen position of the tag button, used to place the popup.
    tag_anchor: Option<Pos2>,
    tag_draft: String,
    ignore_outside_click: bool,
    focus_title: bool,
    cursor_line: usize,
    cursor_col: usize,
    next_id: u64,
}

impl Default for Snippets {
    fn default() -> Self {
        Self {
            root: None,
            file: SnippetFile::default(),
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
            list_width: 320.0,
            assign_tags: false,
            tag_anchor: None,
            tag_draft: String::new(),
            ignore_outside_click: false,
            focus_title: false,
            cursor_line: 1,
            cursor_col: 1,
            next_id: 0,
        }
    }
}

impl Drop for Snippets {
    fn drop(&mut self) {
        self.finish();
    }
}

impl Snippets {
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
            .name("nozhik-snippets".into())
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

    /// True when `path` is this project's snippet file.
    pub(crate) fn is_store_path(&self, path: &Path) -> bool {
        self.root
            .as_ref()
            .is_some_and(|root| path == root.join(FILE_NAME))
    }

    /// Asks the worker to reread the file. Our own saves are ignored when they match memory.
    pub(crate) fn note_disk_change(&self) {
        if let Some(worker) = &self.worker {
            let _sent = worker.send(Task::Reload);
        }
    }

    /// Drains worker replies and writes debounced edits.
    pub(crate) fn tick(&mut self, ctx: &Context) {
        self.poll();
        if !self.loaded && self.worker.is_some() {
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

    /// A new load, save, or limit failure, once per distinct message.
    ///
    /// The boolean-like severity is [`crate::problems::Severity`]: input/output
    /// failures are errors and tag or library limits are warnings.
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

    /// Tag and favorite column. `enabled` is false while a modal owns the pointer.
    pub(crate) fn show_nav(&mut self, ui: &mut Ui, root: Option<&Path>, enabled: bool) {
        if let Some(root) = root {
            self.ensure(root, ui.ctx());
            self.tick(ui.ctx());
        }
        let mut pick = None;
        ScrollArea::vertical()
            .id_salt("snippet_nav")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(8.0);
                ui.set_width((ui.available_width() - 8.0).max(0.0));
                section_label(ui, "Favorites");
                if nav_row(
                    ui,
                    Icon::Project,
                    "All Snippets",
                    None,
                    self.filter == Filter::All,
                    theme::primary(),
                ) {
                    pick = Some(NavPick::All);
                }
                if nav_row(
                    ui,
                    Icon::Project,
                    "Uncategorized",
                    None,
                    self.filter == Filter::Untagged,
                    theme::muted(),
                ) {
                    pick = Some(NavPick::Untagged);
                }
                section_label(ui, "Tags");
                if self.file.snippets.is_empty() && self.loaded {
                    ui.add_space(4.0);
                    ui.weak("No tags yet");
                }
                for (tag, count) in tag_rows(&self.file.snippets) {
                    let selected =
                        self.filter == Filter::Tag && tag_key(&self.tag_filter) == tag_key(&tag);
                    if nav_row(
                        ui,
                        Icon::Snippets,
                        &tag,
                        Some(count),
                        selected,
                        theme::primary(),
                    ) {
                        pick = Some(NavPick::Tag(tag));
                    }
                }
                ui.add_space(12.0);
            });
        if enabled && let Some(pick) = pick {
            match pick {
                NavPick::All => self.filter = Filter::All,
                NavPick::Untagged => self.filter = Filter::Untagged,
                NavPick::Tag(tag) => {
                    self.tag_filter = tag;
                    self.filter = Filter::Tag;
                }
            }
            self.assign_tags = false;
            self.sync_selection();
        }
    }

    /// List and editor. Returns a console request for the shell to paste.
    pub(crate) fn show_main(
        &mut self,
        ui: &mut Ui,
        root: Option<&Path>,
        enabled: bool,
    ) -> Option<SnippetCommand> {
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
            paint_centered(ui, full, "Open a project to store snippets.");
            return None;
        }
        if !self.writable {
            let message = self
                .error
                .clone()
                .unwrap_or_else(|| "snippets.json could not be read. Editing is disabled.".into());
            paint_centered(ui, full, &message);
            return None;
        }
        let list_width = list_column_width(self.list_width, full.width());
        let list_rect = Rect::from_min_size(full.min, Vec2::new(list_width, full.height()));
        let splitter_rect = Rect::from_min_size(
            list_rect.right_top(),
            Vec2::new(theme::SPLITTER, full.height()),
        );
        let editor_rect = Rect::from_min_max(splitter_rect.right_top(), full.max);
        ui.painter().rect_filled(list_rect, 0.0, theme::surface());
        ui.painter()
            .rect_filled(editor_rect, 0.0, theme::background());
        let mut list_ui =
            ui.new_child(egui::UiBuilder::new().max_rect(list_rect.shrink2(Vec2::new(8.0, 8.0))));
        self.show_list(&mut list_ui, enabled);
        let splitter = ui
            .interact(
                splitter_rect,
                ui.id().with("snippet_splitter"),
                Sense::drag(),
            )
            .on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
        if splitter.dragged() && enabled {
            self.list_width = (self.list_width + splitter.drag_delta().x).clamp(220.0, 480.0);
        }
        ui.painter().vline(
            splitter_rect.center().x,
            splitter_rect.y_range(),
            Stroke::new(1.0_f32, theme::border()),
        );
        let mut editor_ui = ui.new_child(egui::UiBuilder::new().max_rect(editor_rect));
        self.show_editor(&mut editor_ui, enabled)
    }

    /// Routes Edit menu clipboard actions while Snippets owns the workspace.
    pub(crate) fn handle_clipboard(&mut self, ctx: &Context, action: &str) -> bool {
        let Some(field) = self.focused_field(ctx) else {
            if action == "ui.copy"
                && let Some(text) = self.selected_body()
            {
                ctx.copy_text(text);
            }
            return true;
        };
        match action {
            "ui.copy" => {
                self.copy_field(ctx, field);
            }
            "ui.cut" => {
                self.cut_field(ctx, field);
            }
            "ui.paste" => ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste),
            _ => {}
        }
        true
    }

    pub(crate) fn editor_focused(&self, ctx: &Context) -> bool {
        self.focused_field(ctx).is_some()
    }

    fn reset(&mut self, root: &Path) {
        let list_width = self.list_width;
        let mut fresh = Self::default();
        fresh.root = Some(root.to_path_buf());
        fresh.list_width = list_width;
        *self = fresh;
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
                    self.writable = true;
                    self.error = None;
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
            self.error = Some("Snippets worker stopped".into());
            self.writable = false;
            return;
        }
        self.pending_save = Some(file);
    }

    const fn can_edit(&self) -> bool {
        self.loaded && self.writable
    }

    fn touch(&mut self) {
        self.dirty = true;
        self.changed = Some(Instant::now());
    }

    fn selected_index(&self) -> Option<usize> {
        let id = self.selected.as_ref()?;
        self.file
            .snippets
            .iter()
            .position(|snippet| &snippet.id == id)
    }

    fn selected_body(&self) -> Option<String> {
        self.selected_index()
            .map(|index| self.file.snippets[index].body.clone())
    }

    fn visible_indices(&self) -> Vec<usize> {
        let query = self.query.trim().to_lowercase();
        let mut indices: Vec<usize> = self
            .file
            .snippets
            .iter()
            .enumerate()
            .filter(|(_, snippet)| self.matches(snippet, &query))
            .map(|(index, _)| index)
            .collect();
        indices.sort_by(|&left, &right| {
            let a = &self.file.snippets[left];
            let b = &self.file.snippets[right];
            b.updated_ms
                .cmp(&a.updated_ms)
                .then_with(|| a.id.cmp(&b.id))
        });
        indices
    }

    fn matches(&self, snippet: &Snippet, query: &str) -> bool {
        let filter_ok = match self.filter {
            Filter::All => true,
            Filter::Untagged => snippet.tags.is_empty(),
            Filter::Tag => snippet
                .tags
                .iter()
                .any(|tag| tag_key(tag) == tag_key(&self.tag_filter)),
        };
        if !filter_ok {
            return false;
        }
        if query.is_empty() {
            return true;
        }
        snippet.title.to_lowercase().contains(query)
            || snippet.body.to_lowercase().contains(query)
            || snippet
                .tags
                .iter()
                .any(|tag| tag.to_lowercase().contains(query))
    }

    fn sync_selection(&mut self) {
        let visible = self.visible_indices();
        if let Some(id) = &self.selected
            && visible
                .iter()
                .any(|&index| &self.file.snippets[index].id == id)
        {
            return;
        }
        self.selected = visible
            .first()
            .map(|&index| self.file.snippets[index].id.clone());
        self.assign_tags = false;
    }

    fn create(&mut self) {
        if !self.can_edit() || self.file.snippets.len() >= MAX_SNIPPETS {
            if self.can_edit() && self.file.snippets.len() >= MAX_SNIPPETS {
                self.notice = Some(format!(
                    "A project can store at most {MAX_SNIPPETS} snippets"
                ));
            }
            return;
        }
        let now = now_ms();
        let id = self.allocate_id();
        self.file.snippets.push(Snippet {
            id: id.clone(),
            title: "untitled snippet".into(),
            body: String::new(),
            tags: Vec::new(),
            created_ms: now,
            updated_ms: now,
        });
        self.filter = Filter::All;
        self.query.clear();
        self.selected = Some(id);
        self.assign_tags = false;
        self.focus_title = true;
        self.cursor_line = 1;
        self.cursor_col = 1;
        self.notice = None;
        self.touch();
    }

    fn delete_selected(&mut self) {
        if !self.can_edit() {
            return;
        }
        let Some(id) = self.selected.clone() else {
            return;
        };
        self.file.snippets.retain(|snippet| snippet.id != id);
        self.assign_tags = false;
        self.selected = self
            .visible_indices()
            .first()
            .map(|&index| self.file.snippets[index].id.clone());
        self.touch();
    }

    fn try_add_tag(&mut self, raw: &str) -> bool {
        if !self.can_edit() {
            return false;
        }
        let Some(tag) = normalize_tag(raw) else {
            return false;
        };
        let Some(index) = self.selected_index() else {
            return false;
        };
        if self.file.snippets[index]
            .tags
            .iter()
            .any(|existing| tag_key(existing) == tag_key(&tag))
        {
            return true;
        }
        if self.file.snippets[index].tags.len() >= MAX_TAGS {
            self.notice = Some(format!("A snippet can have at most {MAX_TAGS} tags"));
            return false;
        }
        self.file.snippets[index].tags.push(tag);
        self.file.snippets[index].updated_ms = now_ms();
        self.notice = None;
        self.touch();
        true
    }

    fn remove_tag(&mut self, tag: &str) {
        if !self.can_edit() {
            return;
        }
        let Some(index) = self.selected_index() else {
            return;
        };
        let before = self.file.snippets[index].tags.len();
        let key = tag_key(tag);
        self.file.snippets[index]
            .tags
            .retain(|existing| tag_key(existing) != key);
        if self.file.snippets[index].tags.len() != before {
            self.file.snippets[index].updated_ms = now_ms();
            self.notice = None;
            self.touch();
        }
    }

    fn allocate_id(&mut self) -> String {
        for _ in 0..1_000 {
            self.next_id = self.next_id.saturating_add(1);
            let id = format!("s{:x}-{:x}", now_ms(), self.next_id);
            if !self.file.snippets.iter().any(|snippet| snippet.id == id) {
                return id;
            }
        }
        self.next_id = self.next_id.saturating_add(1);
        format!("s{:x}-{:x}-extra", now_ms(), self.next_id)
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
            let add = ui
                .add_enabled(
                    enabled && self.can_edit(),
                    egui::Button::new("+").min_size(Vec2::splat(28.0)),
                )
                .on_hover_text("New snippet");
            if add.clicked() {
                self.create();
            }
        });
        if self.query.ends_with('\n') {
            self.query.pop();
        }
        ui.add_space(8.0);
        if !self.loaded {
            ui.weak("Loading snippets…");
            return;
        }
        let indices = self.visible_indices();
        if indices.is_empty() {
            ui.add_space(24.0);
            ui.weak(if self.file.snippets.is_empty() {
                "No snippets yet"
            } else {
                "No matches"
            });
            return;
        }
        let mut chosen = None;
        let mut delete = None;
        ScrollArea::vertical()
            .id_salt("snippet_cards")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for index in indices {
                    let id = self.file.snippets[index].id.clone();
                    let selected = self.selected.as_deref() == Some(id.as_str());
                    let response = snippet_card(ui, &self.file.snippets[index], selected);
                    if response.clicked() && enabled {
                        chosen = Some(id.clone());
                    }
                    response.context_menu(|ui| {
                        if ui.button("Delete").clicked() {
                            delete = Some(id);
                            ui.close();
                        }
                    });
                    ui.add_space(4.0);
                }
            });
        if !enabled {
            return;
        }
        if let Some(id) = delete {
            self.selected = Some(id);
            self.delete_selected();
        } else if let Some(id) = chosen {
            if self.selected.as_deref() != Some(id.as_str()) {
                self.cursor_line = 1;
                self.cursor_col = 1;
                self.assign_tags = false;
            }
            self.selected = Some(id);
        } else if ui.input(|input| input.key_pressed(Key::Delete))
            && !self.editor_focused(ui.ctx())
            && !ui.memory(|memory| memory.has_focus(search_id()))
            && !self.assign_tags
        {
            self.delete_selected();
        }
    }

    fn show_editor(&mut self, ui: &mut Ui, enabled: bool) -> Option<SnippetCommand> {
        let Some(index) = self.selected_index() else {
            paint_centered(
                ui,
                ui.available_rect_before_wrap(),
                if self.loaded {
                    "Select a snippet or create one."
                } else {
                    "Loading snippets…"
                },
            );
            return None;
        };
        let command = self.show_header(ui, index, enabled);
        self.show_body(ui, index, enabled);
        self.show_footer(ui);
        if self.assign_tags {
            self.show_tags(ui.ctx());
        }
        command
    }

    fn show_header(&mut self, ui: &mut Ui, index: usize, enabled: bool) -> Option<SnippetCommand> {
        let header = ui.available_rect_before_wrap();
        let header = Rect::from_min_size(header.min, Vec2::new(header.width(), 48.0));
        ui.painter().rect_filled(header, 0.0, theme::header());
        ui.painter().hline(
            header.x_range(),
            header.bottom(),
            Stroke::new(1.0_f32, theme::border()),
        );
        let mut header_ui =
            ui.new_child(egui::UiBuilder::new().max_rect(header.shrink2(Vec2::new(12.0, 8.0))));
        let focus_title = self.focus_title;
        let editable = enabled && self.can_edit();
        let tags_open = self.assign_tags;
        let id = self.file.snippets[index].id.clone();
        let mut title = self.file.snippets[index].title.clone();
        let mut title_changed = false;
        let mut copy_body = false;
        let mut toggle_tags = false;
        let mut open_console = false;
        let mut anchor = None;
        header_ui.horizontal(|ui| {
            let button_space = 100.0;
            let width = (ui.available_width() - button_space).max(40.0);
            let output = TextEdit::singleline(&mut title)
                .id(title_id(&id))
                .frame(false)
                .font(FontId::proportional(16.0))
                .hint_text("untitled snippet")
                .desired_width(width)
                .interactive(editable)
                .show(ui);
            if focus_title {
                output.response.request_focus();
            }
            if output.response.changed() && editable {
                title_changed = true;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let console = tool_icons::button(ui, Icon::Terminal, false, "Open in console");
                let tags = tool_icons::button(ui, Icon::Snippets, tags_open, "Edit tags");
                let copy = tool_icons::button(ui, Icon::Copy, false, "Copy this snippet");
                if enabled && copy.clicked() {
                    copy_body = true;
                }
                if enabled && tags.clicked() && editable {
                    toggle_tags = true;
                    anchor = Some(tags.rect.left_bottom());
                }
                if enabled && console.clicked() {
                    open_console = true;
                }
            });
        });
        if focus_title {
            self.focus_title = false;
        }
        if title_changed {
            limit_chars(&mut title, MAX_TITLE_CHARS);
            self.file.snippets[index].title = title;
            self.file.snippets[index].updated_ms = now_ms();
            self.touch();
        }
        if copy_body {
            let body = self.file.snippets[index].body.clone();
            ui.ctx().copy_text(body);
        }
        if toggle_tags {
            self.assign_tags = !self.assign_tags;
            self.ignore_outside_click = true;
            if self.assign_tags {
                self.tag_draft.clear();
            }
            self.tag_anchor = anchor;
        }
        let command = open_console
            .then(|| SnippetCommand::OpenInConsole(self.file.snippets[index].body.clone()));
        let _advance = ui.allocate_rect(header, Sense::hover());
        command
    }

    fn show_body(&mut self, ui: &mut Ui, index: usize, enabled: bool) {
        let full = ui.available_rect_before_wrap();
        let body_rect = Rect::from_min_max(full.min, Pos2::new(full.right(), full.bottom() - 28.0));
        if body_rect.height() < 8.0 {
            return;
        }
        let _allocated = ui.allocate_rect(body_rect, Sense::hover());
        let mut body_ui = ui.new_child(egui::UiBuilder::new().max_rect(body_rect));
        let font = body_font(&body_ui);
        let editable = enabled && self.can_edit();
        let snippet_id = self.file.snippets[index].id.clone();
        let mut cursor = None;
        let mut changed = false;
        let lines = line_count(&self.file.snippets[index].body);
        ScrollArea::both()
            .id_salt(("snippet_body", snippet_id.clone()))
            .auto_shrink([false, false])
            .show(&mut body_ui, |ui| {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    let gutter = ui
                        .allocate_exact_size(
                            Vec2::new(gutter_width(lines), 1.0_f32),
                            Sense::hover(),
                        )
                        .0;
                    let output = TextEdit::multiline(&mut self.file.snippets[index].body)
                        .id(body_id(&snippet_id))
                        .font(font.clone())
                        .lock_focus(true)
                        .frame(false)
                        .margin(Margin::symmetric(8, 2))
                        .desired_width(f32::INFINITY)
                        .desired_rows(lines)
                        .interactive(editable)
                        .show(ui);
                    paint_line_numbers(ui, &output, gutter.right() - 8.0, &font);
                    if let Some(range) = output.cursor_range {
                        cursor = Some(line_col(
                            &self.file.snippets[index].body,
                            range.primary.index,
                        ));
                    }
                    if output.response.changed() {
                        changed = true;
                    }
                });
            });
        if let Some((line, column)) = cursor {
            self.cursor_line = line;
            self.cursor_col = column;
        }
        if changed && editable {
            limit_bytes(&mut self.file.snippets[index].body, MAX_BODY_BYTES);
            self.file.snippets[index].updated_ms = now_ms();
            self.touch();
        }
    }

    fn show_footer(&self, ui: &mut Ui) {
        let rect = ui.available_rect_before_wrap();
        let bar = Rect::from_min_max(Pos2::new(rect.left(), rect.bottom() - 28.0), rect.max);
        ui.painter().rect_filled(bar, 0.0, theme::header());
        ui.painter().hline(
            bar.x_range(),
            bar.top(),
            Stroke::new(1.0_f32, theme::border()),
        );
        let mut bar_ui =
            ui.new_child(egui::UiBuilder::new().max_rect(bar.shrink2(Vec2::new(12.0, 4.0))));
        bar_ui.horizontal(|ui| {
            ui.label(RichText::new("Plain Text").color(theme::muted()).size(12.0));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(format!(
                        "Line: {} Col: {}",
                        self.cursor_line, self.cursor_col
                    ))
                    .color(theme::muted())
                    .size(12.0),
                );
            });
        });
        let _advance = ui.allocate_rect(bar, Sense::hover());
    }

    #[allow(
        clippy::too_many_lines,
        reason = "The tag popup keeps the current chips, the draft and suggestions together"
    )]
    fn show_tags(&mut self, ctx: &Context) {
        let Some(anchor) = self.tag_anchor else {
            self.assign_tags = false;
            return;
        };
        let Some(index) = self.selected_index() else {
            self.assign_tags = false;
            return;
        };
        let current = self.file.snippets[index].tags.clone();
        let suggestions: Vec<String> = tag_rows(&self.file.snippets)
            .into_iter()
            .map(|(tag, _)| tag)
            .filter(|tag| !current.iter().any(|have| tag_key(have) == tag_key(tag)))
            .collect();
        let screen = ctx.screen_rect();
        let x = (anchor.x - 250.0).clamp(8.0, (screen.right() - 292.0).max(8.0));
        let y = (anchor.y + 6.0).clamp(8.0, (screen.bottom() - 40.0).max(8.0));
        let mut remove = None;
        let mut add = None;
        let popup = egui::Area::new(Id::new("snippet_assign_tags"))
            .order(egui::Order::Foreground)
            .movable(false)
            .interactable(true)
            .fixed_pos(Pos2::new(x, y))
            .show(ctx, |ui| {
                Frame::popup(ui.style())
                    .fill(theme::surface())
                    .inner_margin(12)
                    .corner_radius(CornerRadius::same(10))
                    .show(ui, |ui| {
                        ui.set_min_width(260.0);
                        ui.set_max_width(280.0);
                        ui.vertical_centered(|ui| {
                            ui.label(RichText::new("Assign tags to snippet").strong());
                        });
                        ui.add_space(8.0);
                        if current.is_empty() {
                            ui.weak("No tags yet");
                        } else {
                            ui.horizontal_wrapped(|ui| {
                                for tag in &current {
                                    if ui.button(format!("{tag}  ×")).clicked() {
                                        remove = Some(tag.clone());
                                    }
                                }
                            });
                        }
                        ui.add_space(8.0);
                        let output = TextEdit::singleline(&mut self.tag_draft)
                            .id(tag_id())
                            .hint_text("Add a tag")
                            .desired_width(f32::INFINITY)
                            .show(ui);
                        let submit = output.response.has_focus()
                            && ui.input(|input| input.key_pressed(Key::Enter));
                        if submit || self.tag_draft.contains(',') {
                            add = Some(self.tag_draft.clone());
                        }
                        if !suggestions.is_empty() {
                            ui.add_space(8.0);
                            ui.label(
                                RichText::new("Suggestions")
                                    .color(theme::muted())
                                    .size(12.0),
                            );
                            ui.horizontal_wrapped(|ui| {
                                for tag in &suggestions {
                                    if ui.button(tag).clicked() {
                                        add = Some(tag.clone());
                                    }
                                }
                            });
                        }
                    });
            });
        if let Some(tag) = remove {
            self.remove_tag(&tag);
        }
        if let Some(raw) = add {
            let mut added = raw.trim().is_empty();
            for part in raw.split(',') {
                if self.try_add_tag(part) {
                    added = true;
                }
            }
            if added {
                self.tag_draft.clear();
            }
        }
        let inside = ctx.input(|input| {
            input
                .pointer
                .interact_pos()
                .is_some_and(|pos| popup.response.rect.contains(pos))
        });
        if self.ignore_outside_click {
            self.ignore_outside_click = false;
        } else if ctx.input(|input| input.pointer.any_click()) && !inside {
            self.assign_tags = false;
            self.tag_draft.clear();
        }
        if ctx.input(|input| input.key_pressed(Key::Escape)) {
            self.assign_tags = false;
            self.tag_draft.clear();
        }
    }

    fn focused_field(&self, ctx: &Context) -> Option<Field> {
        ctx.memory(|memory| {
            if memory.has_focus(search_id()) {
                return Some(Field::Search);
            }
            if memory.has_focus(tag_id()) {
                return Some(Field::Tag);
            }
            let id = self.selected.as_ref()?;
            if memory.has_focus(title_id(id)) {
                Some(Field::Title)
            } else if memory.has_focus(body_id(id)) {
                Some(Field::Body)
            } else {
                None
            }
        })
    }

    fn field_text(&self, field: Field) -> Option<String> {
        match field {
            Field::Search => Some(self.query.clone()),
            Field::Tag => Some(self.tag_draft.clone()),
            Field::Title => self
                .selected_index()
                .map(|index| self.file.snippets[index].title.clone()),
            Field::Body => self.selected_body(),
        }
    }

    fn field_id(&self, field: Field) -> Option<Id> {
        match field {
            Field::Search => Some(search_id()),
            Field::Tag => Some(tag_id()),
            Field::Title => self.selected.as_ref().map(|id| title_id(id)),
            Field::Body => self.selected.as_ref().map(|id| body_id(id)),
        }
    }

    fn copy_field(&self, ctx: &Context, field: Field) {
        let Some(text) = self.field_text(field) else {
            return;
        };
        let Some(id) = self.field_id(field) else {
            return;
        };
        if let Some(state) = TextEdit::load_state(ctx, id)
            && let Some(range) = state.cursor.char_range()
        {
            let selected = range.slice_str(&text);
            if !selected.is_empty() {
                ctx.copy_text(selected.to_owned());
                return;
            }
        }
        if matches!(field, Field::Body) {
            ctx.copy_text(text);
        }
    }

    fn cut_field(&mut self, ctx: &Context, field: Field) {
        if !self.can_edit() {
            return;
        }
        let Some(id) = self.field_id(field) else {
            return;
        };
        let Some(text) = self.field_text(field) else {
            return;
        };
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
        match field {
            Field::Title => limit_chars(&mut next, MAX_TITLE_CHARS),
            Field::Body => limit_bytes(&mut next, MAX_BODY_BYTES),
            Field::Search | Field::Tag => {}
        }
        self.set_field(field, next);
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::one(
                egui::text::CCursor::new(span.start),
            )));
        state.store(ctx, id);
        if matches!(field, Field::Title | Field::Body) {
            if let Some(index) = self.selected_index() {
                self.file.snippets[index].updated_ms = now_ms();
            }
            self.touch();
        }
    }

    fn set_field(&mut self, field: Field, text: String) {
        match field {
            Field::Search => self.query = text,
            Field::Tag => self.tag_draft = text,
            Field::Title => {
                if let Some(index) = self.selected_index() {
                    self.file.snippets[index].title = text;
                }
            }
            Field::Body => {
                if let Some(index) = self.selected_index() {
                    self.file.snippets[index].body = text;
                }
            }
        }
    }
}

/// Tag sidebar surface. The list and editor are drawn in the central panel.
pub fn sidebar_frame() -> Frame {
    Frame::new().fill(theme::panel()).inner_margin(0)
}

/// Editor surface behind the snippet text.
pub fn editor_frame() -> Frame {
    Frame::new().fill(theme::background()).inner_margin(0)
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
        Err(StoreError::Missing) => match write_store(path, &SnippetFile::default()) {
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
                            "snippets.json disappeared before it could be saved".into(),
                        ),
                        Err(StoreError::Invalid(message)) => {
                            writable = false;
                            WorkerReply::Corrupt(message)
                        }
                    }
                } else {
                    WorkerReply::Corrupt("Malformed snippets.json preserved".into())
                };
                let _sent = output.send(reply);
                wake();
            }
            Task::Reload => {
                // A deleted file becomes an empty library in memory and is not recreated
                // until the next explicit save.
                let reply = match read_store(path) {
                    Ok(file) => {
                        writable = true;
                        WorkerReply::Reloaded(file)
                    }
                    Err(StoreError::Missing) => {
                        writable = true;
                        WorkerReply::Reloaded(SnippetFile::default())
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
                    eprintln!("Snippets save: {error}");
                }
                let _sent = done.send(());
                break;
            }
        }
    }
}

fn read_store(path: &Path) -> Result<SnippetFile, StoreError> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(StoreError::Missing);
        }
        Err(error) => return Err(StoreError::Invalid(error.to_string())),
    };
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Err(StoreError::Invalid("snippets.json is empty".into()));
    }
    let file: SnippetFile = serde_json::from_slice(&bytes)
        .map_err(|error| StoreError::Invalid(format!("snippets.json: {error}")))?;
    prepare(file).map_err(StoreError::Invalid)
}

fn write_store(path: &Path, file: &SnippetFile) -> Result<SnippetFile, StoreError> {
    let file = prepare(file.clone()).map_err(StoreError::Invalid)?;
    match read_store(path) {
        Ok(_) | Err(StoreError::Missing) => {}
        Err(error) => return Err(error),
    }
    let parent = path
        .parent()
        .ok_or_else(|| StoreError::Invalid("snippets.json path has no parent".into()))?;
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

fn prepare(mut file: SnippetFile) -> Result<SnippetFile, String> {
    if file.snippets.len() > MAX_SNIPPETS {
        return Err(format!(
            "snippets.json has more than {MAX_SNIPPETS} snippets"
        ));
    }
    let mut ids = HashSet::new();
    for snippet in &mut file.snippets {
        if snippet.id.is_empty()
            || snippet.id.chars().count() > 80
            || snippet
                .id
                .chars()
                .any(|symbol| symbol.is_control() || symbol.is_whitespace())
        {
            return Err("snippets.json has an invalid snippet id".into());
        }
        if !ids.insert(snippet.id.clone()) {
            return Err(format!("snippets.json repeats snippet id {}", snippet.id));
        }
        if snippet.title.chars().count() > MAX_TITLE_CHARS
            || snippet.title.chars().any(char::is_control)
        {
            return Err("snippets.json has an invalid snippet title".into());
        }
        if snippet.body.len() > MAX_BODY_BYTES {
            return Err("a snippet body exceeds 256 KiB".into());
        }
        if snippet.created_ms < 0 || snippet.updated_ms < 0 {
            return Err("snippets.json has an invalid timestamp".into());
        }
        snippet.tags = normalize_tags(&snippet.tags)?;
    }
    Ok(file)
}

fn normalize_tags(tags: &[String]) -> Result<Vec<String>, String> {
    let mut unique = Vec::new();
    for tag in tags {
        let Some(tag) = normalize_tag(tag) else {
            return Err("snippets.json has an invalid tag".into());
        };
        if unique
            .iter()
            .any(|existing: &String| tag_key(existing) == tag_key(&tag))
        {
            continue;
        }
        if unique.len() >= MAX_TAGS {
            return Err(format!("a snippet has more than {MAX_TAGS} tags"));
        }
        unique.push(tag);
    }
    Ok(unique)
}

fn normalize_tag(raw: &str) -> Option<String> {
    let tag = raw.trim();
    if tag.is_empty()
        || tag.chars().count() > MAX_TAG_CHARS
        || tag
            .chars()
            .any(|symbol| symbol.is_control() || symbol == ',')
    {
        return None;
    }
    Some(tag.to_owned())
}

fn tag_key(tag: &str) -> String {
    tag.to_lowercase()
}

fn tag_rows(snippets: &[Snippet]) -> Vec<(String, usize)> {
    let mut rows: Vec<(String, usize)> = Vec::new();
    for snippet in snippets {
        for tag in &snippet.tags {
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
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

fn format_stamp(ms: i64, now: chrono::DateTime<chrono::Local>) -> String {
    let Some(utc) = chrono::DateTime::from_timestamp_millis(ms) else {
        return String::new();
    };
    let local = utc.with_timezone(&chrono::Local);
    let today = local.date_naive();
    if today == now.date_naive() {
        let hour = local.hour();
        let hour12 = match hour % 12 {
            0 => 12,
            value => value,
        };
        let ampm = if hour < 12 { "AM" } else { "PM" };
        format!("{hour12}:{:02} {ampm}", local.minute())
    } else {
        format!(
            "{:02}.{:02}.{:04}",
            today.day(),
            today.month(),
            today.year()
        )
    }
}

fn line_count(text: &str) -> usize {
    text.bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        .saturating_add(1)
}

fn line_col(text: &str, index: usize) -> (usize, usize) {
    let mut line = 1_usize;
    let mut column = 1_usize;
    for (offset, symbol) in text.chars().enumerate() {
        if offset == index {
            break;
        }
        if symbol == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    (line, column)
}

fn gutter_width(lines: usize) -> f32 {
    let mut width = 28.0_f32;
    let mut span = 9_usize;
    while lines > span && width < 80.0 {
        width += 8.0;
        span = span.saturating_mul(10).saturating_add(9);
    }
    width
}

fn limit_chars(text: &mut String, max_chars: usize) {
    if text.chars().count() <= max_chars {
        return;
    }
    let end = text
        .char_indices()
        .nth(max_chars)
        .map_or(text.len(), |(index, _)| index);
    text.truncate(end);
}

fn limit_bytes(text: &mut String, max_bytes: usize) {
    if text.len() <= max_bytes {
        return;
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
}

fn list_column_width(preferred: f32, available: f32) -> f32 {
    if available < 460.0 {
        (available * 0.42).clamp(140.0, available * 0.7)
    } else {
        preferred.clamp(220.0, (available - 280.0).min(480.0))
    }
}

fn search_id() -> Id {
    Id::new("snippet_search")
}

fn tag_id() -> Id {
    Id::new("snippet_tag_draft")
}

fn title_id(id: &str) -> Id {
    Id::new(("snippet_title", id))
}

fn body_id(id: &str) -> Id {
    Id::new(("snippet_body", id))
}

fn body_font(ui: &Ui) -> FontId {
    let named = FontId::new(13.0, FontFamily::Name("JetBrains Mono".into()));
    if ui.fonts(|fonts| fonts.families().contains(&named.family)) {
        named
    } else {
        FontId::monospace(13.0)
    }
}

fn section_label(ui: &mut Ui, text: &str) {
    ui.add_space(10.0);
    ui.label(RichText::new(text).size(12.0).color(theme::muted()));
    ui.add_space(4.0);
}

fn nav_row(
    ui: &mut Ui,
    icon: Icon,
    label: &str,
    count: Option<usize>,
    selected: bool,
    icon_color: Color32,
) -> bool {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 28.0), Sense::click());
    if selected || response.hovered() {
        ui.painter().rect_filled(
            rect.shrink2(Vec2::new(2.0, 1.0)),
            CornerRadius::same(6),
            if selected {
                theme::accent()
            } else {
                theme::hover()
            },
        );
    }
    let center = Pos2::new(rect.left() + 16.0, rect.center().y);
    tool_icons::paint(ui.painter(), center, icon, icon_color);
    ui.painter().text(
        Pos2::new(rect.left() + 32.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(13.5),
        if selected {
            theme::on_selected()
        } else {
            theme::text()
        },
    );
    if let Some(count) = count {
        ui.painter().text(
            Pos2::new(rect.right() - 8.0, rect.center().y),
            Align2::RIGHT_CENTER,
            count.to_string(),
            FontId::proportional(12.0),
            theme::muted(),
        );
    }
    response.clicked()
}

fn snippet_card(ui: &mut Ui, snippet: &Snippet, selected: bool) -> egui::Response {
    let background = ui.painter().add(Shape::Noop);
    let inner = ui
        .vertical(|ui| {
            ui.set_width(ui.available_width());
            ui.add_space(6.0);
            let title = if snippet.title.trim().is_empty() {
                "untitled snippet"
            } else {
                snippet.title.as_str()
            };
            let title_color = if selected {
                theme::on_selected()
            } else {
                theme::text()
            };
            ui.label(RichText::new(title).strong().color(title_color).size(14.0));
            if !snippet.tags.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    for tag in &snippet.tags {
                        let fill = if selected {
                            theme::chip_on_selected()
                        } else {
                            theme::chip()
                        };
                        let color = if selected {
                            theme::chip_ink()
                        } else {
                            theme::text()
                        };
                        chip_label(ui, tag, fill, color);
                    }
                });
            }
            ui.horizontal(|ui| {
                ui.set_width(ui.available_width());
                let (icon_rect, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                paint_folder(
                    ui.painter(),
                    icon_rect.center(),
                    if selected {
                        theme::on_selected()
                    } else {
                        theme::muted()
                    },
                );
                ui.label(
                    RichText::new("Uncategorized")
                        .size(12.0)
                        .color(if selected {
                            theme::on_selected()
                        } else {
                            theme::muted()
                        }),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new(format_stamp(snippet.updated_ms, chrono::Local::now()))
                            .size(12.0)
                            .color(if selected {
                                theme::on_selected()
                            } else {
                                theme::muted()
                            }),
                    );
                });
            });
            ui.add_space(4.0);
        })
        .response;
    let fill = if selected {
        theme::accent()
    } else if inner.hovered() {
        theme::hover()
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().set(
        background,
        Shape::rect_filled(inner.rect.expand(4.0), CornerRadius::same(8), fill),
    );
    ui.interact(
        inner.rect.expand(4.0),
        Id::new(("snippet_card", &snippet.id)),
        Sense::click(),
    )
}

fn chip_label(ui: &mut Ui, text: &str, fill: Color32, color: Color32) {
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), FontId::proportional(12.0), color);
    let (rect, _) = ui.allocate_exact_size(galley.size() + Vec2::new(14.0, 6.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(8), fill);
    let pos = rect.center() - galley.size() * 0.5;
    ui.painter().galley(pos, galley, color);
}

fn paint_folder(painter: &egui::Painter, center: Pos2, color: Color32) {
    let origin = center - Vec2::splat(6.0);
    let point = |x, y| origin + Vec2::new(x, y);
    painter.add(Shape::line(
        vec![
            point(1.0, 4.0),
            point(5.0, 4.0),
            point(6.5, 5.5),
            point(11.0, 5.5),
            point(11.0, 11.0),
            point(1.0, 11.0),
            point(1.0, 4.0),
        ],
        Stroke::new(1.0_f32, color),
    ));
}

fn paint_line_numbers(
    ui: &Ui,
    output: &egui::text_edit::TextEditOutput,
    right: f32,
    font: &FontId,
) {
    let clip = ui.clip_rect();
    for (index, row) in output.galley.rows.iter().enumerate() {
        let y = output.galley_pos.y + row.pos.y;
        if y > clip.bottom() || y + row.rect().height() < clip.top() {
            continue;
        }
        ui.painter().text(
            Pos2::new(right, y),
            Align2::RIGHT_TOP,
            (index + 1).to_string(),
            font.clone(),
            theme::line_number(),
        );
    }
}

fn paint_centered(ui: &Ui, rect: Rect, message: &str) {
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        message,
        FontId::proportional(14.0),
        theme::muted(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn sample(id: &str, title: &str, body: &str, tags: &[&str], updated: i64) -> Snippet {
        Snippet {
            id: id.to_owned(),
            title: title.to_owned(),
            body: body.to_owned(),
            tags: tags.iter().map(|tag| (*tag).to_owned()).collect(),
            created_ms: updated,
            updated_ms: updated,
        }
    }

    fn editable(snippets: Vec<Snippet>) -> Snippets {
        let mut state = Snippets::default();
        state.loaded = true;
        state.writable = true;
        state.selected = snippets.first().map(|snippet| snippet.id.clone());
        state.file.snippets = snippets;
        state.sync_selection();
        state
    }

    fn local_at(
        year: i32,
        month: u32,
        day: u32,
        hour: u32,
        minute: u32,
    ) -> Result<chrono::DateTime<chrono::Local>, String> {
        let naive = chrono::NaiveDate::from_ymd_opt(year, month, day)
            .and_then(|date| date.and_hms_opt(hour, minute, 0))
            .ok_or_else(|| "invalid local time".to_owned())?;
        chrono::Local
            .from_local_datetime(&naive)
            .single()
            .ok_or_else(|| "ambiguous local time".to_owned())
    }

    #[test]
    fn store_roundtrip_preserves_order_and_trailing_newline()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join(FILE_NAME);
        let file = SnippetFile {
            snippets: vec![
                sample("s1", "first", "echo one", &["git"], 10),
                sample("s2", "second", "echo two", &[], 20),
            ],
        };
        let written = write_store(&path, &file)?;
        let text = std::fs::read_to_string(&path)?;
        assert!(text.ends_with('\n'));
        assert!(text.contains("\"snippets\""));
        let loaded = read_store(&path)?;
        assert_eq!(loaded, written);
        assert_eq!(loaded.snippets[0].id, "s1");
        assert_eq!(loaded.snippets[1].body, "echo two");
        Ok(())
    }

    #[test]
    fn corrupt_and_empty_files_are_not_overwritten() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join(FILE_NAME);
        let corrupt = b"{not json";
        std::fs::write(&path, corrupt)?;
        assert!(matches!(read_store(&path), Err(StoreError::Invalid(_))));
        assert!(write_store(&path, &SnippetFile::default()).is_err());
        assert_eq!(std::fs::read(&path)?, corrupt);

        std::fs::write(&path, b" \n\t")?;
        let empty = std::fs::read(&path)?;
        assert!(matches!(read_store(&path), Err(StoreError::Invalid(_))));
        assert!(write_store(&path, &SnippetFile::default()).is_err());
        assert_eq!(std::fs::read(&path)?, empty);
        Ok(())
    }

    #[test]
    fn duplicate_ids_invalid_fields_and_limits_are_rejected() {
        let duplicate = SnippetFile {
            snippets: vec![
                sample("same", "one", "", &[], 1),
                sample("same", "two", "", &[], 2),
            ],
        };
        assert!(prepare(duplicate).is_err());
        assert!(
            prepare(SnippetFile {
                snippets: vec![sample("", "title", "", &[], 1)],
            })
            .is_err()
        );
        let mut negative = sample("id", "title", "", &[], 1);
        negative.updated_ms = -1;
        assert!(
            prepare(SnippetFile {
                snippets: vec![negative],
            })
            .is_err()
        );
        assert!(
            prepare(SnippetFile {
                snippets: vec![sample("id", "bad\ntitle", "", &[], 1)],
            })
            .is_err()
        );
        assert!(
            prepare(SnippetFile {
                snippets: vec![sample("id", "title", "", &["bad,tag"], 1)],
            })
            .is_err()
        );
        let huge = "x".repeat(MAX_BODY_BYTES + 1);
        assert!(
            prepare(SnippetFile {
                snippets: vec![sample("id", "title", &huge, &[], 1)],
            })
            .is_err()
        );
        let snippets = (0..=MAX_SNIPPETS)
            .map(|index| sample(&format!("id{index}"), "title", "", &[], 1))
            .collect();
        assert!(prepare(SnippetFile { snippets }).is_err());
    }

    #[test]
    fn case_duplicate_tags_are_collapsed_in_memory_without_rewriting()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join(FILE_NAME);
        let raw = b"{\n  \"snippets\": [{\n    \"id\": \"a\",\n    \"title\": \"t\",\n    \"body\": \"\",\n    \"tags\": [\"Git\", \"git\"],\n    \"created_ms\": 1,\n    \"updated_ms\": 1\n  }]\n}\n";
        std::fs::write(&path, raw)?;
        let loaded = read_store(&path)?;
        assert_eq!(loaded.snippets[0].tags, vec!["Git".to_owned()]);
        assert_eq!(std::fs::read(&path)?, raw);
        Ok(())
    }

    #[test]
    fn tags_can_be_added_and_removed_without_regard_to_case() {
        let mut state = editable(vec![sample("a", "note", "", &["Git"], 1)]);
        assert!(state.try_add_tag(" git "));
        assert_eq!(state.file.snippets[0].tags, vec!["Git".to_owned()]);
        assert!(state.try_add_tag("php"));
        assert_eq!(
            state.file.snippets[0].tags,
            vec!["Git".to_owned(), "php".to_owned()]
        );
        state.remove_tag("GIT");
        assert_eq!(state.file.snippets[0].tags, vec!["php".to_owned()]);
        assert!(state.dirty);
    }

    #[test]
    fn tag_rows_sort_by_count_then_name_and_search_filters() {
        let rows = tag_rows(&[
            sample("a", "", "", &["rust", "git"], 3),
            sample("b", "", "", &["Git"], 2),
            sample("c", "", "", &["php"], 1),
        ]);
        assert_eq!(
            rows,
            vec![
                ("git".to_owned(), 2),
                ("php".to_owned(), 1),
                ("rust".to_owned(), 1),
            ]
        );
        let mut state = editable(vec![
            sample("newer", "psql", "select 1", &[], 20),
            sample("older", "deploy", "kubectl apply", &["infra"], 10),
        ]);
        assert_eq!(state.visible_indices(), vec![0, 1]);
        state.query = "PSQL".into();
        assert_eq!(state.visible_indices(), vec![0]);
        state.query = "infra".into();
        assert_eq!(state.visible_indices(), vec![1]);
        state.query.clear();
        state.filter = Filter::Untagged;
        assert_eq!(state.visible_indices(), vec![0]);
        state.filter = Filter::Tag;
        state.tag_filter = "INFRA".into();
        assert_eq!(state.visible_indices(), vec![1]);
    }

    #[test]
    fn dates_use_clock_time_today_and_day_month_year_otherwise() -> Result<(), String> {
        let afternoon = local_at(2026, 11, 5, 14, 24)?;
        assert_eq!(
            format_stamp(afternoon.timestamp_millis(), afternoon),
            "2:24 PM"
        );
        let midnight = local_at(2026, 11, 5, 0, 5)?;
        assert_eq!(
            format_stamp(midnight.timestamp_millis(), midnight),
            "12:05 AM"
        );
        let noon = local_at(2026, 11, 5, 12, 0)?;
        assert_eq!(format_stamp(noon.timestamp_millis(), noon), "12:00 PM");
        let yesterday = local_at(2026, 11, 4, 9, 0)?;
        assert_eq!(
            format_stamp(yesterday.timestamp_millis(), afternoon),
            "04.11.2026"
        );
        Ok(())
    }

    #[test]
    fn copy_without_a_focused_field_copies_the_snippet_body() {
        let ctx = egui::Context::default();
        let mut state = editable(vec![sample("a", "note", "echo hi", &[], 1)]);
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            assert!(state.handle_clipboard(ctx, "ui.copy"));
            assert!(state.handle_clipboard(ctx, "ui.paste"));
        });
        assert!(output.platform_output.commands.iter().any(|command| {
            matches!(command, egui::OutputCommand::CopyText(text) if text == "echo hi")
        }));
    }

    #[test]
    fn opening_the_tool_creates_reads_and_reloads_without_rewriting_a_deletion()
    -> Result<(), Box<dyn std::error::Error>> {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let dir = tempfile::tempdir()?;
        let path = dir.path().join(FILE_NAME);
        let mut snippets = Snippets::default();
        let render = |snippets: &mut Snippets, events: Vec<egui::Event>| {
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
                    egui::SidePanel::left("snippet_nav_test")
                        .exact_width(220.0)
                        .show(ctx, |ui| snippets.show_nav(ui, Some(dir.path()), true));
                    egui::CentralPanel::default()
                        .frame(editor_frame())
                        .show(ctx, |ui| {
                            let _command = snippets.show_main(ui, Some(dir.path()), true);
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
        let mut output = render(&mut snippets, Vec::new());
        while Instant::now() < deadline && !snippets.loaded {
            std::thread::sleep(Duration::from_millis(20));
            output = render(&mut snippets, Vec::new());
        }
        assert!(snippets.loaded, "snippets did not load");
        assert!(shows(&output, "All Snippets"));
        assert!(shows(&output, "Uncategorized"));
        assert!(shows(&output, "Tags"));
        let empty = std::fs::read_to_string(&path)?;
        assert!(empty.contains("\"snippets\""));
        assert!(empty.ends_with('\n'));
        let plus = center(&output, "+").ok_or("missing new-snippet button")?;
        let click = |pos: egui::Pos2| {
            let button = |pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            vec![egui::Event::PointerMoved(pos), button(true), button(false)]
        };
        render(&mut snippets, click(plus));
        assert_eq!(snippets.file.snippets.len(), 1);
        assert_eq!(snippets.file.snippets[0].title, "untitled snippet");
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            render(&mut snippets, Vec::new());
            if !snippets.dirty
                && let Ok(bytes) = std::fs::read(&path)
                && let Ok(file) = serde_json::from_slice::<SnippetFile>(&bytes)
                && file.snippets.len() == 1
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(40));
        }
        let saved = read_store(&path)?;
        assert_eq!(saved.snippets.len(), 1);
        assert_eq!(saved.snippets[0].title, "untitled snippet");
        std::fs::remove_file(&path)?;
        snippets.note_disk_change();
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline && !snippets.file.snippets.is_empty() {
            std::thread::sleep(Duration::from_millis(20));
            render(&mut snippets, Vec::new());
        }
        assert_eq!(snippets.file.snippets.len(), 0);
        assert!(!path.exists());
        snippets.finish();
        Ok(())
    }
}
