//! Engine-aware table/object forms with structured ordered column lists.
use super::{
    Column, DbDriverType, Definition, Element, IndexPart, KeyKind, Properties, TableGrant,
};
fn field(ui: &mut egui::Ui, label: &str, value: &mut String) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.add(egui::TextEdit::singleline(value).desired_width(ui.available_width()));
    });
}
fn choice(ui: &mut egui::Ui, label: &str, value: &mut String, values: &[&str]) {
    ui.horizontal(|ui| {
        ui.label(label);
        egui::ComboBox::from_id_salt(label)
            .selected_text(if value.is_empty() {
                "Default"
            } else {
                value.as_str()
            })
            .show_ui(ui, |ui| {
                for option in values {
                    ui.selectable_value(
                        value,
                        (*option).into(),
                        if option.is_empty() { "Default" } else { option },
                    );
                }
            });
    });
}
fn extra(ui: &mut egui::Ui, value: &mut String) {
    ui.collapsing("Additional native SQL clauses", |ui| {
        ui.add(
            egui::TextEdit::multiline(value)
                .font(egui::TextStyle::Monospace)
                .desired_width(f32::INFINITY)
                .desired_rows(2),
        );
    });
}
pub(super) fn table(ui: &mut egui::Ui, t: &mut Definition) {
    ui.heading("Table properties");
    field(ui, "Name", &mut t.name);
    ui.label(format!("Schema: {}", t.schema));
    if t.driver != DbDriverType::Sqlite {
        field(ui, "Comment", &mut t.comment);
    }
    match t.driver {
        DbDriverType::Mysql => {
            field(ui, "Engine", &mut t.engine);
            field(ui, "Collation", &mut t.collation);
            choice(
                ui,
                "Row format",
                &mut t.row_format,
                &[
                    "DEFAULT",
                    "DYNAMIC",
                    "COMPACT",
                    "REDUNDANT",
                    "COMPRESSED",
                    "FIXED",
                ],
            );
            field(ui, "Next AUTO_INCREMENT", &mut t.auto_increment);
        }
        DbDriverType::Postgres => {
            ui.checkbox(&mut t.unlogged, "Unlogged persistence");
            field(ui, "Access method", &mut t.access_method);
            field(ui, "Tablespace", &mut t.tablespace);
            field(ui, "Owner", &mut t.owner);
            field(ui, "Storage options", &mut t.storage_options);
            ui.checkbox(&mut t.row_security, "Enable row-level security");
            ui.checkbox(&mut t.force_row_security, "Force row-level security");
        }
        DbDriverType::Sqlite => {
            ui.checkbox(&mut t.strict, "STRICT");
            ui.checkbox(&mut t.without_rowid, "WITHOUT ROWID");
        }
    }
    if !t.partition_sql.is_empty() {
        ui.label("Partition / inheritance layout (native definition)");
        ui.code(&t.partition_sql);
    }
    if t.driver != DbDriverType::Sqlite {
        extra(ui, &mut t.extra_sql);
        grants_form(ui, t.driver, &mut t.grants);
    }
}
#[allow(
    clippy::too_many_lines,
    reason = "Each table-object kind has an independent engine-aware property form"
)]
pub(super) fn element(ui: &mut egui::Ui, d: DbDriverType, e: &mut Element, names: &[String]) {
    field(ui, "Name", &mut e.name);
    let supports_comment = d == DbDriverType::Postgres
        || (d == DbDriverType::Mysql
            && matches!(e.data, Properties::Column(_) | Properties::Index(_)));
    if supports_comment {
        field(ui, "Comment", &mut e.comment);
    }
    match &mut e.data {
        Properties::Column(c) => column(ui, d, c),
        Properties::Key(k) => {
            match k.kind {
                KeyKind::Check => {
                    ui.label("Check expression");
                    ui.add(
                        egui::TextEdit::multiline(&mut k.expression)
                            .font(egui::TextStyle::Monospace)
                            .desired_width(f32::INFINITY)
                            .desired_rows(3),
                    );
                }
                KeyKind::Foreign => {
                    parts_form(ui, d, &mut k.columns, names, false, false);
                    if d != DbDriverType::Sqlite {
                        field(ui, "Referenced schema", &mut k.referenced_schema);
                    }
                    field(ui, "Referenced table", &mut k.referenced_table);
                    name_list(ui, "Referenced columns", &mut k.referenced_columns);
                    if d == DbDriverType::Postgres {
                        choice(ui, "Match", &mut k.match_type, &["", "SIMPLE", "FULL"]);
                    }
                    choice(
                        ui,
                        "On delete",
                        &mut k.on_delete,
                        &[
                            "",
                            "NO ACTION",
                            "RESTRICT",
                            "CASCADE",
                            "SET NULL",
                            "SET DEFAULT",
                        ],
                    );
                    choice(
                        ui,
                        "On update",
                        &mut k.on_update,
                        &[
                            "",
                            "NO ACTION",
                            "RESTRICT",
                            "CASCADE",
                            "SET NULL",
                            "SET DEFAULT",
                        ],
                    );
                }
                _ => {
                    let mut primary = k.kind == KeyKind::Primary;
                    if ui.checkbox(&mut primary, "Primary key").changed() {
                        k.kind = if primary {
                            KeyKind::Primary
                        } else {
                            KeyKind::Unique
                        };
                    }
                    if d == DbDriverType::Postgres {
                        let mut exclude = k.kind == KeyKind::Exclusion;
                        if ui.checkbox(&mut exclude, "Exclusion constraint").changed() {
                            k.kind = if exclude {
                                KeyKind::Exclusion
                            } else {
                                KeyKind::Unique
                            };
                        }
                        if exclude {
                            field(ui, "Access method", &mut k.method);
                        }
                    }
                    parts_form(
                        ui,
                        d,
                        &mut k.columns,
                        names,
                        k.kind == KeyKind::Exclusion,
                        k.kind == KeyKind::Exclusion,
                    );
                }
            }
            if (d == DbDriverType::Postgres || k.kind == KeyKind::Foreign)
                && d != DbDriverType::Mysql
                && k.kind != KeyKind::Check
            {
                ui.horizontal(|ui| {
                    ui.checkbox(&mut k.deferrable, "Deferrable");
                    ui.add_enabled_ui(k.deferrable, |ui| {
                        ui.checkbox(&mut k.deferred, "Initially deferred");
                    });
                });
            }
            extra(ui, &mut k.extra_sql);
        }
        Properties::Index(i) => {
            ui.checkbox(&mut i.unique, "Unique");
            if d != DbDriverType::Sqlite {
                field(ui, "Access method", &mut i.method);
            }
            parts_form(ui, d, &mut i.columns, names, false, true);
            if d == DbDriverType::Postgres {
                name_list(ui, "Include columns", &mut i.include_columns);
            }
            if d != DbDriverType::Mysql {
                field(ui, "Condition (WHERE)", &mut i.condition);
            }
            extra(ui, &mut i.extra_sql);
        }
        Properties::Trigger(t) => {
            choice(
                ui,
                "Timing",
                &mut t.timing,
                &["BEFORE", "AFTER", "INSTEAD OF"],
            );
            field(ui, "Events (joined with OR)", &mut t.events);
            if d != DbDriverType::Mysql {
                name_list(ui, "UPDATE OF columns", &mut t.update_columns);
            }
            ui.checkbox(&mut t.for_each_row, "For each row");
            if d != DbDriverType::Mysql {
                field(ui, "WHEN condition", &mut t.condition);
            }
            if d == DbDriverType::Postgres {
                ui.checkbox(&mut t.constraint, "Constraint trigger");
                if t.constraint {
                    ui.checkbox(&mut t.deferrable, "Deferrable");
                    ui.checkbox(&mut t.deferred, "Initially deferred");
                    field(ui, "Referenced table", &mut t.referenced_table);
                }
                choice(
                    ui,
                    "State",
                    &mut t.enabled,
                    &["ENABLE", "DISABLE", "ENABLE REPLICA", "ENABLE ALWAYS"],
                );
            }
            if d == DbDriverType::Mysql {
                ui.checkbox(&mut t.preserve_sql_mode, "Use explicit creation SQL_MODE");
                field(ui, "Creation SQL_MODE", &mut t.sql_mode);
                field(ui, "Definer (native account)", &mut t.definer_sql);
            }
            ui.label(if d == DbDriverType::Postgres {
                "Body (EXECUTE FUNCTION …)"
            } else {
                "Body (BEGIN … END or a MySQL statement)"
            });
            ui.add(
                egui::TextEdit::multiline(&mut t.body_sql)
                    .font(egui::TextStyle::Monospace)
                    .desired_rows(6)
                    .desired_width(f32::INFINITY),
            );
            extra(ui, &mut t.extra_sql);
        }
    }
}
fn column(ui: &mut egui::Ui, d: DbDriverType, c: &mut Column) {
    field(ui, "Data type", &mut c.data_type);
    egui::ComboBox::from_id_salt("type_suggestions")
        .selected_text("Common data types")
        .show_ui(ui, |ui| {
            for data_type in match d {
                DbDriverType::Mysql => &[
                    "int",
                    "bigint",
                    "bigint unsigned",
                    "varchar(255)",
                    "text",
                    "decimal(20,5)",
                    "datetime(6)",
                    "json",
                    "blob",
                ][..],
                DbDriverType::Postgres => &[
                    "integer",
                    "bigint",
                    "varchar(255)",
                    "text",
                    "numeric(20,5)",
                    "timestamp with time zone",
                    "boolean",
                    "jsonb",
                    "bytea",
                    "uuid",
                ][..],
                DbDriverType::Sqlite => &["INTEGER", "TEXT", "REAL", "BLOB", "NUMERIC", "ANY"][..],
            } {
                ui.selectable_value(&mut c.data_type, (*data_type).into(), *data_type);
            }
        });
    ui.checkbox(&mut c.not_null, "Not Null");
    field(ui, "Default expression", &mut c.default_sql);
    field(ui, "Collation", &mut c.collation);
    if d == DbDriverType::Postgres {
        let before = c.identity.clone();
        choice(
            ui,
            "Identity kind",
            &mut c.identity,
            &["", "ALWAYS", "BY DEFAULT"],
        );
        if before != c.identity && !c.identity.is_empty() {
            c.default_sql.clear();
        }
        if !c.identity.is_empty() {
            sequence_options(ui, &mut c.identity_options);
            if !c.identity_sequence.is_empty() {
                ui.small(format!("Sequence: {}", c.identity_sequence));
            }
        }
        field(ui, "Type conversion (USING)", &mut c.using_sql);
    } else {
        ui.checkbox(&mut c.auto_increment, "Auto increment");
        if d == DbDriverType::Sqlite {
            ui.checkbox(&mut c.primary, "Primary key");
            ui.checkbox(&mut c.unique, "Unique");
        }
    }
    field(ui, "Generated expression", &mut c.generated_sql);
    if !c.generated_sql.is_empty() {
        ui.checkbox(&mut c.stored, "Stored generated column");
    }
    extra(ui, &mut c.extra_sql);
}
fn parts_form(
    ui: &mut egui::Ui,
    d: DbDriverType,
    values: &mut Vec<IndexPart>,
    names: &[String],
    exclusion: bool,
    index_options: bool,
) {
    ui.separator();
    ui.label("Columns / expressions");
    let mut remove = None;
    let mut swap = None;
    let len = values.len();
    for (index, p) in values.iter_mut().enumerate() {
        ui.push_id(index, |ui| {
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut p.expression)
                        .hint_text("Column or expression")
                        .desired_width((ui.available_width() - 100.0).max(100.0)),
                );
                egui::ComboBox::from_id_salt("column_choices")
                    .selected_text("Choose")
                    .show_ui(ui, |ui| {
                        for name in names {
                            ui.selectable_value(
                                &mut p.expression,
                                nozhik_database::connection::quote_identifier(d, name),
                                name,
                            );
                        }
                    });
                if ui.button("−").clicked() {
                    remove = Some(index);
                }
            });
            ui.horizontal(|ui| {
                if ui.add_enabled(index > 0, egui::Button::new("Up")).clicked() {
                    swap = Some((index, index - 1));
                }
                if ui
                    .add_enabled(index + 1 < len, egui::Button::new("Down"))
                    .clicked()
                {
                    swap = Some((index, index + 1));
                }
                if index_options {
                    choice(
                        ui,
                        "Order",
                        &mut p.order,
                        &[
                            "",
                            "ASC",
                            "DESC",
                            "ASC NULLS FIRST",
                            "ASC NULLS LAST",
                            "DESC NULLS FIRST",
                            "DESC NULLS LAST",
                        ],
                    );
                }
            });
            if index_options && (d == DbDriverType::Postgres || d == DbDriverType::Sqlite) {
                field(ui, "Collation", &mut p.collation);
            }
            if index_options && d == DbDriverType::Postgres {
                field(ui, "Operator class", &mut p.operator_class);
            }
            if exclusion {
                field(ui, "Exclude operator", &mut p.exclude_operator);
            }
        });
    }
    if let Some(index) = remove {
        values.remove(index);
    }
    if let Some((a, b)) = swap {
        values.swap(a, b);
    }
    if ui.button("+ Column").clicked() {
        values.push(IndexPart {
            expression: names.first().map_or(String::new(), |n| {
                nozhik_database::connection::quote_identifier(d, n)
            }),
            ..IndexPart::default()
        });
    }
}
fn name_list(ui: &mut egui::Ui, label: &str, values: &mut Vec<String>) {
    ui.push_id(label, |ui| {
        ui.label(label);
        let mut remove = None;
        for (index, name) in values.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.text_edit_singleline(name);
                if ui.button("−").clicked() {
                    remove = Some(index);
                }
            });
        }
        if let Some(index) = remove {
            values.remove(index);
        }
        if ui.button("+ Name").clicked() {
            values.push(String::new());
        }
    });
}
fn sequence_options(ui: &mut egui::Ui, value: &mut String) {
    let ts = value.split_whitespace().collect::<Vec<_>>();
    let read = |word: &str, offset: usize| {
        ts.iter()
            .position(|s| s.eq_ignore_ascii_case(word))
            .and_then(|i| ts.get(i + offset))
            .map_or(String::new(), |s| (*s).into())
    };
    let mut fields = [
        ("Start", "START WITH", read("START", 2)),
        ("Step", "INCREMENT BY", read("INCREMENT", 2)),
        ("Min", "MINVALUE", read("MINVALUE", 1)),
        ("Max", "MAXVALUE", read("MAXVALUE", 1)),
        ("Cache size", "CACHE", read("CACHE", 1)),
    ];
    let mut cycle = ts.iter().enumerate().any(|(i, s)| {
        s.eq_ignore_ascii_case("CYCLE")
            && !i
                .checked_sub(1)
                .and_then(|j| ts.get(j))
                .is_some_and(|s| s.eq_ignore_ascii_case("NO"))
    });
    let mut changed = false;
    for (label, _, v) in &mut fields {
        ui.horizontal(|ui| {
            ui.label(*label);
            changed |= ui.text_edit_singleline(v).changed();
        });
    }
    changed |= ui.checkbox(&mut cycle, "Cycled").changed();
    if changed {
        *value = fields
            .iter()
            .filter(|(_, _, v)| !v.is_empty())
            .map(|(_, key, v)| format!("{key} {v}"))
            .chain(std::iter::once(if cycle {
                "CYCLE".into()
            } else {
                "NO CYCLE".into()
            }))
            .collect::<Vec<_>>()
            .join(" ");
    }
}
fn grants_form(ui: &mut egui::Ui, d: DbDriverType, grants: &mut Vec<TableGrant>) {
    ui.separator();
    ui.label("Grants (explicit table privileges)");
    let mut remove = None;
    for (index, g) in grants.iter_mut().enumerate() {
        ui.push_id(index, |ui| {
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut g.user)
                        .hint_text("User / role")
                        .desired_width(120.0),
                );
                if d == DbDriverType::Mysql {
                    ui.label("@");
                    ui.add(egui::TextEdit::singleline(&mut g.host).desired_width(90.0));
                }
                egui::ComboBox::from_id_salt("table_privilege")
                    .selected_text(&g.privilege)
                    .show_ui(ui, |ui| {
                        for privilege in [
                            "SELECT",
                            "INSERT",
                            "UPDATE",
                            "DELETE",
                            "REFERENCES",
                            "TRIGGER",
                        ] {
                            ui.selectable_value(&mut g.privilege, privilege.into(), privilege);
                        }
                    });
                if ui.button("−").clicked() {
                    remove = Some(index);
                }
            });
            ui.checkbox(&mut g.grant_option, "With grant option");
        });
    }
    if let Some(index) = remove {
        grants.remove(index);
    }
    if ui.button("+ Grant").clicked() {
        grants.push(TableGrant {
            host: "localhost".into(),
            privilege: "SELECT".into(),
            ..TableGrant::default()
        });
    }
}
