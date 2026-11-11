//! Redis connection forms and a structured, binary-safe key browser.
use crate::NozhikUi;
use nozhik_database::{
    CancellationToken,
    redis::{
        Atom, Data, KeyRow, Mutation, Profile, Snapshot,
        worker::{Action, Output, Request, Worker},
    },
};
use nozhik_remote_ssh::{SshSession, credentials::Secret};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use zeroize::Zeroize;

#[derive(Default)]
pub struct RedisUi {
    worker: Option<Worker>,
    profiles: Vec<Profile>,
    connected: HashSet<String>,
    form: Option<Form>,
    parked: HashMap<String, BrowserState>,
    name: String,
    database: u32,
    query: String,
    scanned_query: String,
    cursor: u64,
    scan_status: ScanStatus,
    rows: Vec<KeyRow>,
    selected: Option<Snapshot>,
    baseline: String,
    editor: String,
    ttl: String,
    stream_fields: String,
    stream_id: String,
    new_key: String,
    creating: bool,
    delete_confirm: bool,
    pending: Option<Pending>,
    ssh: Option<(u64, Profile, Secret, bool)>,
    next_id: u64,
    error: String,
    status: String,
}
#[derive(Default)]
struct BrowserState {
    name: String,
    database: u32,
    query: String,
    scanned_query: String,
    cursor: u64,
    scan_status: ScanStatus,
    rows: Vec<KeyRow>,
    selected: Option<Snapshot>,
    baseline: String,
    editor: String,
    ttl: String,
    stream_fields: String,
    stream_id: String,
    new_key: String,
    creating: bool,
    delete_confirm: bool,
    pending: Option<Pending>,
    error: String,
    status: String,
}
impl BrowserState {
    fn dirty(&self) -> bool {
        self.editor != self.baseline
            || self.creating
            || self
                .pending
                .as_ref()
                .is_some_and(|p| matches!(p.kind, PendingKind::Write(_)))
    }
    fn writing(&self) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|p| matches!(p.kind, PendingKind::Write(_)))
    }
}
#[derive(Default, PartialEq, Eq)]
enum ScanStatus {
    #[default]
    NotStarted,
    More,
    Complete,
}
struct Form {
    original: Option<Profile>,
    profile: Profile,
    password: String,
    tested: bool,
}
impl Drop for Form {
    fn drop(&mut self) {
        self.password.zeroize();
    }
}
struct Pending {
    id: u64,
    cancel: CancellationToken,
    kind: PendingKind,
}
enum PendingKind {
    Load,
    Save,
    Connect(Profile, bool),
    Scan(bool),
    Read,
    Write(bool),
}
impl RedisUi {
    fn take_browser(&mut self) -> BrowserState {
        BrowserState {
            name: std::mem::take(&mut self.name),
            database: std::mem::take(&mut self.database),
            query: std::mem::take(&mut self.query),
            scanned_query: std::mem::take(&mut self.scanned_query),
            cursor: std::mem::take(&mut self.cursor),
            scan_status: std::mem::take(&mut self.scan_status),
            rows: std::mem::take(&mut self.rows),
            selected: std::mem::take(&mut self.selected),
            baseline: std::mem::take(&mut self.baseline),
            editor: std::mem::take(&mut self.editor),
            ttl: std::mem::take(&mut self.ttl),
            stream_fields: std::mem::take(&mut self.stream_fields),
            stream_id: std::mem::take(&mut self.stream_id),
            new_key: std::mem::take(&mut self.new_key),
            creating: std::mem::take(&mut self.creating),
            delete_confirm: std::mem::take(&mut self.delete_confirm),
            pending: std::mem::take(&mut self.pending),
            error: std::mem::take(&mut self.error),
            status: std::mem::take(&mut self.status),
        }
    }
    fn restore_browser(&mut self, browser: BrowserState) {
        self.name = browser.name;
        self.database = browser.database;
        self.query = browser.query;
        self.scanned_query = browser.scanned_query;
        self.cursor = browser.cursor;
        self.scan_status = browser.scan_status;
        self.rows = browser.rows;
        self.selected = browser.selected;
        self.baseline = browser.baseline;
        self.editor = browser.editor;
        self.ttl = browser.ttl;
        self.stream_fields = browser.stream_fields;
        self.stream_id = browser.stream_id;
        self.new_key = browser.new_key;
        self.creating = browser.creating;
        self.delete_confirm = browser.delete_confirm;
        self.pending = browser.pending;
        self.error = browser.error;
        self.status = browser.status;
    }
    fn select_browser(&mut self, name: &str, database: u32) {
        if self.name == name {
            return;
        }
        let replacement = self.parked.remove(name).unwrap_or_else(|| BrowserState {
            name: name.into(),
            database,
            ..Default::default()
        });
        let old = self.take_browser();
        self.parked.insert(old.name.clone(), old);
        self.restore_browser(replacement);
    }
    pub(crate) fn any_dirty(&self) -> bool {
        self.dirty() || self.parked.values().any(BrowserState::dirty)
    }
    pub(crate) fn any_writing(&self) -> bool {
        self.writing() || self.parked.values().any(BrowserState::writing)
    }
    pub(crate) fn dirty_for(&self, name: &str) -> bool {
        if self.name == name {
            self.dirty()
        } else {
            self.parked.get(name).is_some_and(BrowserState::dirty)
        }
    }
    pub(crate) fn close(&mut self, name: &str) {
        let browser = if self.name == name {
            Some(self.take_browser())
        } else {
            self.parked.remove(name)
        };
        if let Some(browser) = browser
            && let Some(pending) = browser.pending
        {
            pending.cancel.cancel();
        }
    }
    fn apply_reply(&mut self, reply: nozhik_database::redis::worker::Reply) {
        let original = self.name.clone();
        let database = self.database;
        if self.pending.as_ref().is_none_or(|p| p.id != reply.id) {
            let Some(name) = self
                .parked
                .iter()
                .find(|(_, b)| b.pending.as_ref().is_some_and(|p| p.id == reply.id))
                .map(|(name, _)| name.clone())
            else {
                return;
            };
            self.select_browser(&name, 0);
        }
        self.apply_current_reply(reply);
        self.select_browser(&original, database);
    }
    #[allow(
        clippy::too_many_lines,
        reason = "Correlated replies update exactly one Redis browser without changing the active editor tab"
    )]
    fn apply_current_reply(&mut self, reply: nozhik_database::redis::worker::Reply) {
        let state = self;
        if state.pending.as_ref().is_none_or(|p| p.id != reply.id) {
            return;
        }
        let Some(pending) = state.pending.take() else {
            return;
        };
        if pending.cancel.is_cancelled()
            && !matches!(pending.kind, PendingKind::Write(_) | PendingKind::Save)
        {
            state.status = "Redis operation cancelled".into();
            return;
        }
        match reply.result {
            Err(error) => {
                state.error = error.to_string();
                state.status = "Redis operation failed; local edits retained".into();
            }
            Ok(Output::Profiles(profiles)) => {
                if matches!(pending.kind, PendingKind::Save) {
                    if let Some(form) = state.form.take() {
                        state.connected.remove(&form.profile.name);
                        if let Some(old) = &form.original {
                            state.connected.remove(&old.name);
                        }
                    }
                    state.status = "Redis connection saved; reconnect to apply settings".into();
                }
                if matches!(pending.kind, PendingKind::Save) {
                    state.reset();
                }
                state.profiles = profiles;
            }
            Ok(Output::Connected { name, test }) => {
                if let PendingKind::Connect(profile, expected_test) = pending.kind {
                    if test && expected_test {
                        if let Some(form) = &mut state.form {
                            form.tested = form.profile == profile;
                        }
                        state.status = "Redis connection test succeeded".into();
                    } else {
                        state.connected.insert(name.clone());
                        state.name = name;
                        state.database = profile.database;
                        state.reset();
                        state.form = None;
                        state.scan(true);
                    }
                }
            }
            Ok(Output::Disconnected(name)) => {
                state.connected.remove(&name);
                state.reset();
                state.status = "Redis disconnected".into();
            }
            Ok(Output::Page(page)) => {
                let reset = matches!(pending.kind, PendingKind::Scan(true));
                if reset {
                    state.rows.clear();
                }
                let mut known: HashSet<_> = state
                    .rows
                    .iter()
                    .filter_map(|r| r.key.bytes().ok())
                    .collect();
                for row in page.rows {
                    if let Ok(bytes) = row.key.bytes()
                        && known.insert(bytes)
                    {
                        state.rows.push(row);
                    }
                }
                state.cursor = page.cursor;
                state.scan_status = if page.cursor == 0 {
                    ScanStatus::Complete
                } else {
                    ScanStatus::More
                };
                state.status = format!(
                    "{} keys loaded{}",
                    state.rows.len(),
                    if state.scan_status == ScanStatus::Complete {
                        "; scan complete"
                    } else {
                        "; more keys may exist"
                    }
                );
                // Empty cursor pages are valid. Continue until matches or completion, with cancellation available.
                if state.rows.is_empty() && state.scan_status == ScanStatus::More {
                    state.scan(false);
                }
                if state.rows.len() >= 10_000 {
                    state.rows.truncate(10_000);
                    state.scan_status = ScanStatus::Complete;
                    state.status =
                        "Search result limit: 10,000 keys; narrow the substring and search again"
                            .into();
                }
            }
            Ok(Output::Value(snapshot)) => {
                if let Some(row) = state.rows.iter_mut().find(|row| row.key == snapshot.key) {
                    row.ttl_ms = snapshot.ttl_ms;
                    row.preview = match &snapshot.data {
                        Data::String(value) => value.preview(),
                        data => serde_json::to_string(data)
                            .unwrap_or_default()
                            .chars()
                            .take(256)
                            .collect(),
                    };
                }
                match serde_json::to_string_pretty(&snapshot.data) {
                    Ok(text) => {
                        state.editor.clone_from(&text);
                        state.baseline = text;
                    }
                    Err(error) => state.error = error.to_string(),
                }
                state.ttl.clear();
                state.stream_fields = "[[\"field\", \"value\"]]".into();
                state.stream_id.clear();
                state.selected = Some(snapshot);
            }
            Ok(Output::Written) => {
                let delete = matches!(pending.kind, PendingKind::Write(true));
                state.status = "Redis write completed".into();
                state.creating = false;
                state.baseline.clone_from(&state.editor);
                if delete {
                    state.reset();
                    state.scan(true);
                } else {
                    let key = state
                        .selected
                        .as_ref()
                        .map_or_else(|| Atom::Text(state.new_key.clone()), |s| s.key.clone());
                    state.queue(
                        Action::Read {
                            name: state.name.clone(),
                            database: state.database,
                            key,
                        },
                        PendingKind::Read,
                    );
                }
            }
        }
    }

    pub(crate) fn writing(&self) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|p| matches!(p.kind, PendingKind::Write(_)))
    }
    pub(crate) fn dirty(&self) -> bool {
        self.editor != self.baseline
            || self.creating
            || self
                .pending
                .as_ref()
                .is_some_and(|p| matches!(p.kind, PendingKind::Write(_)))
    }
    pub(super) const fn modal_open(&self) -> bool {
        self.form.is_some()
    }
    pub(super) fn shutdown(&mut self) {
        if let Some(worker) = self.worker.take() {
            worker.shutdown();
        }
    }
    pub(super) fn cancel_ssh(&mut self, id: u64, error: &str) -> bool {
        if self.ssh.as_ref().is_none_or(|(request, ..)| *request != id) {
            return false;
        }
        let original = (self.name.clone(), self.database);
        if let Some((_, profile, _, _)) = &self.ssh {
            let target = (profile.name.clone(), profile.database);
            self.select_browser(&target.0, target.1);
        }
        self.ssh = None;
        self.pending = None;
        self.error = error.into();
        self.select_browser(&original.0, original.1);
        true
    }
    pub(super) fn accept_ssh(&mut self, id: u64, session: SshSession) -> bool {
        if self.ssh.as_ref().is_none_or(|(request, ..)| *request != id) {
            return false;
        }
        let original = (self.name.clone(), self.database);
        if let Some((_, profile, password, test)) = self.ssh.take() {
            self.select_browser(&profile.name, profile.database);
            let action = Action::Connect {
                profile,
                password: (!password.expose().is_empty()).then_some(password),
                ssh: Some(session),
                test,
            };
            if let Some(pending) = &self.pending {
                let request = Request {
                    id,
                    action,
                    cancel: pending.cancel.clone(),
                };
                if !self.worker.as_ref().is_some_and(|w| w.send(request)) {
                    self.error = "Redis queue full or stopped".into();
                    self.pending = None;
                }
            }
        }
        self.select_browser(&original.0, original.1);
        true
    }
    pub(super) fn incidents(&self) -> Vec<crate::problems::Incident> {
        std::iter::once((&self.name, &self.error))
            .chain(
                self.parked
                    .iter()
                    .map(|(name, browser)| (name, &browser.error)),
            )
            .filter(|(_, error)| !error.is_empty())
            .map(|(name, error)| {
                crate::problems::incident(
                    format!("redis:{name}"),
                    crate::problems::Severity::Error,
                    format!("Redis · {name}"),
                    error,
                )
            })
            .collect()
    }
    const fn new_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id | (1 << 63)
    }
    fn queue(&mut self, action: Action, kind: PendingKind) {
        if self.pending.is_some() {
            return;
        }
        let id = self.new_id();
        let cancel = CancellationToken::new();
        if self.worker.as_ref().is_some_and(|w| {
            w.send(Request {
                id,
                action,
                cancel: cancel.clone(),
            })
        }) {
            self.pending = Some(Pending { id, cancel, kind });
            self.error.clear();
        } else {
            self.error = "Redis queue full or stopped".into();
        }
    }
    fn reset(&mut self) {
        self.rows.clear();
        self.cursor = 0;
        self.scan_status = ScanStatus::NotStarted;
        self.selected = None;
        self.editor.clear();
        self.baseline.clear();
        self.creating = false;
        self.delete_confirm = false;
    }
    fn scan(&mut self, reset: bool) {
        if self.dirty() {
            self.error = "Submit or Revert the Redis edits first".into();
            return;
        }
        if reset {
            self.selected = None;
            self.scan_status = ScanStatus::NotStarted;
            self.scanned_query.clone_from(&self.query);
        }
        self.queue(
            Action::Scan {
                name: self.name.clone(),
                database: self.database,
                cursor: if reset { 0 } else { self.cursor },
                substring: self.query.clone(),
            },
            PendingKind::Scan(reset),
        );
    }
    fn write(&mut self, mutation: Mutation, delete: bool) {
        if let Some(snapshot) = self.selected.clone() {
            self.queue(
                Action::Mutate {
                    name: self.name.clone(),
                    database: self.database,
                    snapshot,
                    mutation,
                },
                PendingKind::Write(delete),
            );
        }
    }
}
impl NozhikUi {
    #[allow(
        clippy::too_many_lines,
        reason = "Correlated replies update profiles, scan pages and the key editor together"
    )]
    pub(super) fn redis_tick(&mut self, ctx: &egui::Context) {
        if self.database.redis.worker.is_none() {
            let context = ctx.clone();
            self.database.redis.worker = Some(Worker::start(
                self.settings
                    .path()
                    .with_file_name("redis_connections.json"),
                Arc::new(move || context.request_repaint()),
            ));
            self.database.redis.queue(Action::Load, PendingKind::Load);
        }
        let state = &mut self.database.redis;
        let replies: Vec<_> = state
            .worker
            .as_mut()
            .into_iter()
            .flat_map(|w| std::iter::from_fn(|| w.output.try_recv().ok()).take(32))
            .collect();
        for reply in replies {
            state.apply_reply(reply);
        }
    }
    pub(super) fn redis_connections(&mut self, ui: &mut egui::Ui) {
        let state = &mut self.database.redis;
        ui.horizontal(|ui| {
            ui.strong("Redis");
            if ui
                .add_enabled(
                    !state.any_dirty() && state.pending.is_none(),
                    egui::Button::new("+ Redis"),
                )
                .clicked()
            {
                state.form = Some(Form {
                    original: None,
                    profile: Profile::default(),
                    password: String::new(),
                    tested: false,
                });
            }
        });
        let profiles = state.profiles.clone();
        let mut open = None;
        for profile in profiles {
            ui.horizontal(|ui| {
                if ui
                    .selectable_label(
                        state.name == profile.name,
                        format!("{} · DB {}", profile.name, profile.database),
                    )
                    .clicked()
                {
                    open = Some((profile.name.clone(), profile.database));
                }
                if ui
                    .add_enabled(
                        !state.any_dirty() && state.pending.is_none(),
                        egui::Button::new("Edit"),
                    )
                    .clicked()
                {
                    state.form = Some(Form {
                        original: Some(profile.clone()),
                        profile,
                        password: String::new(),
                        tested: false,
                    });
                }
            });
        }
        ui.separator();
        if let Some((name, database)) = open {
            self.open_redis_browser(&name, database);
        }
    }
    pub(super) fn redis_dialogs(&mut self, ctx: &egui::Context) {
        self.redis_form(ctx);
    }
    #[allow(
        clippy::too_many_lines,
        reason = "One render pass collects browser controls and key selection without network I/O"
    )]
    pub(super) fn redis_editor_view(&mut self, ui: &mut egui::Ui, enabled: bool) {
        let Some(name) = self
            .database
            .active
            .and_then(|i| self.database.tabs.get(i))
            .filter(|t| t.redis)
            .map(|t| t.connection.clone())
        else {
            return;
        };
        self.database.redis.select_browser(&name, 0);
        self.editor_focus = None;
        self.integration.last_editor = None;
        let mut action = None;
        let mut connect = None;
        let state = &mut self.database.redis;
        ui.push_id(("redis_browser", &name), |ui| {
            ui.add_enabled_ui(enabled, |ui| {
                let connected = state.connected.contains(&state.name);
                let ready = state.pending.is_none();
                ui.horizontal(|ui| {
                    ui.label(if connected {
                        "Connected"
                    } else {
                        "Disconnected"
                    });
                    if ui
                        .add_enabled(
                            ready && !state.dirty(),
                            egui::Button::new(if connected {
                                "Disconnect"
                            } else {
                                "Connect…"
                            }),
                        )
                        .clicked()
                    {
                        if connected {
                            action =
                                Some((Action::Disconnect(state.name.clone()), PendingKind::Load));
                        } else {
                            connect = state
                                .profiles
                                .iter()
                                .find(|p| p.name == state.name)
                                .cloned();
                        }
                    }
                    ui.label("Database index");
                    let before = state.database;
                    ui.add_enabled(
                        ready && !state.dirty(),
                        egui::DragValue::new(&mut state.database).range(0..=u32::MAX),
                    );
                    if before != state.database {
                        state.reset();
                    }
                    if state.pending.as_ref().is_some_and(|p| {
                        !matches!(p.kind, PendingKind::Write(_) | PendingKind::Save)
                    }) && ui.button("Cancel operation").clicked()
                        && let Some(pending) = &state.pending
                    {
                        pending.cancel.cancel();
                    }
                });
                ui.add_enabled_ui(connected && ready && !state.dirty(), |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Key contains");
                        let input = ui.add(
                            crate::form::singleline(&mut state.query)
                                .hint_text("Literal part of a key"),
                        );
                        if ui.button("Search").clicked()
                            || (input.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                        {
                            state.scan(true);
                        }
                        if ui
                            .add_enabled(
                                state.scan_status == ScanStatus::More
                                    && state.query == state.scanned_query,
                                egui::Button::new("Load more"),
                            )
                            .clicked()
                        {
                            state.scan(false);
                        }
                        if ui.button("New Key").clicked() {
                            state.selected = None;
                            state.creating = true;
                            state.new_key.clear();
                            state.editor = "{\"type\":\"string\",\"value\":\"\"}".into();
                            state.baseline.clear();
                        }
                    });
                });
                ui.label(&state.status);
                if !state.error.is_empty() {
                    ui.label("Operation failed — see Problems for details.");
                }
                if state.pending.is_some() {
                    ui.label("Working…");
                }
                ui.separator();
                let mut read_key = None;
                egui::ScrollArea::both()
                    .id_salt("redis_keys")
                    .max_height(220.0)
                    .show_rows(ui, 24.0, state.rows.len() + 1, |ui, range| {
                        for index in range {
                            ui.horizontal(|ui| {
                                if index == 0 {
                                    for (width, title) in [
                                        (280.0, "Key"),
                                        (90.0, "Type"),
                                        (400.0, "Value / preview"),
                                        (100.0, "TTL at fetch"),
                                    ] {
                                        ui.add_sized(
                                            [width, 22.0],
                                            egui::Label::new(egui::RichText::new(title).strong()),
                                        );
                                    }
                                } else if let Some(row) = state.rows.get(index - 1) {
                                    let selected =
                                        state.selected.as_ref().is_some_and(|s| s.key == row.key);
                                    if ui
                                        .add_enabled_ui(ready && !state.dirty(), |ui| {
                                            ui.add_sized(
                                                [280.0, 22.0],
                                                egui::Button::new(row.key.preview())
                                                    .selected(selected),
                                            )
                                        })
                                        .inner
                                        .clicked()
                                    {
                                        read_key = Some(row.key.clone());
                                    }
                                    ui.add_sized([90.0, 22.0], egui::Label::new(&row.kind));
                                    ui.add_sized(
                                        [400.0, 22.0],
                                        egui::Label::new(&row.preview).truncate(),
                                    );
                                    ui.add_sized(
                                        [100.0, 22.0],
                                        egui::Label::new(ttl_text(row.ttl_ms)),
                                    );
                                }
                            });
                        }
                    });
                if let Some(key) = read_key {
                    action = Some((
                        Action::Read {
                            name: state.name.clone(),
                            database: state.database,
                            key,
                        },
                        PendingKind::Read,
                    ));
                }
                ui.separator();
                redis_editor(ui, state, &mut action);
            })
        });
        if let Some((request, kind)) = action {
            state.queue(request, kind);
        }
        if let Some(profile) = connect {
            // Reuse the form for an explicit session password or an empty/password-store login.
            self.database.redis.form = Some(Form {
                original: Some(profile.clone()),
                profile,
                password: String::new(),
                tested: false,
            });
        }
    }
    fn open_redis_browser(&mut self, name: &str, database: u32) {
        self.database.redis.select_browser(name, database);
        if let Some(index) = self
            .database
            .tabs
            .iter()
            .position(|t| t.redis && t.connection == name)
        {
            self.database.active = Some(index);
        } else {
            self.new_sql_console(name.into(), String::new(), "", "Redis".into());
            if let Some(tab) = self
                .database
                .active
                .and_then(|i| self.database.tabs.get_mut(i))
            {
                tab.redis = true;
            }
        }
        if matches!(
            self.left,
            Some(
                crate::LeftTool::Tools
                    | crate::LeftTool::Snippets
                    | crate::LeftTool::HttpRequests
                    | crate::LeftTool::Commit
            )
        ) {
            self.left = Some(crate::LeftTool::Database);
        }
    }
    fn redis_form(&mut self, ctx: &egui::Context) {
        let hosts = self.remote.database_profile_names();
        let state = &mut self.database.redis;
        let busy = state.pending.is_some();
        let mut close = false;
        let mut connect = None;
        let mut save = None;
        if let Some(form) = &mut state.form {
            let old = form.profile.clone();
            let old_password = Secret::new(form.password.clone());
            let modal = egui::Modal::new(egui::Id::new("redis_connection")).show(ctx, |ui| {
                crate::form::begin(ui);
                ui.heading("Redis Connection");
                ui.add_enabled_ui(!busy, |ui| {
                    egui::Grid::new("redis_connection_fields").num_columns(2).show(ui, |ui| {
                        ui.label("Name"); ui.add(crate::form::singleline(&mut form.profile.name)); ui.end_row();
                        ui.label("Host"); ui.add(crate::form::singleline(&mut form.profile.host)); ui.end_row();
                        ui.label("Port"); ui.add(egui::DragValue::new(&mut form.profile.port).range(1..=65535)); ui.end_row();
                        ui.label("ACL user (optional)"); ui.add(crate::form::singleline(&mut form.profile.user)); ui.end_row();
                        ui.label("Password"); ui.add(crate::form::singleline(&mut form.password).password(true).hint_text("Empty: session / saved password")); ui.end_row();
                        ui.label("Database index"); ui.add(egui::DragValue::new(&mut form.profile.database).range(0..=u32::MAX)); ui.end_row();
                        ui.label("SSH tunnel");
                        egui::ComboBox::from_id_salt("redis_ssh").selected_text(form.profile.ssh_profile_id.as_deref().unwrap_or("Direct connection")).show_ui(ui, |ui| {
                            ui.selectable_value(&mut form.profile.ssh_profile_id, None, "Direct connection");
                            for host in hosts { ui.selectable_value(&mut form.profile.ssh_profile_id, Some(host.clone()), host); }
                        }); ui.end_row();
                        ui.label("Password storage"); ui.checkbox(&mut form.profile.save_password, "Save in OS credential store"); ui.end_row();
                    });
                    ui.label("Create SSH profiles in Remote Host / SSH. Database selection is checked by the server.");
                    ui.label("Standalone Redis over TCP or SSH; direct TLS and Redis Cluster are not supported yet.");
                    if form.tested { ui.colored_label(crate::theme::success(), "Test succeeded"); }
                    if !state.error.is_empty() { ui.label("Operation failed — see Problems."); }
                    ui.horizontal(|ui| {
                        if ui.button("Test Connection").clicked() { connect = Some((form.profile.clone(), Secret::new(form.password.clone()), true)); }
                        if ui.button("Connect").clicked() { connect = Some((form.profile.clone(), Secret::new(form.password.clone()), false)); }
                        if ui.add_enabled(!form.profile.save_password || form.tested, egui::Button::new("Save")).clicked() {
                            save = Some((form.original.clone(), form.profile.clone(), Secret::new(form.password.clone())));
                        }
                        if ui.button("Cancel").clicked() { close = true; }
                    });
                });
                if busy { ui.label("Connecting / saving…"); }
            });
            if old != form.profile || old_password.expose() != form.password {
                form.tested = false;
            }
            close |= modal.should_close() && !busy;
        }
        if close {
            state.form = None;
        }
        if let Some((original, profile, password)) = save {
            state.queue(
                Action::Save {
                    original,
                    profile,
                    password: Some(password),
                },
                PendingKind::Save,
            );
        }
        if let Some((profile, password, test)) = connect {
            self.start_redis_connection(profile, password, test);
        }
    }
    fn start_redis_connection(&mut self, profile: Profile, password: Secret, test: bool) {
        if let Err(error) = profile.validate() {
            self.database.redis.error = error.to_string();
            return;
        }
        if !test {
            self.open_redis_browser(&profile.name, profile.database);
        }
        self.database
            .redis
            .select_browser(&profile.name, profile.database);
        let state = &mut self.database.redis;
        if state.pending.is_some() {
            return;
        }
        if let Some(host) = &profile.ssh_profile_id {
            let host = host.clone();
            let id = state.new_id();
            if self.remote.connect_database(&host, id) {
                state.pending = Some(Pending {
                    id,
                    cancel: CancellationToken::new(),
                    kind: PendingKind::Connect(profile.clone(), test),
                });
                state.ssh = Some((id, profile, password, test));
                state.error.clear();
            } else {
                state.error = "SSH profile missing or worker queue full".into();
            }
        } else {
            let secret = (!password.expose().is_empty()).then_some(password);
            state.queue(
                Action::Connect {
                    profile: profile.clone(),
                    password: secret,
                    ssh: None,
                    test,
                },
                PendingKind::Connect(profile, test),
            );
        }
    }
}
fn ttl_text(ms: i64) -> String {
    match ms {
        -1 => "persistent".into(),
        -2 => "expired".into(),
        _ => format!(
            "{:.1} s",
            std::time::Duration::from_millis(ms.unsigned_abs()).as_secs_f64()
        ),
    }
}
#[allow(
    clippy::too_many_lines,
    reason = "The six structure editors share explicit Submit, TTL and deletion controls"
)]
fn redis_editor(
    ui: &mut egui::Ui,
    state: &mut RedisUi,
    action: &mut Option<(Action, PendingKind)>,
) {
    let ready = state.pending.is_none() && state.connected.contains(&state.name);
    if state.creating {
        ui.horizontal(|ui| {
            ui.label("New key");
            ui.add(crate::form::singleline(&mut state.new_key));
        });
        ui.label("Supported types: string, hash, list, set, zset. Use the JSON examples below.");
    } else if let Some(snapshot) = &state.selected {
        ui.strong(format!(
            "{} · {} · {}",
            snapshot.key.preview(),
            snapshot.data.kind(),
            ttl_text(snapshot.ttl_ms)
        ));
        if snapshot.truncated {
            ui.label("Partial value (1000 elements / 1 MiB limit); read-only.");
        }
    } else {
        ui.label("Select a key to view or edit its structure.");
        return;
    }
    let editable = state.creating
        || state.selected.as_ref().is_some_and(|s| {
            !s.truncated
                && !s.dump.is_empty()
                && !matches!(s.data, Data::Stream(_) | Data::Unsupported(_))
        });
    ui.label("JSON editor: strings stay strings; binary bytes use {\"hex\":\"00ff\"}. Submit preserves the current TTL.");
    egui::ScrollArea::both()
        .id_salt("redis_value")
        .max_height(230.0)
        .show(ui, |ui| {
            ui.add_enabled(
                editable && ready,
                egui::TextEdit::multiline(&mut state.editor)
                    .font(egui::TextStyle::Monospace)
                    .desired_width(f32::INFINITY)
                    .desired_rows(8),
            );
        });
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                editable && ready && state.dirty(),
                egui::Button::new("Submit"),
            )
            .clicked()
        {
            match Data::parse(&state.editor) {
                Ok(data) if state.creating => {
                    *action = Some((
                        Action::Create {
                            name: state.name.clone(),
                            database: state.database,
                            key: Atom::Text(state.new_key.clone()),
                            data,
                        },
                        PendingKind::Write(false),
                    ));
                }
                Ok(data) => state.write(Mutation::Replace(data), false),
                Err(error) => state.error = error.to_string(),
            }
        }
        if ui.add_enabled(ready, egui::Button::new("Revert")).clicked() {
            state.editor.clone_from(&state.baseline);
            state.creating = false;
        }
        if ui
            .add_enabled(
                ready && !state.dirty() && state.selected.is_some(),
                egui::Button::new("Reload Key"),
            )
            .clicked()
            && let Some(snapshot) = &state.selected
        {
            *action = Some((
                Action::Read {
                    name: state.name.clone(),
                    database: state.database,
                    key: snapshot.key.clone(),
                },
                PendingKind::Read,
            ));
        }
        ui.menu_button("JSON examples", |ui| {
            for example in [
                r#"{"type":"string","value":"hello"}"#,
                r#"{"type":"hash","value":[["field","value"]]}"#,
                r#"{"type":"list","value":["first","second"]}"#,
                r#"{"type":"set","value":["one","two"]}"#,
                r#"{"type":"zset","value":[["member","1.5"]]}"#,
            ] {
                if ui.button(example).clicked() && editable && ready {
                    state.editor = example.into();
                    ui.close();
                }
            }
        });
    });
    if state.creating {
        return;
    }
    let writable = ready
        && !state.dirty()
        && state
            .selected
            .as_ref()
            .is_some_and(|s| !s.truncated && !s.dump.is_empty());
    ui.add_enabled_ui(writable, |ui| {
        ui.horizontal(|ui| {
            ui.label("TTL seconds"); ui.add(crate::form::singleline(&mut state.ttl).desired_width(110.0));
            if ui.button("Set TTL").clicked() {
                match state.ttl.parse::<u64>() { Ok(seconds) => state.write(Mutation::Expire(Some(seconds)), false), Err(_) => state.error = "Enter a positive TTL in seconds".into() }
            }
            if ui.button("Persist").clicked() { state.write(Mutation::Expire(None), false); }
            if ui.button("Delete Key…").clicked() { state.delete_confirm = true; }
        });
        if state.delete_confirm {
            ui.horizontal(|ui| {
                ui.label("Permanently delete this key?");
                if ui.button("Delete Key").clicked() { state.write(Mutation::Delete, true); state.delete_confirm = false; }
                if ui.button("Cancel deletion").clicked() { state.delete_confirm = false; }
            });
        }
        if state.selected.as_ref().is_some_and(|s| matches!(s.data, Data::Stream(_))) {
            ui.label("Stream records are immutable. Append new field/value pairs or delete an existing ID.");
            ui.horizontal(|ui| {
                ui.add(crate::form::singleline(&mut state.stream_fields).hint_text("[[\"field\",\"value\"]]"));
                if ui.button("Append entry").clicked() {
                    match serde_json::from_str(&state.stream_fields) { Ok(fields) => state.write(Mutation::Append(fields), false), Err(error) => state.error = error.to_string() }
                }
            });
            ui.horizontal(|ui| {
                ui.add(crate::form::singleline(&mut state.stream_id).hint_text("Stream ID to delete"));
                if ui.button("Delete entry").clicked() { state.write(Mutation::DeleteEntry(state.stream_id.clone()), false); }
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use nozhik_config::{Config, ConfigManager};
    use nozhik_event_bus::EventBus;
    use std::time::{Duration, Instant};

    fn shell(path: &std::path::Path) -> Result<NozhikUi, Box<dyn std::error::Error>> {
        let bus = EventBus::new(32)?;
        let settings = ConfigManager::from_path(path.join("settings.toml"), bus.clone())?;
        Ok(NozhikUi::new(bus, settings, Config::default()))
    }
    fn frame(
        shell: &mut NozhikUi,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1400.0, 1000.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                shell.database_tick(ctx);
                egui::SidePanel::left("redis_test_explorer")
                    .default_width(280.0)
                    .show(ctx, |ui| shell.database_panel(ui));
                shell.editor_area(ctx);
                shell.integration_dialogs(ctx);
            },
        )
    }
    fn click(
        shell: &mut NozhikUi,
        ctx: &egui::Context,
        label: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        frame(shell, ctx, Vec::new());
        let output = frame(shell, ctx, Vec::new());
        let point = output
            .shapes
            .iter()
            .rev()
            .find_map(|s| match &s.shape {
                egui::Shape::Text(text)
                    if text.galley.text() == label && s.clip_rect.contains(text.pos) =>
                {
                    Some(text.pos + text.galley.size() * 0.5)
                }
                _ => None,
            })
            .ok_or_else(|| format!("Button not rendered: {label}"))?;
        frame(
            shell,
            ctx,
            vec![
                egui::Event::PointerMoved(point),
                egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        frame(
            shell,
            ctx,
            vec![egui::Event::PointerButton {
                pos: point,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        Ok(())
    }
    fn settle(shell: &mut NozhikUi, ctx: &egui::Context) {
        let deadline = Instant::now() + Duration::from_secs(8);
        while shell.database.redis.pending.is_some() && Instant::now() < deadline {
            frame(shell, ctx, Vec::new());
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            shell.database.redis.pending.is_none(),
            "Redis worker timed out"
        );
        assert!(
            shell.database.redis.error.is_empty(),
            "{}",
            shell.database.redis.error
        );
    }
    #[test]
    fn hidden_redis_drafts_guard_project_close_and_keep_ssh_identities_separate()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let mut shell = shell(dir.path())?;
        shell.database.redis.editor = "local draft".into();
        assert!(shell.close_target_dirty(crate::integration::CloseTarget::Project));
        shell.save_all();
        assert!(
            shell
                .integration
                .save_error
                .as_ref()
                .is_some_and(|s| s.contains("Redis"))
        );
        shell.database.redis.editor.clear();
        assert!(!shell.database.redis.dirty());
        assert_ne!(shell.database.redis.new_id() & (1 << 63), 0);
        assert!(!shell.database.redis.cancel_ssh(1, "unrelated SQL request"));
        Ok(())
    }
    #[test]
    fn connections_open_editor_tabs_preserve_drafts_and_guard_close()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::integration::CloseTarget;
        let dir = tempfile::tempdir()?;
        let mut shell = shell(dir.path())?;
        let ctx = egui::Context::default();
        frame(&mut shell, &ctx, Vec::new());
        settle(&mut shell, &ctx);
        shell.database.redis.profiles = vec![
            Profile {
                name: "first".into(),
                ..Default::default()
            },
            Profile {
                name: "second".into(),
                database: 1,
                ..Default::default()
            },
        ];
        click(&mut shell, &ctx, "first · DB 0")?;
        assert_eq!(shell.database.tabs.len(), 1);
        assert!(shell.database.tabs[0].redis);
        assert!(!shell.database.modal_open());
        assert!(shell.active_buffer().is_none());
        click(&mut shell, &ctx, "first · DB 0")?;
        assert_eq!(shell.database.tabs.len(), 1);
        shell.database.redis.database = 17;
        shell.database.redis.query = "first query".into();
        shell.database.redis.editor = "unsaved first".into();
        click(&mut shell, &ctx, "second · DB 1")?;
        assert_eq!(shell.database.tabs.len(), 2);
        assert_eq!(shell.database.redis.database, 1);
        assert_eq!(shell.database.redis.editor, "");
        assert!(shell.database.redis.any_dirty());
        assert!(shell.close_target_dirty(CloseTarget::Sql(0)));
        assert!(!shell.close_target_dirty(CloseTarget::Sql(1)));
        shell.database.redis.query = "second query".into();
        click(&mut shell, &ctx, "Redis · first")?;
        assert_eq!(shell.database.redis.database, 17);
        assert_eq!(shell.database.redis.query, "first query");
        assert_eq!(shell.database.redis.editor, "unsaved first");
        shell.loaded_file(dir.path().join("file.txt"), "file text", false);
        frame(&mut shell, &ctx, Vec::new());
        assert!(shell.database.active.is_none());
        assert!(shell.active_buffer().is_some());
        click(&mut shell, &ctx, "Redis · second")?;
        assert_eq!(shell.database.redis.query, "second query");
        click(&mut shell, &ctx, "file.txt")?;
        assert!(shell.database.active.is_none());
        shell.request_close(CloseTarget::Sql(0), &ctx);
        assert!(matches!(shell.integration.close, Some(CloseTarget::Sql(0))));
        click(&mut shell, &ctx, "Cancel")?;
        assert_eq!(shell.database.tabs.len(), 2);
        assert!(shell.database.redis.any_dirty());
        shell.request_close(CloseTarget::Sql(0), &ctx);
        click(&mut shell, &ctx, "Discard")?;
        assert_eq!(shell.database.tabs.len(), 1);
        assert!(!shell.database.redis.any_dirty());
        assert!(shell.database.active.is_none());
        shell.database.shutdown();
        Ok(())
    }
    #[test]
    fn background_redis_reply_updates_its_tab_without_stealing_focus_or_reopening_it()
    -> Result<(), Box<dyn std::error::Error>> {
        use nozhik_database::redis::worker::Reply;
        let dir = tempfile::tempdir()?;
        let mut shell = shell(dir.path())?;
        let ctx = egui::Context::default();
        shell.open_redis_browser("first", 0);
        let id = shell.database.redis.new_id();
        shell.database.redis.pending = Some(Pending {
            id,
            cancel: CancellationToken::new(),
            kind: PendingKind::Read,
        });
        shell.open_redis_browser("second", 2);
        let value = Snapshot {
            key: Atom::Text("key".into()),
            data: Data::String(Atom::Text("loaded first".into())),
            dump: vec![1],
            ttl_ms: -1,
            truncated: false,
        };
        shell.database.redis.apply_reply(Reply {
            id,
            result: Ok(Output::Value(value.clone())),
        });
        assert_eq!(shell.database.active, Some(1));
        assert_eq!(shell.database.redis.name, "second");
        assert_eq!(shell.database.redis.editor, "");
        shell.open_redis_browser("first", 0);
        assert!(shell.database.redis.editor.contains("loaded first"));
        shell.open_redis_browser("second", 2);
        shell.request_close(crate::integration::CloseTarget::Sql(0), &ctx);
        shell.database.redis.apply_reply(Reply {
            id,
            result: Ok(Output::Value(value)),
        });
        assert_eq!(shell.database.tabs.len(), 1);
        assert_eq!(shell.database.active, Some(0));
        assert_eq!(shell.database.redis.name, "second");
        assert!(!shell.database.redis.parked.contains_key("first"));
        Ok(())
    }
    #[test]
    #[ignore = "requires redis-server; override executable with NOZHIK_REDIS_SERVER"]
    fn redis_browser_search_edit_submit_and_revert_buttons()
    -> Result<(), Box<dyn std::error::Error>> {
        struct Process(std::process::Child);
        impl Drop for Process {
            fn drop(&mut self) {
                if self.0.kill().is_ok() {
                    let _status = self.0.wait();
                }
            }
        }
        let dir = tempfile::tempdir()?;
        let socket = std::net::TcpListener::bind("127.0.0.1:0")?;
        let port = socket.local_addr()?.port();
        drop(socket);
        let executable =
            std::env::var_os("NOZHIK_REDIS_SERVER").unwrap_or_else(|| "redis-server".into());
        let _process = Process(
            std::process::Command::new(executable)
                .args([
                    "--bind",
                    "127.0.0.1",
                    "--port",
                    &port.to_string(),
                    "--save",
                    "",
                    "--appendonly",
                    "no",
                ])
                .current_dir(dir.path())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()?,
        );
        let profile = Profile {
            name: "Redis UI test".into(),
            host: "127.0.0.1".into(),
            port,
            ..Default::default()
        };
        let runtime = tokio::runtime::Runtime::new()?;
        runtime.block_on(async {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                match nozhik_database::redis::Connection::connect(&profile, None, None).await {
                    Ok(db) => {
                        db.create(
                            0,
                            &Atom::Text("ui:key".into()),
                            &Data::Hash(vec![(
                                Atom::Text("field".into()),
                                Atom::Text("before".into()),
                            )]),
                        )
                        .await?;
                        break;
                    }
                    Err(error) => {
                        if Instant::now() > deadline {
                            return Err(error);
                        }
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                }
            }
            Ok::<_, nozhik_database::redis::Error>(())
        })?;
        let mut shell = shell(dir.path())?;
        let ctx = egui::Context::default();
        frame(&mut shell, &ctx, Vec::new());
        settle(&mut shell, &ctx);
        shell.database.redis.profiles = vec![profile.clone()];
        shell.start_redis_connection(profile, Secret::new(String::new()), false);
        settle(&mut shell, &ctx);
        shell.database.redis.query = "ui:".into();
        click(&mut shell, &ctx, "Search")?;
        settle(&mut shell, &ctx);
        assert_eq!(shell.database.redis.rows.len(), 1);
        click(&mut shell, &ctx, "ui:key")?;
        settle(&mut shell, &ctx);
        assert!(shell.database.redis.editor.contains("before"));
        shell.database.redis.editor = shell.database.redis.editor.replace("before", "discarded");
        click(&mut shell, &ctx, "Revert")?;
        assert!(shell.database.redis.editor.contains("before"));
        shell.database.redis.editor = shell.database.redis.editor.replace("before", "after");
        assert!(shell.close_target_dirty(crate::integration::CloseTarget::Project));
        click(&mut shell, &ctx, "Submit")?;
        settle(&mut shell, &ctx);
        assert!(!shell.database.redis.dirty());
        assert!(shell.database.redis.editor.contains("after"));
        shell.database.shutdown();
        Ok(())
    }
}
