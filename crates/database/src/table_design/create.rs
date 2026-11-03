//! New-table plans reuse the designer's object renderers and option diffs.
use super::plan::{
    column, index, key, mysql_mode, mysql_plan, postgres_plan, q, trigger, validate,
};
use super::{Definition, Properties, Result, invalid};
use crate::{CancellationToken, Connection, DbDriverType};
use std::fmt::Write as _;

/// Generate the exact statements used to create a new table and its objects.
/// # Errors
/// Rejects invalid definitions or properties unsupported by the dialect.
pub fn creation_sql(new: &Definition) -> Result<String> {
    let empty = Definition {
        driver: new.driver,
        schema: new.schema.clone(),
        name: new.name.clone(),
        ..Definition::default()
    };
    validate(&empty, new)?;
    let definitions = new
        .elements
        .iter()
        .filter_map(|e| match e.data {
            Properties::Column(_) => Some(column(new.driver, e)),
            Properties::Key(_) => Some(key(new.driver, e)),
            // MySQL AUTO_INCREMENT needs its index in the initial CREATE statement.
            Properties::Index(_) if new.driver == DbDriverType::Mysql => {
                Some(index(new, e, true).map(|s| s.trim_start_matches("ADD ").to_owned()))
            }
            _ => None,
        })
        .collect::<Result<Vec<_>>>()?;
    let mut create = format!(
        "CREATE TABLE {} (\n    {}\n)",
        new.qualified_name(),
        definitions.join(",\n    ")
    );
    let mut steps = Vec::new();
    match new.driver {
        DbDriverType::Sqlite => {
            create.push_str(match (new.strict, new.without_rowid) {
                (true, true) => " STRICT, WITHOUT ROWID",
                (true, false) => " STRICT",
                (false, true) => " WITHOUT ROWID",
                _ => "",
            });
            steps.push(create);
            for e in &new.elements {
                match e.data {
                    Properties::Index(_) => steps.push(index(new, e, false)?),
                    Properties::Trigger(_) => steps.extend(trigger(new, e)?),
                    _ => {}
                }
            }
        }
        DbDriverType::Mysql | DbDriverType::Postgres => {
            let mut baseline = empty;
            baseline.elements = new
                .elements
                .iter()
                .filter(|e| {
                    matches!(e.data, Properties::Column(_) | Properties::Key(_))
                        || new.driver == DbDriverType::Mysql
                            && matches!(e.data, Properties::Index(_))
                })
                .cloned()
                .collect();
            if new.driver == DbDriverType::Mysql {
                // Storage defaults must be established before creating columns and FKs.
                for (label, value) in [("ENGINE", &new.engine), ("COLLATE", &new.collation)] {
                    if !value.is_empty() {
                        super::sql::fragment(value)?;
                        if !value.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                            return Err(invalid("Invalid table engine/collation"));
                        }
                        write!(create, " {label}={}", q(new.driver, value)).ok();
                    }
                }
                baseline.engine.clone_from(&new.engine);
                baseline.collation.clone_from(&new.collation);
                steps.push(mysql_mode().into());
                steps.push(create);
                steps.extend(mysql_plan(&baseline, new)?.steps);
            } else {
                for e in &mut baseline.elements {
                    e.comment.clear();
                }
                steps.push(create);
                steps.extend(postgres_plan(&baseline, new)?.steps);
            }
        }
    }
    if new.driver != DbDriverType::Mysql {
        steps.insert(
            0,
            if new.driver == DbDriverType::Sqlite {
                "BEGIN IMMEDIATE"
            } else {
                "BEGIN"
            }
            .into(),
        );
        steps.push("COMMIT".into());
    }
    Ok(steps.join(";\n") + ";")
}
impl Connection {
    /// Create a table without replacing an existing relation, then load its snapshot.
    /// # Errors
    /// Returns validation, duplicate-name, cancellation or native DDL errors.
    pub async fn create_table(
        &self,
        new: &Definition,
        cancel: &CancellationToken,
    ) -> Result<Definition> {
        if cancel.is_cancelled() {
            return Err(crate::Error::Cancelled);
        }
        if new.driver != self.driver {
            return Err(invalid("Table dialect does not match its connection"));
        }
        self.execute_schema_script(&creation_sql(new)?, &new.schema, cancel)
            .await?;
        self.table_definition(&new.schema, &new.name).await
    }
}
