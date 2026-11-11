//! Native `ClickHouse` definitions and complete streaming table exports.
use super::{Artifact, ExportOptions, csv_line, csv_write, finish, temporary};
use crate::{
    ColumnInfo, Connection, DbValue, Error, Result,
    clickhouse::{Client, text},
    connection::quote_identifier,
};
use std::fmt::Write as _;
use tokio::io::AsyncWriteExt;

impl Connection {
    pub(super) async fn clickhouse_dump(
        &self,
        client: &Client,
        schema: &str,
        options: Option<&ExportOptions>,
        ddl_only: bool,
    ) -> Result<Artifact> {
        if !ddl_only && !options.is_some_and(|o| o.only_table) {
            return Err(Error::Invalid("ClickHouse SQL data export requires one table; use CSV for a selected table or Generate DDL for database definitions".into()));
        }
        let tables = if let Some(o) = options.filter(|o| o.only_table) {
            vec![o.table.clone()]
        } else {
            let mut tables = client
                .catalog()
                .await?
                .schemas
                .into_iter()
                .find(|s| s.name == schema)
                .ok_or_else(|| Error::Invalid("Database disappeared".into()))?
                .tables;
            // Ordinary views follow their base relations. Native DDL keeps dependencies explicit.
            tables.sort_by_key(|t| t.is_view);
            tables.into_iter().map(|t| t.name).collect()
        };
        let file = temporary().await?;
        let mut out = tokio::io::BufWriter::new(tokio::fs::File::from_std(file.reopen()?));
        for table in tables {
            let info = client.describe(schema, &table, false).await?;
            if let Some(error) = info.objects.error {
                return Err(Error::Invalid(error));
            }
            if ddl_only || options.is_some_and(|o| o.ddl) {
                out.write_all(info.objects.ddl.as_bytes()).await?;
                out.write_all(b"\n\n").await?;
            }
            if ddl_only {
                continue;
            }
            let engine = info
                .objects
                .keys
                .iter()
                .find(|k| k.name == "Engine")
                .map_or("", |k| k.definition.as_str());
            if engine.ends_with("View") || engine == "Dictionary" {
                return Err(Error::Invalid(
                    "SQL data export requires a base table; use CSV for views and dictionaries"
                        .into(),
                ));
            }
            let q = |s: &str| quote_identifier(self.driver, s);
            let relation = format!("{}.{}", q(schema), q(&table));
            let sql = format!(
                "SELECT name FROM system.columns WHERE database={} AND table={} AND default_kind NOT IN ('MATERIALIZED','ALIAS') ORDER BY position",
                text(schema),
                text(&table)
            );
            let columns = client
                .query(&sql, schema, &crate::CancellationToken::new())
                .await?
                .pop()
                .unwrap_or_default();
            if columns.truncated {
                return Err(Error::Invalid("Column catalog exceeds export limit".into()));
            }
            let columns = columns
                .rows
                .iter()
                .filter_map(|r| r.first())
                .map(|v| q(&v.to_string()))
                .collect::<Vec<_>>()
                .join(", ");
            if columns.is_empty() {
                return Err(Error::Invalid("No insertable columns".into()));
            }
            let prefix = format!("INSERT INTO {relation} ({columns}) VALUES ");
            // Let the server serialize native arrays, tuples, maps, binary strings and exact numbers.
            let sql =
                format!("SELECT hex(formatRowNoNewline('Values', {columns})) FROM {relation}");
            client
                .each_row(&sql, schema, async |_, row| {
                    let value = row.first().ok_or_else(|| {
                        Error::Invalid("Missing serialized ClickHouse row".into())
                    })?;
                    out.write_all(prefix.as_bytes()).await?;
                    out.write_all(ascii_values(&value.to_string())?.as_bytes())
                        .await?;
                    out.write_all(b";\n").await?;
                    Ok(())
                })
                .await?;
        }
        out.flush().await?;
        finish(file).await
    }
    pub(super) async fn clickhouse_csv_export(
        &self,
        client: &Client,
        schema: &str,
        options: &ExportOptions,
    ) -> Result<Artifact> {
        let info = client.describe(schema, &options.table, false).await?;
        let columns: Vec<_> =
            info.columns
                .iter()
                .filter(|c| {
                    options.generated
                        || !c.default_value.as_ref().is_some_and(|s| {
                            s.starts_with("ALIAS ") || s.starts_with("MATERIALIZED ")
                        })
                })
                .map(|c| c.name.clone())
                .collect();
        if columns.is_empty() {
            return Err(Error::Invalid("No exportable columns".into()));
        }
        let q = |s: &str| quote_identifier(self.driver, s);
        let query = format!(
            "SELECT {} FROM {}.{}",
            columns.iter().map(|s| q(s)).collect::<Vec<_>>().join(", "),
            q(schema),
            q(&options.table)
        );
        let file = temporary().await?;
        let mut out = tokio::io::BufWriter::new(tokio::fs::File::from_std(file.reopen()?));
        let mut matrix = Vec::new();
        let mut retained = 0;
        if options.column_header {
            let mut header = columns;
            if options.row_header {
                header.insert(0, "#".into());
            }
            csv_write(&mut out, &header, options, &mut matrix, &mut retained).await?;
        }
        let mut index = 0_u64;
        client
            .each_row(&query, schema, async |_: &[ColumnInfo], row| {
                index += 1;
                let mut values = row
                    .into_iter()
                    .map(|v| {
                        if v == DbValue::Null {
                            options.null.clone()
                        } else {
                            v.to_string()
                        }
                    })
                    .collect::<Vec<_>>();
                if options.row_header {
                    values.insert(0, index.to_string());
                }
                csv_write(&mut out, &values, options, &mut matrix, &mut retained).await
            })
            .await?;
        if options.transpose {
            for index in 0..matrix.first().map_or(0, Vec::len) {
                let values = matrix
                    .iter()
                    .map(|r| r.get(index).cloned().unwrap_or_default())
                    .collect::<Vec<_>>();
                out.write_all(csv_line(&values, options.delimiter).as_bytes())
                    .await?;
            }
        }
        out.flush().await?;
        finish(file).await
    }
}

// Values serialization can contain raw non-UTF-8 String bytes. Escape those
// bytes before putting SQL into a UTF-8 document or script; never replace them.
fn ascii_values(hex: &str) -> Result<String> {
    if !hex.len().is_multiple_of(2) || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::Invalid("Invalid serialized ClickHouse row".into()));
    }
    let mut sql = String::new();
    for i in (0..hex.len()).step_by(2) {
        let byte =
            u8::from_str_radix(&hex[i..i + 2], 16).map_err(|e| Error::Invalid(e.to_string()))?;
        if (32..127).contains(&byte) {
            sql.push(char::from(byte));
        } else {
            write!(sql, "\\x{byte:02X}").map_err(|e| Error::Invalid(e.to_string()))?;
        }
    }
    Ok(sql)
}
