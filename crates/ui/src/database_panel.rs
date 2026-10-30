//! Database explorer, connection dialogs and typed worker integration.
use crate::{NozhikUi, data_grid::DataGrid, sql_console::SqlConsole};
use nozhik_database::{
    CancellationToken, ConnectionProfile, DatabaseSchemaTree, DbDriverType,
    worker::{Command, Reply, Worker},
};
use nozhik_remote_ssh::{SshSession, credentials::Secret};
use std::{collections::HashMap, sync::Arc};
use zeroize::Zeroize;

#[derive(Default)]
pub struct DatabaseUi {
    worker: Option<Worker>,
    profiles: Vec<ConnectionProfile>,
    trees: HashMap<String, DatabaseSchemaTree>,
    passwords: HashMap<String, Secret>,
    pub tabs: Vec<SqlConsole>,
    pub active: Option<usize>,
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
}
struct PendingTable {
    name: String,
    schema: String,
    table: String,
    open: bool,
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
impl DatabaseUi {
    pub(crate) fn new_connection(&mut self) {
        self.form = Some(Form::default());
        self.error.clear();
    }
    pub const fn modal_open(&self) -> bool {
        self.form.is_some() || self.login.is_some()
    }
    pub fn table_modal(&self) -> bool {
        self.active
            .and_then(|i| self.tabs.get(i))
            .and_then(|t| t.table.as_ref())
            .is_some_and(crate::table_editor::TableEditor::modal_open)
    }
    pub fn shutdown(&mut self) {
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
        self.pending.remove(&id);
        self.error = error;
    }
    pub fn accept_ssh(&mut self, id: u64, session: SshSession) {
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
                        if tab.connection == old_name {
                            if let Some((_, token)) = tab.pending.take() {
                                token.cancel();
                            }
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
                    self.database.trees.remove(&name);
                    self.database.tables_pending.retain(|_, p| p.name != name);
                    self.database.table_errors.retain(|(n, _, _), _| n != &name);
                }
                Reply::Schema(name, result) => match result {
                    Ok(tree) => {
                        self.database.tables_pending.retain(|_, p| p.name != name);
                        self.database.table_errors.retain(|(n, _, _), _| n != &name);
                        self.database.trees.insert(name, tree);
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
                                if let Some(table) = &mut tab.table {
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
                                    tab.results = results.into_iter().map(DataGrid::new).collect();
                                }
                                tab.selected_result = 0;
                                tab.error.clear();
                            }
                            Err(e) => tab.error = e.to_string(),
                        }
                    }
                }
            }
        }
        // SSH trust/password dialogs must remain above the DB form while authorizing.
        if !self.workspace.background && !self.remote.modal_open() {
            self.database_dialogs(ctx);
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
        if !self.database.error.is_empty() {
            ui.colored_label(crate::theme::ERROR, &self.database.error);
        }
        egui::ScrollArea::both().show(ui, |ui| {
            for profile in &self.database.profiles {
                let tree = self.database.trees.get(&profile.name);
                egui::CollapsingHeader::new(format!(
                    "{} [{}]",
                    profile.name,
                    if self
                        .database
                        .pending
                        .values()
                        .any(|p| !p.test && p.profile.name == profile.name)
                    {
                        "Connecting…"
                    } else if tree.is_some() {
                        "Online"
                    } else {
                        "Offline"
                    }
                ))
                .id_salt((&profile.name, "db"))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(
                                !self
                                    .database
                                    .pending
                                    .values()
                                    .any(|p| p.profile.name == profile.name),
                                egui::Button::new("Connect"),
                            )
                            .clicked()
                        {
                            connect = Some(profile.clone());
                        }
                        if tree.is_some()
                            && ui
                                .small_button("↻")
                                .on_hover_text("Refresh metadata")
                                .clicked()
                        {
                            inspect = Some(profile.name.clone());
                        }
                        if tree.is_some() && ui.small_button("Disconnect").clicked() {
                            disconnect = Some(profile.name.clone());
                        }
                    });
                    if ui.button("Edit Connection…").clicked() {
                        edit = Some(profile.clone());
                    }
                    if profile.driver != DbDriverType::Sqlite
                        && ui.button("Connect with password…").clicked()
                    {
                        self.database.login = Some(profile.clone());
                    }
                    if ui.button("New SQL Console").clicked() {
                        console = Some((profile.name.clone(), String::new()));
                    }
                    if let Some(tree) = tree {
                        for schema in &tree.schemas {
                            egui::CollapsingHeader::new(&schema.name)
                                .id_salt((&profile.name, &schema.name))
                                .show(ui, |ui| {
                                    for (views, label) in [(false, "Tables"), (true, "Views")] {
                                        egui::CollapsingHeader::new(format!(
                                            "{label} {}",
                                            schema
                                                .tables
                                                .iter()
                                                .filter(|t| t.is_view == views)
                                                .count()
                                        ))
                                        .id_salt((&profile.name, &schema.name, label))
                                        .show(ui, |ui| {
                                            for table in
                                                schema.tables.iter().filter(|t| t.is_view == views)
                                            {
                                                let response =
                                                    egui::CollapsingHeader::new(&table.name)
                                                        .id_salt((
                                                            &profile.name,
                                                            &schema.name,
                                                            &table.name,
                                                        ))
                                                        .show(ui, |ui| {
                                                            if !table.details_loaded {
                                                                let key = (
                                                                    profile.name.clone(),
                                                                    schema.name.clone(),
                                                                    table.name.clone(),
                                                                );
                                                                if let Some(error) = self
                                                                    .database
                                                                    .table_errors
                                                                    .get(&key)
                                                                {
                                                                    ui.colored_label(
                                                                        crate::theme::ERROR,
                                                                        error,
                                                                    );
                                                                    if ui
                                                                        .button(
                                                                            "Retry table metadata",
                                                                        )
                                                                        .clicked()
                                                                    {
                                                                        describe.push((
                                                                            profile.name.clone(),
                                                                            schema.name.clone(),
                                                                            table.clone(),
                                                                            false,
                                                                            true,
                                                                        ));
                                                                    }
                                                                } else {
                                                                    ui.horizontal(|ui| {
                                                                ui.spinner();
                                                                ui.label("Loading table metadata…");
                                                            });
                                                                    describe.push((
                                                                        profile.name.clone(),
                                                                        schema.name.clone(),
                                                                        table.clone(),
                                                                        false,
                                                                        false,
                                                                    ));
                                                                }
                                                                return;
                                                            }
                                                            egui::CollapsingHeader::new(format!(
                                                                "columns {}",
                                                                table.columns.len()
                                                            ))
                                                            .id_salt((
                                                                "columns",
                                                                &profile.name,
                                                                &schema.name,
                                                                &table.name,
                                                            ))
                                                            .show(ui, |ui| {
                                                                for c in &table.columns {
                                                                    ui.label(format!(
                                                                        "{}{}  {}{}",
                                                                        if c.is_primary_key {
                                                                            "PK "
                                                                        } else {
                                                                            ""
                                                                        },
                                                                        c.name,
                                                                        c.data_type,
                                                                        if c.nullable {
                                                                            " ?"
                                                                        } else {
                                                                            ""
                                                                        }
                                                                    ));
                                                                }
                                                            });
                                                            if let Some(error) =
                                                                &table.objects.error
                                                            {
                                                                ui.colored_label(
                                                            crate::theme::ERROR,
                                                            format!(
                                                                "Keys/DDL unavailable: {error}"
                                                            ),
                                                        );
                                                                if ui
                                                                    .button("Retry table metadata")
                                                                    .clicked()
                                                                {
                                                                    describe.push((
                                                                        profile.name.clone(),
                                                                        schema.name.clone(),
                                                                        table.clone(),
                                                                        false,
                                                                        true,
                                                                    ));
                                                                }
                                                                return;
                                                            }
                                                            for (label, objects) in [
                                                                ("keys", &table.objects.keys),
                                                                (
                                                                    "foreign keys",
                                                                    &table.objects.foreign_keys,
                                                                ),
                                                                ("indexes", &table.objects.indexes),
                                                            ] {
                                                                egui::CollapsingHeader::new(
                                                                    format!(
                                                                        "{label} {}",
                                                                        objects.len()
                                                                    ),
                                                                )
                                                                .id_salt((
                                                                    label,
                                                                    &profile.name,
                                                                    &schema.name,
                                                                    &table.name,
                                                                ))
                                                                .show(ui, |ui| {
                                                                    for object in objects {
                                                                        ui.horizontal(|ui| {
                                                                            ui.label(&object.name);
                                                                            ui.weak(
                                                                                &object.definition,
                                                                            );
                                                                        });
                                                                    }
                                                                });
                                                            }
                                                        });
                                                if response.header_response.double_clicked() {
                                                    open = Some((
                                                        profile.clone(),
                                                        schema.name.clone(),
                                                        table.clone(),
                                                    ));
                                                }
                                                response.header_response.context_menu(|ui| {
                                                    if ui
                                                        .button("Open Table (Select top 1000)")
                                                        .clicked()
                                                    {
                                                        open = Some((
                                                            profile.clone(),
                                                            schema.name.clone(),
                                                            table.clone(),
                                                        ));
                                                        ui.close();
                                                    }
                                                    if ui.button("New SQL Console").clicked() {
                                                        console = Some((
                                                            profile.name.clone(),
                                                            schema.name.clone(),
                                                        ));
                                                        ui.close();
                                                    }
                                                });
                                            }
                                        });
                                    }
                                });
                        }
                    }
                });
            }
        });
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
            if table.details_loaded {
                self.open_database_table(profile, schema, table);
            } else {
                self.request_database_table(profile.name, schema, table, true, true);
            }
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
        let sql = nozhik_database::connection::preview_sql(profile.driver, &schema, &table.name);
        self.new_sql_console(profile.name, schema, &sql, table.name.clone());
        if let Some(index) = self.database.active
            && let Some(tab) = self.database.tabs.get_mut(index)
        {
            tab.table = Some(crate::table_editor::TableEditor::new(profile.driver, table));
        }
        self.execute_database_sql(sql);
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
        if let Some(pending) = self
            .database
            .tables_pending
            .values_mut()
            .find(|p| p.name == name && p.schema == schema && p.table == table.name)
        {
            pending.open |= open;
            return;
        }
        let key = (name.clone(), schema.clone(), table.name.clone());
        if retry {
            self.database.table_errors.remove(&key);
        }
        if self.database.table_errors.contains_key(&key) || !self.database.trees.contains_key(&name)
        {
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
                },
            );
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
                self.database.table_errors.remove(&(
                    pending.name.clone(),
                    pending.schema.clone(),
                    pending.table,
                ));
                if pending.open
                    && let Some(profile) = self
                        .database
                        .profiles
                        .iter()
                        .find(|p| p.name == pending.name)
                        .cloned()
                {
                    self.open_database_table(profile, pending.schema, info);
                }
            }
            Err(e) => {
                let error = format!("{}.{}: {e}", pending.schema, pending.table);
                self.database
                    .table_errors
                    .insert((pending.name, pending.schema, pending.table), error.clone());
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
    pub(crate) fn database_editor(&mut self, ui: &mut egui::Ui, enabled: bool) {
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
            let action = tab.table.as_mut().and_then(|t| t.action.take());
            self.editor_focus = tab.focus_id;
            self.integration.last_editor = tab.editor_id;
            let mut layout = self.layout.value;
            layout.sql = tab.split;
            self.layout.set(layout);
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
                    crate::table_editor::TableAction::Reload => self.reload_database_table(index),
                    crate::table_editor::TableAction::Submit => self.submit_database_table(index),
                    crate::table_editor::TableAction::Ddl => {
                        let tab = &self.database.tabs[index];
                        if let Some(table) = &tab.table {
                            let name = tab.connection.clone();
                            let schema = tab.schema.clone();
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
        let Some(table) = &tab.table else {
            return;
        };
        let sql =
            nozhik_database::connection::preview_sql(table.driver, &tab.schema, &table.info.name);
        self.queue_database_sql(index, sql);
    }
    pub(crate) fn submit_database_table(&mut self, index: usize) {
        let Some(tab) = self.database.tabs.get(index) else {
            return;
        };
        if tab.pending.is_some() || tab.connection_invalid {
            return;
        }
        let Some(changes) = tab.table.as_ref().and_then(|t| t.changes.clone()) else {
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
            if tab.pending.is_some() || tab.connection_invalid {
                return;
            }
            let command = Command::Query {
                id,
                name: tab.connection.clone(),
                sql,
                schema: Some(tab.schema.clone()),
                cancel: token.clone(),
            };
            if self.database.queue(command)
                && let Some(tab) = self.database.tabs.get_mut(index)
            {
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
            let modal = egui::Modal::new(egui::Id::new("new_database")).show(ctx, |ui| {
                ui.heading(if form.original.is_some() {
                    "Edit Database Connection"
                } else {
                    "New Database Connection"
                });
                ui.set_width(480.0);
                egui::ScrollArea::vertical()
                    .max_height((ctx.screen_rect().height() - 100.0).max(100.0))
                    .show(ui, |ui| {
                        ui.add_enabled_ui(!self.database.saving, |ui| {
                            ui.label("Connection name");
                            ui.text_edit_singleline(&mut form.profile.name);
                            egui::ComboBox::from_id_salt("db_driver")
                                .selected_text(format!("{:?}", form.profile.driver))
                                .show_ui(ui, |ui| {
                                    for (driver, label) in [
                                        (DbDriverType::Postgres, "PostgreSQL"),
                                        (DbDriverType::Mysql, "MySQL / MariaDB"),
                                        (DbDriverType::Sqlite, "SQLite"),
                                    ] {
                                        if ui
                                            .selectable_value(
                                                &mut form.profile.driver,
                                                driver,
                                                label,
                                            )
                                            .changed()
                                        {
                                            form.profile.port = if driver == DbDriverType::Mysql {
                                                3306
                                            } else {
                                                5432
                                            };
                                            if driver == DbDriverType::Sqlite {
                                                form.profile.ssh_profile_id = None;
                                            }
                                        }
                                    }
                                });
                            if form.profile.driver != DbDriverType::Sqlite {
                                ui.horizontal(|ui| {
                                    ui.label("Host");
                                    ui.text_edit_singleline(&mut form.profile.host);
                                    ui.label("Port");
                                    ui.add(
                                        egui::DragValue::new(&mut form.profile.port)
                                            .range(1..=65535),
                                    );
                                });
                                ui.label("User");
                                ui.text_edit_singleline(&mut form.profile.user);
                                ui.label(if form.original.is_some() {
                                    "Password (blank keeps saved password for unchanged endpoint)"
                                } else {
                                    "Password"
                                });
                                ui.add(
                                    egui::TextEdit::singleline(&mut form.password).password(true),
                                );
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
                            ui.text_edit_singleline(&mut form.profile.database);
                            if !self.database.error.is_empty() {
                                ui.label(&self.database.error);
                            }
                            if form.tested {
                                ui.colored_label(crate::theme::SUCCESS, "Test succeeded");
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
            let modal = egui::Modal::new(egui::Id::new("database_login")).show(ctx, |ui| {
                ui.heading(format!("Connect: {}", profile.name));
                ui.label("Session password");
                ui.add(egui::TextEdit::singleline(&mut self.database.password).password(true));
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

    fn frame(shell: &mut NozhikUi, ctx: &egui::Context, events: Vec<Event>) -> egui::FullOutput {
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
        output.shapes.iter().find_map(|s| match &s.shape {
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
    fn click_label(
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
    fn wait(
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
    fn replace_cell(
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
        click_label(&mut shell, &ctx, "Table Test [Online]")?;
        click_label(&mut shell, &ctx, "main")?;
        click_label(&mut shell, &ctx, "Tables 1")?;
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
        if text(&output, "columns 2").is_none() {
            click_label(&mut shell, &ctx, "items")?;
        }
        click_label(&mut shell, &ctx, "columns 2")?;
        click_label(&mut shell, &ctx, "indexes 1")?;
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
        click_label(&mut shell, &ctx, "Editor [Online]")?;
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
        click_label(&mut shell, &ctx, "Renamed [Online]")?;
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
