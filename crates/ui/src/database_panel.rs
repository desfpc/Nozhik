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
}
struct Pending {
    profile: ConnectionProfile,
    password: Option<Secret>,
    test: bool,
}
#[derive(Default)]
struct Form {
    profile: ConnectionProfile,
    password: String,
    tested: bool,
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
                }
                Reply::Schema(name, result) => match result {
                    Ok(tree) => {
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
                                    form.tested = form.profile == pending.profile
                                        && pending
                                            .password
                                            .as_ref()
                                            .is_some_and(|s| s.expose() == form.password);
                                }
                                self.database.error = "Connection test succeeded".into();
                            } else {
                                self.database.trees.insert(name, tree);
                                self.database.error.clear();
                            }
                        }
                        Err(e) => self.database.error = e.to_string(),
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
                                tab.results = results.into_iter().map(DataGrid::new).collect();
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
        if !self.remote.modal_open() {
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
                    if tree.is_some() { "Online" } else { "Offline" }
                ))
                .id_salt((&profile.name, "db"))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if ui.button("Connect").clicked() {
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
                                        egui::CollapsingHeader::new(label)
                                            .id_salt((&profile.name, &schema.name, label))
                                            .show(ui, |ui| {
                                                for table in schema
                                                    .tables
                                                    .iter()
                                                    .filter(|t| t.is_view == views)
                                                {
                                                    let response =
                                                        egui::CollapsingHeader::new(&table.name)
                                                            .id_salt((
                                                                &profile.name,
                                                                &schema.name,
                                                                &table.name,
                                                            ))
                                                            .show(ui, |ui| {
                                                                for c in &table.columns {
                                                                    let icon = if c.is_primary_key {
                                                                        "⚿"
                                                                    } else if c
                                                                        .data_type
                                                                        .to_ascii_uppercase()
                                                                        .contains("INT")
                                                                    {
                                                                        "#"
                                                                    } else if c
                                                                        .data_type
                                                                        .to_ascii_uppercase()
                                                                        .contains("DATE")
                                                                    {
                                                                        "◷"
                                                                    } else {
                                                                        "T"
                                                                    };
                                                                    ui.label(format!(
                                                                        "{icon} {} : {}{}",
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
                                                    response.header_response.context_menu(|ui| {
                                                        if ui
                                                            .button("Open Table (Select top 1000)")
                                                            .clicked()
                                                        {
                                                            open = Some((
                                                                profile.clone(),
                                                                schema.name.clone(),
                                                                table.name.clone(),
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
            let sql = nozhik_database::connection::preview_sql(profile.driver, &schema, &table);
            self.new_sql_console(profile.name, schema, &sql, table);
            self.execute_database_sql(sql);
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
    pub(crate) fn database_tabs(&mut self, ui: &mut egui::Ui) {
        let mut close = None;
        for (index, tab) in self
            .database
            .tabs
            .iter()
            .enumerate()
            .filter(|(_, t)| t.open)
        {
            if ui
                .selectable_label(
                    self.database.active == Some(index),
                    format!("▤ {} · {}", tab.title, tab.connection),
                )
                .clicked()
            {
                self.database.active = Some(index);
            }
            if ui.small_button("×").clicked() {
                close = Some(index);
            }
        }
        if let Some(index) = close {
            self.request_close(crate::integration::CloseTarget::Sql(index), ui.ctx());
        }
    }

    pub(crate) fn database_editor(&mut self, ui: &mut egui::Ui, enabled: bool) {
        if let Some(index) = self.database.active
            && let Some(tab) = self.database.tabs.get_mut(index)
        {
            tab.split = self.layout.value.sql;
            let tree = self.database.trees.get(&tab.connection);
            let request = tab.show(ui, &self.database.profiles, tree, enabled);
            self.editor_focus = tab.focus_id;
            self.integration.last_editor = tab.editor_id;
            let mut layout = self.layout.value;
            layout.sql = tab.split;
            self.layout.set(layout);
            if let Some(sql) = request {
                self.execute_database_sql(sql);
            }
        }
    }
    fn execute_database_sql(&mut self, sql: String) {
        if sql.trim().is_empty() {
            return;
        }
        let id = self.database.request_id();
        let token = CancellationToken::new();
        if let Some(index) = self.database.active
            && let Some(tab) = self.database.tabs.get(index)
        {
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
                ui.heading("New Database Connection");
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
                                    .selectable_value(&mut form.profile.driver, driver, label)
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
                            ui.add(egui::DragValue::new(&mut form.profile.port).range(1..=65535));
                        });
                        ui.label("User");
                        ui.text_edit_singleline(&mut form.profile.user);
                        ui.label("Password");
                        ui.add(egui::TextEdit::singleline(&mut form.password).password(true));
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
                            test = Some((form.profile.clone(), Secret::new(form.password.clone())));
                        }
                        if ui
                            .add_enabled(
                                !form.profile.save_password || form.tested,
                                egui::Button::new("Save"),
                            )
                            .clicked()
                        {
                            save = Some((form.profile.clone(), Secret::new(form.password.clone())));
                        }
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                    });
                });
            });
            if form.profile != old || form.password != old_password.expose() {
                form.tested = false;
            }
            close |= modal.should_close() && !self.database.saving;
        }
        if let Some((profile, password)) = test {
            self.start_database_connection(profile, Some(password), true);
        }
        if let Some((profile, password)) = save {
            if let Err(e) = profile.validate() {
                self.database.error = e.to_string();
            } else {
                self.database.pending_save =
                    Some((profile.name.clone(), Secret::new(password.expose().into())));
                self.database.saving = self.database.queue(Command::Save(profile, Some(password)));
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
