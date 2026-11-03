//! Schema context actions and modal forms; database and file work stays on the worker.
use super::{Command, NozhikUi};
use nozhik_database::{
    CancellationToken, ConnectionProfile, DbDriverType,
    schema::{
        Artifact, ExportOptions, Format, Grant, Output, Settings, Task, modification_sql,
        privileges,
    },
};
use std::{path::PathBuf, sync::Arc};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Action {
    Console,
    Modify,
    Ddl,
    Run,
    Export,
}
enum After {
    Load,
    Preview,
    Clipboard,
    Save(PathBuf),
    Modify,
    Run,
    Saved,
    Copied,
}
pub(super) struct Dialog {
    profile: ConnectionProfile,
    schema: String,
    table: Option<String>,
    action: Action,
    original: Option<Settings>,
    draft: Settings,
    options: ExportOptions,
    tables: Vec<String>,
    artifact: Option<Arc<Artifact>>,
    pending: Option<(u64, CancellationToken, After)>,
    error: String,
    message: String,
    output: String,
}
#[cfg(test)]
impl Dialog {
    pub(super) const fn busy(&self) -> bool {
        self.pending.is_some()
    }
}
impl Drop for Dialog {
    fn drop(&mut self) {
        if let Some((_, token, _)) = &self.pending {
            token.cancel();
        }
    }
}
impl NozhikUi {
    pub(super) fn schema_action(
        &mut self,
        ctx: &egui::Context,
        profile: ConnectionProfile,
        schema: String,
        action: Action,
    ) {
        self.start_schema_action(ctx, profile, schema, action, None);
    }
    pub(super) fn table_export_action(
        &mut self,
        ctx: &egui::Context,
        profile: ConnectionProfile,
        schema: String,
        table: String,
        action: Action,
    ) {
        self.start_schema_action(ctx, profile, schema, action, Some(table));
    }
    fn start_schema_action(
        &mut self,
        ctx: &egui::Context,
        profile: ConnectionProfile,
        schema: String,
        action: Action,
        table: Option<String>,
    ) {
        if self.database.modal_open() {
            return;
        }
        if action == Action::Console {
            self.new_sql_console(profile.name, schema, "SELECT 1;", "SQL Console".into());
            return;
        }
        let path = if action == Action::Run {
            match crate::project_dialog::pick(crate::project_dialog::Picker::File, None) {
                Ok(Some(path)) => Some(path),
                Ok(None) => return,
                Err(e) => {
                    self.database.error = e;
                    return;
                }
            }
        } else {
            None
        };
        let tables = self
            .database
            .trees
            .get(&profile.name)
            .and_then(|t| t.schemas.iter().find(|s| s.name == schema))
            .map(|s| {
                s.tables
                    .iter()
                    .filter(|t| !t.is_view)
                    .map(|t| t.name.clone())
                    .collect()
            })
            .unwrap_or_default();
        let options = ExportOptions {
            only_table: table.is_some(),
            table: table.clone().unwrap_or_default(),
            ..ExportOptions::default()
        };
        self.database.schema_dialog = Some(Dialog {
            profile,
            schema,
            table: table.clone(),
            action,
            original: None,
            draft: Settings::default(),
            options,
            tables: table.clone().map_or(tables, |t| vec![t]),
            artifact: None,
            pending: None,
            error: String::new(),
            message: String::new(),
            output: String::new(),
        });
        match action {
            Action::Modify => self.queue_schema_task(Task::Settings, After::Load),
            Action::Ddl => {
                self.queue_schema_task(table.map_or(Task::Ddl, Task::TableDdl), After::Clipboard);
            }
            Action::Run => {
                if let Some(path) = path {
                    self.queue_schema_task(Task::RunFile(path), After::Run);
                }
            }
            _ => {}
        }
        ctx.request_repaint();
    }
    fn queue_schema_task(&mut self, task: Task, after: After) {
        let id = self.database.request_id();
        let Some(dialog) = &self.database.schema_dialog else {
            return;
        };
        if dialog.pending.is_some() {
            return;
        }
        let name = dialog.profile.name.clone();
        let schema = dialog.schema.clone();
        let cancel = CancellationToken::new();
        if self.database.queue(Command::SchemaTask {
            id,
            name,
            schema,
            task,
            cancel: cancel.clone(),
        }) {
            if let Some(dialog) = &mut self.database.schema_dialog {
                dialog.pending = Some((id, cancel, after));
                dialog.error.clear();
                dialog.message.clear();
            }
        } else if let Some(dialog) = &mut self.database.schema_dialog {
            dialog.error.clone_from(&self.database.error);
        }
    }
    pub(super) fn schema_task_reply(
        &mut self,
        ctx: &egui::Context,
        id: u64,
        result: nozhik_database::Result<Output>,
    ) {
        let Some(dialog) = &mut self.database.schema_dialog else {
            return;
        };
        if dialog
            .pending
            .as_ref()
            .is_none_or(|(request, _, _)| *request != id)
        {
            return;
        }
        let Some((_, _, after)) = dialog.pending.take() else {
            return;
        };
        let mut followup = None;
        let mut refresh = None;
        let mut close = false;
        match result {
            Err(e) => dialog.error = e.to_string(),
            Ok(Output::Table(_)) => dialog.error = "Unexpected table result".into(),
            Ok(Output::Settings(settings)) => {
                dialog.draft = settings.clone();
                dialog.original = Some(settings);
            }
            Ok(Output::Clipboard(text)) => {
                ctx.copy_text(text);
                dialog.message = "Copied to clipboard".into();
                if dialog.action == Action::Ddl {
                    close = true;
                    self.status = if dialog.table.is_some() {
                        "Table DDL copied to clipboard"
                    } else {
                        "Schema DDL copied to clipboard"
                    }
                    .into();
                }
            }
            Ok(Output::Artifact(artifact)) => {
                dialog.artifact = Some(artifact.clone());
                followup = match after {
                    After::Clipboard => Some((Task::Clipboard(artifact), After::Copied)),
                    After::Save(path) => Some((Task::Save(artifact, path), After::Saved)),
                    _ => None,
                };
            }
            Ok(Output::Done(message)) => {
                self.status.clone_from(&message);
                dialog.message = message;
                if matches!(after, After::Modify | After::Run) {
                    refresh = Some(dialog.profile.name.clone());
                    close = true;
                }
            }
        }
        if let Some(name) = refresh {
            self.database.queue(Command::Inspect(name));
        }
        if close {
            self.database.schema_dialog = None;
        }
        if let Some((task, after)) = followup {
            self.queue_schema_task(task, after);
        }
    }
    #[allow(
        clippy::too_many_lines,
        reason = "One modal routes schema settings, immutable preview and export actions"
    )]
    pub(super) fn schema_dialog(&mut self, ctx: &egui::Context) {
        let Some(dialog) = &mut self.database.schema_dialog else {
            return;
        };
        let mut close = false;
        let mut request = None;
        let mut export = false;
        let busy = dialog.pending.is_some();
        let modal = egui::Modal::new(egui::Id::new("schema_actions")).show(ctx, |ui| {
            ui.set_width(900.0_f32.min(ctx.screen_rect().width() - 60.0));
            ui.heading(match dialog.action { Action::Modify => "Modify Schema", Action::Export => "Export Data", Action::Ddl => "Generate DDL to Clipboard", Action::Run => "Run SQL Script", Action::Console => "SQL Console" });
            ui.label(format!("{} / {}{}", dialog.profile.name, dialog.schema,dialog.table.as_ref().map_or(String::new(),|t|format!(" / {t}"))));
            ui.separator();
            if dialog.action == Action::Modify && let Some(original) = &dialog.original {
                egui::ScrollArea::vertical().id_salt("schema_settings").max_height(300.0).show(ui, |ui| {
                    ui.add_enabled_ui(!busy, |ui| settings_form(ui, dialog.profile.driver, &mut dialog.draft));
                });
                ui.separator(); ui.label("SQL preview — commands to execute");
                match modification_sql(dialog.profile.driver, original, &dialog.draft) {
                    Ok(sql) => {
                        sql_preview(ui, &sql);
                        if ui.add_enabled(!busy && !sql.is_empty(), egui::Button::new("Apply")).clicked() { request = Some((Task::Modify(Box::new(original.clone()), Box::new(dialog.draft.clone())), After::Modify)); }
                    }
                    Err(e) => { ui.colored_label(crate::theme::ERROR, e.to_string()); }
                }
                ui.label("DDL can commit immediately. On error or cancellation, completed commands may remain applied.");
            }
            if dialog.action == Action::Export {
                ui.columns(2, |columns| {
                    let left = &mut columns[0];
                    let before = dialog.options.clone();
                    left.add_enabled_ui(!busy, |ui| export_form(ui, &mut dialog.options, &dialog.tables));
                    if before != dialog.options { dialog.artifact = None; dialog.message.clear(); }
                    left.separator(); left.label("Output file:");
                    left.label(if dialog.output.is_empty() { "Choose with Export to File…" } else { &dialog.output });
                    if dialog.options.format == Format::Sql && dialog.profile.driver != DbDriverType::Sqlite { left.small(if dialog.table.is_some() {"Generated columns are computed when the data is restored."} else {"SQL exports the entire schema. Generated columns are computed when the data is restored."}); }
                    left.small(if dialog.table.is_some(){"Exports only this table, without other tables or schema settings."}else{"CSV exports the selected table. Choose SQL for the entire schema."});
                    if left.add_enabled(!busy, egui::Button::new("Update Preview")).clicked() { request = Some((Task::Export(dialog.options.clone()), After::Preview)); }
                    columns[1].label("Export preview:");
                    sql_preview(&mut columns[1], dialog.artifact.as_ref().map_or("Generate a preview or export directly.", |a| a.preview.as_str()));
                });
                if let Some(artifact) = &dialog.artifact { ui.label(format!("Complete export: {} bytes", artifact.bytes)); }
                ui.horizontal(|ui| {
                    if ui.add_enabled(!busy, egui::Button::new("Copy to Clipboard")).clicked() {
                        request = Some(dialog.artifact.as_ref().map_or_else(|| (Task::Export(dialog.options.clone()), After::Clipboard), |a| (Task::Clipboard(a.clone()), After::Copied)));
                    }
                    if ui.add_enabled(!busy, egui::Button::new("Export to File…")).clicked() { export = true; }
                });
            }
            if busy { ui.horizontal(|ui| { ui.spinner(); ui.label("Working…"); }); }
            if dialog.action == Action::Run { ui.label("Completed statements may remain applied if execution fails or is cancelled."); }
            if !dialog.error.is_empty() { ui.colored_label(crate::theme::ERROR, &dialog.error); }
            if !dialog.message.is_empty() { ui.label(&dialog.message); }
            if ui.button(if busy { "Cancel" } else { "Close" }).clicked() { close = true; }
        });
        close |= modal.should_close();
        if export {
            let extension = if dialog.options.format == Format::Sql {
                "sql"
            } else {
                "csv"
            };
            let stem = if dialog.options.format == Format::Csv {
                &dialog.options.table
            } else {
                dialog.table.as_ref().unwrap_or(&dialog.schema)
            };
            let safe_stem: String = stem
                .chars()
                .map(|c| {
                    if c.is_alphanumeric() || c == '_' || c == '-' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect();
            match crate::project_dialog::save(&format!("{safe_stem}.{extension}")) {
                Ok(Some(path)) => {
                    dialog.output = path.display().to_string();
                    request = Some(dialog.artifact.as_ref().map_or_else(
                        || {
                            (
                                Task::Export(dialog.options.clone()),
                                After::Save(path.clone()),
                            )
                        },
                        |a| (Task::Save(a.clone(), path.clone()), After::Saved),
                    ));
                }
                Ok(None) => {}
                Err(e) => dialog.error = e,
            }
            ctx.request_repaint();
        }
        if close {
            self.database.schema_dialog = None;
        } else if let Some((task, after)) = request {
            self.queue_schema_task(task, after);
        }
    }
}
pub(super) fn sql_preview(ui: &mut egui::Ui, sql: &str) {
    sql_preview_height(ui, sql, 280.0);
}
pub(super) fn sql_preview_height(ui: &mut egui::Ui, sql: &str, height: f32) {
    let mut job = egui::text::LayoutJob::default();
    for (range, token) in crate::sql_console::tokens(sql) {
        if let Some(text) = sql.get(range) {
            job.append(
                text,
                0.0,
                egui::TextFormat {
                    font_id: egui::FontId::monospace(13.0),
                    color: token.map_or(crate::theme::TEXT, crate::theme::syntax_color),
                    ..Default::default()
                },
            );
        }
    }
    job.wrap.max_width = f32::INFINITY;
    egui::ScrollArea::both()
        .id_salt("schema_sql_preview")
        .max_height(height)
        .show(ui, |ui| {
            ui.set_min_height(height.min(140.0));
            ui.add(egui::Label::new(job).selectable(true));
        });
}
fn settings_form(ui: &mut egui::Ui, driver: DbDriverType, draft: &mut Settings) {
    ui.horizontal(|ui| {
        ui.label("Name:");
        ui.add_enabled(
            driver == DbDriverType::Postgres,
            egui::TextEdit::singleline(&mut draft.name),
        );
    });
    match driver {
        DbDriverType::Mysql => {
            egui::ComboBox::from_label("Collation")
                .selected_text(&draft.collation)
                .show_ui(ui, |ui| {
                    for collation in &draft.collations {
                        ui.selectable_value(&mut draft.collation, collation.clone(), collation);
                    }
                });
            ui.small(
                "Changes the database default. Existing table/column collations are retained.",
            );
        }
        DbDriverType::Postgres => {
            ui.horizontal(|ui| {
                ui.label("Owner:");
                ui.text_edit_singleline(&mut draft.owner);
            });
            ui.horizontal(|ui| {
                ui.label("Comment:");
                ui.text_edit_singleline(&mut draft.comment);
            });
        }
        DbDriverType::Sqlite => {
            ui.horizontal(|ui| {
                ui.label("User version:");
                ui.add(egui::DragValue::new(&mut draft.user_version));
            });
            ui.horizontal(|ui| {
                ui.label("Application ID:");
                ui.add(egui::DragValue::new(&mut draft.application_id));
            });
            ui.small("SQLite has no schema collation, owner or GRANT privileges.");
        }
    }
    if driver != DbDriverType::Sqlite {
        grants_form(ui, driver, &mut draft.grants);
    }
}
fn grants_form(ui: &mut egui::Ui, driver: DbDriverType, grants: &mut Vec<Grant>) {
    ui.separator();
    ui.label("Grants (explicit schema privileges visible to this login)");
    let mut remove = None;
    for (index, grant) in grants.iter_mut().enumerate() {
        ui.push_id(index, |ui| {
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut grant.user)
                        .hint_text("User / role")
                        .desired_width(150.0),
                );
                if driver == DbDriverType::Mysql {
                    ui.label("@");
                    ui.add(
                        egui::TextEdit::singleline(&mut grant.host)
                            .hint_text("Host")
                            .desired_width(130.0),
                    );
                }
                egui::ComboBox::from_id_salt("privilege")
                    .selected_text(&grant.privilege)
                    .show_ui(ui, |ui| {
                        for privilege in privileges(driver) {
                            ui.selectable_value(
                                &mut grant.privilege,
                                (*privilege).into(),
                                *privilege,
                            );
                        }
                    });
                if ui.button("−").clicked() {
                    remove = Some(index);
                }
            });
        });
    }
    if let Some(index) = remove {
        grants.remove(index);
    }
    if ui.button("+ Grant").clicked() {
        grants.push(Grant {
            user: String::new(),
            host: "localhost".into(),
            privilege: privileges(driver)
                .first()
                .copied()
                .unwrap_or("USAGE")
                .into(),
        });
    }
}
fn export_form(ui: &mut egui::Ui, options: &mut ExportOptions, tables: &[String]) {
    ui.label("Extractor:");
    ui.horizontal(|ui| {
        ui.selectable_value(&mut options.format, Format::Sql, "SQL Inserts");
        ui.selectable_value(&mut options.format, Format::Csv, "CSV");
    });
    if options.format == Format::Sql {
        ui.checkbox(&mut options.ddl, "Add schema/table definitions (DDL)");
    } else {
        if options.table.is_empty()
            && let Some(first) = tables.first()
        {
            options.table.clone_from(first);
        }
        egui::ComboBox::from_label("Table")
            .selected_text(&options.table)
            .show_ui(ui, |ui| {
                for table in tables {
                    ui.selectable_value(&mut options.table, table.clone(), table);
                }
            });
        ui.checkbox(&mut options.generated, "Add computed / generated columns");
        ui.checkbox(&mut options.transpose, "Transpose");
        ui.checkbox(&mut options.column_header, "Add column header");
        ui.checkbox(&mut options.row_header, "Add row header");
        egui::ComboBox::from_label("Delimiter")
            .selected_text(if options.delimiter == '\t' {
                "Tab".into()
            } else {
                options.delimiter.to_string()
            })
            .show_ui(ui, |ui| {
                for (c, label) in [
                    (',', "Comma"),
                    (';', "Semicolon"),
                    ('\t', "Tab"),
                    ('|', "Pipe"),
                ] {
                    ui.selectable_value(&mut options.delimiter, c, label);
                }
            });
        ui.horizontal(|ui| {
            ui.label("NULL value:");
            ui.text_edit_singleline(&mut options.null);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::super::table_tests::{click_label, connection_menu, frame, wait};
    use super::*;
    use nozhik_config::{Config, ConfigManager};
    use nozhik_database::{Connection, DbValue};
    use nozhik_event_bus::EventBus;

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "One pointer-driven schema workflow verifies menu routing, worker replies, SQL preview and clipboard"
    )]
    fn schema_menu_console_modify_ddl_and_export_preview() -> Result<(), Box<dyn std::error::Error>>
    {
        let dir = tempfile::tempdir()?;
        let profile = ConnectionProfile {
            name: "Schema UI".into(),
            database: dir.path().join("test.sqlite").display().to_string(),
            ..ConnectionProfile::default()
        };
        let runtime = tokio::runtime::Runtime::new()?;
        runtime.block_on(async {let db=Connection::connect(&profile,None,None).await?;db.query("CREATE TABLE items(id INTEGER PRIMARY KEY,name TEXT); INSERT INTO items VALUES(1,'first');",None,&CancellationToken::new()).await?;db.close().await;Ok::<_,nozhik_database::Error>(())})?;
        let bus = EventBus::new(32)?;
        let settings = ConfigManager::from_path(dir.path().join("settings.toml"), bus.clone())?;
        let mut shell = NozhikUi::new(bus, settings, Config::default());
        let ctx = egui::Context::default();
        ctx.style_mut(|s| s.animation_time = 0.0);
        frame(&mut shell, &ctx, vec![]);
        shell.database.profiles = vec![profile.clone()];
        shell.start_database_connection(profile.clone(), None, false);
        wait(&mut shell, &ctx, |s| {
            s.database.trees.contains_key("Schema UI")
        })?;
        shell.database.profiles = vec![profile.clone()];
        click_label(&mut shell, &ctx, "Schema UI")?;
        connection_menu(&mut shell, &ctx, "main")?;
        click_label(&mut shell, &ctx, "New SQL console")?;
        assert_eq!(shell.database.tabs[0].schema, "main");
        connection_menu(&mut shell, &ctx, "main")?;
        click_label(&mut shell, &ctx, "Modify Schema...")?;
        wait(&mut shell, &ctx, |s| {
            s.database
                .schema_dialog
                .as_ref()
                .is_some_and(|d| d.original.is_some())
        })?;
        assert!(shell.database.modal_open());
        shell
            .database
            .schema_dialog
            .as_mut()
            .ok_or("dialog")?
            .draft
            .user_version = 21;
        let output = frame(&mut shell, &ctx, vec![]);
        assert!(output.shapes.iter().any(|s|matches!(&s.shape,egui::Shape::Text(t) if t.galley.text().contains("user_version = 21"))));
        click_label(&mut shell, &ctx, "Apply")?;
        wait(&mut shell, &ctx, |s| s.database.schema_dialog.is_none())?;
        runtime.block_on(async {
            let db = Connection::connect(&profile, None, None).await?;
            let result = db
                .query("PRAGMA user_version", None, &CancellationToken::new())
                .await?;
            assert_eq!(result[0].rows[0][0], DbValue::Int(21));
            db.close().await;
            Ok::<_, nozhik_database::Error>(())
        })?;
        connection_menu(&mut shell, &ctx, "main")?;
        click_label(&mut shell, &ctx, "Export Data to File")?;
        click_label(&mut shell, &ctx, "SQL Inserts")?;
        click_label(&mut shell, &ctx, "CSV")?;
        click_label(&mut shell, &ctx, "Update Preview")?;
        wait(&mut shell, &ctx, |s| {
            s.database
                .schema_dialog
                .as_ref()
                .is_some_and(|d| d.pending.is_none() && d.artifact.is_some())
        })?;
        assert!(
            shell
                .database
                .schema_dialog
                .as_ref()
                .and_then(|d| d.artifact.as_ref())
                .ok_or("CSV artifact")?
                .preview
                .contains("id,name\r\n1,first")
        );
        // A format change invalidates the old immutable export rather than copying stale CSV.
        click_label(&mut shell, &ctx, "CSV")?;
        click_label(&mut shell, &ctx, "SQL Inserts")?;
        assert_eq!(
            shell
                .database
                .schema_dialog
                .as_ref()
                .ok_or("export")?
                .options
                .format,
            Format::Sql
        );
        assert!(
            shell
                .database
                .schema_dialog
                .as_ref()
                .ok_or("export")?
                .artifact
                .is_none()
        );
        click_label(&mut shell, &ctx, "Close")?;
        connection_menu(&mut shell, &ctx, "main")?;
        click_label(&mut shell, &ctx, "Generate DDL to Clipboard")?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut copied = false;
        while shell.database.schema_dialog.is_some() && std::time::Instant::now() < deadline {
            let output = frame(&mut shell, &ctx, vec![]);
            copied|=output.platform_output.commands.iter().any(|c|matches!(c,egui::OutputCommand::CopyText(s) if s.contains("CREATE TABLE items")));
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(copied);
        assert!(shell.database.schema_dialog.is_none());
        shell.database.shutdown();
        Ok(())
    }
}
