//! One migration planner supplies both the preview and the executed statements.
use super::{Definition, Element, IndexPart, KeyKind, Properties, Result, invalid, sql};
use crate::{
    CancellationToken, Connection, DbDriverType, Error,
    connection::{Pool, quote_identifier},
};
use sqlx::Connection as _;
use std::{
    collections::HashSet,
    fmt::Write as _,
    hash::{Hash, Hasher},
};

pub(super) fn q(d: DbDriverType, s: &str) -> String {
    quote_identifier(d, s)
}
fn text(d: DbDriverType, s: &str) -> String {
    if d == DbDriverType::Postgres {
        format!("E'{}'", s.replace('\\', "\\\\").replace('\'', "''"))
    } else {
        format!("'{}'", s.replace('\'', "''"))
    }
}
fn qualified(d: DbDriverType, s: &str) -> Result<String> {
    let ts = sql::tokens(s)?;
    if ts.is_empty()
        || ts.iter().any(|t| {
            t.literal
                || (!t.quoted
                    && t.value
                        .chars()
                        .any(|c| !c.is_alphanumeric() && c != '_' && c != '$' && c != '.'))
        })
    {
        return Err(invalid("Expected a collation/sequence identifier"));
    }
    Ok(ts
        .into_iter()
        .map(|t| if t.is(".") { t.value } else { q(d, &t.value) })
        .collect::<String>())
}
fn parts(d: DbDriverType, columns: &[IndexPart]) -> Result<String> {
    if columns.is_empty() {
        return Err(invalid("Choose at least one column or expression"));
    }
    columns
        .iter()
        .map(|p| {
            sql::fragment(&p.expression)?;
            if p.expression.trim().is_empty() {
                return Err(invalid("Choose an index/key column or expression"));
            }
            let mut s = p.expression.clone();
            if !p.collation.is_empty() {
                write!(s, " COLLATE {}", qualified(d, &p.collation)?).ok();
            }
            if !p.operator_class.is_empty() {
                sql::fragment(&p.operator_class)?;
                write!(s, " {}", p.operator_class).ok();
            }
            if !p.order.is_empty() {
                if ![
                    "ASC",
                    "DESC",
                    "ASC NULLS FIRST",
                    "ASC NULLS LAST",
                    "DESC NULLS FIRST",
                    "DESC NULLS LAST",
                    "NULLS FIRST",
                    "NULLS LAST",
                ]
                .contains(&p.order.as_str())
                {
                    return Err(invalid("Invalid index sort order"));
                }
                write!(s, " {}", p.order).ok();
            }
            if !p.exclude_operator.is_empty() {
                sql::fragment(&p.exclude_operator)?;
                write!(s, " WITH {}", p.exclude_operator).ok();
            }
            Ok(s)
        })
        .collect::<Result<Vec<_>>>()
        .map(|s| s.join(", "))
}
pub(super) fn column(d: DbDriverType, e: &Element) -> Result<String> {
    let Properties::Column(c) = &e.data else {
        return Err(invalid("Column expected"));
    };
    for s in [
        &c.data_type,
        &c.default_sql,
        &c.generated_sql,
        &c.extra_sql,
        &c.using_sql,
        &c.identity_options,
    ] {
        sql::fragment(s)?;
    }
    if c.data_type.trim().is_empty() {
        return Err(invalid("Column data type is required"));
    }
    if !c.generated_sql.is_empty()
        && (!c.default_sql.is_empty() || !c.identity.is_empty() || c.auto_increment)
    {
        return Err(invalid(
            "A generated column cannot also have a default or identity",
        ));
    }
    let mut out = format!("{} {}", q(d, &e.name), c.data_type);
    if !c.collation.is_empty() {
        write!(out, " COLLATE {}", qualified(d, &c.collation)?).ok();
    }
    if !c.generated_sql.is_empty() {
        write!(
            out,
            " GENERATED ALWAYS AS ({}) {}",
            c.generated_sql,
            if c.stored { "STORED" } else { "VIRTUAL" }
        )
        .ok();
    } else if !c.identity.is_empty() {
        if d != DbDriverType::Postgres || !["ALWAYS", "BY DEFAULT"].contains(&c.identity.as_str()) {
            return Err(invalid("Invalid identity kind"));
        }
        write!(out, " GENERATED {} AS IDENTITY", c.identity).ok();
        if !c.identity_options.is_empty() {
            write!(out, " ({})", c.identity_options).ok();
        }
    } else if !c.default_sql.is_empty() {
        write!(out, " DEFAULT {}", c.default_sql).ok();
    }
    if c.not_null {
        out.push_str(" NOT NULL");
    }
    if c.primary {
        out.push_str(" PRIMARY KEY");
    }
    if c.unique {
        out.push_str(" UNIQUE");
    }
    if c.auto_increment {
        out.push_str(if d == DbDriverType::Mysql {
            " AUTO_INCREMENT"
        } else if d == DbDriverType::Sqlite {
            " AUTOINCREMENT"
        } else {
            return Err(invalid("Use PostgreSQL identity instead of AUTO_INCREMENT"));
        });
    }
    if d == DbDriverType::Mysql && !e.comment.is_empty() {
        write!(out, " COMMENT {}", text(d, &e.comment)).ok();
    }
    if !c.extra_sql.is_empty() {
        write!(out, " {}", c.extra_sql).ok();
    }
    Ok(out)
}
#[allow(
    clippy::too_many_lines,
    reason = "Constraint categories share identifier quoting, ordered columns and deferrability"
)]
pub(super) fn key(d: DbDriverType, e: &Element) -> Result<String> {
    let Properties::Key(k) = &e.data else {
        return Err(invalid("Key expected"));
    };
    let mut s = if d == DbDriverType::Mysql && k.kind == KeyKind::Primary {
        String::new()
    } else {
        format!("CONSTRAINT {} ", q(d, &e.name))
    };
    match k.kind {
        KeyKind::Primary => write!(s, "PRIMARY KEY ({})", parts(d, &k.columns)?).ok(),
        KeyKind::Unique => write!(s, "UNIQUE ({})", parts(d, &k.columns)?).ok(),
        KeyKind::Check => {
            sql::fragment(&k.expression)?;
            if k.expression.trim().is_empty() {
                return Err(invalid("Check expression is required"));
            }
            write!(s, "CHECK ({})", k.expression).ok()
        }
        KeyKind::Exclusion => {
            if d != DbDriverType::Postgres {
                return Err(invalid("Exclusion constraints require PostgreSQL"));
            }
            write!(
                s,
                "EXCLUDE {} ({})",
                if k.method.is_empty() {
                    String::new()
                } else {
                    format!("USING {}", qualified(d, &k.method)?)
                },
                parts(d, &k.columns)?
            )
            .ok()
        }
        KeyKind::Foreign => {
            if k.referenced_table.is_empty() {
                return Err(invalid("Referenced table is required"));
            }
            let target = if k.referenced_schema.is_empty() {
                q(d, &k.referenced_table)
            } else {
                format!(
                    "{}.{}",
                    q(d, &k.referenced_schema),
                    q(d, &k.referenced_table)
                )
            };
            write!(
                s,
                "FOREIGN KEY ({}) REFERENCES {target}",
                parts(d, &k.columns)?
            )
            .ok();
            if !k.referenced_columns.is_empty() {
                write!(
                    s,
                    " ({})",
                    k.referenced_columns
                        .iter()
                        .map(|n| q(d, n))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
                .ok();
            }
            if !k.match_type.is_empty() {
                if !["SIMPLE", "FULL", "PARTIAL"].contains(&k.match_type.as_str()) {
                    return Err(invalid("Invalid foreign-key MATCH type"));
                }
                write!(s, " MATCH {}", k.match_type).ok();
            }
            for (name, action) in [("DELETE", &k.on_delete), ("UPDATE", &k.on_update)] {
                if !action.is_empty() {
                    if ![
                        "NO ACTION",
                        "RESTRICT",
                        "CASCADE",
                        "SET NULL",
                        "SET DEFAULT",
                    ]
                    .contains(&action.as_str())
                    {
                        return Err(invalid("Invalid foreign-key action"));
                    }
                    write!(s, " ON {name} {action}").ok();
                }
            }
            Some(())
        }
    };
    let mut extras = k.extra_sql.clone();
    sql::fragment(&extras)?;
    let not_valid = sql::tokens(&extras)?
        .windows(2)
        .find(|w| w[0].is("NOT") && w[1].is("VALID"))
        .map(|w| w[0].range.start..w[1].range.end);
    let is_not_valid = not_valid.is_some();
    if let Some(range) = not_valid {
        extras.replace_range(range, "");
    }
    if !extras.trim().is_empty() {
        write!(s, " {}", extras.trim()).ok();
    }
    if k.deferrable {
        if d == DbDriverType::Mysql || (d == DbDriverType::Sqlite && k.kind != KeyKind::Foreign) {
            return Err(invalid(
                "This engine only supports the selected deferrability for foreign keys",
            ));
        }
        s.push_str(" DEFERRABLE");
        if k.deferred {
            s.push_str(" INITIALLY DEFERRED");
        }
    }
    if is_not_valid {
        s.push_str(" NOT VALID");
    }
    Ok(s)
}
pub(super) fn index(def: &Definition, e: &Element, alter: bool) -> Result<String> {
    let Properties::Index(i) = &e.data else {
        return Err(invalid("Index expected"));
    };
    let d = def.driver;
    let special = d == DbDriverType::Mysql && ["FULLTEXT", "SPATIAL"].contains(&i.method.as_str());
    let qualifier = if special {
        format!("{} ", i.method)
    } else if i.unique {
        "UNIQUE ".into()
    } else {
        String::new()
    };
    let mut s = if alter {
        format!("ADD {qualifier}INDEX {}", q(d, &e.name))
    } else {
        format!(
            "CREATE {qualifier}INDEX {} ON {}",
            if d == DbDriverType::Sqlite {
                format!("{}.{}", q(d, &def.schema), q(d, &e.name))
            } else {
                q(d, &e.name)
            },
            if d == DbDriverType::Sqlite {
                q(d, &def.name)
            } else {
                def.qualified_name()
            }
        )
    };
    if !i.method.is_empty() && !special {
        if d == DbDriverType::Sqlite {
            return Err(invalid("SQLite chooses the index method"));
        }
        write!(s, " USING {}", qualified(d, &i.method)?).ok();
    }
    write!(s, " ({})", parts(d, &i.columns)?).ok();
    if !i.include_columns.is_empty() {
        if d != DbDriverType::Postgres {
            return Err(invalid("INCLUDE columns require PostgreSQL"));
        }
        write!(
            s,
            " INCLUDE ({})",
            i.include_columns
                .iter()
                .map(|n| q(d, n))
                .collect::<Vec<_>>()
                .join(", ")
        )
        .ok();
    }
    if !i.extra_sql.is_empty() {
        sql::fragment(&i.extra_sql)?;
        write!(s, " {}", i.extra_sql).ok();
    }
    if !i.condition.is_empty() {
        if d == DbDriverType::Mysql {
            return Err(invalid("MySQL does not support partial indexes"));
        }
        sql::fragment(&i.condition)?;
        write!(s, " WHERE {}", i.condition).ok();
    }
    if d == DbDriverType::Mysql && !e.comment.is_empty() {
        write!(s, " COMMENT {}", text(d, &e.comment)).ok();
    }
    Ok(s)
}
#[allow(
    clippy::too_many_lines,
    reason = "Trigger creation must retain engine-specific modes, constraint timing and activation"
)]
pub(super) fn trigger(def: &Definition, e: &Element) -> Result<Vec<String>> {
    let Properties::Trigger(t) = &e.data else {
        return Err(invalid("Trigger expected"));
    };
    let d = def.driver;
    if !["BEFORE", "AFTER", "INSTEAD OF"].contains(&t.timing.as_str()) {
        return Err(invalid("Choose a trigger timing"));
    }
    let mut events = t.events.clone();
    for event in events.split(" OR ") {
        if !["INSERT", "UPDATE", "DELETE", "TRUNCATE"].contains(&event.trim()) {
            return Err(invalid("Choose valid trigger events"));
        }
    }
    if !t.update_columns.is_empty() {
        events = format!(
            "UPDATE OF {}",
            t.update_columns
                .iter()
                .map(|n| q(d, n))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    if t.body_sql.trim().is_empty() {
        return Err(invalid("Trigger body is required"));
    }
    let mut s = format!(
        "CREATE {}{}TRIGGER {} {} {events} ON {}",
        if d == DbDriverType::Mysql && !t.definer_sql.is_empty() {
            sql::fragment(&t.definer_sql)?;
            format!("DEFINER={} ", t.definer_sql)
        } else {
            String::new()
        },
        if d == DbDriverType::Postgres && t.constraint {
            "CONSTRAINT "
        } else {
            ""
        },
        if d == DbDriverType::Postgres {
            q(d, &e.name)
        } else {
            format!("{}.{}", q(d, &def.schema), q(d, &e.name))
        },
        t.timing,
        if d == DbDriverType::Sqlite {
            q(d, &def.name)
        } else {
            def.qualified_name()
        }
    );
    if d == DbDriverType::Postgres {
        if !t.referenced_table.is_empty() {
            write!(s, " FROM {}", qualified(d, &t.referenced_table)?).ok();
        }
        if t.constraint && t.deferrable {
            s.push_str(" DEFERRABLE");
            if t.deferred {
                s.push_str(" INITIALLY DEFERRED");
            }
        }
        if !t.extra_sql.is_empty() {
            sql::fragment(&t.extra_sql)?;
            write!(s, " {}", t.extra_sql).ok();
        }
    }
    if t.for_each_row || d == DbDriverType::Mysql {
        s.push_str(" FOR EACH ROW");
    } else if d == DbDriverType::Postgres {
        s.push_str(" FOR EACH STATEMENT");
    }
    if !t.condition.is_empty() {
        sql::fragment(&t.condition)?;
        write!(s, " WHEN ({})", t.condition).ok();
    }
    if d != DbDriverType::Postgres && !t.extra_sql.is_empty() {
        sql::fragment(&t.extra_sql)?;
        write!(s, " {}", t.extra_sql).ok();
    }
    write!(s, " {}", t.body_sql.trim().trim_end_matches(';')).ok();
    let mut out = Vec::new();
    if d == DbDriverType::Mysql && (t.preserve_sql_mode || !t.sql_mode.is_empty()) {
        if !t
            .sql_mode
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ',')
        {
            return Err(invalid("Invalid trigger SQL mode"));
        }
        out.push(format!("SET SQL_MODE='{}'", t.sql_mode));
    }
    out.push(s);
    if d == DbDriverType::Mysql {
        out.push(mysql_mode().into());
    }
    if d == DbDriverType::Postgres && !t.enabled.is_empty() {
        if !["ENABLE", "DISABLE", "ENABLE REPLICA", "ENABLE ALWAYS"].contains(&t.enabled.as_str()) {
            return Err(invalid("Invalid trigger state"));
        }
        out.push(format!(
            "ALTER TABLE {} {} TRIGGER {}",
            def.qualified_name(),
            t.enabled,
            q(d, &e.name)
        ));
    }
    Ok(out)
}
pub(super) const fn mysql_mode() -> &'static str {
    "SET SQL_MODE='STRICT_ALL_TABLES,NO_AUTO_VALUE_ON_ZERO,NO_BACKSLASH_ESCAPES'"
}
fn changed(old: &Element, new: &Element) -> bool {
    old.name != new.name || old.data != new.data
}
pub(super) fn validate(old: &Definition, new: &Definition) -> Result<()> {
    if old.driver != new.driver || old.schema != new.schema {
        return Err(invalid(
            "The connection/schema of a table edit cannot change",
        ));
    }
    for name in [&new.schema, &new.name] {
        if name.is_empty() || name.chars().any(char::is_control) {
            return Err(invalid(
                "Table/schema name is required, without control characters",
            ));
        }
    }
    let mut ids = HashSet::new();
    let mut names = HashSet::new();
    for e in &new.elements {
        if !ids.insert(e.id) {
            return Err(invalid("Duplicate draft identity"));
        }
        if e.name.is_empty() || e.name.chars().any(char::is_control) {
            return Err(invalid("Every object needs a valid name"));
        }
        let category = match e.data {
            Properties::Column(_) => 0,
            Properties::Key(_) => 1,
            Properties::Index(_) => 2,
            Properties::Trigger(_) => 3,
        };
        if !names.insert((
            category,
            if new.driver == DbDriverType::Postgres {
                e.name.clone()
            } else {
                e.name.to_ascii_lowercase()
            },
        )) {
            return Err(invalid(format!("Duplicate object name: {}", e.name)));
        }
    }
    if new.driver != DbDriverType::Postgres
        && !new
            .elements
            .iter()
            .any(|e| matches!(e.data, Properties::Column(_)))
    {
        return Err(invalid("A table must have at least one column"));
    }
    validate_dependencies(old, new)?;
    if old.partition_sql != new.partition_sql {
        return Err(invalid(
            "Partition/inheritance layout must be changed with an explicit SQL migration",
        ));
    }
    Ok(())
}
fn validate_dependencies(old: &Definition, new: &Definition) -> Result<()> {
    let removed = old
        .elements
        .iter()
        .filter(|e| {
            matches!(e.data, Properties::Column(_)) && !new.elements.iter().any(|n| n.id == e.id)
        })
        .map(|e| e.name.as_str())
        .collect::<Vec<_>>();
    if removed.is_empty() {
        return Ok(());
    }
    for e in &new.elements {
        let baseline = old.elements.iter().find(|o| o.id == e.id);
        let expressions: Vec<&str> = match &e.data {
            Properties::Index(i) => i
                .columns
                .iter()
                .map(|p| p.expression.as_str())
                .chain(std::iter::once(i.condition.as_str()))
                .collect(),
            Properties::Key(k) => k
                .columns
                .iter()
                .map(|p| p.expression.as_str())
                .chain(std::iter::once(k.expression.as_str()))
                .collect(),
            Properties::Column(c) => vec![&c.generated_sql],
            Properties::Trigger(t) => vec![&t.body_sql, &t.condition],
        };
        for expression in expressions {
            let ts = if new.driver == DbDriverType::Mysql
                && let Properties::Trigger(t) = &e.data
            {
                sql::lex(
                    expression,
                    t.preserve_sql_mode
                        && !t.sql_mode.split(',').any(|s| s == "NO_BACKSLASH_ESCAPES"),
                )?
            } else {
                sql::tokens(expression)?
            };
            for (at, t) in ts.iter().enumerate() {
                if t.literal
                    || !removed.iter().any(|name| {
                        if new.driver == DbDriverType::Postgres {
                            *name == t.value
                        } else {
                            name.eq_ignore_ascii_case(&t.value)
                        }
                    })
                {
                    continue;
                }
                if matches!(e.data, Properties::Trigger(_))
                    && !(at >= 2
                        && ts[at - 1].is(".")
                        && (ts[at - 2].is("NEW") || ts[at - 2].is("OLD")))
                {
                    continue;
                }
                if !t.quoted && ts.get(at + 1).is_some_and(|t| t.is("(")) {
                    continue;
                }
                let replacement_exists = new
                    .elements
                    .iter()
                    .any(|c| matches!(c.data, Properties::Column(_)) && c.name == t.value);
                if !replacement_exists || baseline.is_some_and(|o| o.data == e.data) {
                    return Err(invalid(format!(
                        "{} references removed column {}; edit or remove that dependent object in the same draft",
                        e.name, t.value
                    )));
                }
            }
        }
    }
    Ok(())
}
pub(super) struct Plan {
    pub(super) steps: Vec<String>,
    rebuild: bool,
}
/// Generate the exact SQL preview for a table draft.
/// # Errors
/// Rejects invalid properties and changes unsupported by the connected dialect.
pub fn migration_sql(old: &Definition, new: &Definition) -> Result<String> {
    let p = plan(old, new)?;
    if p.steps.is_empty() {
        return Ok(String::new());
    }
    let mut steps = p.steps;
    if old.driver == DbDriverType::Sqlite {
        steps.insert(0, "BEGIN IMMEDIATE".into());
        steps.insert(0, "PRAGMA foreign_keys=OFF".into());
        steps.push(format!(
            "PRAGMA {}.foreign_key_check",
            q(old.driver, &old.schema)
        ));
        steps.push("COMMIT".into());
        steps.push(format!(
            "PRAGMA legacy_alter_table={}",
            i32::from(old.sqlite_legacy_alter)
        ));
        steps.push(format!(
            "PRAGMA foreign_keys={}",
            i32::from(old.sqlite_foreign_keys)
        ));
    } else if old.driver == DbDriverType::Postgres {
        steps.insert(0, "BEGIN".into());
        steps.push("COMMIT".into());
    }
    Ok(steps.join(";\n") + ";")
}
fn plan(old: &Definition, new: &Definition) -> Result<Plan> {
    validate(old, new)?;
    match old.driver {
        DbDriverType::Sqlite => sqlite_plan(old, new),
        DbDriverType::Mysql => mysql_plan(old, new),
        DbDriverType::Postgres => postgres_plan(old, new),
    }
}
fn old_element<'a>(old: &'a Definition, e: &Element) -> Option<&'a Element> {
    old.elements.iter().find(|x| x.id == e.id)
}
fn removed<'a>(old: &'a Definition, new: &'a Definition) -> impl Iterator<Item = &'a Element> {
    old.elements.iter().filter(|e| {
        new.elements
            .iter()
            .find(|n| n.id == e.id)
            .is_none_or(|n| changed(e, n))
    })
}
fn added<'a>(old: &'a Definition, new: &'a Definition) -> impl Iterator<Item = &'a Element> {
    new.elements
        .iter()
        .filter(|e| old_element(old, e).is_none_or(|o| changed(o, e)))
}
fn drop_trigger(def: &Definition, e: &Element) -> String {
    let name = if def.driver == DbDriverType::Postgres {
        q(def.driver, &e.name)
    } else {
        format!("{}.{}", q(def.driver, &def.schema), q(def.driver, &e.name))
    };
    format!(
        "DROP TRIGGER {name}{}",
        if def.driver == DbDriverType::Postgres {
            format!(" ON {}", def.qualified_name())
        } else {
            String::new()
        }
    )
}
fn comments(old: &Definition, new: &Definition, steps: &mut Vec<String>) {
    if new.driver != DbDriverType::Postgres {
        return;
    }
    let target = old.qualified_name();
    if old.comment != new.comment {
        steps.push(format!(
            "COMMENT ON TABLE {target} IS {}",
            text(new.driver, &new.comment)
        ));
    }
    for e in &new.elements {
        if old_element(old, e).is_none_or(|o| o.comment != e.comment || changed(o, e)) {
            let object = match e.data {
                Properties::Column(_) => format!("COLUMN {target}.{}", q(new.driver, &e.name)),
                Properties::Index(_) => format!(
                    "INDEX {}.{}",
                    q(new.driver, &new.schema),
                    q(new.driver, &e.name)
                ),
                Properties::Key(_) => format!("CONSTRAINT {} ON {target}", q(new.driver, &e.name)),
                Properties::Trigger(_) => format!("TRIGGER {} ON {target}", q(new.driver, &e.name)),
            };
            steps.push(format!(
                "COMMENT ON {object} IS {}",
                text(new.driver, &e.comment)
            ));
        }
    }
}
fn grants(old: &Definition, new: &Definition, steps: &mut Vec<String>) -> Result<()> {
    if old.driver == DbDriverType::Sqlite {
        return Ok(());
    }
    for (revoke, values, other) in [
        (true, &old.grants, &new.grants),
        (false, &new.grants, &old.grants),
    ] {
        for g in values.iter().filter(|g| !other.contains(g)) {
            if ![
                "SELECT",
                "INSERT",
                "UPDATE",
                "DELETE",
                "TRUNCATE",
                "REFERENCES",
                "TRIGGER",
                "CREATE",
                "DROP",
                "ALTER",
                "INDEX",
                "CREATE VIEW",
                "SHOW VIEW",
            ]
            .contains(&g.privilege.as_str())
                || g.user.is_empty()
            {
                return Err(invalid("Choose a valid table privilege and account"));
            }
            let account = if new.driver == DbDriverType::Mysql {
                format!("{}@{}", q(new.driver, &g.user), q(new.driver, &g.host))
            } else if g.user == "PUBLIC" {
                "PUBLIC".into()
            } else {
                q(new.driver, &g.user)
            };
            steps.push(format!(
                "{} {} ON {} {} {}{}",
                if revoke { "REVOKE" } else { "GRANT" },
                g.privilege,
                new.qualified_name(),
                if revoke { "FROM" } else { "TO" },
                account,
                if !revoke && g.grant_option {
                    " WITH GRANT OPTION"
                } else {
                    ""
                }
            ));
        }
    }
    Ok(())
}

#[allow(
    clippy::too_many_lines,
    reason = "One ALTER statement must order mutually dependent key, column and table changes"
)]
pub(super) fn mysql_plan(old: &Definition, new: &Definition) -> Result<Plan> {
    let d = new.driver;
    let mut steps = Vec::new();
    let mut clauses = Vec::new();
    for e in removed(old, new) {
        match &e.data {
            Properties::Trigger(_) => steps.push(drop_trigger(old, e)),
            Properties::Index(_) => clauses.push(format!("DROP INDEX {}", q(d, &e.name))),
            Properties::Key(k) => clauses.push(match k.kind {
                KeyKind::Primary => "DROP PRIMARY KEY".into(),
                KeyKind::Foreign => format!("DROP FOREIGN KEY {}", q(d, &e.name)),
                KeyKind::Check => format!(
                    "DROP {} {}",
                    if old.server_version.to_ascii_lowercase().contains("mariadb") {
                        "CONSTRAINT"
                    } else {
                        "CHECK"
                    },
                    q(d, &e.name)
                ),
                _ => format!("DROP INDEX {}", q(d, &e.name)),
            }),
            Properties::Column(_) => {
                if !new.elements.iter().any(|n| n.id == e.id) {
                    clauses.push(format!("DROP COLUMN {}", q(d, &e.name)));
                }
            }
        }
    }
    let cols = new
        .elements
        .iter()
        .filter(|e| matches!(e.data, Properties::Column(_)))
        .collect::<Vec<_>>();
    let old_cols = old
        .elements
        .iter()
        .filter(|e| matches!(e.data, Properties::Column(_)))
        .map(|e| e.id)
        .collect::<Vec<_>>();
    for (position, e) in cols.iter().enumerate() {
        let old = old_element(old, e);
        let moved = old_cols.get(position) != Some(&e.id);
        if old.is_none_or(|o| changed(o, e) || o.comment != e.comment) || moved {
            let after = if position == 0 {
                " FIRST".into()
            } else {
                format!(" AFTER {}", q(d, &cols[position - 1].name))
            };
            clauses.push(match old {
                Some(o) => format!("CHANGE COLUMN {} {}{after}", q(d, &o.name), column(d, e)?),
                None => format!("ADD COLUMN {}{after}", column(d, e)?),
            });
        }
    }
    for e in added(old, new) {
        match e.data {
            Properties::Key(_) => clauses.push(format!("ADD {}", key(d, e)?)),
            Properties::Index(_) => clauses.push(index(new, e, true)?),
            _ => {}
        }
    }
    for (before, after, option) in [
        (&old.engine, &new.engine, "ENGINE"),
        (&old.collation, &new.collation, "DEFAULT COLLATE"),
        (&old.row_format, &new.row_format, "ROW_FORMAT"),
    ] {
        if before != after {
            if after.is_empty() {
                return Err(invalid(format!("{option} cannot be empty")));
            }
            let value = if option == "ROW_FORMAT" {
                let value = after.to_ascii_uppercase();
                if ![
                    "DEFAULT",
                    "DYNAMIC",
                    "COMPACT",
                    "REDUNDANT",
                    "COMPRESSED",
                    "FIXED",
                ]
                .contains(&value.as_str())
                {
                    return Err(invalid("Choose a valid MySQL row format"));
                }
                value
            } else {
                qualified(d, after)?
            };
            clauses.push(format!("{option}={value}"));
        }
    }
    if old.auto_increment != new.auto_increment && !new.auto_increment.is_empty() {
        let n = new
            .auto_increment
            .parse::<u64>()
            .map_err(|_| invalid("AUTO_INCREMENT must be an unsigned integer"))?;
        clauses.push(format!("AUTO_INCREMENT={n}"));
    }
    if old.comment != new.comment {
        clauses.push(format!("COMMENT={}", text(d, &new.comment)));
    }
    if old.extra_sql != new.extra_sql {
        sql::fragment(&new.extra_sql)?;
        if !new.extra_sql.is_empty() {
            clauses.push(new.extra_sql.clone());
        }
    }
    if old.name != new.name {
        clauses.push(format!("RENAME TO {}", new.qualified_name()));
    }
    if !clauses.is_empty() {
        steps.push(format!(
            "ALTER TABLE {}\n    {}",
            old.qualified_name(),
            clauses.join(",\n    ")
        ));
    }
    for e in added(old, new) {
        if matches!(e.data, Properties::Trigger(_)) {
            steps.extend(trigger(new, e)?);
        }
    }
    grants(old, new, &mut steps)?;
    if !steps.is_empty() {
        steps.insert(0, mysql_mode().into());
    }
    Ok(Plan {
        steps,
        rebuild: false,
    })
}
#[allow(
    clippy::too_many_lines,
    reason = "Transactional DDL phases must order drops, renames, column changes and dependent objects"
)]
pub(super) fn postgres_plan(old: &Definition, new: &Definition) -> Result<Plan> {
    let d = new.driver;
    let target = old.qualified_name();
    let mut steps = Vec::new();
    let old_order = old
        .elements
        .iter()
        .filter(|e| {
            matches!(e.data, Properties::Column(_)) && new.elements.iter().any(|n| n.id == e.id)
        })
        .map(|e| e.id)
        .collect::<Vec<_>>();
    let new_order = new
        .elements
        .iter()
        .filter(|e| {
            matches!(e.data, Properties::Column(_)) && old.elements.iter().any(|n| n.id == e.id)
        })
        .map(|e| e.id)
        .collect::<Vec<_>>();
    if old_order != new_order {
        return Err(invalid(
            "PostgreSQL cannot reorder existing columns with ALTER TABLE",
        ));
    }
    let mut renamed = HashSet::new();
    for e in removed(old, new) {
        let replacement = new.elements.iter().find(|n| n.id == e.id);
        if replacement.is_some_and(|n| n.data == e.data && n.name != e.name)
            && !matches!(e.data, Properties::Column(_))
        {
            let n = replacement.ok_or_else(|| invalid("Missing renamed object"))?;
            let statement = match e.data {
                Properties::Index(_) => format!(
                    "ALTER INDEX {}.{} RENAME TO {}",
                    q(d, &old.schema),
                    q(d, &e.name),
                    q(d, &n.name)
                ),
                Properties::Key(_) => format!(
                    "ALTER TABLE {target} RENAME CONSTRAINT {} TO {}",
                    q(d, &e.name),
                    q(d, &n.name)
                ),
                Properties::Trigger(_) => format!(
                    "ALTER TRIGGER {} ON {target} RENAME TO {}",
                    q(d, &e.name),
                    q(d, &n.name)
                ),
                Properties::Column(_) => String::new(),
            };
            steps.push(statement);
            renamed.insert(e.id);
            continue;
        }
        match &e.data {
            Properties::Index(_) => steps.push(format!(
                "DROP INDEX {}.{}",
                q(d, &old.schema),
                q(d, &e.name)
            )),
            Properties::Key(_) => steps.push(format!(
                "ALTER TABLE {target} DROP CONSTRAINT {}",
                q(d, &e.name)
            )),
            Properties::Trigger(_) => steps.push(drop_trigger(old, e)),
            Properties::Column(_) => {
                if replacement.is_none() {
                    steps.push(format!(
                        "ALTER TABLE {target} DROP COLUMN {}",
                        q(d, &e.name)
                    ));
                }
            }
        }
    }
    for e in &new.elements {
        let Properties::Column(c) = &e.data else {
            continue;
        };
        let Some(o) = old_element(old, e) else {
            steps.push(format!("ALTER TABLE {target} ADD COLUMN {}", column(d, e)?));
            continue;
        };
        let Properties::Column(b) = &o.data else {
            return Err(invalid("An object's type cannot change"));
        };
        if o.name != e.name {
            steps.push(format!(
                "ALTER TABLE {target} RENAME COLUMN {} TO {}",
                q(d, &o.name),
                q(d, &e.name)
            ));
        }
        let prefix = format!("ALTER TABLE {target} ALTER COLUMN {}", q(d, &e.name));
        if b.generated_sql != c.generated_sql || b.stored != c.stored {
            steps.push(format!(
                "ALTER TABLE {target} DROP COLUMN {}",
                q(d, &e.name)
            ));
            steps.push(format!("ALTER TABLE {target} ADD COLUMN {}", column(d, e)?));
            continue;
        }
        if b.data_type != c.data_type || b.collation != c.collation || !c.using_sql.is_empty() {
            sql::fragment(&c.data_type)?;
            sql::fragment(&c.using_sql)?;
            steps.push(format!(
                "{prefix} TYPE {}{}{}",
                c.data_type,
                if c.collation.is_empty() {
                    String::new()
                } else {
                    format!(" COLLATE {}", qualified(d, &c.collation)?)
                },
                if c.using_sql.is_empty() {
                    String::new()
                } else {
                    format!(" USING {}", c.using_sql)
                }
            ));
        }
        if b.identity != c.identity {
            if !b.identity.is_empty() && c.identity.is_empty() {
                steps.push(format!("{prefix} DROP IDENTITY"));
            } else if b.identity.is_empty() {
                if !b.default_sql.is_empty() {
                    steps.push(format!("{prefix} DROP DEFAULT"));
                }
                steps.push(format!(
                    "{prefix} ADD GENERATED {} AS IDENTITY{}",
                    c.identity,
                    if c.identity_options.is_empty() {
                        String::new()
                    } else {
                        format!(" ({})", c.identity_options)
                    }
                ));
            } else {
                steps.push(format!("{prefix} SET GENERATED {}", c.identity));
            }
        }
        if b.identity_options != c.identity_options
            && !b.identity.is_empty()
            && !c.identity.is_empty()
        {
            sql::fragment(&c.identity_options)?;
            steps.push(format!(
                "ALTER SEQUENCE {} {}",
                qualified(d, &b.identity_sequence)?,
                c.identity_options
            ));
        }
        if b.default_sql != c.default_sql && c.identity.is_empty() {
            sql::fragment(&c.default_sql)?;
            steps.push(if c.default_sql.is_empty() {
                format!("{prefix} DROP DEFAULT")
            } else {
                format!("{prefix} SET DEFAULT {}", c.default_sql)
            });
        }
        if b.not_null != c.not_null {
            steps.push(format!(
                "{prefix} {} NOT NULL",
                if c.not_null { "SET" } else { "DROP" }
            ));
        }
        if b.extra_sql != c.extra_sql {
            return Err(invalid(
                "Edit supported column properties; use explicit SQL for additional native clauses",
            ));
        }
    }
    for e in added(old, new) {
        if renamed.contains(&e.id) {
            continue;
        }
        match e.data {
            Properties::Key(_) => steps.push(format!("ALTER TABLE {target} ADD {}", key(d, e)?)),
            Properties::Index(_) => {
                let mut current = new.clone();
                current.name.clone_from(&old.name);
                steps.push(index(&current, e, false)?);
            }
            Properties::Trigger(_) => {
                let mut current = new.clone();
                current.name.clone_from(&old.name);
                steps.extend(trigger(&current, e)?);
            }
            Properties::Column(_) => {}
        }
    }
    if old.unlogged != new.unlogged {
        steps.push(format!(
            "ALTER TABLE {target} SET {}",
            if new.unlogged { "UNLOGGED" } else { "LOGGED" }
        ));
    }
    if old.access_method != new.access_method {
        steps.push(format!(
            "ALTER TABLE {target} SET ACCESS METHOD {}",
            qualified(d, &new.access_method)?
        ));
    }
    if old.tablespace != new.tablespace {
        steps.push(format!(
            "ALTER TABLE {target} SET TABLESPACE {}",
            q(
                d,
                if new.tablespace.is_empty() {
                    "pg_default"
                } else {
                    &new.tablespace
                }
            )
        ));
    }
    if old.storage_options != new.storage_options {
        sql::fragment(&new.storage_options)?;
        let new_options = sql::parts(&new.storage_options)?;
        let keys = new_options
            .iter()
            .filter_map(|s| s.split_once('='))
            .map(|(k, _)| k.trim())
            .collect::<HashSet<_>>();
        let reset = sql::parts(&old.storage_options)?
            .into_iter()
            .filter_map(|s| s.split_once('=').map(|(key, _)| key.trim().to_owned()))
            .filter(|s| !keys.contains(s.as_str()))
            .collect::<Vec<_>>();
        if !reset.is_empty() {
            steps.push(format!("ALTER TABLE {target} RESET ({})", reset.join(", ")));
        }
        if !new.storage_options.is_empty() {
            steps.push(format!(
                "ALTER TABLE {target} SET ({})",
                new.storage_options
            ));
        }
    }
    for (before, after, what) in [
        (old.row_security, new.row_security, "ROW LEVEL SECURITY"),
        (
            old.force_row_security,
            new.force_row_security,
            "FORCE ROW LEVEL SECURITY",
        ),
    ] {
        if before != after {
            steps.push(if what.starts_with("FORCE") {
                format!(
                    "ALTER TABLE {target} {} ROW LEVEL SECURITY",
                    if after { "FORCE" } else { "NO FORCE" }
                )
            } else {
                format!(
                    "ALTER TABLE {target} {} ROW LEVEL SECURITY",
                    if after { "ENABLE" } else { "DISABLE" }
                )
            });
        }
    }
    if old.extra_sql != new.extra_sql && !new.extra_sql.trim().is_empty() {
        sql::fragment(&new.extra_sql)?;
        steps.push(format!("ALTER TABLE {target} {}", new.extra_sql));
    }
    comments(old, new, &mut steps);
    if old.name != new.name {
        steps.push(format!(
            "ALTER TABLE {target} RENAME TO {}",
            q(d, &new.name)
        ));
    }
    grants(old, new, &mut steps)?;
    if old.owner != new.owner {
        steps.push(format!(
            "ALTER TABLE {} OWNER TO {}",
            new.qualified_name(),
            q(d, &new.owner)
        ));
    }
    Ok(Plan {
        steps,
        rebuild: false,
    })
}
#[allow(
    clippy::too_many_lines,
    reason = "Native ALTER and transactional rebuild plans share data-preservation validation"
)]
fn sqlite_plan(old: &Definition, new: &Definition) -> Result<Plan> {
    let d = new.driver;
    let mut steps = Vec::new();
    let old_cols = old
        .elements
        .iter()
        .filter(|e| matches!(e.data, Properties::Column(_)))
        .collect::<Vec<_>>();
    let cols = new
        .elements
        .iter()
        .filter(|e| matches!(e.data, Properties::Column(_)))
        .collect::<Vec<_>>();
    let renamed_columns = cols
        .iter()
        .filter_map(|e| {
            old_element(old, e)
                .filter(|o| o.name != e.name)
                .map(|o| (o, *e))
        })
        .collect::<Vec<_>>();
    let mut rebuild = old.strict != new.strict
        || old.without_rowid != new.without_rowid
        || old_cols.len() != cols.len();
    if !rebuild {
        rebuild = old_cols
            .iter()
            .zip(&cols)
            .any(|(a, b)| a.id != b.id || a.data != b.data);
    }
    rebuild |= removed(old, new)
        .chain(added(old, new))
        .any(|e| matches!(e.data, Properties::Key(_)));
    if rebuild && (old.name != new.name || !renamed_columns.is_empty()) {
        return Err(invalid(
            "Apply table/column renames separately before changes that rebuild a SQLite table",
        ));
    }
    if !rebuild {
        if old.name != new.name || !renamed_columns.is_empty() {
            steps.push("PRAGMA legacy_alter_table=OFF".into());
        }
        for (a, b) in renamed_columns {
            steps.push(format!(
                "ALTER TABLE {} RENAME COLUMN {} TO {}",
                old.qualified_name(),
                q(d, &a.name),
                q(d, &b.name)
            ));
        }
        if old.name != new.name {
            steps.push(format!(
                "ALTER TABLE {} RENAME TO {}",
                old.qualified_name(),
                q(d, &new.name)
            ));
        }
        for e in removed(old, new) {
            match e.data {
                Properties::Index(_) => steps.push(format!(
                    "DROP INDEX {}.{}",
                    q(d, &old.schema),
                    q(d, &e.name)
                )),
                Properties::Trigger(_) => steps.push(drop_trigger(old, e)),
                _ => {}
            }
        }
        for e in added(old, new) {
            match e.data {
                Properties::Index(_) => steps.push(index(new, e, false)?),
                Properties::Trigger(_) => steps.extend(trigger(new, e)?),
                _ => {}
            }
        }
        return Ok(Plan { steps, rebuild });
    }
    let mut hash = std::hash::DefaultHasher::new();
    old.schema.hash(&mut hash);
    old.name.hash(&mut hash);
    let temp = format!("__nozhik_edit_{:x}", hash.finish());
    let temporary = format!("{}.{}", q(d, &old.schema), q(d, &temp));
    let definitions = new
        .elements
        .iter()
        .filter(|e| matches!(e.data, Properties::Column(_) | Properties::Key(_)))
        .map(|e| {
            if let Some(o) = old_element(old, e)
                && !changed(o, e)
                && !e.native_sql.is_empty()
            {
                return Ok(e.native_sql.clone());
            }
            match e.data {
                Properties::Column(_) => column(d, e),
                Properties::Key(_) => key(d, e),
                _ => Err(invalid("Unexpected table object")),
            }
        })
        .collect::<Result<Vec<_>>>()?;
    steps.push("PRAGMA legacy_alter_table=ON".into());
    steps.push(format!(
        "CREATE TABLE {temporary} (\n    {}\n){}",
        definitions.join(",\n    "),
        match (new.strict, new.without_rowid) {
            (true, true) => " STRICT, WITHOUT ROWID",
            (true, false) => " STRICT",
            (false, true) => " WITHOUT ROWID",
            _ => "",
        }
    ));
    let retained = cols
        .iter()
        .filter(|e| matches!(&e.data,Properties::Column(c) if c.generated_sql.is_empty()))
        .filter_map(|e| old_element(old, e).map(|o| (*e, o)))
        .collect::<Vec<_>>();
    if retained.is_empty() {
        return Err(invalid(
            "At least one original ordinary column is required to preserve existing rows",
        ));
    }
    let mut into = retained
        .iter()
        .map(|(e, _)| q(d, &e.name))
        .collect::<Vec<_>>();
    let mut from = retained
        .iter()
        .map(|(_, o)| q(d, &o.name))
        .collect::<Vec<_>>();
    if !old.without_rowid
        && !new.without_rowid
        && let Some(alias) = ["rowid", "_rowid_", "oid"].iter().find(|alias| {
            !cols
                .iter()
                .chain(&old_cols)
                .any(|e| e.name.eq_ignore_ascii_case(alias))
        })
    {
        into.insert(0, q(d, alias));
        from.insert(0, q(d, alias));
    }
    steps.push(format!(
        "INSERT INTO {temporary} ({}) SELECT {} FROM {}",
        into.join(", "),
        from.join(", "),
        old.qualified_name()
    ));
    if cols
        .iter()
        .any(|e| matches!(&e.data,Properties::Column(c) if c.auto_increment))
    {
        steps.push(format!("UPDATE {}.sqlite_sequence SET seq=MAX(seq,COALESCE((SELECT seq FROM {}.sqlite_sequence WHERE name={}),seq)) WHERE name={}",q(d,&old.schema),q(d,&old.schema),text(d,&old.name),text(d,&temp)));
    }
    steps.push(format!("DROP TABLE {}", old.qualified_name()));
    steps.push(format!(
        "ALTER TABLE {temporary} RENAME TO {}",
        q(d, &new.name)
    ));
    for e in &new.elements {
        match e.data {
            Properties::Index(_) => steps.push(index(new, e, false)?),
            Properties::Trigger(_) => steps.extend(trigger(new, e)?),
            _ => {}
        }
    }
    Ok(Plan { steps, rebuild })
}
impl Connection {
    /// Apply the same validated migration displayed by Modify Table.
    /// # Errors
    /// Returns stale snapshot, validation, cancellation or native DDL failures.
    pub async fn modify_table(
        &self,
        old: &Definition,
        new: &Definition,
        cancel: &CancellationToken,
    ) -> Result<Definition> {
        if old.driver != self.driver {
            return Err(invalid("Table dialect does not match its connection"));
        }
        let current = tokio::select! {biased;()=cancel.cancelled()=>return Err(Error::Cancelled),result=self.table_definition(&old.schema,&old.name)=>result?};
        if !old.same_schema(&current) {
            return Err(invalid(
                "Table definition changed on the server; reload Modify Table before applying",
            ));
        }
        let p = plan(old, new)?;
        if p.steps.is_empty() {
            return Ok(current);
        }
        if let Pool::Sqlite(pool) = &self.pool {
            let mut c = pool.acquire().await?;
            let foreign: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
                .fetch_one(&mut *c)
                .await?;
            let legacy: i64 = sqlx::query_scalar("PRAGMA legacy_alter_table")
                .fetch_one(&mut *c)
                .await?;
            sqlx::raw_sql("PRAGMA foreign_keys=OFF")
                .execute(&mut *c)
                .await?;
            let token = cancel.clone();
            c.lock_handle()
                .await?
                .set_progress_handler(1000, move || !token.is_cancelled());
            let mut started = false;
            let result=async {
                let mut tx=c.begin_with("BEGIN IMMEDIATE").await?; started=true;
                let mut actual=super::catalog::sqlite(&mut tx,&old.schema,&old.name).await?;actual.sqlite_foreign_keys=foreign!=0;actual.sqlite_legacy_alter=legacy!=0;if !old.same_schema(&actual){return Err(invalid("Table definition changed while waiting for its write lock"));}
                for step in &p.steps{if cancel.is_cancelled(){return Err(Error::Cancelled);}sqlx::raw_sql(step).execute(&mut *tx).await?;}
                let violations=sqlx::query(&format!("PRAGMA {}.foreign_key_check",q(self.driver,&old.schema))).fetch_all(&mut *tx).await?;if !violations.is_empty(){return Err(invalid("Migration would leave foreign-key violations; changes have been rolled back"));}
                // Rebuilds must not leave a dependent view referring to dropped columns.
                if p.rebuild {let views:Vec<String>=sqlx::query_scalar(&format!("SELECT name FROM {}.sqlite_master WHERE type='view'",q(self.driver,&old.schema))).fetch_all(&mut *tx).await?;for view in views{sqlx::query(&format!("SELECT * FROM {}.{} LIMIT 0",q(self.driver,&old.schema),q(self.driver,&view))).fetch_all(&mut *tx).await?;}}
                if cancel.is_cancelled(){return Err(Error::Cancelled);}tx.commit().await?;Ok::<_,Error>(())
            }.await;
            c.lock_handle().await?.remove_progress_handler();
            // Drain the asynchronous transaction rollback before restoring pragmas.
            if started {
                let _rollback = sqlx::raw_sql("ROLLBACK").execute(&mut *c).await;
            }
            sqlx::raw_sql(&format!(
                "PRAGMA legacy_alter_table={legacy}; PRAGMA foreign_keys={foreign}"
            ))
            .execute(&mut *c)
            .await?;
            if cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            result?;
        } else {
            self.execute_schema_script(&migration_sql(old, new)?, &old.schema, cancel)
                .await?;
        }
        self.table_definition(&new.schema, &new.name).await
    }
}
