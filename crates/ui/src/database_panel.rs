//! Database explorer, connection dialogs and typed worker integration.
use crate::{NozhikUi, sql_console::SqlConsole};
use nozhik_database::{
    CancellationToken, ConnectionProfile, DatabaseSchemaTree, DbDriverType,
    worker::{Command, Reply, Worker},
};
use nozhik_remote_ssh::{SshSession, credentials::Secret};
use std::{collections::HashMap, sync::Arc};
use zeroize::Zeroize;

mod designer;
mod redis;
mod schema;
mod tree;

#[derive(Default)]
pub struct DatabaseUi {
    pub(crate) redis: redis::RedisUi,
    worker: Option<Worker>,
    profiles: Vec<ConnectionProfile>,
    trees: HashMap<String, DatabaseSchemaTree>,
    passwords: HashMap<String, Secret>,
    pub tabs: Vec<SqlConsole>,
    pub active: Option<usize>,
    schema_dialog: Option<schema::Dialog>,
    table_designer: Option<designer::Dialog>,
    form: Option<Form>,
    login: Option<ConnectionProfile>,
    password: String,
    pending: HashMap<u64, Pending>,
    next_id: u64,
    error: String,
    saving: bool,
    pending_save: Option<(String, Secret)>,
    tables_pending: HashMap<u64, PendingTable>,
    table_errors: HashMap<(String, String, String), String>,
    selected_object: Option<egui::Id>,
}
struct PendingTable {
    name: String,
    schema: String,
    table: String,
    open: bool,
    open_tab: Option<nozhik_core::BufferId>,
}
struct Pending {
    profile: ConnectionProfile,
    password: Option<Secret>,
    test: bool,
}
#[derive(Default)]
struct Form {
    original: Option<ConnectionProfile>,
    profile: ConnectionProfile,
    password: String,
    tested: bool,
}
fn preserves_password(form: &Form) -> bool {
    form.password.is_empty()
        && form.original.as_ref().is_some_and(|original| {
            if !original.save_password || !form.profile.save_password {
                return false;
            }
            let mut previous = original.clone();
            previous.name.clone_from(&form.profile.name);
            previous.save_password = form.profile.save_password;
            previous == form.profile
        })
}
impl Drop for Form {
    fn drop(&mut self) {
        self.password.zeroize();
    }
}
impl Drop for DatabaseUi {
    fn drop(&mut self) {
        self.password.zeroize();
    }
}
fn catalog_object_incidents(
    trees: &HashMap<String, DatabaseSchemaTree>,
) -> Vec<crate::problems::Incident> {
    let mut incidents = Vec::new();
    for (profile, tree) in trees {
        for schema in &tree.schemas {
            for table in &schema.tables {
                if let Some(error) = table.objects.error.as_deref() {
                    incidents.push(crate::problems::incident(
                        format!("table-objects:{profile}:{}:{}", schema.name, table.name),
                        crate::problems::Severity::Error,
                        "Table metadata",
                        format!("Keys/DDL unavailable: {error}"),
                    ));
                }
            }
        }
    }
    incidents
}

impl DatabaseUi {
    /// Connection, console and metadata failures for the project problem log.
    pub(crate) fn incidents(&self) -> Vec<crate::problems::Incident> {
        let mut incidents = Vec::new();
        if !self.error.is_empty() && self.error != "Connection test succeeded" {
            incidents.push(crate::problems::incident(
                "database",
                crate::problems::severity_of(&self.error),
                "Database",
                &self.error,
            ));
        }
        if let Some(error) = self
            .schema_dialog
            .as_ref()
            .and_then(schema::Dialog::failure)
            && error != self.error
        {
            incidents.push(crate::problems::incident(
                "database-schema",
                crate::problems::severity_of(&error),
                "Database schema",
                error,
            ));
        }
        if let Some(error) = self
            .table_designer
            .as_ref()
            .and_then(designer::Dialog::failure)
            && error != self.error
        {
            incidents.push(crate::problems::incident(
                "database-designer",
                crate::problems::severity_of(&error),
                "Table definition",
                error,
            ));
        }
        for (index, tab) in self.tabs.iter().enumerate() {
            if !tab.error.trim().is_empty() && tab.error != self.error {
                let title = if tab.title.is_empty() {
                    "SQL".to_owned()
                } else {
                    format!("SQL · {}", tab.title)
                };
                incidents.push(crate::problems::incident(
                    format!("sql-{index}"),
                    crate::problems::Severity::Error,
                    title,
                    tab.error.trim(),
                ));
            }
            if let Some(error) = tab
                .table
                .as_ref()
                .and_then(crate::table_editor::TableEditor::preview_problem)
                && error != tab.error
            {
                incidents.push(crate::problems::incident(
                    format!("sql-{index}-dml"),
                    crate::problems::Severity::Error,
                    "Table data",
                    error,
                ));
            }
            for (result_index, editor) in tab.result_editors.iter().enumerate() {
                if let Some(error) = editor.preview_problem() {
                    incidents.push(crate::problems::incident(
                        format!("sql-{index}-result-{result_index}"),
                        crate::problems::Severity::Error,
                        "SQL result",
                        error,
                    ));
                }
            }
        }
        for ((profile, schema_name, table), error) in &self.table_errors {
            if !error.trim().is_empty() {
                incidents.push(crate::problems::incident(
                    format!("table:{profile}:{schema_name}:{table}"),
                    crate::problems::Severity::Error,
                    "Table metadata",
                    error.trim(),
                ));
            }
        }
        incidents.extend(catalog_object_incidents(&self.trees));
        incidents.extend(self.redis.incidents());
        incidents
    }

    pub(crate) fn new_connection(&mut self) {
        self.form = Some(Form::default());
        self.error.clear();
    }
    pub const fn modal_open(&self) -> bool {
        self.redis.modal_open()
            || self.form.is_some()
            || self.login.is_some()
            || self.schema_dialog.is_some()
            || self.table_designer.is_some()
    }
    pub fn table_modal(&self) -> bool {
        self.active
            .and_then(|i| self.tabs.get(i))
            .and_then(SqlConsole::active_table)
            .is_some_and(crate::table_editor::TableEditor::modal_open)
    }
    pub fn shutdown(&mut self) {
        self.redis.shutdown();
        self.schema_dialog = None;
        self.table_designer = None;
        if let Some(worker) = self.worker.take() {
            worker.shutdown();
        }
        self.pending.clear();
        for tab in &mut self.tabs {
            if let Some((_, t)) = tab.pending.take() {
                t.cancel();
            }
        }
    }
    fn queue(&mut self, command: Command) -> bool {
        if self.worker.as_ref().is_some_and(|w| w.send(command)) {
            true
        } else {
            self.error = "Database queue full or stopped".into();
            false
        }
    }
    const fn request_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }
    pub fn cancel_ssh(&mut self, id: u64, error: String) {
        if self.redis.cancel_ssh(id, &error) {
            return;
        }
        self.pending.remove(&id);
        self.error = error;
    }
    pub fn accept_ssh(&mut self, id: u64, session: SshSession) {
        if id & (1 << 63) != 0 {
            self.redis.accept_ssh(id, session);
            return;
        }
        if let Some(pending) = self.pending.remove(&id) {
            self.pending.insert(
                id,
                Pending {
                    profile: pending.profile.clone(),
                    password: pending
                        .password
                        .as_ref()
                        .map(|s| Secret::new(s.expose().into())),
                    test: pending.test,
                },
            );
            self.queue(Command::Connect {
                id,
                profile: pending.profile,
                password: pending.password,
                ssh: Some(session),
                test: pending.test,
            });
        }
    }
}
impl NozhikUi {
    #[allow(
        clippy::too_many_lines,
        reason = "Typed replies update connection, console and table state by request identity"
    )]
    pub(crate) fn database_tick(&mut self, ctx: &egui::Context) {
        self.redis_tick(ctx);
        if self.database.worker.is_none() {
            let ctx = ctx.clone();
            self.database.worker = Some(Worker::start(
                self.settings
                    .path()
                    .with_file_name("database_connections.json"),
                Arc::new(move || ctx.request_repaint()),
            ));
            self.database.queue(Command::Load);
        }
        let replies: Vec<_> = self
            .database
            .worker
            .as_mut()
            .into_iter()
            .flat_map(|w| std::iter::from_fn(|| w.output.try_recv().ok()).take(64))
            .collect();
        for reply in replies {
            match reply {
                Reply::Updated(old_name, profiles) => {
                    self.database.trees.remove(&old_name);
                    self.database.passwords.remove(&old_name);
                    self.database
                        .pending
                        .retain(|_, p| p.profile.name != old_name);
                    self.database
                        .tables_pending
                        .retain(|_, p| p.name != old_name);
                    self.database
                        .table_errors
                        .retain(|(n, _, _), _| n != &old_name);
                    for tab in &mut self.database.tabs {
                        if !tab.redis && tab.connection == old_name {
                            if let Some((_, token)) = tab.pending.take() {
                                token.cancel();
                            }
                            tab.metadata_loading = false;
                            tab.connection_invalid = true;
                            tab.error="Connection settings changed. Reopen the table/console to use the updated connection. Pending edits have been retained.".into();
                        }
                    }
                    self.database.profiles = profiles;
                    self.database.saving = false;
                    self.database.form = None;
                    if let Some((name, password)) = self.database.pending_save.take() {
                        self.database.passwords.insert(name, password);
                    }
                    self.database.error.clear();
                    self.status = "Database connection updated; reconnect to apply settings".into();
                }
                Reply::SchemaTask(id, result) => {
                    if self
                        .database
                        .table_designer
                        .as_ref()
                        .is_some_and(|d| d.accepts(id))
                    {
                        self.table_designer_reply(id, result);
                    } else {
                        self.schema_task_reply(ctx, id, result);
                    }
                }
                Reply::Table(id, result) => self.database_table_reply(id, result),
                Reply::Profiles(profiles) => {
                    self.database.profiles = profiles;
                    if self.database.saving {
                        self.database.form = None;
                        if let Some((name, password)) = self.database.pending_save.take() {
                            self.database.passwords.insert(name, password);
                        }
                    }
                    self.database.saving = false;
                    self.database.error.clear();
                }
                Reply::SaveError(error) => {
                    self.database.error = error;
                    self.database.saving = false;
                    self.database.pending_save = None;
                }
                Reply::Error(error) => self.database.error = error,
                Reply::Disconnected(name) => {
                    for tab in &mut self.database.tabs {
                        if tab.connection == name && tab.metadata_loading {
                            tab.metadata_loading = false;
                            tab.error = "Connection disconnected while loading the table".into();
                        }
                    }
                    self.database.trees.remove(&name);
                    self.database.tables_pending.retain(|_, p| p.name != name);
                    self.database.table_errors.retain(|(n, _, _), _| n != &name);
                }
                Reply::Schema(name, result) => match result {
                    Ok(tree) => {
                        let pending = self
                            .database
                            .tables_pending
                            .values()
                            .filter(|p| p.name == name)
                            .filter_map(|p| {
                                p.open_tab.map(|id| (id, p.schema.clone(), p.table.clone()))
                            })
                            .collect::<Vec<_>>();
                        self.database.tables_pending.retain(|_, p| p.name != name);
                        self.database.table_errors.retain(|(n, _, _), _| n != &name);
                        self.database.trees.insert(name.clone(), tree);
                        let active = self.database.active;
                        for (id, schema, table) in pending {
                            if !self.database.tabs.iter().any(|t| {
                                t.buffer.snapshot().buffer_id == id && !t.connection_invalid
                            }) {
                                continue;
                            }
                            let info = self
                                .database
                                .trees
                                .get(&name)
                                .and_then(|t| t.schemas.iter().find(|s| s.name == schema))
                                .and_then(|s| s.tables.iter().find(|t| t.name == table))
                                .cloned();
                            if let Some(info) = info {
                                self.request_database_table(name.clone(), schema, info, true, true);
                            } else if let Some(tab) = self
                                .database
                                .tabs
                                .iter_mut()
                                .find(|t| t.buffer.snapshot().buffer_id == id)
                            {
                                tab.metadata_loading = false;
                                tab.error = "Table no longer exists after metadata refresh".into();
                            }
                        }
                        self.database.active = active;
                    }
                    Err(e) => self.database.error = e.to_string(),
                },
                Reply::Connected(id, name, test, result) => {
                    let Some(pending) = self.database.pending.remove(&id) else {
                        continue;
                    };
                    match result {
                        Ok(tree) => {
                            if test {
                                if let Some(form) = &mut self.database.form {
                                    form.tested = (form.profile == pending.profile
                                        || (preserves_password(form)
                                            && form.original.as_ref() == Some(&pending.profile)))
                                        && pending.password.as_ref().map_or_else(
                                            || preserves_password(form),
                                            |s| s.expose() == form.password,
                                        );
                                }
                                self.status = format!("Database {name}: Connection test succeeded");
                                self.database.error = "Connection test succeeded".into();
                            } else {
                                self.status = format!("Database {name}: Connected");
                                self.database.trees.insert(name, tree);
                                self.database.error.clear();
                            }
                        }
                        Err(e) => {
                            self.status = format!("Database {name}: {e}");
                            self.database.error = e.to_string();
                        }
                    }
                }
                Reply::Applied(id, result) => {
                    if let Some(index) = self.database.tabs.iter().position(|t| {
                        t.pending
                            .as_ref()
                            .is_some_and(|(pending, _)| *pending == id)
                    }) {
                        let tab = &mut self.database.tabs[index];
                        tab.pending = None;
                        match result {
                            Ok(()) => {
                                if let Some(table) = tab.active_table_mut() {
                                    table.changes = None;
                                }
                                self.reload_database_table(index);
                            }
                            Err(e) => tab.error = e.to_string(),
                        }
                    }
                }
                Reply::Query(id, result) => {
                    if let Some(tab) = self.database.tabs.iter_mut().find(|t| {
                        t.pending
                            .as_ref()
                            .is_some_and(|(pending, _)| *pending == id)
                    }) {
                        tab.pending = None;
                        match result {
                            Ok(results) => {
                                if let Some(table) = &mut tab.table {
                                    if let Some(result) = results.into_iter().next() {
                                        table.loaded(&tab.schema, result);
                                    }
                                } else {
                                    let driver = self
                                        .database
                                        .profiles
                                        .iter()
                                        .find(|p| p.name == tab.connection)
                                        .map_or(nozhik_database::DbDriverType::Sqlite, |p| {
                                            p.driver
                                        });
                                    tab.loaded_results(
                                        results,
                                        self.database.trees.get(&tab.connection),
                                        driver,
                                    );
                                }
                                tab.selected_result = 0;
                                tab.error.clear();
                            }
                            Err(e) => {
                                if let Some(table) = &mut tab.table {
                                    table.query_failed();
                                }
                                tab.error = e.to_string();
                            }
                        }
                    }
                }
            }
        }
        // SSH trust/password dialogs must remain above the DB form while authorizing.
        if !self.workspace.background && !self.remote.modal_open() {
            self.database_dialogs(ctx);
            self.schema_dialog(ctx);
            self.table_designer_dialog(ctx);
            self.redis_dialogs(ctx);
        }
    }
    fn start_database_connection(
        &mut self,
        profile: ConnectionProfile,
        password: Option<Secret>,
        test: bool,
    ) {
        if let Err(e) = profile.validate() {
            self.database.error = e.to_string();
            return;
        }
        if self
            .database
            .pending
            .values()
            .any(|p| p.profile.name == profile.name && p.test == test)
        {
            return;
        }
        self.status = format!(
            "Database {}: Connecting / loading table names…",
            profile.name
        );
        self.database.error.clear();
        let id = self.database.request_id();
        let password = password.or_else(|| {
            self.database
                .passwords
                .get(&profile.name)
                .map(|s| Secret::new(s.expose().into()))
        });
        self.database.pending.insert(
            id,
            Pending {
                profile: profile.clone(),
                password: password.as_ref().map(|s| Secret::new(s.expose().into())),
                test,
            },
        );
        if let Some(name) = &profile.ssh_profile_id {
            self.database.pending.insert(
                id,
                Pending {
                    profile: profile.clone(),
                    password,
                    test,
                },
            );
            if !self.remote.connect_database(name, id) {
                self.database.pending.remove(&id);
                self.database.error = "SSH profile missing or worker queue full".into();
            }
        } else if !self.database.queue(Command::Connect {
            id,
            profile,
            password,
            ssh: None,
            test,
        }) {
            self.database.pending.remove(&id);
        }
    }
    #[allow(
        clippy::too_many_lines,
        reason = "Explorer nested namespace/category/relation tree and its deferred actions share one render pass"
    )]
    pub(crate) fn database_panel(&mut self, ui: &mut egui::Ui) {
        let mut connect = None;
        let mut edit = None;
        let mut describe = Vec::new();
        let mut inspect = None;
        let mut disconnect = None;
        let mut open = None;
        let mut console = None;
        ui.horizontal(|ui| {
            ui.heading("Database Explorer");
            if ui.button("+").on_hover_text("New Connection").clicked() {
                self.database.form = Some(Form::default());
                self.database.error.clear();
            }
        });
        let mut selected = self.database.selected_object;
        self.redis_connections(ui);
        let actions = egui::ScrollArea::both()
            .show(ui, |ui| tree::show(ui, &self.database, &mut selected))
            .inner;
        self.database.selected_object = selected;
        for action in actions {
            match action {
                tree::Action::Schema(profile, schema, action) => {
                    self.schema_action(ui.ctx(), profile, schema, action);
                }
                tree::Action::Table(profile, schema, table, action) => {
                    self.table_context_action(ui.ctx(), profile, schema, table, action);
                }
                tree::Action::Connect(profile) => connect = Some(profile),
                tree::Action::Password(profile) => self.database.login = Some(profile),
                tree::Action::Edit(profile) => edit = Some(profile),
                tree::Action::Refresh(name) => inspect = Some(name),
                tree::Action::Disconnect(name) => disconnect = Some(name),
                tree::Action::Console(name, schema) => console = Some((name, schema)),
                tree::Action::Describe(name, schema, table, retry) => {
                    describe.push((name, schema, table, false, retry));
                }
                tree::Action::Open(profile, schema, table) => open = Some((profile, schema, table)),
            }
        }
        if let Some(profile) = edit {
            self.edit_database_connection(profile);
            ui.ctx().request_repaint();
        }
        for (name, schema, table, open, retry) in describe {
            self.request_database_table(name, schema, table, open, retry);
        }
        if let Some(name) = inspect {
            self.database.queue(Command::Inspect(name));
        }
        if let Some(name) = disconnect
            && self.database.queue(Command::Disconnect(name.clone()))
        {
            self.database.trees.remove(&name);
        }
        if let Some(profile) = connect {
            if profile.driver != DbDriverType::Sqlite
                && !profile.save_password
                && !self.database.passwords.contains_key(&profile.name)
            {
                self.database.login = Some(profile);
            } else {
                self.start_database_connection(profile, None, false);
            }
        }
        if let Some((name, schema)) = console {
            self.new_sql_console(name, schema, "SELECT 1;", "SQL Console".into());
        }
        if let Some((profile, schema, table)) = open {
            self.open_database_table(profile, schema, table);
        }
    }
    fn open_database_table(
        &mut self,
        profile: ConnectionProfile,
        schema: String,
        table: nozhik_database::introspection::TableInfo,
    ) {
        if let Some(index) = self.database.tabs.iter().position(|t| {
            !t.connection_invalid
                && t.connection == profile.name
                && t.schema == schema
                && t.table.as_ref().is_some_and(|v| v.info.name == table.name)
        }) {
            self.database.active = Some(index);
            return;
        }
        let loading = !table.details_loaded;
        let title = table.name.clone();
        let editor = crate::table_editor::TableEditor::new(profile.driver, table.clone());
        self.new_sql_console(profile.name.clone(), schema.clone(), "", title);
        if let Some(index) = self.database.active {
            self.database.tabs[index].metadata_loading = loading;
            self.database.tabs[index].table = Some(editor);
            if loading {
                self.request_database_table(profile.name, schema, table, true, true);
            } else {
                self.reload_database_table(index);
            }
        }
    }

    fn edit_database_connection(&mut self, profile: ConnectionProfile) {
        self.database.form = Some(Form {
            original: Some(profile.clone()),
            profile,
            password: String::new(),
            tested: false,
        });
        self.database.error.clear();
    }
    fn request_database_table(
        &mut self,
        name: String,
        schema: String,
        table: nozhik_database::introspection::TableInfo,
        open: bool,
        retry: bool,
    ) {
        let open_tab = if open {
            let existing = self.database.tabs.iter().position(|t| {
                !t.connection_invalid
                    && t.connection == name
                    && t.schema == schema
                    && t.table.as_ref().is_some_and(|v| v.info.name == table.name)
            });
            let index = if let Some(index) = existing {
                index
            } else {
                let Some(driver) = self
                    .database
                    .profiles
                    .iter()
                    .find(|p| p.name == name)
                    .map(|p| p.driver)
                else {
                    return;
                };
                self.new_sql_console(name.clone(), schema.clone(), "", table.name.clone());
                let Some(index) = self.database.active else {
                    return;
                };
                self.database.tabs[index].table =
                    Some(crate::table_editor::TableEditor::new(driver, table.clone()));
                index
            };
            self.database.active = Some(index);
            self.database.tabs[index].metadata_loading = true;
            Some(self.database.tabs[index].buffer.snapshot().buffer_id)
        } else {
            None
        };
        if let Some(pending) = self
            .database
            .tables_pending
            .values_mut()
            .find(|p| p.name == name && p.schema == schema && p.table == table.name)
        {
            pending.open |= open;
            if open_tab.is_some() {
                pending.open_tab = open_tab;
            }
            return;
        }
        let key = (name.clone(), schema.clone(), table.name.clone());
        if retry {
            self.database.table_errors.remove(&key);
        }
        if self.database.table_errors.contains_key(&key) || !self.database.trees.contains_key(&name)
        {
            if let Some(tab_id) = open_tab
                && let Some(tab) = self
                    .database
                    .tabs
                    .iter_mut()
                    .find(|t| t.buffer.snapshot().buffer_id == tab_id)
            {
                tab.metadata_loading = false;
                tab.error = self
                    .database
                    .table_errors
                    .get(&key)
                    .cloned()
                    .unwrap_or_else(|| "Connection offline; reconnect before retrying".into());
            }
            return;
        }
        let id = self.database.request_id();
        if self.database.queue(Command::Describe {
            id,
            name: name.clone(),
            schema: schema.clone(),
            table: table.name.clone(),
            is_view: table.is_view,
        }) {
            self.database.tables_pending.insert(
                id,
                PendingTable {
                    name,
                    schema,
                    table: table.name,
                    open,
                    open_tab,
                },
            );
        } else if let Some(tab_id) = open_tab
            && let Some(tab) = self
                .database
                .tabs
                .iter_mut()
                .find(|t| t.buffer.snapshot().buffer_id == tab_id)
        {
            tab.metadata_loading = false;
            tab.error.clone_from(&self.database.error);
        }
    }
    fn database_table_reply(
        &mut self,
        id: u64,
        result: nozhik_database::Result<nozhik_database::introspection::TableInfo>,
    ) {
        let Some(pending) = self.database.tables_pending.remove(&id) else {
            return;
        };
        let Some(table) = self
            .database
            .trees
            .get_mut(&pending.name)
            .and_then(|t| t.schemas.iter_mut().find(|s| s.name == pending.schema))
            .and_then(|s| s.tables.iter_mut().find(|t| t.name == pending.table))
        else {
            return;
        };
        match result {
            Ok(info) => {
                *table = info.clone();
                for tab in &mut self.database.tabs {
                    if !tab.connection_invalid
                        && tab.connection == pending.name
                        && tab.schema == pending.schema
                        && let Some(view) = &mut tab.table
                        && view.info.name == info.name
                    {
                        view.info = info.clone();
                    }
                }
                let tree = self.database.trees.get(&pending.name);
                for tab in &mut self.database.tabs {
                    if tab.table.is_none()
                        && !tab.connection_invalid
                        && tab.result_connection == pending.name
                        && tab.result_context_valid()
                        && tab.pending.is_none()
                        && tab
                            .result_editors
                            .iter()
                            .all(|e| e.info.is_view && !e.dirty())
                        && let Some(driver) = tab.result_editors.first().map(|e| e.driver)
                    {
                        tab.rebuild_result_editors(tree, driver);
                    }
                }
                self.database.table_errors.remove(&(
                    pending.name.clone(),
                    pending.schema.clone(),
                    pending.table,
                ));
                if let Some(tab_id) = pending.open_tab
                    && let Some(index) = self.database.tabs.iter().position(|t| {
                        t.buffer.snapshot().buffer_id == tab_id && !t.connection_invalid
                    })
                {
                    self.database.tabs[index].metadata_loading = false;
                    self.reload_database_table(index);
                }
            }
            Err(e) => {
                let error = format!("{}.{}: {e}", pending.schema, pending.table);
                self.database
                    .table_errors
                    .insert((pending.name, pending.schema, pending.table), error.clone());
                if let Some(tab_id) = pending.open_tab
                    && let Some(tab) = self
                        .database
                        .tabs
                        .iter_mut()
                        .find(|t| t.buffer.snapshot().buffer_id == tab_id)
                {
                    tab.metadata_loading = false;
                    tab.error.clone_from(&error);
                }
                if pending.open {
                    self.database.error = error.clone();
                }
                self.status = error;
            }
        }
    }
    pub(crate) fn open_database_console(&mut self) {
        if let Some(profile) = self.database.profiles.first() {
            let name = profile.name.clone();
            self.new_sql_console(name, String::new(), "SELECT 1;", "SQL Console".into());
        } else {
            self.database.new_connection();
        }
    }
    fn new_sql_console(&mut self, name: String, schema: String, sql: &str, title: String) {
        let id = self.next_buffer_id;
        self.next_buffer_id += 1;
        self.database
            .tabs
            .push(SqlConsole::new(id, name, schema, sql, title));
        self.database.active = self.database.tabs.len().checked_sub(1);
    }
    #[allow(
        clippy::too_many_lines,
        reason = "Routes console and shared table actions after scheduling lazy SQL metadata"
    )]
    pub(crate) fn database_editor(&mut self, ui: &mut egui::Ui, enabled: bool) {
        if self
            .database
            .active
            .and_then(|i| self.database.tabs.get(i))
            .is_some_and(|t| t.redis)
        {
            self.redis_editor_view(ui, enabled);
            return;
        }
        if let Some(tab) = self.database.active.and_then(|i| self.database.tabs.get(i)) {
            let name = tab.connection.clone();
            let needed = self
                .database
                .trees
                .get(&name)
                .map_or_else(Vec::new, |tree| {
                    crate::sql_support::referenced_tables(
                        &tab.buffer.rope().to_string(),
                        &tab.schema,
                        tree,
                    )
                });
            for (schema, table) in needed {
                self.request_database_table(name.clone(), schema, table, false, false);
            }
        }
        if let Some(index) = self.database.active
            && let Some(tab) = self.database.tabs.get_mut(index)
        {
            tab.split = self.layout.value.sql;
            let tree = self.database.trees.get(&tab.connection);
            let request = tab.show(
                ui,
                &self.database.profiles,
                tree,
                enabled && !tab.connection_invalid,
            );
            let retry = if std::mem::take(&mut tab.retry_metadata) {
                tab.table
                    .as_ref()
                    .map(|t| (tab.connection.clone(), tab.schema.clone(), t.info.clone()))
            } else {
                None
            };
            let action = tab.active_table_mut().and_then(|t| t.action.take());
            self.editor_focus = tab.focus_id;
            self.integration.last_editor = tab.editor_id;
            let mut layout = self.layout.value.clone();
            layout.sql = tab.split;
            self.layout.set(layout);
            if let Some((name, schema, info)) = retry {
                self.request_database_table(name, schema, info, true, true);
            }
            if let Some(sql) = request {
                self.execute_database_sql(sql);
            }
            if let Some(action) = action {
                ui.ctx().request_repaint();
                match action {
                    crate::table_editor::TableAction::Cancel => {
                        if let Some((_, token)) = &self.database.tabs[index].pending {
                            token.cancel();
                        }
                    }
                    crate::table_editor::TableAction::Page { offset, size } => {
                        if self.database.tabs[index].pending.is_some()
                            || self.database.tabs[index].connection_invalid
                        {
                            return;
                        }
                        if let Some(table) = &mut self.database.tabs[index].table
                            && !table.dirty()
                        {
                            table.pending_page = Some((offset, size));
                            self.reload_database_table(index);
                            if self.database.tabs[index].pending.is_none()
                                && let Some(table) = &mut self.database.tabs[index].table
                            {
                                table.query_failed();
                            }
                        }
                    }
                    crate::table_editor::TableAction::Query => {
                        if self.database.tabs[index].pending.is_some()
                            || self.database.tabs[index].connection_invalid
                        {
                            return;
                        }
                        if let Some(table) = &mut self.database.tabs[index].table
                            && !table.dirty()
                        {
                            table.request_query();
                            self.reload_database_table(index);
                        }
                    }
                    crate::table_editor::TableAction::Reload => self.reload_database_table(index),
                    crate::table_editor::TableAction::Submit => self.submit_database_table(index),
                    crate::table_editor::TableAction::Ddl => {
                        let tab = &self.database.tabs[index];
                        if let Some(table) = tab.active_table() {
                            let name = tab.connection.clone();
                            let schema = table
                                .changes
                                .as_ref()
                                .map_or_else(|| tab.schema.clone(), |c| c.schema.clone());
                            let title = format!("{}.ddl.sql", table.info.name);
                            let ddl = table.info.objects.ddl.clone();
                            if let Some(index) = self.database.tabs.iter().position(|t| {
                                t.ddl
                                    && !t.connection_invalid
                                    && t.connection == name
                                    && t.schema == schema
                                    && t.title == title
                            }) {
                                self.database.active = Some(index);
                            } else {
                                self.new_sql_console(name, schema, &ddl, title);
                                if let Some(index) = self.database.active {
                                    self.database.tabs[index].ddl = true;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    fn reload_database_table(&mut self, index: usize) {
        let Some(tab) = self.database.tabs.get(index) else {
            return;
        };
        let Some(table) = tab.active_table() else {
            return;
        };
        if table.console_result {
            if !tab.result_context_valid() {
                return;
            }
            let sql = tab.executed_sql.clone();
            self.queue_database_sql(index, sql);
            return;
        }
        let sql = table.page_sql(&tab.schema);
        match sql {
            Ok(sql) => self.queue_database_sql(index, sql),
            Err(error) => {
                self.database.tabs[index].error = error.to_string();
            }
        }
        if self.database.tabs[index].pending.is_none()
            && let Some(table) = &mut self.database.tabs[index].table
        {
            table.query_failed();
        }
    }
    pub(crate) fn submit_database_table(&mut self, index: usize) {
        let Some(tab) = self.database.tabs.get(index) else {
            return;
        };
        if tab.pending.is_some()
            || tab.connection_invalid
            || (tab.table.is_none() && (!tab.result_context_valid()))
        {
            return;
        }
        let Some(changes) = tab.active_table().and_then(|t| t.changes.clone()) else {
            return;
        };
        if !changes.is_dirty() {
            return;
        }
        if let Err(e) = changes.statements() {
            self.database.tabs[index].error = e.to_string();
            return;
        }
        let name = tab.connection.clone();
        let id = self.database.request_id();
        let cancel = CancellationToken::new();
        if self.database.queue(Command::Apply {
            id,
            name,
            changes,
            cancel: cancel.clone(),
        }) {
            let tab = &mut self.database.tabs[index];
            tab.pending = Some((id, cancel));
            tab.error.clear();
        }
    }
    fn execute_database_sql(&mut self, sql: String) {
        if let Some(index) = self.database.active {
            self.queue_database_sql(index, sql);
        }
    }
    fn queue_database_sql(&mut self, index: usize, sql: String) {
        if sql.trim().is_empty() {
            return;
        }
        let id = self.database.request_id();
        let token = CancellationToken::new();
        if let Some(tab) = self.database.tabs.get(index) {
            if tab.pending.is_some()
                || tab.redis
                || tab.connection_invalid
                || tab
                    .result_editors
                    .iter()
                    .any(crate::table_editor::TableEditor::dirty)
            {
                return;
            }
            let command = Command::Query {
                id,
                name: tab.connection.clone(),
                sql: sql.clone(),
                schema: Some(tab.schema.clone()),
                cancel: token.clone(),
            };
            if self.database.queue(command)
                && let Some(tab) = self.database.tabs.get_mut(index)
            {
                tab.pending_sql = sql;
                tab.pending = Some((id, token));
                tab.error.clear();
            }
        }
    }
    #[allow(
        clippy::too_many_lines,
        reason = "Connection form fields, validation and actions share one modal"
    )]
    fn database_dialogs(&mut self, ctx: &egui::Context) {
        let mut test = None;
        let mut save = None;
        let mut close = false;
        if let Some(form) = &mut self.database.form {
            let old = form.profile.clone();
            let old_password = Secret::new(form.password.clone());
            let modal = crate::dialog::Dialog::new("new_database", [640.0, 650.0]).show(ctx, |ui| {
                ui.heading(if form.original.is_some() {
                    "Edit Database Connection"
                } else {
                    "New Database Connection"
                });
                ui.scope(|ui| {
                        ui.add_enabled_ui(!self.database.saving, |ui| {
                            crate::form::begin(ui);
                            ui.label("Connection name");
                            ui.add(crate::form::singleline(&mut form.profile.name));
                            egui::ComboBox::from_id_salt("db_driver")
                                .selected_text(format!("{:?}", form.profile.driver))
                                .show_ui(ui, |ui| {
                                    for (driver, label) in [
                                        (DbDriverType::Postgres, "PostgreSQL"),
                                        (DbDriverType::Mysql, "MySQL / MariaDB"),
                                        (DbDriverType::Sqlite, "SQLite"),
                                        (DbDriverType::ClickHouse, "ClickHouse (HTTP / HTTPS)"),
                                    ] {
                                        if ui
                                            .selectable_value(
                                                &mut form.profile.driver,
                                                driver,
                                                label,
                                            )
                                            .changed()
                                        {
                                            form.profile.port = match driver { DbDriverType::ClickHouse => 8123, DbDriverType::Mysql => 3306, _ => 5432 };
                                            form.profile.tls = false;
                                            if driver == DbDriverType::ClickHouse {
                                                form.profile.user = "default".into();
                                                form.profile.database = "default".into();
                                            }
                                            if driver == DbDriverType::Sqlite {
                                                form.profile.ssh_profile_id = None;
                                            }
                                        }
                                    }
                                });
                            if form.profile.driver != DbDriverType::Sqlite {
                                ui.horizontal(|ui| {
                                    ui.label("Host");
                                    ui.add(crate::form::singleline(&mut form.profile.host));
                                    ui.label("Port");
                                    ui.add(
                                        egui::DragValue::new(&mut form.profile.port)
                                            .range(1..=65535),
                                    );
                                });
                                if form.profile.driver == DbDriverType::ClickHouse {
                                    if ui.checkbox(&mut form.profile.tls, "HTTPS (verify TLS certificate)").changed() {
                                        if form.profile.tls && form.profile.port == 8123 { form.profile.port = 8443; }
                                        else if !form.profile.tls && form.profile.port == 8443 { form.profile.port = 8123; }
                                    }
                                    ui.weak("Use the HTTP port (8123), HTTPS port (8443), or your server's configured HTTP endpoint.");
                                }
                                ui.label("User");
                                ui.add(crate::form::singleline(&mut form.profile.user));
                                ui.label(if form.original.is_some() {
                                    "Password (blank keeps saved password for unchanged endpoint)"
                                } else {
                                    "Password"
                                });
                                ui.add(crate::form::singleline(&mut form.password).password(true));
                                ui.checkbox(
                                    &mut form.profile.save_password,
                                    "Save password in system credential store",
                                );
                                ui.weak("Unchecked: session only; passwords never enter JSON.");
                                let mut ssh = form.profile.ssh_profile_id.is_some();
                                if ui.checkbox(&mut ssh, "Use SSH Tunnel").changed() {
                                    form.profile.ssh_profile_id =
                                        if ssh { Some(String::new()) } else { None };
                                }
                                if let Some(name) = &mut form.profile.ssh_profile_id {
                                    egui::ComboBox::from_id_salt("database_ssh")
                                        .selected_text(if name.is_empty() {
                                            "Select SSH host"
                                        } else {
                                            name.as_str()
                                        })
                                        .show_ui(ui, |ui| {
                                            for profile in self.remote.database_profiles() {
                                                ui.selectable_value(
                                                    name,
                                                    profile.name.clone(),
                                                    &profile.name,
                                                );
                                            }
                                        });
                                }
                            }
                            ui.label(if form.profile.driver == DbDriverType::Sqlite {
                                "SQLite file path / :memory:"
                            } else {
                                "Database"
                            });
                            ui.add(crate::form::singleline(&mut form.profile.database));
                            if form.tested {
                                ui.colored_label(crate::theme::success(), "Test succeeded");
                            }
                            ui.horizontal(|ui| {
                                if ui
                                    .add_enabled(
                                        !self.database.pending.values().any(|p| p.test),
                                        egui::Button::new("Test Connection"),
                                    )
                                    .clicked()
                                {
                                    test = Some(if preserves_password(form) {
                                        (
                                            form.original
                                                .clone()
                                                .unwrap_or_else(|| form.profile.clone()),
                                            None,
                                        )
                                    } else {
                                        (
                                            form.profile.clone(),
                                            Some(Secret::new(form.password.clone())),
                                        )
                                    });
                                }
                                if ui
                                    .add_enabled(
                                        !form.profile.save_password
                                            || form.tested
                                            || preserves_password(form),
                                        egui::Button::new("Save"),
                                    )
                                    .clicked()
                                {
                                    save = Some((
                                        form.original.clone(),
                                        form.profile.clone(),
                                        if preserves_password(form)
                                            || (form.original.is_some()
                                                && form.password.is_empty()
                                                && !form.profile.save_password)
                                        {
                                            None
                                        } else {
                                            Some(Secret::new(form.password.clone()))
                                        },
                                    ));
                                }
                                if ui.button("Cancel").clicked() {
                                    close = true;
                                }
                            });
                        });
                    });
            });
            if form.profile != old || form.password != old_password.expose() {
                form.tested = false;
            }
            close |= modal.should_close() && !self.database.saving;
        }
        if let Some((profile, password)) = test {
            self.start_database_connection(profile, password, true);
        }
        if let Some((original, profile, password)) = save {
            if let Err(e) = profile.validate() {
                self.database.error = e.to_string();
            } else {
                self.database.pending_save = password
                    .as_ref()
                    .map(|p| (profile.name.clone(), Secret::new(p.expose().into())))
                    .or_else(|| {
                        let old = original.as_ref()?;
                        let mut previous = old.clone();
                        previous.name.clone_from(&profile.name);
                        previous.save_password = profile.save_password;
                        (previous == profile)
                            .then(|| self.database.passwords.get(&old.name))
                            .flatten()
                            .map(|p| (profile.name.clone(), Secret::new(p.expose().into())))
                    });
                let command = original.map_or_else(
                    || {
                        Command::Save(
                            profile.clone(),
                            password.as_ref().map(|p| Secret::new(p.expose().into())),
                        )
                    },
                    |original| Command::Update {
                        original,
                        profile: profile.clone(),
                        password: password.as_ref().map(|p| Secret::new(p.expose().into())),
                    },
                );
                self.database.saving = self.database.queue(command);
                if !self.database.saving {
                    self.database.pending_save = None;
                }
            }
        }
        if close {
            self.database.form = None;
            self.database.pending.retain(|_, p| !p.test);
        }
        let mut login = false;
        let mut cancel = false;
        if let Some(profile) = &self.database.login {
            let modal =
                crate::dialog::Dialog::new("database_login", [560.0, 260.0]).show(ctx, |ui| {
                    crate::form::begin(ui);
                    ui.heading(format!("Connect: {}", profile.name));
                    ui.label("Session password");
                    ui.add(crate::form::singleline(&mut self.database.password).password(true));
                    ui.horizontal(|ui| {
                        login = ui.button("Connect").clicked();
                        cancel = ui.button("Cancel").clicked();
                    });
                });
            cancel |= modal.should_close();
        }
        if login && let Some(profile) = self.database.login.take() {
            let secret = Secret::new(std::mem::take(&mut self.database.password));
            self.database
                .passwords
                .insert(profile.name.clone(), Secret::new(secret.expose().into()));
            self.start_database_connection(profile, Some(secret), false);
        }
        if cancel {
            self.database.login = None;
            self.database.password.zeroize();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nozhik_config::{Config, ConfigManager};
    use nozhik_event_bus::EventBus;
    use std::time::{Duration, Instant};

    #[test]
    fn new_connection_form_shows_first_and_last_fields_and_resize_grip()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let bus = EventBus::new(32)?;
        let settings = ConfigManager::from_path(dir.path().join("settings.toml"), bus.clone())?;
        let mut shell = NozhikUi::new(bus, settings, Config::default());
        shell.database.form = Some(Form {
            profile: ConnectionProfile {
                driver: DbDriverType::ClickHouse,
                user: "default".into(),
                database: "default".into(),
                ..ConnectionProfile::default()
            },
            original: None,
            password: String::new(),
            tested: false,
        });
        let ctx = egui::Context::default();
        let draw = |shell: &mut NozhikUi| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1440.0, 1000.0),
                    )),
                    ..Default::default()
                },
                |ctx| shell.database_dialogs(ctx),
            )
        };
        draw(&mut shell);
        let output = draw(&mut shell);
        for label in [
            "Connection name",
            "User",
            "Password",
            "Database",
            "Test Connection",
            "Save",
            "Cancel",
        ] {
            assert!(output.shapes.iter().any(|s| matches!(&s.shape, egui::Shape::Text(t) if t.galley.text() == label && s.clip_rect.contains_rect(egui::Rect::from_min_size(t.pos, t.galley.size())))), "Not visible: {label}");
        }
        let grip = ctx
            .read_response(egui::Id::new("new_database").with("resize_grip"))
            .ok_or("grip")?;
        assert!(grip.rect.is_positive());
        Ok(())
    }

    #[test]
    fn sqlite_explorer_console_shortcut_results_and_modal_guards()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let bus = EventBus::new(32)?;
        let settings = ConfigManager::from_path(dir.path().join("settings.toml"), bus.clone())?;
        let mut shell = NozhikUi::new(bus, settings, Config::default());
        let ctx = egui::Context::default();
        let profile = ConnectionProfile {
            name: "UI SQLite".into(),
            ..ConnectionProfile::default()
        };
        let _frame = ctx.run(egui::RawInput::default(), |ctx| shell.database_tick(ctx));
        shell.start_database_connection(profile.clone(), None, false);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !shell.database.trees.contains_key(&profile.name) && Instant::now() < deadline {
            let _frame = ctx.run(egui::RawInput::default(), |ctx| shell.database_tick(ctx));
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            shell.database.trees.contains_key(&profile.name),
            "{}",
            shell.database.error
        );
        shell.database.profiles = vec![profile.clone()];
        shell.new_sql_console(
            profile.name.clone(),
            "main".into(),
            "SELECT 19 AS value;",
            "Console".into(),
        );
        let input = egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::COMMAND,
            }],
            ..egui::RawInput::default()
        };
        let _frame = ctx.run(input, |ctx| shell.editor_area(ctx));
        assert!(
            shell
                .database
                .tabs
                .first()
                .ok_or("no tab")?
                .pending
                .is_some()
        );
        assert_eq!(
            shell
                .database
                .tabs
                .first()
                .ok_or("no tab")?
                .buffer
                .rope()
                .to_string(),
            "SELECT 19 AS value;"
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while shell
            .database
            .tabs
            .first()
            .is_some_and(|t| t.pending.is_some())
            && Instant::now() < deadline
        {
            let _frame = ctx.run(egui::RawInput::default(), |ctx| {
                shell.database_tick(ctx);
                shell.editor_area(ctx);
            });
            std::thread::sleep(Duration::from_millis(5));
        }
        let tab = shell.database.tabs.first().ok_or("no tab")?;
        assert!(tab.error.is_empty(), "{}", tab.error);
        assert_eq!(
            tab.results
                .first()
                .and_then(|g| g.result.rows.first())
                .and_then(|r| r.first()),
            Some(&nozhik_database::DbValue::Int(19))
        );
        shell.database.form = Some(Form {
            original: None,
            profile,
            password: "secret-not-painted".into(),
            tested: false,
        });
        let _frame = ctx.run(egui::RawInput::default(), |ctx| {
            shell.editor_area(ctx);
            shell.database_dialogs(ctx);
        });
        assert!(shell.database.modal_open());
        assert!(!shell.action_context().editor_focus);
        shell.database.shutdown();
        Ok(())
    }
}

#[cfg(test)]
mod table_tests {
    use super::*;
    use egui::{Event, Pos2, Rect, vec2};
    use nozhik_config::{Config, ConfigManager};
    use nozhik_database::{Connection, DbValue};
    use nozhik_event_bus::EventBus;
    use std::time::{Duration, Instant};

    pub(super) fn frame(
        shell: &mut NozhikUi,
        ctx: &egui::Context,
        events: Vec<Event>,
    ) -> egui::FullOutput {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1200.0, 700.0))),
                events,
                ..Default::default()
            },
            |ctx| {
                shell.database_tick(ctx);
                egui::SidePanel::left("test_database")
                    .default_width(300.0)
                    .show(ctx, |ui| shell.database_panel(ui));
                shell.editor_area(ctx);
            },
        )
    }
    fn text(output: &egui::FullOutput, label: &str) -> Option<Rect> {
        output.shapes.iter().rev().find_map(|s| match &s.shape {
            egui::Shape::Text(t) if t.galley.text() == label && s.clip_rect.contains(t.pos) => {
                Some(Rect::from_min_size(t.pos, t.galley.size()))
            }
            _ => None,
        })
    }
    fn click(shell: &mut NozhikUi, ctx: &egui::Context, point: Pos2) -> egui::FullOutput {
        frame(
            shell,
            ctx,
            vec![
                Event::PointerMoved(point),
                Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        )
    }
    pub(super) fn click_label(
        shell: &mut NozhikUi,
        ctx: &egui::Context,
        label: &str,
    ) -> Result<egui::FullOutput, Box<dyn std::error::Error>> {
        frame(shell, ctx, vec![]);
        let output = frame(shell, ctx, vec![]);
        let rect = (if label == "+" {
            output.shapes.iter().rev().find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text() == label && s.clip_rect.contains(t.pos) => {
                    Some(Rect::from_min_size(t.pos, t.galley.size()))
                }
                _ => None,
            })
        } else {
            text(&output, label)
        })
        .ok_or_else(|| format!("Missing UI label: {label}"))?;
        Ok(click(shell, ctx, rect.center()))
    }
    pub(super) fn connection_menu(
        shell: &mut NozhikUi,
        ctx: &egui::Context,
        name: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let output = frame(shell, ctx, vec![]);
        let point = output
            .shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Text(t)
                    if t.galley.text() == name
                        && t.pos.x < 300.0
                        && s.clip_rect.contains(t.pos) =>
                {
                    Some(Rect::from_min_size(t.pos, t.galley.size()).center())
                }
                _ => None,
            })
            .ok_or("connection/schema row")?;
        frame(
            shell,
            ctx,
            vec![
                Event::PointerMoved(point),
                Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Secondary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Secondary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        Ok(())
    }
    pub(super) fn wait(
        shell: &mut NozhikUi,
        ctx: &egui::Context,
        ready: impl Fn(&NozhikUi) -> bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !ready(shell) && Instant::now() < deadline {
            frame(shell, ctx, vec![]);
            std::thread::sleep(Duration::from_millis(5));
        }
        if !ready(shell) {
            return Err(format!(
                "Timed out: {}; trees={}, tabs={:?}",
                shell.database.error,
                shell.database.trees.len(),
                shell
                    .database
                    .tabs
                    .iter()
                    .map(|t| (&t.title, &t.error, t.pending.is_some(), t.table.is_some()))
                    .collect::<Vec<_>>()
            )
            .into());
        }
        Ok(())
    }
    pub(super) fn replace_cell(
        shell: &mut NozhikUi,
        ctx: &egui::Context,
        old: &str,
        new: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        click_label(shell, ctx, old)?;
        frame(
            shell,
            ctx,
            vec![
                Event::Key {
                    key: egui::Key::A,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::COMMAND,
                },
                Event::Text(new.into()),
            ],
        );
        frame(
            shell,
            ctx,
            vec![Event::Key {
                key: egui::Key::A,
                physical_key: None,
                pressed: false,
                repeat: false,
                modifiers: egui::Modifiers::COMMAND,
            }],
        );
        Ok(())
    }
    fn set_table_clause(
        shell: &mut NozhikUi,
        ctx: &egui::Context,
        label: &str,
        value: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let output = frame(shell, ctx, vec![]);
        let label = text(&output, label).ok_or("clause label")?;
        click(shell, ctx, label.right_center() + vec2(35.0, 0.0));
        frame(
            shell,
            ctx,
            vec![
                Event::Key {
                    key: egui::Key::A,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::COMMAND,
                },
                if value.is_empty() {
                    Event::Key {
                        key: egui::Key::Backspace,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    }
                } else {
                    Event::Text(value.into())
                },
                Event::Key {
                    key: egui::Key::Enter,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        frame(shell, ctx, vec![]);
        Ok(())
    }
    fn click_table_sort(
        shell: &mut NozhikUi,
        ctx: &egui::Context,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let output = frame(shell, ctx, vec![]);
        let header = text(&output, "PK id\nINTEGER").ok_or("id header")?;
        click(shell, ctx, header.min + vec2(162.0, 16.0));
        wait(shell, ctx, |s| s.database.tabs[0].pending.is_none())
    }
    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "End-to-end console staging, reload and provenance regression uses the actual worker and pointer input"
    )]
    fn sql_console_shared_grid_add_edit_submit_delete_and_reload()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let profile = ConnectionProfile {
            name: "Console test".into(),
            database: dir.path().join("console.sqlite").display().to_string(),
            ..Default::default()
        };
        let runtime = tokio::runtime::Runtime::new()?;
        runtime.block_on(async {
            let db = Connection::connect(&profile,None,None).await?;
            db.query("CREATE TABLE goods(id INTEGER PRIMARY KEY,type INTEGER,name TEXT DEFAULT 'new'); INSERT INTO goods VALUES(1,11,'first'),(2,12,'other'); WITH RECURSIVE n(x) AS (VALUES(3) UNION ALL SELECT x+1 FROM n WHERE x<602) INSERT INTO goods SELECT x,11,'item' FROM n",None,&CancellationToken::new()).await?;
            db.close().await; Ok::<_,nozhik_database::Error>(())
        })?;
        let bus = EventBus::new(32)?;
        let settings = ConfigManager::from_path(dir.path().join("settings.toml"), bus.clone())?;
        let mut shell = NozhikUi::new(bus, settings, Config::default());
        let ctx = egui::Context::default();
        ctx.style_mut(|s| s.animation_time = 0.0);
        frame(&mut shell, &ctx, vec![]);
        shell.database.profiles = vec![profile.clone()];
        shell.start_database_connection(profile, None, false);
        wait(&mut shell, &ctx, |s| {
            s.database.trees.contains_key("Console test")
        })?;
        let sql = "SELECT * FROM goods WHERE type=11 ORDER BY id";
        shell.new_sql_console("Console test".into(), "main".into(), sql, "Console".into());
        shell.execute_database_sql(sql.into());
        wait(&mut shell, &ctx, |s| {
            s.database.tabs[0].pending.is_none()
                && s.database.tabs[0]
                    .active_table()
                    .is_some_and(|t| !t.info.is_view)
        })?;
        assert_eq!(
            shell.database.tabs[0]
                .active_table()
                .and_then(|t| t.changes.as_ref())
                .ok_or("changes")?
                .rows
                .len(),
            601
        );
        let output = frame(&mut shell, &ctx, vec![]);
        assert!(text(&output, "WHERE").is_none());
        assert!(text(&output, "ORDER BY").is_none());
        replace_cell(&mut shell, &ctx, "first", "edited")?;
        assert!(shell.database.tabs[0].data_dirty());
        // Execute cannot discard staged edits, even through the non-UI entry point.
        shell.execute_database_sql("SELECT 1".into());
        assert!(shell.database.tabs[0].pending.is_none());
        click_label(&mut shell, &ctx, "Preview Pending Changes")?;
        assert!(text(&frame(&mut shell, &ctx, vec![]), "DML Preview").is_some());
        assert!(shell.database.table_modal());
        click_label(&mut shell, &ctx, "Cancel")?;
        click_label(&mut shell, &ctx, "Submit")?;
        wait(&mut shell, &ctx, |s| s.database.tabs[0].pending.is_none())?;
        assert_eq!(shell.database.tabs[0].error, "");
        assert_eq!(
            shell.database.tabs[0]
                .active_table()
                .and_then(|t| t.changes.as_ref())
                .ok_or("changes")?
                .rows[0]
                .values[2],
            Some(DbValue::Text("edited".into()))
        );
        click_label(&mut shell, &ctx, "+")?;
        let changes = shell.database.tabs[0]
            .active_table()
            .and_then(|t| t.changes.as_ref())
            .ok_or("changes")?;
        assert_eq!(
            changes.rows.last().ok_or("new row")?.values[1],
            Some(DbValue::Int(11))
        );
        // Changing editor text after execution must not change reload or insertion defaults.
        shell.database.tabs[0].buffer.insert(0, "-- draft\n")?;
        click_label(&mut shell, &ctx, "Submit")?;
        wait(&mut shell, &ctx, |s| s.database.tabs[0].pending.is_none())?;
        assert_eq!(shell.database.tabs[0].error, "");
        assert_eq!(
            shell.database.tabs[0]
                .active_table()
                .and_then(|t| t.changes.as_ref())
                .ok_or("changes")?
                .rows
                .len(),
            602
        );
        replace_cell(&mut shell, &ctx, "edited", "delete me")?;
        click_label(&mut shell, &ctx, "−")?;
        click_label(&mut shell, &ctx, "Submit")?;
        wait(&mut shell, &ctx, |s| s.database.tabs[0].pending.is_none())?;
        assert_eq!(shell.database.tabs[0].error, "");
        assert_eq!(
            shell.database.tabs[0]
                .active_table()
                .and_then(|t| t.changes.as_ref())
                .ok_or("changes")?
                .rows
                .len(),
            601
        );
        assert_eq!(shell.database.tabs[0].executed_sql, sql);
        shell.database.shutdown();
        Ok(())
    }
    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "Pointer-driven server selection/sorting flow checks pagination, stale errors and edit gates together"
    )]
    fn table_where_order_and_header_sort_apply_to_the_whole_selection()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let profile = ConnectionProfile {
            name: "Query test".into(),
            database: dir.path().join("query.sqlite").display().to_string(),
            ..Default::default()
        };
        let runtime = tokio::runtime::Runtime::new()?;
        let table=runtime.block_on(async {
            let db=Connection::connect(&profile,None,None).await?;
            db.query("CREATE TABLE goods(id INTEGER PRIMARY KEY,name TEXT); WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1201) INSERT INTO goods SELECT x,'item' FROM n",None,&CancellationToken::new()).await?;
            let info=db.describe_table("main","goods",false).await?; db.close().await;
            Ok::<_,nozhik_database::Error>(info)
        })?;
        let bus = EventBus::new(32)?;
        let settings = ConfigManager::from_path(dir.path().join("settings.toml"), bus.clone())?;
        let mut shell = NozhikUi::new(bus, settings, Config::default());
        let ctx = egui::Context::default();
        ctx.style_mut(|s| s.animation_time = 0.0);
        frame(&mut shell, &ctx, vec![]);
        shell.database.profiles = vec![profile.clone()];
        shell.start_database_connection(profile.clone(), None, false);
        wait(&mut shell, &ctx, |s| {
            s.database.trees.contains_key("Query test")
        })?;
        shell.open_database_table(profile, "main".into(), table);
        wait(&mut shell, &ctx, |s| s.database.tabs[0].pending.is_none())?;
        assert_eq!(
            shell.database.tabs[0]
                .table
                .as_ref()
                .ok_or("table")?
                .query
                .order,
            ""
        );
        for (order, first) in [("\"id\" ASC", 1), ("\"id\" DESC", 1201), ("", 1)] {
            click_table_sort(&mut shell, &ctx)?;
            let tab = &shell.database.tabs[0];
            assert_eq!(tab.error, "");
            let table = tab.table.as_ref().ok_or("table")?;
            assert_eq!(table.query.order, order);
            assert_eq!(table.order_input, order);
            assert_eq!(
                table.changes.as_ref().ok_or("snapshot")?.baseline.rows[0][0],
                DbValue::Int(first)
            );
        }
        set_table_clause(&mut shell, &ctx, "WHERE", "id > 700")?;
        wait(&mut shell, &ctx, |s| s.database.tabs[0].pending.is_none())?;
        set_table_clause(&mut shell, &ctx, "ORDER BY", "id DESC")?;
        wait(&mut shell, &ctx, |s| s.database.tabs[0].pending.is_none())?;
        assert_eq!(shell.database.tabs[0].error, "");
        assert_eq!(
            shell.database.tabs[0]
                .table
                .as_ref()
                .and_then(|t| t.changes.as_ref())
                .ok_or("snapshot")?
                .baseline
                .rows[0][0],
            DbValue::Int(1201)
        );
        click_label(&mut shell, &ctx, ">")?;
        wait(&mut shell, &ctx, |s| s.database.tabs[0].pending.is_none())?;
        assert!(text(&frame(&mut shell, &ctx, vec![]), "501–501").is_some());
        assert_eq!(
            shell.database.tabs[0]
                .table
                .as_ref()
                .and_then(|t| t.changes.as_ref())
                .ok_or("snapshot")?
                .baseline
                .rows[0][0],
            DbValue::Int(701)
        );
        set_table_clause(&mut shell, &ctx, "WHERE", "id < 5")?;
        wait(&mut shell, &ctx, |s| s.database.tabs[0].pending.is_none())?;
        assert!(text(&frame(&mut shell, &ctx, vec![]), "1–4").is_some());
        replace_cell(&mut shell, &ctx, "item", "edited")?;
        click_table_sort(&mut shell, &ctx)?;
        click_label(&mut shell, &ctx, "Apply")?;
        assert!(shell.database.tabs[0].is_dirty());
        assert!(shell.database.tabs[0].pending.is_none());
        assert_eq!(
            shell.database.tabs[0]
                .table
                .as_ref()
                .ok_or("table")?
                .query
                .order,
            "id DESC"
        );
        click_label(&mut shell, &ctx, "Submit")?;
        wait(&mut shell, &ctx, |s| s.database.tabs[0].pending.is_none())?;
        assert_eq!(shell.database.tabs[0].error, "");
        assert_eq!(
            shell.database.tabs[0]
                .table
                .as_ref()
                .ok_or("table")?
                .query
                .predicate,
            "id < 5"
        );
        set_table_clause(&mut shell, &ctx, "WHERE", "missing_column = 1")?;
        wait(&mut shell, &ctx, |s| s.database.tabs[0].pending.is_none())?;
        assert_ne!(shell.database.tabs[0].error, "");
        let table = shell.database.tabs[0].table.as_ref().ok_or("table")?;
        assert_eq!(table.query.predicate, "id < 5");
        assert_eq!(
            table
                .changes
                .as_ref()
                .ok_or("snapshot")?
                .baseline
                .rows
                .len(),
            4
        );
        set_table_clause(&mut shell, &ctx, "WHERE", "id < 0")?;
        wait(&mut shell, &ctx, |s| s.database.tabs[0].pending.is_none())?;
        assert_eq!(shell.database.tabs[0].error, "");
        assert!(text(&frame(&mut shell, &ctx, vec![]), "0–0").is_some());
        set_table_clause(&mut shell, &ctx, "WHERE", "")?;
        wait(&mut shell, &ctx, |s| s.database.tabs[0].pending.is_none())?;
        assert!(text(&frame(&mut shell, &ctx, vec![]), "1–500").is_some());
        assert_eq!(
            shell.database.tabs[0]
                .table
                .as_ref()
                .ok_or("table")?
                .query
                .predicate,
            ""
        );
        replace_cell(&mut shell, &ctx, "id DESC", "name ASC, id DESC")?;
        assert_eq!(
            shell.database.tabs[0]
                .table
                .as_ref()
                .ok_or("table")?
                .query
                .order,
            "id DESC"
        );
        click_label(&mut shell, &ctx, "Apply")?;
        wait(&mut shell, &ctx, |s| s.database.tabs[0].pending.is_none())?;
        assert_eq!(shell.database.tabs[0].error, "");
        assert_eq!(
            shell.database.tabs[0]
                .table
                .as_ref()
                .ok_or("table")?
                .query
                .order,
            "name ASC, id DESC"
        );
        // The last clear button belongs to ORDER BY; clearing applies immediately.
        click_label(&mut shell, &ctx, "×")?;
        wait(&mut shell, &ctx, |s| s.database.tabs[0].pending.is_none())?;
        assert_eq!(
            shell.database.tabs[0]
                .table
                .as_ref()
                .ok_or("table")?
                .query
                .order,
            ""
        );
        Ok(())
    }

    #[test]
    fn table_pagination_fetches_pages_and_preserves_snapshot_on_failure()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let profile = ConnectionProfile {
            name: "Pages".into(),
            database: dir.path().join("pages.sqlite").display().to_string(),
            ..Default::default()
        };
        let runtime = tokio::runtime::Runtime::new()?;
        let table = runtime.block_on(async {
            let db = Connection::connect(&profile, None, None).await?;
            db.query("CREATE TABLE goods(id INTEGER PRIMARY KEY, name TEXT); WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1201) INSERT INTO goods SELECT x,'item' FROM n", None, &CancellationToken::new()).await?;
            let table = db.describe_table("main", "goods", false).await?;
            db.close().await;
            Ok::<_, nozhik_database::Error>(table)
        })?;
        let bus = EventBus::new(32)?;
        let settings = ConfigManager::from_path(dir.path().join("settings.toml"), bus.clone())?;
        let mut shell = NozhikUi::new(bus, settings, Config::default());
        let ctx = egui::Context::default();
        ctx.style_mut(|style| style.animation_time = 0.0);
        frame(&mut shell, &ctx, vec![]);
        shell.database.profiles = vec![profile.clone()];
        shell.start_database_connection(profile.clone(), None, false);
        wait(&mut shell, &ctx, |s| s.database.trees.contains_key("Pages"))?;
        shell.open_database_table(profile, "main".into(), table);
        wait(&mut shell, &ctx, |s| s.database.tabs[0].pending.is_none())?;
        let output = frame(&mut shell, &ctx, vec![]);
        let range = text(&output, "1–500").ok_or("first page range")?;
        assert!(
            range.top() > 600.0 && range.bottom() < 700.0,
            "footer must stay below viewport: {range:?}"
        );
        assert!(text(&output, "of 501+").is_some());
        assert_eq!(
            shell.database.tabs[0]
                .table
                .as_ref()
                .and_then(|t| t.changes.as_ref())
                .ok_or("snapshot")?
                .baseline
                .rows
                .len(),
            500
        );
        click_label(&mut shell, &ctx, ">")?;
        wait(&mut shell, &ctx, |s| s.database.tabs[0].pending.is_none())?;
        let output = frame(&mut shell, &ctx, vec![]);
        assert!(text(&output, "501–1000").is_some());
        assert!(text(&output, "of 1001+").is_some());
        click_label(&mut shell, &ctx, ">")?;
        wait(&mut shell, &ctx, |s| s.database.tabs[0].pending.is_none())?;
        let output = frame(&mut shell, &ctx, vec![]);
        assert!(text(&output, "1001–1201").is_some());
        assert!(text(&output, "of 1201").is_some());
        click_label(&mut shell, &ctx, ">")?;
        assert!(shell.database.tabs[0].pending.is_none());
        click_label(&mut shell, &ctx, "|<")?;
        wait(&mut shell, &ctx, |s| s.database.tabs[0].pending.is_none())?;
        click_label(&mut shell, &ctx, "500 rows")?;
        click_label(&mut shell, &ctx, "100 rows")?;
        wait(&mut shell, &ctx, |s| s.database.tabs[0].pending.is_none())?;
        assert!(text(&frame(&mut shell, &ctx, vec![]), "1–100").is_some());
        replace_cell(&mut shell, &ctx, "item", "changed")?;
        click_label(&mut shell, &ctx, ">")?;
        assert!(shell.database.tabs[0].pending.is_none());
        assert!(shell.database.tabs[0].is_dirty());
        click_label(&mut shell, &ctx, "Revert")?;
        runtime.block_on(async {
            let db = Connection::connect(
                &ConnectionProfile {
                    name: "fixture".into(),
                    database: dir.path().join("pages.sqlite").display().to_string(),
                    ..Default::default()
                },
                None,
                None,
            )
            .await?;
            db.query("DROP TABLE goods", None, &CancellationToken::new())
                .await?;
            db.close().await;
            Ok::<_, nozhik_database::Error>(())
        })?;
        click_label(&mut shell, &ctx, ">")?;
        wait(&mut shell, &ctx, |s| s.database.tabs[0].pending.is_none())?;
        assert_ne!(shell.database.tabs[0].error, "");
        assert!(
            shell.database.tabs[0]
                .table
                .as_ref()
                .ok_or("table")?
                .pending_page
                .is_none()
        );
        assert!(text(&frame(&mut shell, &ctx, vec![]), "1–100").is_some());
        Ok(())
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "Pointer-driven explorer/table/DDL flow exercises the shared asynchronous UI state"
    )]
    fn table_double_click_edit_preview_revert_submit_and_ddl()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let profile = ConnectionProfile {
            name: "Table Test".into(),
            database: dir.path().join("test.sqlite").display().to_string(),
            ..Default::default()
        };
        let runtime = tokio::runtime::Runtime::new()?;
        runtime.block_on(async {let db=Connection::connect(&profile,None,None).await?;db.query("CREATE TABLE items(id INTEGER PRIMARY KEY,name TEXT DEFAULT 'default'); CREATE INDEX items_name ON items(name); INSERT INTO items VALUES(1,'first'),(2,'second');",None,&CancellationToken::new()).await?;db.close().await;Ok::<_,nozhik_database::Error>(())})?;
        let bus = EventBus::new(32)?;
        let settings = ConfigManager::from_path(dir.path().join("settings.toml"), bus.clone())?;
        let mut shell = NozhikUi::new(bus, settings, Config::default());
        let ctx = egui::Context::default();
        ctx.style_mut(|style| style.animation_time = 0.0);
        frame(&mut shell, &ctx, vec![]);
        shell.database.profiles = vec![profile.clone()];
        shell.start_database_connection(profile.clone(), None, false);
        wait(&mut shell, &ctx, |s| {
            s.database.trees.contains_key("Table Test")
        })?;
        shell.database.profiles = vec![profile.clone()];
        click_label(&mut shell, &ctx, "Table Test")?;
        click_label(&mut shell, &ctx, "main")?;
        click_label(&mut shell, &ctx, "tables")?;
        for _ in 0..40 {
            frame(&mut shell, &ctx, vec![]);
        }
        let output = frame(&mut shell, &ctx, vec![]);
        let point = text(&output, "items").ok_or("table tree item")?.center();
        click(&mut shell, &ctx, point);
        click(&mut shell, &ctx, point);
        wait(&mut shell, &ctx, |s| {
            s.database
                .tabs
                .first()
                .is_some_and(|t| t.table.as_ref().is_some_and(|v| v.changes.is_some()))
        })?;
        assert_eq!(shell.database.tabs.len(), 1);
        // Reopening the same table retains the same editing session.
        let table = shell.database.trees["Table Test"].schemas[0].tables[0].clone();
        shell.open_database_table(profile.clone(), "main".into(), table);
        assert_eq!(shell.database.tabs.len(), 1);
        let output = frame(&mut shell, &ctx, vec![]);
        if text(&output, "columns").is_none() {
            click_label(&mut shell, &ctx, "items")?;
        }
        click_label(&mut shell, &ctx, "columns")?;
        click_label(&mut shell, &ctx, "indexes")?;
        replace_cell(&mut shell, &ctx, "first", "edited")?;
        assert!(shell.database.tabs[0].is_dirty());
        assert!(shell.database.tabs[0].pending.is_none());
        let before = runtime.block_on(async {
            let db = Connection::connect(&profile, None, None).await?;
            let result = db
                .query(
                    "SELECT name FROM items WHERE id=1",
                    None,
                    &CancellationToken::new(),
                )
                .await?;
            db.close().await;
            Ok::<_, nozhik_database::Error>(result)
        })?;
        assert_eq!(before[0].rows[0][0], DbValue::Text("first".into()));
        click_label(&mut shell, &ctx, "Preview Pending Changes")?;
        let output = frame(&mut shell, &ctx, vec![]);
        assert!(text(&output, "DML Preview").is_some());
        click_label(&mut shell, &ctx, "Cancel")?;
        click_label(&mut shell, &ctx, "Revert")?;
        assert!(!shell.database.tabs[0].is_dirty());
        replace_cell(&mut shell, &ctx, "first", "submitted")?;
        click_label(&mut shell, &ctx, "Submit")?;
        wait(&mut shell, &ctx, |s| s.database.tabs[0].pending.is_none())?;
        let tab = &shell.database.tabs[0];
        assert!(tab.error.is_empty(), "{}", tab.error);
        assert!(!tab.is_dirty());
        assert_eq!(
            tab.table
                .as_ref()
                .and_then(|v| v.changes.as_ref())
                .ok_or("changes")?
                .baseline
                .rows[0][1],
            DbValue::Text("submitted".into())
        );
        click_label(&mut shell, &ctx, "+")?;
        assert!(shell.database.tabs[0].is_dirty());
        click_label(&mut shell, &ctx, "Submit")?;
        wait(&mut shell, &ctx, |s| s.database.tabs[0].pending.is_none())?;
        assert_eq!(
            shell.database.tabs[0]
                .table
                .as_ref()
                .and_then(|v| v.changes.as_ref())
                .ok_or("changes")?
                .rows
                .len(),
            3
        );
        // Adding then deleting an unsent row leaves no changes or phantom row.
        click_label(&mut shell, &ctx, "+")?;
        click_label(&mut shell, &ctx, "−")?;
        assert!(!shell.database.tabs[0].is_dirty());
        click_label(&mut shell, &ctx, "second")?;
        click_label(&mut shell, &ctx, "−")?;
        assert!(shell.close_target_dirty(crate::integration::CloseTarget::Sql(0)));
        click_label(&mut shell, &ctx, "Submit")?;
        wait(&mut shell, &ctx, |s| s.database.tabs[0].pending.is_none())?;
        assert_eq!(
            shell.database.tabs[0]
                .table
                .as_ref()
                .and_then(|v| v.changes.as_ref())
                .ok_or("changes")?
                .rows
                .len(),
            2
        );
        click_label(&mut shell, &ctx, "DDL")?;
        assert_eq!(shell.database.tabs.len(), 2);
        assert_eq!(shell.database.active, Some(1));
        assert!(shell.database.tabs[1].ddl);
        assert!(
            shell.database.tabs[1]
                .buffer
                .rope()
                .to_string()
                .contains("CREATE TABLE items")
        );
        shell.database.shutdown();
        Ok(())
    }
    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "Pointer-driven connection editing verifies persistence and protection of pending table edits"
    )]
    fn edit_connection_persists_and_never_retargets_existing_tabs()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let original = ConnectionProfile {
            name: "Editor".into(),
            database: dir.path().join("original.sqlite").display().to_string(),
            ..Default::default()
        };
        let mut edited = original.clone();
        edited.name = "Renamed".into();
        edited.database = dir.path().join("new.sqlite").display().to_string();
        let other = ConnectionProfile {
            name: "Other".into(),
            ..Default::default()
        };
        let runtime = tokio::runtime::Runtime::new()?;
        for (profile, value) in [(&original, "old data"), (&edited, "new data")] {
            runtime.block_on(async {let db=Connection::connect(profile,None,None).await?;db.query(&format!("CREATE TABLE items(id INTEGER PRIMARY KEY,name TEXT); INSERT INTO items VALUES(1,'{value}');"),None,&CancellationToken::new()).await?;db.close().await;Ok::<_,nozhik_database::Error>(())})?;
        }
        let path = dir.path().join("database_connections.json");
        nozhik_database::save_profiles(&path, &[original.clone(), other.clone()])?;
        let bus = EventBus::new(32)?;
        let settings = ConfigManager::from_path(dir.path().join("settings.toml"), bus.clone())?;
        let mut shell = NozhikUi::new(bus, settings, Config::default());
        let ctx = egui::Context::default();
        ctx.style_mut(|s| s.animation_time = 0.0);
        frame(&mut shell, &ctx, vec![]);
        wait(&mut shell, &ctx, |s| s.database.profiles.len() == 2)?;
        shell.start_database_connection(original.clone(), None, false);
        assert!(shell.status.contains("Connecting"));
        wait(&mut shell, &ctx, |s| {
            s.database.trees.contains_key("Editor")
        })?;
        assert_eq!(shell.status, "Database Editor: Connected");
        let table = shell.database.trees["Editor"].schemas[0].tables[0].clone();
        assert!(!table.details_loaded);
        shell.request_database_table("Editor".into(), "main".into(), table, true, false);
        wait(&mut shell, &ctx, |s| {
            s.database
                .tabs
                .first()
                .is_some_and(|t| t.table.as_ref().is_some_and(|v| v.changes.is_some()))
        })?;
        replace_cell(&mut shell, &ctx, "old data", "pending data")?;
        connection_menu(&mut shell, &ctx, "Editor")?;
        click_label(&mut shell, &ctx, "Edit Connection…")?;
        replace_cell(&mut shell, &ctx, "Editor", "Renamed")?;
        replace_cell(&mut shell, &ctx, &original.database, &edited.database)?;
        click_label(&mut shell, &ctx, "Save")?;
        wait(&mut shell, &ctx, |s| {
            !s.database.saving && s.database.form.is_none()
        })?;
        assert_eq!(
            nozhik_database::load_profiles(&path)?,
            vec![edited.clone(), other]
        );
        assert!(!shell.database.trees.contains_key("Editor"));
        assert!(shell.database.tabs[0].connection_invalid);
        assert!(shell.database.tabs[0].is_dirty());
        shell.submit_database_table(0);
        assert!(shell.database.tabs[0].pending.is_none());
        shell.start_database_connection(edited, None, false);
        wait(&mut shell, &ctx, |s| {
            s.database.trees.contains_key("Renamed")
        })?;
        let table = shell.database.trees["Renamed"].schemas[0].tables[0].clone();
        shell.request_database_table("Renamed".into(), "main".into(), table, true, false);
        wait(&mut shell, &ctx, |s| {
            s.database.tabs.len() == 2
                && s.database.tabs[1]
                    .table
                    .as_ref()
                    .is_some_and(|v| v.changes.is_some())
        })?;
        assert_eq!(
            shell.database.tabs[1]
                .table
                .as_ref()
                .and_then(|t| t.changes.as_ref())
                .ok_or("new grid")?
                .baseline
                .rows[0][1],
            DbValue::Text("new data".into())
        );
        assert_eq!(
            shell.database.tabs[0]
                .table
                .as_ref()
                .and_then(|t| t.changes.as_ref())
                .ok_or("old grid")?
                .rows[0]
                .values[1],
            Some(DbValue::Text("pending data".into()))
        );
        let persisted = std::fs::read(&path)?;
        connection_menu(&mut shell, &ctx, "Renamed")?;
        click_label(&mut shell, &ctx, "Edit Connection…")?;
        replace_cell(&mut shell, &ctx, "Renamed", "Other")?;
        click_label(&mut shell, &ctx, "Save")?;
        wait(&mut shell, &ctx, |s| !s.database.saving)?;
        assert!(shell.database.form.is_some());
        assert!(shell.database.error.contains("already exists"));
        assert_eq!(std::fs::read(&path)?, persisted);
        click_label(&mut shell, &ctx, "Cancel")?;
        assert!(shell.database.form.is_none());
        shell.database.shutdown();
        Ok(())
    }
}
