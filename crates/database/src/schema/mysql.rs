//! MySQL/MariaDB dump through SHOW CREATE and a single `SQLx` connection.
use super::{Artifact, Connection, Error, ExportOptions, Pool, Result, finish, temporary};
use crate::connection::quote_identifier;
use futures_util::TryStreamExt;
use sqlx::{Column, Connection as _, Row};
use tokio::io::AsyncWriteExt;

type Writer = tokio::io::BufWriter<tokio::fs::File>;
impl Connection {
    #[allow(
        clippy::too_many_lines,
        reason = "Ordered schema/data/program phases share a consistent snapshot and output stream"
    )]
    pub(super) async fn mysql_dump(
        &self,
        schema: &str,
        options: Option<&ExportOptions>,
        ddl_only: bool,
    ) -> Result<Artifact> {
        let Pool::Mysql(pool) = &self.pool else {
            return Err(Error::Invalid("MySQL required".into()));
        };
        let ddl = options.is_none_or(|o| o.ddl);
        let target = options.filter(|o| o.only_table).map(|o| o.table.as_str());
        let file = temporary().await?;
        let mut out = Writer::new(tokio::fs::File::from_std(file.reopen()?));
        let mut connection = pool.acquire().await?;
        connection.close_on_drop();
        let q = |s: &str| quote_identifier(self.driver, s);
        sqlx::raw_sql(&format!("USE {}; SET NAMES utf8mb4; SET SQL_MODE='NO_AUTO_VALUE_ON_ZERO'; SET sql_quote_show_create=1; SET time_zone='+00:00'; SET TRANSACTION ISOLATION LEVEL REPEATABLE READ;",q(schema))).execute(&mut *connection).await?;
        let mut tx = connection.begin().await?;
        // Session settings make zero dates, auto-increment zero and escapes restorable.
        out.write_all(b"-- Nozhik SQL export (MySQL/MariaDB)\nSET @NOZHIK_OLD_FK=@@FOREIGN_KEY_CHECKS, @NOZHIK_OLD_MODE=@@SQL_MODE, @NOZHIK_OLD_TZ=@@TIME_ZONE;\nSET FOREIGN_KEY_CHECKS=0;\nSET SQL_MODE='NO_AUTO_VALUE_ON_ZERO';\nSET NAMES utf8mb4;\nSET TIME_ZONE='+00:00';\n").await?;
        if ddl && target.is_none() {
            let row = sqlx::query(&format!("SHOW CREATE DATABASE {}", q(schema)))
                .fetch_one(&mut *tx)
                .await?;
            let definition: String = row.try_get_unchecked(1)?;
            out.write_all(format!("{definition};\n").as_bytes()).await?;
        }
        out.write_all(format!("USE {};\n", q(schema)).as_bytes())
            .await?;
        let mut relations: Vec<(String,String)> = sqlx::query_as("SELECT CAST(TABLE_NAME AS CHAR), CAST(TABLE_TYPE AS CHAR) FROM information_schema.TABLES WHERE TABLE_SCHEMA=? ORDER BY TABLE_NAME").bind(schema).fetch_all(&mut *tx).await?;
        if let Some(table) = target {
            relations.retain(|(name, _)| name == table);
            if relations.is_empty() {
                return Err(Error::Invalid("Table no longer exists".into()));
            }
        }
        if ddl {
            for (name, kind) in &relations {
                if kind == "VIEW" {
                    // Placeholders let views reference each other in any catalog order.
                    let columns: Vec<(String,String)> = sqlx::query_as("SELECT CAST(COLUMN_NAME AS CHAR), CAST(COLUMN_TYPE AS CHAR) FROM information_schema.COLUMNS WHERE TABLE_SCHEMA=? AND TABLE_NAME=? ORDER BY ORDINAL_POSITION").bind(schema).bind(name).fetch_all(&mut *tx).await?;
                    let definitions = columns
                        .iter()
                        .map(|(n, t)| format!("{} {t}", q(n)))
                        .collect::<Vec<_>>()
                        .join(", ");
                    out.write_all(
                        format!("CREATE TABLE {}.{} ({definitions});\n", q(schema), q(name))
                            .as_bytes(),
                    )
                    .await?;
                } else {
                    let kind = if kind == "SEQUENCE" {
                        "SEQUENCE"
                    } else {
                        "TABLE"
                    };
                    let row = sqlx::query(&format!("SHOW CREATE {kind} {}.{}", q(schema), q(name)))
                        .fetch_one(&mut *tx)
                        .await?;
                    let definition: String = row.try_get_unchecked(1)?;
                    out.write_all(format!("{definition};\n").as_bytes()).await?;
                }
            }
            // Routines precede data (defaults may use stored functions).
            let routines: Vec<(String,String)> = sqlx::query_as("SELECT CAST(ROUTINE_NAME AS CHAR), CAST(ROUTINE_TYPE AS CHAR) FROM information_schema.ROUTINES WHERE ROUTINE_SCHEMA=? AND ? IS NULL ORDER BY ROUTINE_NAME").bind(schema).bind(target).fetch_all(&mut *tx).await?;
            for (name, kind) in routines {
                if !["FUNCTION", "PROCEDURE"].contains(&kind.as_str()) {
                    return Err(Error::Invalid(format!("Unsupported routine type: {kind}")));
                }
                let row = sqlx::query(&format!("SHOW CREATE {kind} {}.{}", q(schema), q(&name)))
                    .fetch_one(&mut *tx)
                    .await?;
                program(
                    &mut out,
                    &row,
                    if kind == "FUNCTION" {
                        "Create Function"
                    } else {
                        "Create Procedure"
                    },
                )
                .await?;
            }
        }
        if !ddl_only {
            out.write_all(b"START TRANSACTION;\n").await?;
            for (name, kind) in &relations {
                if kind == "VIEW" {
                    continue;
                }
                if kind == "SEQUENCE" {
                    let row = sqlx::query(&format!(
                        "SELECT next_not_cached_value, increment FROM {}.{}",
                        q(schema),
                        q(name)
                    ))
                    .fetch_one(&mut *tx)
                    .await?;
                    let value: i64 = row.try_get("next_not_cached_value")?;

                    out.write_all(
                        format!("SELECT SETVAL({}.{}, {value}, 0);\n", q(schema), q(name))
                            .as_bytes(),
                    )
                    .await?;
                    continue;
                }
                let columns: Vec<(String,String)> = sqlx::query_as("SELECT CAST(COLUMN_NAME AS CHAR),CAST(DATA_TYPE AS CHAR) FROM information_schema.COLUMNS WHERE TABLE_SCHEMA=? AND TABLE_NAME=? AND COALESCE(GENERATION_EXPRESSION,'')='' ORDER BY ORDINAL_POSITION").bind(schema).bind(name).fetch_all(&mut *tx).await?;
                if columns.is_empty() {
                    return Err(Error::Invalid(format!("No insertable columns in {name}")));
                }
                let names = columns
                    .iter()
                    .map(|(n, _)| q(n))
                    .collect::<Vec<_>>()
                    .join(", ");
                let projections = columns
                    .iter()
                    .map(|(n, t)| sql_literal(&q(n), t))
                    .collect::<Vec<_>>()
                    .join(", ");
                let sql = format!("SELECT {projections} FROM {}.{}", q(schema), q(name));
                let prefix = format!("INSERT INTO {}.{} ({names}) VALUES (", q(schema), q(name));
                let mut stream = sqlx::raw_sql(&sql).fetch(&mut *tx);
                while let Some(row) = stream.try_next().await? {
                    let values = (0..columns.len())
                        .map(|i| row.try_get_unchecked::<String, _>(i).map_err(Error::from))
                        .collect::<Result<Vec<_>>>()?;
                    out.write_all(format!("{prefix}{});\n", values.join(", ")).as_bytes())
                        .await?;
                }
            }
            out.write_all(b"COMMIT;\n").await?;
        }
        if ddl {
            for (name, kind) in &relations {
                if kind != "VIEW" {
                    continue;
                }
                let row = sqlx::query(&format!("SHOW CREATE VIEW {}.{}", q(schema), q(name)))
                    .fetch_one(&mut *tx)
                    .await?;
                let definition: String = row.try_get_unchecked(1)?;
                out.write_all(
                    format!("DROP TABLE {}.{};\n{definition};\n", q(schema), q(name)).as_bytes(),
                )
                .await?;
            }
            for (catalog, column, kind, definition) in [
                (
                    "TRIGGERS",
                    "TRIGGER_NAME",
                    "TRIGGER",
                    "SQL Original Statement",
                ),
                ("EVENTS", "EVENT_NAME", "EVENT", "Create Event"),
            ] {
                let schema_column = if kind == "TRIGGER" {
                    "TRIGGER_SCHEMA"
                } else {
                    "EVENT_SCHEMA"
                };
                let names: Vec<String> = sqlx::query_scalar(&format!("SELECT CAST({column} AS CHAR) FROM information_schema.{catalog} WHERE {schema_column}=?{} ORDER BY {column}",if kind=="TRIGGER" {" AND (? IS NULL OR EVENT_OBJECT_TABLE=?)"} else {" AND ? IS NULL AND ? IS NULL"})).bind(schema).bind(target).bind(target).fetch_all(&mut *tx).await?;
                for name in names {
                    let row =
                        sqlx::query(&format!("SHOW CREATE {kind} {}.{}", q(schema), q(&name)))
                            .fetch_one(&mut *tx)
                            .await?;
                    program(&mut out, &row, definition).await?;
                }
            }
        }
        out.write_all(b"SET FOREIGN_KEY_CHECKS=@NOZHIK_OLD_FK, SQL_MODE=@NOZHIK_OLD_MODE, TIME_ZONE=@NOZHIK_OLD_TZ;\n").await?;
        out.flush().await?;
        tx.commit().await?;
        finish(file).await
    }
}
async fn program(out: &mut Writer, row: &sqlx::mysql::MySqlRow, column: &str) -> Result<()> {
    let index = row
        .columns()
        .iter()
        .position(|c| c.name().eq_ignore_ascii_case(column))
        .ok_or_else(|| {
            Error::Invalid(format!(
                "Missing {column}; server columns: {:?}",
                row.columns().iter().map(Column::name).collect::<Vec<_>>()
            ))
        })?;
    let definition = row
        .try_get_unchecked::<Option<String>, _>(index)?
        .ok_or_else(|| {
            Error::Invalid(format!(
                "Server did not disclose {column}; export requires SHOW CREATE privileges"
            ))
        })?;
    let mode_index = row
        .columns()
        .iter()
        .position(|c| c.name().eq_ignore_ascii_case("sql_mode"))
        .ok_or_else(|| Error::Invalid("Server omitted routine SQL mode".into()))?;
    let mode: String = row.try_get_unchecked(mode_index)?;
    if !mode
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ',')
    {
        return Err(Error::Invalid("Invalid server SQL mode".into()));
    }
    let mut delimiter = "NOZHIK_END".to_owned();
    while definition.contains(&delimiter) {
        delimiter.push('_');
    }
    out.write_all(format!("SET SQL_MODE='{mode}';\nDELIMITER {delimiter}\n{definition}{delimiter}\nDELIMITER ;\nSET SQL_MODE='NO_AUTO_VALUE_ON_ZERO';\n").as_bytes()).await?;
    Ok(())
}

fn is_binary(kind: &str) -> bool {
    [
        "binary",
        "varbinary",
        "tinyblob",
        "blob",
        "mediumblob",
        "longblob",
        "bit",
    ]
    .contains(&kind)
}
fn is_geometry(kind: &str) -> bool {
    [
        "geometry",
        "point",
        "linestring",
        "polygon",
        "multipoint",
        "multilinestring",
        "multipolygon",
        "geometrycollection",
    ]
    .contains(&kind)
}
fn sql_literal(column: &str, kind: &str) -> String {
    let value = if is_geometry(kind) {
        format!(
            "CONCAT('ST_GeomFromWKB(X',CHAR(39),HEX(ST_AsWKB({column})),CHAR(39),', ',ST_SRID({column}),')')"
        )
    } else if is_binary(kind) {
        format!("CONCAT('X',CHAR(39),HEX({column}),CHAR(39))")
    } else if [
        "tinyint",
        "smallint",
        "mediumint",
        "int",
        "integer",
        "bigint",
        "float",
        "double",
        "decimal",
        "numeric",
        "year",
    ]
    .contains(&kind)
    {
        format!("CAST({column} AS CHAR)")
    } else {
        format!(
            "CONCAT('CONVERT(X',CHAR(39),HEX(CONVERT({column} USING utf8mb4)),CHAR(39),' USING utf8mb4)')"
        )
    };
    format!("IF({column} IS NULL,'NULL',{value})")
}
pub(super) fn csv_projection(column: &str, kind: &str) -> String {
    if is_binary(kind) {
        format!("CONCAT('0x',HEX({column}))")
    } else if is_geometry(kind) {
        format!("CONCAT('SRID=',ST_SRID({column}),';',ST_AsText({column}))")
    } else {
        format!("CAST({column} AS CHAR CHARACTER SET utf8mb4)")
    }
}
