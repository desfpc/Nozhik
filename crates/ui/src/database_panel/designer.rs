//! Table design modal: object tree, editable properties and the shared DDL plan.
use super::{Command, NozhikUi};
use nozhik_database::{
    CancellationToken, ConnectionProfile, DbDriverType,
    schema::{Output, Task},
    table_design::{
        Column, Definition, Element, Index, IndexPart, Key, KeyKind, Properties, TableGrant,
        Trigger, creation_sql, migration_sql,
    },
};
mod forms;
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Action {
    Modify,
    Create,
    Ddl,
    Export,
    Truncate,
}
pub(super) struct Dialog {
    profile: ConnectionProfile,
    schema: String,
    target: String,
    original: Option<Definition>,
    draft: Option<Definition>,
    selected: u64,
    next_id: u64,
    pending: Option<(u64, CancellationToken, bool)>,
    close_after: bool,
    creation_attempted: bool,
    error: String,
    message: String,
}
impl Dialog {
    /// Stored table-design failure, without surrounding whitespace.
    pub(super) fn failure(&self) -> Option<String> {
        let trimmed = self.error.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_owned())
        }
    }

    pub(super) fn accepts(&self, id: u64) -> bool {
        self.pending.as_ref().is_some_and(|(i, _, _)| *i == id)
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
    pub(super) fn table_context_action(
        &mut self,
        ctx: &egui::Context,
        profile: ConnectionProfile,
        schema: String,
        table: String,
        action: Action,
    ) {
        if self.database.modal_open() {
            return;
        }
        if !matches!(action, Action::Modify | Action::Create) {
            self.table_export_action(
                ctx,
                profile,
                schema,
                table,
                if action == Action::Ddl {
                    super::schema::Action::Ddl
                } else if action == Action::Truncate {
                    super::schema::Action::Truncate
                } else {
                    super::schema::Action::Export
                },
            );
            return;
        }
        let blocked = action == Action::Modify
            && self.database.tabs.iter().any(|t| {
                t.connection == profile.name
                    && ((t.schema == schema
                        && t.table.as_ref().is_some_and(|e| e.info.name == table))
                        || t.result_editors.iter().any(|e| e.info.name == table))
                    && (t.is_dirty() || t.pending.is_some() || t.metadata_loading)
            });
        if blocked {
            self.database.error =
                "Submit or revert table edits and wait for loading before modifying its definition"
                    .into();
            return;
        }
        let draft = (action == Action::Create).then(|| Definition {
            driver: profile.driver,
            schema: schema.clone(),
            name: table.clone(),
            ..Definition::default()
        });
        self.database.table_designer = Some(Dialog {
            profile,
            schema,
            target: table.clone(),
            original: None,
            draft,
            selected: 0,
            next_id: 1,
            pending: None,
            close_after: false,
            creation_attempted: false,
            error: String::new(),
            message: String::new(),
        });
        if action == Action::Modify {
            self.queue_table_design(Task::LoadTable(table), false);
        }
        ctx.request_repaint();
    }
    fn queue_table_design(&mut self, task: Task, applying: bool) {
        let id = self.database.request_id();
        let Some(d) = &self.database.table_designer else {
            return;
        };
        if d.pending.is_some() {
            return;
        }
        let token = CancellationToken::new();
        let name = d.profile.name.clone();
        let schema = d.schema.clone();
        if self.database.queue(Command::SchemaTask {
            id,
            name,
            schema,
            task,
            cancel: token.clone(),
        }) {
            if let Some(d) = &mut self.database.table_designer {
                if applying && d.original.is_none() {
                    d.creation_attempted = true;
                    if let Some(draft) = &d.draft {
                        d.target.clone_from(&draft.name);
                    }
                }
                d.pending = Some((id, token, applying));
                d.error.clear();
                d.message.clear();
            }
        } else if let Some(d) = &mut self.database.table_designer {
            d.error.clone_from(&self.database.error);
        }
    }
    pub(super) fn table_designer_reply(
        &mut self,
        id: u64,
        result: nozhik_database::Result<Output>,
    ) {
        let Some(d) = &mut self.database.table_designer else {
            return;
        };
        if !d.accepts(id) {
            return;
        }
        let Some((_, _, applying)) = d.pending.take() else {
            return;
        };
        let mut refresh = None;
        let mut close = false;
        match result {
            Ok(Output::Table(def)) => {
                if applying {
                    refresh = Some((
                        d.profile.name.clone(),
                        d.schema.clone(),
                        d.target.clone(),
                        def.name.clone(),
                        d.original.is_none(),
                    ));
                    d.message = if d.original.is_none() {
                        "Table created"
                    } else {
                        "Table definition updated"
                    }
                    .into();
                    close = d.close_after;
                }
                d.target.clone_from(&def.name);
                d.next_id = def.next_id();
                d.original = Some((*def).clone());
                d.draft = Some(*def);
                d.selected = 0;
            }
            Ok(_) => d.error = "Unexpected table-design response".into(),
            Err(e) => {
                d.error = e.to_string();
                d.close_after = false;
                if applying && d.original.is_none() {
                    d.message = "If the table was partially created, use Reload from Server before editing it.".into();
                }
                if applying && d.original.is_some() {
                    if let Some(table) = self
                        .database
                        .trees
                        .get_mut(&d.profile.name)
                        .and_then(|t| t.schemas.iter_mut().find(|s| s.name == d.schema))
                        .and_then(|s| s.tables.iter_mut().find(|t| t.name == d.target))
                    {
                        table.details_loaded = false;
                    }
                    for t in &mut self.database.tabs {
                        if t.connection == d.profile.name
                            && t.schema == d.schema
                            && t.table.as_ref().is_some_and(|v| v.info.name == d.target)
                        {
                            t.connection_invalid = true;
                            t.error="Table DDL failed or was cancelled; refresh metadata and reopen this table before editing data".into();
                        }
                    }
                }
            }
        }
        if let Some((connection, schema, old, new, created)) = refresh {
            if created {
                self.database.queue(Command::Inspect(connection));
            } else {
                self.refresh_designed_table(&connection, &schema, &old, &new);
            }
        }
        if close {
            self.database.table_designer = None;
        }
    }
    fn refresh_designed_table(&mut self, connection: &str, schema: &str, old: &str, new: &str) {
        self.database
            .tables_pending
            .retain(|_, p| p.name != connection || p.schema != schema || p.table != old);
        let mut info = None;
        if let Some(s) = self
            .database
            .trees
            .get_mut(connection)
            .and_then(|t| t.schemas.iter_mut().find(|s| s.name == schema))
        {
            for table in &mut s.tables {
                if table.name == old {
                    table.name = new.into();
                    table.details_loaded = false;
                    info = Some(table.clone());
                }
            }
        }
        let mut tab_exists = false;
        for t in &mut self.database.tabs {
            if t.connection != connection {
                continue;
            }
            if t.schema == schema
                && let Some(table) = &mut t.table
                && table.info.name == old
            {
                table.info.name = new.into();
                table.info.details_loaded = false;
                table.changes = None;
                t.title = new.into();
                t.metadata_loading = true;
                tab_exists = true;
            }
            if t.result_editors.iter().any(|e| e.info.name == old) {
                t.result_editors.clear();
            }
        }
        self.database
            .table_errors
            .retain(|(n, s, t), _| n != connection || s != schema || t != old);
        if let Some(info) = info {
            let active = self.database.active;
            self.request_database_table(connection.into(), schema.into(), info, tab_exists, true);
            self.database.active = active;
        }
    }
    pub(super) fn table_designer_dialog(&mut self, ctx: &egui::Context) {
        let Some(dialog) = &mut self.database.table_designer else {
            return;
        };
        let mut controls = Controls::default();
        let modal = egui::Modal::new(egui::Id::new("modify_table"))
            .show(ctx, |ui| draw_dialog(ui, ctx, dialog, &mut controls));
        let sql_error = controls.sql_error.take();
        if controls.close || modal.should_close() {
            self.database.table_designer = None;
        } else if let Some((task, applying)) = controls.request {
            self.queue_table_design(task, applying);
        }
        if let Some(error) = sql_error {
            self.note_problem(
                "table-design-sql",
                "Table definition",
                crate::problems::Severity::Error,
                &error,
            );
        }
    }
}
#[derive(Default)]
struct Controls {
    close: bool,
    request: Option<(Task, bool)>,
    sql_error: Option<String>,
}
#[allow(
    clippy::too_many_lines,
    reason = "Fixed split layout, live preview and commit controls render as one modal"
)]
fn draw_dialog(ui: &mut egui::Ui, ctx: &egui::Context, d: &mut Dialog, controls: &mut Controls) {
    let busy = d.pending.is_some();
    ui.set_width((ctx.screen_rect().width() - 60.0).clamp(600.0, 1180.0));
    ui.heading("Modify Table");
    if d.original.is_none() && d.draft.is_some() {
        ui.label("Create new table");
    }
    ui.label(format!("{} / {} / {}", d.profile.name, d.schema, d.target));
    ui.separator();
    if let Some(draft) = &mut d.draft {
        let height = (ctx.screen_rect().height() * 0.45).clamp(200.0, 470.0);
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), height),
            egui::Sense::hover(),
        );
        let left = (rect.width() * 0.33).clamp(220.0, 380.0);
        let left_rect =
            egui::Rect::from_min_max(rect.min, egui::pos2(rect.left() + left, rect.bottom()));
        let right_rect =
            egui::Rect::from_min_max(egui::pos2(rect.left() + left + 12.0, rect.top()), rect.max);
        ui.painter().line_segment(
            [
                egui::pos2(rect.left() + left + 6.0, rect.top()),
                egui::pos2(rect.left() + left + 6.0, rect.bottom()),
            ],
            egui::Stroke::new(1.0_f32, crate::theme::border()),
        );
        let mut tree_ui = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("design_left")
                .max_rect(left_rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        tree_ui.set_clip_rect(ui.clip_rect().intersect(left_rect));
        if busy {
            tree_ui.disable();
        }
        draw_tree(
            &mut tree_ui,
            draft,
            d.original.as_ref(),
            &mut d.selected,
            &mut d.next_id,
            height,
        );
        let mut property_ui = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("design_right")
                .max_rect(right_rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        property_ui.set_clip_rect(ui.clip_rect().intersect(right_rect));
        if busy {
            property_ui.disable();
        }
        egui::ScrollArea::vertical()
            .id_salt(("design_properties", &d.target, d.selected))
            .max_height(height)
            .show(&mut property_ui, |ui| {
                let columns = draft
                    .elements
                    .iter()
                    .filter(|e| matches!(e.data, Properties::Column(_)))
                    .map(|e| e.name.clone())
                    .collect::<Vec<_>>();
                if d.selected == 0 {
                    forms::table(ui, draft);
                } else if let Some(e) = draft.elements.iter_mut().find(|e| e.id == d.selected) {
                    ui.push_id(e.id, |ui| forms::element(ui, draft.driver, e, &columns));
                }
            });
        ui.separator();
        ui.label("SQL preview — commands to apply");
        {
            let sql = d.original.as_ref().map_or_else(
                || creation_sql(draft),
                |original| migration_sql(original, draft),
            );
            let task = || {
                d.original.as_ref().map_or_else(
                    || Task::CreateTable(Box::new(draft.clone())),
                    |original| {
                        Task::ModifyTable(Box::new(original.clone()), Box::new(draft.clone()))
                    },
                )
            };
            match sql {
                Ok(sql) => {
                    super::schema::sql_preview_height(
                        ui,
                        &sql,
                        (ctx.screen_rect().height() - height - 220.0).clamp(80.0, 220.0),
                    );
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(!busy && !sql.is_empty(), egui::Button::new("Apply"))
                            .clicked()
                        {
                            d.close_after = false;
                            controls.request = Some((task(), true));
                        }
                        if ui.add_enabled(!busy, egui::Button::new("OK")).clicked() {
                            if sql.is_empty() {
                                controls.close = true;
                            } else {
                                d.close_after = true;
                                controls.request = Some((task(), true));
                            }
                        }
                    });
                }
                Err(error) => controls.sql_error = Some(error.to_string()),
            }
        }
        if draft.driver == DbDriverType::Mysql {
            ui.small("DDL may commit immediately; completed commands can remain after errors or cancellation.");
        }
    }
    if busy {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(if d.pending.as_ref().is_some_and(|(_, _, apply)| *apply) {
                "Applying table definition…"
            } else {
                "Loading table definition…"
            });
        });
    }
    if !d.message.is_empty() {
        ui.colored_label(crate::theme::success(), &d.message);
    }
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                !busy && (d.original.is_some() || d.creation_attempted),
                egui::Button::new("Reload from Server"),
            )
            .clicked()
        {
            controls.request = Some((Task::LoadTable(d.target.clone()), false));
        }
        if ui
            .button(if busy { "Cancel operation" } else { "Cancel" })
            .clicked()
        {
            controls.close = true;
        }
    });
}
#[allow(
    clippy::too_many_lines,
    reason = "Apply draft toolbar actions before rendering the stable-identity object tree"
)]
fn draw_tree(
    ui: &mut egui::Ui,
    draft: &mut Definition,
    original: Option<&Definition>,
    selected: &mut u64,
    next_id: &mut u64,
    height: f32,
) {
    let mut add = None;
    let mut remove = false;
    let mut duplicate = false;
    let mut move_by = 0;
    ui.horizontal(|ui| {
        ui.menu_button("+", |ui| {
            for (label, kind) in [
                ("Column", 0),
                ("Unique Key", 1),
                ("Foreign Key", 2),
                ("Index", 3),
                ("Check", 4),
                ("Generated Column", 5),
                ("Trigger", 6),
            ] {
                if ui.button(label).clicked() {
                    add = Some(kind);
                    ui.close();
                }
            }
        });
        remove = ui
            .add_enabled(*selected != 0, egui::Button::new("−"))
            .on_hover_text("Delete selected object")
            .clicked();
        duplicate = ui
            .add_enabled(*selected != 0, egui::Button::new("Duplicate"))
            .clicked();
        let can_move = (original.is_none() || draft.driver != DbDriverType::Postgres)
            && draft
                .elements
                .iter()
                .any(|e| e.id == *selected && matches!(e.data, Properties::Column(_)));
        if ui.add_enabled(can_move, egui::Button::new("Up")).clicked() {
            move_by = -1;
        }
        if ui
            .add_enabled(can_move, egui::Button::new("Down"))
            .clicked()
        {
            move_by = 1;
        }
    });
    if let Some(kind) = add {
        let id = *next_id;
        *next_id += 1;
        let mut e = new_element(id, kind);
        if draft.driver == DbDriverType::Postgres
            && let Properties::Column(c) = &mut e.data
        {
            c.stored = true;
        }
        while draft.elements.iter().any(|old| old.name == e.name) {
            e.name.push('_');
        }
        *selected = id;
        draft.elements.push(e);
    }
    if remove {
        draft.elements.retain(|e| e.id != *selected);
        *selected = 0;
    }
    if duplicate && let Some(e) = draft.elements.iter().find(|e| e.id == *selected).cloned() {
        let mut e = e;
        e.id = *next_id;
        *next_id += 1;
        e.name.push_str("_copy");
        while draft.elements.iter().any(|old| old.name == e.name) {
            e.name.push('_');
        }
        e.native_sql.clear();
        *selected = e.id;
        draft.elements.push(e);
    }
    if move_by != 0
        && let Some(at) = draft.elements.iter().position(|e| e.id == *selected)
    {
        let next = if move_by < 0 {
            (0..at)
                .rev()
                .find(|i| matches!(draft.elements[*i].data, Properties::Column(_)))
        } else {
            (at + 1..draft.elements.len())
                .find(|i| matches!(draft.elements[*i].data, Properties::Column(_)))
        };
        if let Some(other) = next {
            draft.elements.swap(at, other);
        }
    }
    egui::ScrollArea::both()
        .id_salt("design_tree")
        .max_height(height - 32.0)
        .show(ui, |ui| {
            if ui.selectable_label(*selected == 0, &draft.name).clicked() {
                *selected = 0;
            }
            for (category, label) in [
                (0, "columns"),
                (1, "keys"),
                (2, "foreign keys"),
                (3, "indexes"),
                (4, "checks"),
                (5, "triggers"),
            ] {
                egui::CollapsingHeader::new(format!(
                    "{label} {}",
                    draft
                        .elements
                        .iter()
                        .filter(|e| group(e) == category)
                        .count()
                ))
                .id_salt(category)
                .default_open(true)
                .show(ui, |ui| {
                    for e in draft.elements.iter().filter(|e| group(e) == category) {
                        let baseline =
                            original.and_then(|o| o.elements.iter().find(|o| o.id == e.id));
                        let color = if baseline.is_none() {
                            crate::theme::success()
                        } else if baseline != Some(e) {
                            crate::theme::warning()
                        } else {
                            crate::theme::text()
                        };
                        if ui
                            .selectable_label(
                                *selected == e.id,
                                egui::RichText::new(&e.name).color(color),
                            )
                            .clicked()
                        {
                            *selected = e.id;
                        }
                    }
                });
            }
        });
}
const fn group(e: &Element) -> u8 {
    match &e.data {
        Properties::Column(_) => 0,
        Properties::Key(k) => match k.kind {
            KeyKind::Foreign => 2,
            KeyKind::Check => 4,
            _ => 1,
        },
        Properties::Index(_) => 3,
        Properties::Trigger(_) => 5,
    }
}
fn new_element(id: u64, kind: i32) -> Element {
    let data = match kind {
        1 => Properties::Key(Key {
            kind: KeyKind::Unique,
            ..Key::default()
        }),
        2 => Properties::Key(Key {
            kind: KeyKind::Foreign,
            ..Key::default()
        }),
        3 => Properties::Index(Index::default()),
        4 => Properties::Key(Key {
            kind: KeyKind::Check,
            ..Key::default()
        }),
        5 => Properties::Column(Column {
            data_type: "INTEGER".into(),
            generated_sql: "0".into(),
            ..Column::default()
        }),
        6 => Properties::Trigger(Trigger {
            timing: "AFTER".into(),
            events: "INSERT".into(),
            for_each_row: true,
            enabled: "ENABLE".into(),
            ..Trigger::default()
        }),
        _ => Properties::Column(Column {
            data_type: "INTEGER".into(),
            ..Column::default()
        }),
    };
    Element {
        id,
        name: format!(
            "new_{}_{}",
            match kind {
                1 => "key",
                2 => "fk",
                3 => "index",
                4 => "check",
                6 => "trigger",
                _ => "column",
            },
            id
        ),
        comment: String::new(),
        native_sql: String::new(),
        data,
    }
}

#[cfg(test)]
mod tests {
    use super::super::table_tests::{click_label, connection_menu, frame, replace_cell, wait};
    use super::*;
    use nozhik_config::{Config, ConfigManager};
    use nozhik_database::Connection;
    use nozhik_event_bus::EventBus;
    fn has(output: &egui::FullOutput, label: &str) -> bool {
        output
            .shapes
            .iter()
            .any(|s| matches!(&s.shape,egui::Shape::Text(t) if t.galley.text().contains(label)))
    }
    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "Pointer-driven table design and immediate loading regression uses the real database worker"
    )]
    fn table_menu_design_exports_and_immediate_loading() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let profile = ConnectionProfile {
            name: "Design UI".into(),
            database: directory
                .path()
                .join("fixture.sqlite")
                .display()
                .to_string(),
            ..ConnectionProfile::default()
        };
        let runtime = tokio::runtime::Runtime::new()?;
        runtime.block_on(async{let db=Connection::connect(&profile,None,None).await?;db.query("CREATE TABLE items(id INTEGER PRIMARY KEY,name TEXT); INSERT INTO items VALUES(1,'first'),(2,'second'); CREATE TABLE unrelated(value TEXT);",None,&CancellationToken::new()).await?;db.close().await;Ok::<_,nozhik_database::Error>(())})?;
        let bus = EventBus::new(32)?;
        let settings =
            ConfigManager::from_path(directory.path().join("settings.toml"), bus.clone())?;
        let mut shell = NozhikUi::new(bus, settings, Config::default());
        let ctx = egui::Context::default();
        ctx.style_mut(|s| s.animation_time = 0.0);
        frame(&mut shell, &ctx, vec![]);
        shell.database.profiles = vec![profile.clone()];
        shell.start_database_connection(profile.clone(), None, false);
        wait(&mut shell, &ctx, |s| {
            s.database.trees.contains_key("Design UI")
        })?;
        shell.database.profiles = vec![profile.clone()];
        let info = shell.database.trees["Design UI"].schemas[0]
            .tables
            .iter()
            .find(|t| t.name == "items")
            .ok_or("items")?
            .clone();
        assert!(!info.details_loaded);
        shell.open_database_table(profile.clone(), "main".into(), info.clone());
        assert_eq!(shell.database.tabs.len(), 1);
        assert!(shell.database.tabs[0].metadata_loading);
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1200.0, 700.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    shell.database_editor(ui, true);
                });
            },
        );
        assert!(has(&output, "Loading table metadata"));
        shell.new_sql_console(
            profile.name.clone(),
            "main".into(),
            "SELECT 1",
            "Other console".into(),
        );
        wait(&mut shell, &ctx, |s| {
            s.database.tabs[0]
                .table
                .as_ref()
                .is_some_and(|t| t.changes.is_some())
        })?;
        assert_eq!(shell.database.active, Some(1));
        assert_eq!(shell.database.tabs.len(), 2);
        shell.open_database_table(profile.clone(), "main".into(), info);
        assert_eq!(shell.database.tabs.len(), 2);
        assert_eq!(shell.database.active, Some(0));
        click_label(&mut shell, &ctx, "Design UI")?;
        click_label(&mut shell, &ctx, "main")?;
        click_label(&mut shell, &ctx, "tables")?;
        connection_menu(&mut shell, &ctx, "tables")?;
        click_label(&mut shell, &ctx, "+ New Table")?;
        assert!(
            shell
                .database
                .table_designer
                .as_ref()
                .is_some_and(|d| d.original.is_none() && d.draft.is_some())
        );
        replace_cell(&mut shell, &ctx, "new_table", "created_in_ui")?;
        click_label(&mut shell, &ctx, "+")?;
        click_label(&mut shell, &ctx, "Column")?;
        let preview = frame(&mut shell, &ctx, vec![]);
        assert!(has(&preview, "CREATE TABLE"));
        click_label(&mut shell, &ctx, "Apply")?;
        wait(&mut shell, &ctx, |s| {
            s.database
                .table_designer
                .as_ref()
                .is_some_and(|d| d.pending.is_none())
        })?;
        let dialog = shell
            .database
            .table_designer
            .as_ref()
            .ok_or("create dialog")?;
        assert_eq!(dialog.error, "");
        assert!(
            dialog
                .original
                .as_ref()
                .is_some_and(|d| d.name == "created_in_ui")
        );
        click_label(&mut shell, &ctx, "OK")?;
        wait(&mut shell, &ctx, |s| {
            s.database.trees["Design UI"].schemas[0]
                .tables
                .iter()
                .any(|t| t.name == "created_in_ui")
        })?;
        // Cancel leaves a draft entirely local.
        connection_menu(&mut shell, &ctx, "tables")?;
        click_label(&mut shell, &ctx, "+ New Table")?;
        click_label(&mut shell, &ctx, "Cancel")?;
        assert!(shell.database.table_designer.is_none());
        assert!(
            !shell.database.trees["Design UI"].schemas[0]
                .tables
                .iter()
                .any(|t| t.name == "new_table")
        );
        connection_menu(&mut shell, &ctx, "items")?;
        click_label(&mut shell, &ctx, "Modify Table")?;
        wait(&mut shell, &ctx, |s| {
            s.database
                .table_designer
                .as_ref()
                .is_some_and(|d| d.draft.is_some())
        })?;
        assert!(shell.database.modal_open());
        click_label(&mut shell, &ctx, "name")?;
        replace_cell(&mut shell, &ctx, "TEXT", "VARCHAR(120)")?;
        click_label(&mut shell, &ctx, "+")?;
        click_label(&mut shell, &ctx, "Column")?;
        let added = shell
            .database
            .table_designer
            .as_ref()
            .and_then(|d| d.draft.as_ref())
            .and_then(|d| d.elements.last())
            .ok_or("new column")?
            .name
            .clone();
        replace_cell(&mut shell, &ctx, &added, "added_value")?;
        let preview = frame(&mut shell, &ctx, vec![]);
        assert!(has(&preview, "VARCHAR(120)"));
        assert!(has(&preview, "CREATE TABLE"));
        assert!(has(&preview, "added_value"));
        click_label(&mut shell, &ctx, "Apply")?;
        wait(&mut shell, &ctx, |s| {
            s.database
                .table_designer
                .as_ref()
                .is_some_and(|d| d.pending.is_none())
        })?;
        assert_eq!(
            shell
                .database
                .table_designer
                .as_ref()
                .ok_or("designer")?
                .error,
            ""
        );
        click_label(&mut shell, &ctx, "Cancel")?;
        wait(&mut shell, &ctx, |s| {
            s.database.tabs[0].table.as_ref().is_some_and(|t| {
                t.changes.is_some() && t.info.columns.iter().any(|c| c.name == "added_value")
            })
        })?;
        assert_eq!(
            shell.database.tabs[0]
                .table
                .as_ref()
                .and_then(|t| t.changes.as_ref())
                .ok_or("rows")?
                .baseline
                .rows
                .len(),
            2
        );
        connection_menu(&mut shell, &ctx, "items")?;
        click_label(&mut shell, &ctx, "Export Data to File")?;
        let output = frame(&mut shell, &ctx, vec![]);
        assert!(has(&output, "items"));
        click_label(&mut shell, &ctx, "Update Preview")?;
        wait(&mut shell, &ctx, |s| {
            s.database.schema_dialog.as_ref().is_some_and(|d| !d.busy())
        })?;
        click_label(&mut shell, &ctx, "Close")?;
        // Closing an unresolved metadata tab must not recreate it on a late reply.
        let mut placeholder = shell.database.trees["Design UI"].schemas[0]
            .tables
            .iter()
            .find(|t| t.name == "unrelated")
            .ok_or("unrelated")?
            .clone();
        placeholder.details_loaded = false;
        shell.open_database_table(profile, "main".into(), placeholder);
        let index = shell.database.active.ok_or("loading tab")?;
        assert!(shell.database.tabs[index].metadata_loading);
        assert!(!shell.database.tabs[index].is_dirty());
        shell.database.tabs.remove(index);
        shell.database.active = Some(0);
        wait(&mut shell, &ctx, |s| s.database.tables_pending.is_empty())?;
        assert_eq!(shell.database.tabs.len(), 2);
        shell.database.shutdown();
        Ok(())
    }
}
