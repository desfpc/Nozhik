//! Local table edits and dialect-safe DML shared by preview and submission.
use crate::{DbDriverType, DbValue, Error, QueryResult, Result, connection::quote_identifier};

/// One staged row; `None` cells in new rows request the server default.
#[derive(Clone, Debug)]
pub struct EditedRow {
    /// Original server values, absent for an insertion.
    pub original: Option<Vec<DbValue>>,
    /// Current values in result-column order.
    pub values: Vec<Option<DbValue>>,
    /// Staged deletion.
    pub deleted: bool,
}
/// Table snapshot with local edits. No operation here sends SQL.
#[derive(Clone, Debug)]
pub struct TableChanges {
    /// Dialect used by the owning connection.
    pub driver: DbDriverType,
    /// Namespace identifier.
    pub schema: String,
    /// Relation identifier.
    pub table: String,
    /// Original column descriptors and retained results.
    pub baseline: QueryResult,
    /// Primary-key column positions, including composite keys.
    pub keys: Vec<usize>,
    /// Staged rows in server order followed by insertions.
    pub rows: Vec<EditedRow>,
}
impl TableChanges {
    /// Create a local editing snapshot.
    #[must_use]
    pub fn new(
        driver: DbDriverType,
        schema: String,
        table: String,
        baseline: QueryResult,
        keys: Vec<usize>,
    ) -> Self {
        let mut changes = Self {
            driver,
            schema,
            table,
            baseline,
            keys,
            rows: Vec::new(),
        };
        changes.revert();
        changes
    }
    /// Discard all pending edits.
    pub fn revert(&mut self) {
        self.rows = self
            .baseline
            .rows
            .iter()
            .map(|row| EditedRow {
                original: Some(row.clone()),
                values: row.iter().cloned().map(Some).collect(),
                deleted: false,
            })
            .collect();
    }
    /// Whether edits differ from the retained server snapshot.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.rows.iter().any(|r| {
            r.original.as_ref().map_or(!r.deleted, |original| {
                r.deleted
                    || r.values
                        .iter()
                        .map(Option::as_ref)
                        .ne(original.iter().map(Some))
            })
        })
    }
    /// Append a row whose omitted cells use database defaults.
    pub fn add_row(&mut self) {
        self.rows.push(EditedRow {
            original: None,
            values: vec![None; self.baseline.columns.len()],
            deleted: false,
        });
    }
    /// Generate the exact statements used by Submit.
    /// # Errors
    /// Rejects malformed snapshots; row identity uses the full original snapshot.
    pub fn statements(&self) -> Result<Vec<String>> {
        let quote = |s: &str| quote_identifier(self.driver, s);
        let table = format!("{}.{}", quote(&self.schema), quote(&self.table));
        let mut statements = Vec::new();
        for row in &self.rows {
            if row.values.len() != self.baseline.columns.len() {
                return Err(Error::Invalid("Invalid row width".into()));
            }
            if let Some(original) = &row.original {
                let assignments = row
                    .values
                    .iter()
                    .zip(original)
                    .enumerate()
                    .filter(|(_, (a, b))| a.as_ref() != Some(*b))
                    .map(|(i, (value, _))| {
                        Ok(format!(
                            "{} = {}",
                            quote(&self.baseline.columns[i].name),
                            value.as_ref().map_or_else(
                                || Ok("DEFAULT".into()),
                                |v| literal(self.driver, v)
                            )?
                        ))
                    })
                    .collect::<Result<Vec<_>>>()?;
                if !row.deleted && assignments.is_empty() {
                    continue;
                }
                if original.len() != self.baseline.columns.len() || original.is_empty() {
                    return Err(Error::Invalid("Invalid original row width".into()));
                }
                // Include original values to detect concurrent edits. A transaction verifies
                // exactly one matched row, including tables without primary keys.
                let predicate = original
                    .iter()
                    .zip(&self.baseline.columns)
                    .map(|(value, column)| {
                        let name = quote(&column.name);
                        if matches!(value, DbValue::Null) {
                            return Ok(format!("{name} IS NULL"));
                        }
                        let value = literal(self.driver, value)?;
                        Ok(
                            if self.driver == DbDriverType::Postgres
                                && column.data_type.eq_ignore_ascii_case("JSON")
                            {
                                format!("{name}::jsonb = {value}::jsonb")
                            } else {
                                format!("{name} = {value}")
                            },
                        )
                    })
                    .collect::<Result<Vec<_>>>()?
                    .join(" AND ");
                statements.push(if row.deleted {
                    format!("DELETE FROM {table} WHERE {predicate};")
                } else {
                    format!(
                        "UPDATE {table} SET {} WHERE {predicate};",
                        assignments.join(", ")
                    )
                });
            } else if !row.deleted {
                let mut columns = Vec::new();
                let mut values = Vec::new();
                for (column, value) in self.baseline.columns.iter().zip(&row.values) {
                    if let Some(value) = value {
                        columns.push(quote(&column.name));
                        values.push(literal(self.driver, value)?);
                    }
                }
                statements.push(if columns.is_empty() {
                    if self.driver == DbDriverType::Mysql {
                        format!("INSERT INTO {table} () VALUES ();")
                    } else {
                        format!("INSERT INTO {table} DEFAULT VALUES;")
                    }
                } else {
                    format!(
                        "INSERT INTO {table} ({}) VALUES ({});",
                        columns.join(", "),
                        values.join(", ")
                    )
                });
            }
        }
        Ok(statements)
    }
}
/// Encode a value without depending on connection string-escape modes.
/// # Errors
/// Rejects non-finite floats, which have no portable SQL literal.
pub fn literal(driver: DbDriverType, value: &DbValue) -> Result<String> {
    use std::fmt::Write;
    let hex = |bytes: &[u8]| {
        let mut s = String::new();
        for b in bytes {
            write!(s, "{b:02x}").ok();
        }
        s
    };
    Ok(match value {
        DbValue::Null => "NULL".into(),
        DbValue::Bool(v) => if *v { "TRUE" } else { "FALSE" }.into(),
        DbValue::Int(v) => v.to_string(),
        DbValue::Float(v) if v.is_finite() => v.to_string(),
        DbValue::Float(_) => return Err(Error::Invalid("Non-finite number".into())),
        DbValue::Bytes(v) => {
            if driver == DbDriverType::Postgres {
                format!("decode('{}', 'hex')", hex(v))
            } else {
                format!("X'{}'", hex(v))
            }
        }
        DbValue::Text(s) | DbValue::Json(s) | DbValue::DateTime(s) => match driver {
            DbDriverType::Postgres => format!("E'{}'", s.replace('\\', "\\\\").replace('\'', "''")),
            DbDriverType::Mysql => format!("CONVERT(X'{}' USING utf8mb4)", hex(s.as_bytes())),
            DbDriverType::Sqlite => format!("CAST(X'{}' AS TEXT)", hex(s.as_bytes())),
        },
    })
}
