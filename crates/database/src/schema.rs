//! Schema settings, catalog DDL dumps, streaming exports and script execution.
use crate::{
    CancellationToken, Connection, DbDriverType, DbValue, Error, Result,
    connection::{Pool, quote_identifier},
};
use futures_util::TryStreamExt;
use sqlx::Row;
use std::{path::PathBuf, sync::Arc};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
mod clickhouse;
mod mysql;
mod postgres;
mod script;
use script::script_batches;

/// Editable schema privilege. Accounts and identifiers are quoted by the backend.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grant {
    /// PostgreSQL role or MySQL account user.
    pub user: String,
    /// MySQL account host; unused by PostgreSQL.
    pub host: String,
    /// One privilege from the engine's allowlist.
    pub privilege: String,
}
/// Snapshot and draft of persistent schema settings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Settings {
    /// Namespace name (MySQL/SQLite names are not renameable).
    pub name: String,
    /// MySQL default collation.
    pub collation: String,
    /// Available server collations.
    pub collations: Vec<String>,
    /// PostgreSQL owner.
    pub owner: String,
    /// PostgreSQL schema comment.
    pub comment: String,
    /// SQLite application-defined file version.
    pub user_version: i32,
    /// SQLite application file identifier.
    pub application_id: i32,
    /// Explicit schema privileges visible to this login.
    pub grants: Vec<Grant>,
}
/// Export serialization.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Format {
    /// Native SQL statements.
    #[default]
    Sql,
    /// CSV for one selected table.
    Csv,
}
/// Export options shared with the preview and final output.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "Independent export checkboxes, not mutually exclusive states"
)]
pub struct ExportOptions {
    /// Output syntax.
    pub format: Format,
    /// Optional relation; CSV requires one, SQL exports the whole schema.
    pub table: String,
    /// Restrict SQL/DDL to the named table.
    pub only_table: bool,
    /// Include definitions with SQL data.
    pub ddl: bool,
    /// Include generated/computed columns in CSV (SQL dump tools handle these themselves).
    pub generated: bool,
    /// CSV column names.
    pub column_header: bool,
    /// CSV one-based row numbers.
    pub row_header: bool,
    /// Transpose the CSV matrix (bounded to 16 MiB).
    pub transpose: bool,
    /// CSV field separator.
    pub delimiter: char,
    /// CSV representation of SQL NULL.
    pub null: String,
}
impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            format: Format::Sql,
            table: String::new(),
            only_table: false,
            ddl: true,
            generated: true,
            column_header: true,
            row_header: false,
            transpose: false,
            delimiter: ',',
            null: String::new(),
        }
    }
}
/// Temporary complete export; dropping the last owner removes it.
pub struct Artifact {
    file: tempfile::NamedTempFile,
    /// Bounded UTF-8 prefix for display, never used as the final export.
    pub preview: String,
    /// Complete file size in bytes.
    pub bytes: u64,
}
/// Background schema action.
pub enum Task {
    /// Load one editable table definition.
    LoadTable(String),
    /// Create a new table using the shared preview plan.
    CreateTable(Box<crate::table_design::Definition>),
    /// Apply a table draft after checking its original snapshot.
    ModifyTable(
        Box<crate::table_design::Definition>,
        Box<crate::table_design::Definition>,
    ),
    /// Fetch current settings.
    Settings,
    /// Clear a `ClickHouse` table without row mutation or transactional promises.
    Truncate(String),
    /// Apply the exact validated diff shown in the preview.
    Modify(Box<Settings>, Box<Settings>),
    /// Create DDL only for the selected table.
    TableDdl(String),
    /// Create a complete schema DDL artifact.
    Ddl,
    /// Create a complete export artifact.
    Export(ExportOptions),
    /// Execute a UTF-8 SQL file without retaining query results.
    RunFile(PathBuf),
    /// Atomically save an already generated artifact.
    Save(Arc<Artifact>, PathBuf),
    /// Read a generated artifact for clipboard transfer (maximum 32 MiB).
    Clipboard(Arc<Artifact>),
}
/// Background schema result.
pub enum Output {
    /// Editable table snapshot.
    Table(Box<crate::table_design::Definition>),
    /// Current editable values.
    Settings(Settings),
    /// Complete export plus bounded preview.
    Artifact(Arc<Artifact>),
    /// Clipboard text.
    Clipboard(String),
    /// Operation succeeded.
    Done(String),
}
/// Editable privilege choices for one dialect.
#[must_use]
pub const fn privileges(driver: DbDriverType) -> &'static [&'static str] {
    match driver {
        DbDriverType::Mysql => &[
            "SELECT",
            "INSERT",
            "UPDATE",
            "DELETE",
            "CREATE",
            "DROP",
            "ALTER",
            "INDEX",
            "REFERENCES",
            "CREATE VIEW",
            "SHOW VIEW",
            "TRIGGER",
            "EVENT",
            "EXECUTE",
            "CREATE ROUTINE",
            "ALTER ROUTINE",
            "CREATE TEMPORARY TABLES",
            "LOCK TABLES",
        ],
        DbDriverType::Postgres => &["USAGE", "CREATE"],
        DbDriverType::Sqlite | DbDriverType::ClickHouse => &[],
    }
}
/// Build the SQL displayed by Modify Schema, with dialect quoting and validation.
/// # Errors
/// Rejects unsupported renames, invalid collations/accounts and privileges.
#[allow(
    clippy::too_many_lines,
    reason = "Three engine-specific settings diffs share quoting and privilege validation"
)]
pub fn modification_sql(driver: DbDriverType, old: &Settings, new: &Settings) -> Result<String> {
    let q = |s: &str| quote_identifier(driver, s);
    if new.name.is_empty() || new.name.chars().any(char::is_control) {
        return Err(Error::Invalid(
            "Schema name is required and cannot contain control characters".into(),
        ));
    }
    let mut sql = Vec::new();
    match driver {
        DbDriverType::ClickHouse => {
            return Err(Error::Invalid(
                "Use ClickHouse DDL in the SQL console".into(),
            ));
        }
        DbDriverType::Mysql => {
            if old.name != new.name {
                return Err(Error::Invalid(
                    "MySQL/MariaDB cannot rename a database with ALTER DATABASE".into(),
                ));
            }
            if old.collation != new.collation {
                if !old.collations.contains(&new.collation) {
                    return Err(Error::Invalid("Choose a server collation".into()));
                }
                sql.push(format!(
                    "ALTER DATABASE {} COLLATE {};",
                    q(&old.name),
                    q(&new.collation)
                ));
            }
        }
        DbDriverType::Postgres => {
            if old.comment != new.comment {
                sql.push(format!(
                    "COMMENT ON SCHEMA {} IS {};",
                    q(&old.name),
                    crate::changes::literal(driver, &DbValue::Text(new.comment.clone()))?
                ));
            }
        }
        DbDriverType::Sqlite => {
            if old.name != new.name {
                return Err(Error::Invalid(
                    "SQLite database aliases cannot be renamed".into(),
                ));
            }
            for (name, before, after) in [
                ("user_version", old.user_version, new.user_version),
                ("application_id", old.application_id, new.application_id),
            ] {
                if before != after {
                    sql.push(format!("PRAGMA {}.{name} = {after};", q(&old.name)));
                }
            }
        }
    }
    for (removed, grants) in [(true, &old.grants), (false, &new.grants)] {
        let other = if removed { &new.grants } else { &old.grants };
        for grant in grants.iter().filter(|g| !other.contains(g)) {
            if !privileges(driver).contains(&grant.privilege.as_str())
                || grant.user.is_empty()
                || grant
                    .user
                    .chars()
                    .chain(grant.host.chars())
                    .any(char::is_control)
            {
                return Err(Error::Invalid(
                    "Choose a valid privilege and account".into(),
                ));
            }
            let account = if driver == DbDriverType::Mysql {
                format!("{}@{}", q(&grant.user), q(&grant.host))
            } else if grant.user == "PUBLIC" {
                "PUBLIC".into()
            } else {
                q(&grant.user)
            };
            let object = if driver == DbDriverType::Mysql {
                format!("{}.*", q(&old.name))
            } else {
                format!("SCHEMA {}", q(&old.name))
            };
            sql.push(if removed {
                format!("REVOKE {} ON {object} FROM {account};", grant.privilege)
            } else {
                format!("GRANT {} ON {object} TO {account};", grant.privilege)
            });
        }
    }
    if driver == DbDriverType::Postgres {
        if old.name != new.name {
            sql.push(format!(
                "ALTER SCHEMA {} RENAME TO {};",
                q(&old.name),
                q(&new.name)
            ));
        }
        if old.owner != new.owner {
            if new.owner.is_empty() {
                return Err(Error::Invalid("Owner is required".into()));
            }
            sql.push(format!(
                "ALTER SCHEMA {} OWNER TO {};",
                q(&new.name),
                q(&new.owner)
            ));
        }
    }
    Ok(sql.join("\n"))
}
impl Connection {
    /// Execute a cancellable schema operation on this connection's transport.
    /// # Errors
    /// Reports SQL, filesystem, catalog, serialization or validation failures.
    #[allow(
        clippy::too_many_lines,
        reason = "Typed dispatch keeps cancellation and artifact ownership in one match"
    )]
    pub async fn schema_task(
        &self,
        schema: &str,
        task: Task,
        cancel: &CancellationToken,
    ) -> Result<Output> {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        match task {
            Task::LoadTable(table) => Ok(Output::Table(Box::new(
                cancellable(cancel, self.table_definition(schema, &table)).await?,
            ))),
            Task::CreateTable(new) => {
                if new.schema != schema {
                    return Err(Error::Invalid("Table schema changed".into()));
                }
                Ok(Output::Table(Box::new(
                    self.create_table(&new, cancel).await?,
                )))
            }
            Task::ModifyTable(old, new) => {
                if old.schema != schema {
                    return Err(Error::Invalid("Table schema changed".into()));
                }
                Ok(Output::Table(Box::new(
                    self.modify_table(&old, &new, cancel).await?,
                )))
            }
            Task::Settings => Ok(Output::Settings(
                cancellable(cancel, self.schema_settings(schema)).await?,
            )),
            Task::Modify(old, new) => {
                if old.name != schema {
                    return Err(Error::Invalid("Schema target changed".into()));
                }
                let current = cancellable(cancel, self.schema_settings(schema)).await?;
                if current != *old {
                    return Err(Error::Invalid(
                        "Schema settings changed on the server; reopen Modify Schema".into(),
                    ));
                }
                let sql = modification_sql(self.driver, &old, &new)?;
                if sql.is_empty() {
                    return Err(Error::Invalid("No changes".into()));
                }
                self.execute_schema_script(&sql, schema, cancel).await?;
                Ok(Output::Done("Schema updated".into()))
            }
            Task::Truncate(table) => {
                if self.driver != DbDriverType::ClickHouse {
                    return Err(Error::Invalid("TRUNCATE action requires ClickHouse".into()));
                }
                let q = |s: &str| quote_identifier(self.driver, s);
                self.query(
                    &format!("TRUNCATE TABLE {}.{} SYNC", q(schema), q(&table)),
                    Some(schema),
                    cancel,
                )
                .await?;
                Ok(Output::Done(
                    "Table cleared; reload open table pages to see current data".into(),
                ))
            }
            Task::RunFile(path) => {
                let file =
                    cancellable(cancel, async { Ok(tokio::fs::File::open(path).await?) }).await?;
                let mut sql = String::new();
                cancellable(cancel, async {
                    Ok(file
                        .take(64 * 1024 * 1024 + 1)
                        .read_to_string(&mut sql)
                        .await?)
                })
                .await?;
                if sql.len() > 64 * 1024 * 1024 {
                    return Err(Error::Invalid("SQL script exceeds 64 MiB".into()));
                }
                self.execute_schema_script(sql.trim_start_matches('\u{feff}'), schema, cancel)
                    .await?;
                Ok(Output::Done("SQL script completed".into()))
            }
            Task::TableDdl(table) => {
                let options = ExportOptions {
                    table,
                    only_table: true,
                    ..ExportOptions::default()
                };
                Ok(Output::Artifact(Arc::new(
                    cancellable(cancel, self.dump(schema, Some(&options), true)).await?,
                )))
            }
            Task::Ddl => Ok(Output::Artifact(Arc::new(
                cancellable(cancel, self.dump(schema, None, true)).await?,
            ))),
            Task::Export(options) => {
                let artifact = if options.format == Format::Csv {
                    cancellable(cancel, self.csv_export(schema, &options)).await?
                } else {
                    cancellable(cancel, self.dump(schema, Some(&options), false)).await?
                };
                Ok(Output::Artifact(Arc::new(artifact)))
            }
            Task::Clipboard(artifact) => {
                if artifact.bytes > 32 * 1024 * 1024 {
                    return Err(Error::Invalid(
                        "Clipboard limit is 32 MiB; export to a file instead".into(),
                    ));
                }
                Ok(Output::Clipboard(
                    tokio::fs::read_to_string(artifact.file.path()).await?,
                ))
            }
            Task::Save(artifact, destination) => {
                let cancel = cancel.clone();
                tokio::task::spawn_blocking(move || -> Result<()> {
                    let parent = destination
                        .parent()
                        .ok_or_else(|| Error::Invalid("Choose an output directory".into()))?;
                    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
                    std::io::copy(&mut artifact.file.reopen()?, &mut temporary)?;
                    temporary.as_file().sync_all()?;
                    if cancel.is_cancelled() {
                        return Err(Error::Cancelled);
                    }
                    temporary.persist(destination).map_err(|e| e.error)?;
                    Ok(())
                })
                .await
                .map_err(|e| Error::Invalid(e.to_string()))??;
                Ok(Output::Done("Export saved".into()))
            }
        }
    }
    async fn schema_settings(&self, schema: &str) -> Result<Settings> {
        let mut settings = Settings {
            name: schema.into(),
            ..Settings::default()
        };
        match &self.pool {
            Pool::ClickHouse(_) => {
                return Err(Error::Invalid(
                    "Use ClickHouse DDL in the SQL console".into(),
                ));
            }
            Pool::Mysql(pool) => {
                settings.collation = sqlx::query_scalar("SELECT CAST(DEFAULT_COLLATION_NAME AS CHAR) FROM information_schema.SCHEMATA WHERE SCHEMA_NAME=?").bind(schema).fetch_one(pool).await?;
                settings.collations = sqlx::query_scalar("SELECT CAST(COLLATION_NAME AS CHAR) FROM information_schema.COLLATIONS ORDER BY COLLATION_NAME").fetch_all(pool).await?;
                for (account, privilege) in sqlx::query_as::<_, (String, String)>("SELECT CAST(GRANTEE AS CHAR), CAST(PRIVILEGE_TYPE AS CHAR) FROM information_schema.SCHEMA_PRIVILEGES WHERE TABLE_SCHEMA=? ORDER BY GRANTEE,PRIVILEGE_TYPE").bind(schema).fetch_all(pool).await? {
                    let (user, host) = mysql_account(&account)?;
                    settings.grants.push(Grant { user, host, privilege });
                }
            }
            Pool::Postgres(pool) => {
                let row = sqlx::query("SELECT pg_get_userbyid(nspowner) AS owner, COALESCE(obj_description(oid,'pg_namespace'),'') AS comment FROM pg_namespace WHERE nspname=$1").bind(schema).fetch_one(pool).await?;
                settings.owner = row.try_get("owner")?;
                settings.comment = row.try_get("comment")?;
                for (user, privilege) in sqlx::query_as::<_, (String, String)>("SELECT CASE WHEN a.grantee=0 THEN 'PUBLIC' ELSE pg_get_userbyid(a.grantee) END, a.privilege_type FROM pg_namespace n CROSS JOIN LATERAL aclexplode(n.nspacl) a WHERE n.nspname=$1 ORDER BY 1,2").bind(schema).fetch_all(pool).await? {
                    settings.grants.push(Grant { user, host: String::new(), privilege });
                }
            }
            Pool::Sqlite(pool) => {
                let q = quote_identifier(self.driver, schema);
                let mut connection = pool.acquire().await?;
                connection.lock_handle().await?.remove_progress_handler();
                settings.user_version = sqlx::query_scalar(&format!("PRAGMA {q}.user_version"))
                    .fetch_one(&mut *connection)
                    .await?;
                settings.application_id = sqlx::query_scalar(&format!("PRAGMA {q}.application_id"))
                    .fetch_one(&mut *connection)
                    .await?;
            }
        }
        Ok(settings)
    }
    pub(crate) async fn execute_schema_script(
        &self,
        sql: &str,
        schema: &str,
        cancel: &CancellationToken,
    ) -> Result<()> {
        // execute() drains every result, including SELECTs larger than console limits.
        // Server connections are retired even after errors/cancellation to avoid leaking
        // USE/search_path, transactions or unfinished protocol state to other tabs.
        macro_rules! run {
            ($pool:expr, $setup:expr, $server:expr) => {{
                let mut connection = $pool.acquire().await?;
                if $server { connection.close_on_drop(); }
                let work = async {
                    if let Some(setup) = $setup { sqlx::raw_sql(&setup).execute(&mut *connection).await?; }
                    for batch in script_batches(sql, self.driver)? { sqlx::raw_sql(&batch).execute(&mut *connection).await?; }
                    Ok(())
                };
                tokio::select! { biased; () = cancel.cancelled() => Err(Error::Cancelled), result = work => result }
            }};
        }
        let q = quote_identifier(self.driver, schema);
        match &self.pool {
            Pool::ClickHouse(client) => client.execute(sql, schema, cancel).await,
            Pool::Postgres(p) => run!(p, Some(format!("SET search_path TO {q}")), true),
            Pool::Mysql(p) => run!(p, Some(format!("USE {q}")), true),
            Pool::Sqlite(p) => {
                if schema != "main" {
                    return Err(Error::Invalid("SQLite scripts require the main database; attached aliases are connection-local".into()));
                }
                let mut connection = p.acquire().await?;
                let token = cancel.clone();
                connection
                    .lock_handle()
                    .await?
                    .set_progress_handler(1000, move || !token.is_cancelled());
                let result = sqlx::raw_sql(sql).execute(&mut *connection).await;
                connection.lock_handle().await?.remove_progress_handler();
                // Raw BEGIN is not tracked by SQLx. A successful ROLLBACK means
                // the script left a transaction open; report that its changes were discarded.
                let unfinished = sqlx::raw_sql("ROLLBACK")
                    .execute(&mut *connection)
                    .await
                    .is_ok();
                if result.is_ok() && unfinished {
                    return Err(Error::Invalid(
                        "Script left a transaction open; it has been rolled back".into(),
                    ));
                }
                if cancel.is_cancelled() {
                    Err(Error::Cancelled)
                } else {
                    result.map(|_| ()).map_err(Error::from)
                }
            }
        }
    }
}
fn mysql_account(value: &str) -> Result<(String, String)> {
    let parts = value
        .strip_prefix('\'')
        .and_then(|s| s.strip_suffix('\''))
        .and_then(|s| s.rsplit_once("'@'"));
    parts
        .map(|(u, h)| (u.replace("''", "'"), h.replace("''", "'")))
        .ok_or_else(|| Error::Invalid("Cannot decode server grant account".into()))
}
async fn cancellable<T>(
    cancel: &CancellationToken,
    operation: impl std::future::Future<Output = Result<T>>,
) -> Result<T> {
    tokio::select! { biased; () = cancel.cancelled() => Err(Error::Cancelled), result = operation => result }
}
async fn temporary() -> Result<tempfile::NamedTempFile> {
    tokio::task::spawn_blocking(tempfile::NamedTempFile::new)
        .await
        .map_err(|e| Error::Invalid(e.to_string()))?
        .map_err(Error::from)
}
async fn finish(file: tempfile::NamedTempFile) -> Result<Artifact> {
    let mut input = tokio::fs::File::open(file.path()).await?;
    let bytes = input.metadata().await?.len();
    let mut prefix =
        vec![0; usize::try_from(bytes.min(64 * 1024)).map_err(|e| Error::Invalid(e.to_string()))?];
    input.read_exact(&mut prefix).await?;
    let mut preview = String::from_utf8_lossy(&prefix).into_owned();
    if bytes > 64 * 1024 {
        preview.push_str("\n… Preview limited to 64 KiB; the file contains the complete export.");
    }
    Ok(Artifact {
        file,
        preview,
        bytes,
    })
}
impl Connection {
    async fn dump(
        &self,
        schema: &str,
        options: Option<&ExportOptions>,
        ddl_only: bool,
    ) -> Result<Artifact> {
        match &self.pool {
            Pool::ClickHouse(client) => {
                self.clickhouse_dump(client, schema, options, ddl_only)
                    .await
            }
            Pool::Sqlite(_) => self.sqlite_dump(schema, options, ddl_only).await,
            Pool::Mysql(_) => self.mysql_dump(schema, options, ddl_only).await,
            Pool::Postgres(_) => self.postgres_dump(schema, options, ddl_only).await,
        }
    }
    #[allow(
        clippy::too_many_lines,
        reason = "Schema/table export shares one SQLite snapshot and ordered restoration stream"
    )]
    async fn sqlite_dump(
        &self,
        schema: &str,
        options: Option<&ExportOptions>,
        ddl_only: bool,
    ) -> Result<Artifact> {
        let Pool::Sqlite(pool) = &self.pool else {
            return Err(Error::Invalid("SQLite required".into()));
        };
        let file = temporary().await?;
        let mut out = tokio::io::BufWriter::new(tokio::fs::File::from_std(file.reopen()?));
        let mut connection = pool.acquire().await?;
        connection.lock_handle().await?.remove_progress_handler();
        let mut transaction = sqlx::Connection::begin(&mut *connection).await?;
        let q = quote_identifier(self.driver, schema);
        let target = options.filter(|o| o.only_table).map(|o| o.table.as_str());
        let (mut objects, rowid_tables) =
            sqlite_dump_catalog(&mut transaction, &q, ddl_only, target).await?;
        if let Some(table) = target {
            let names = sqlx::query_scalar::<_, String>(&format!(
                "SELECT name FROM {q}.sqlite_master WHERE tbl_name=?"
            ))
            .bind(table)
            .fetch_all(&mut *transaction)
            .await?
            .into_iter()
            .collect::<std::collections::HashSet<_>>();
            objects.retain(|(_, name, _)| names.contains(name));
            if objects.is_empty() {
                return Err(Error::Invalid("Table no longer exists".into()));
            }
        }
        let ddl = options.is_none_or(|o| o.ddl);
        out.write_all(b"PRAGMA foreign_keys=OFF;\nBEGIN TRANSACTION;\n")
            .await?;
        for (kind, name, definition) in &objects {
            if kind != "table" {
                continue;
            }
            if ddl {
                out.write_all(format!("{definition};\n").as_bytes()).await?;
            }
            if ddl_only {
                continue;
            }
            let mut columns = sqlite_columns(&mut transaction, schema, name, false).await?;
            if rowid_tables.contains(name)
                && let Some(alias) = ["rowid", "_rowid_", "oid"]
                    .iter()
                    .find(|alias| !columns.iter().any(|c| c.eq_ignore_ascii_case(alias)))
            {
                columns.insert(0, (*alias).into());
            }
            let names = columns
                .iter()
                .map(|c| quote_identifier(self.driver, c))
                .collect::<Vec<_>>()
                .join(", ");
            let query = format!(
                "SELECT {names} FROM {q}.{}",
                quote_identifier(self.driver, name)
            );
            let mut rows = sqlx::raw_sql(&query).fetch(&mut *transaction);
            while let Some(row) = rows.try_next().await? {
                let values = (0..columns.len())
                    .map(|i| {
                        crate::worker::sqlite_value(&row, i)
                            .and_then(|v| crate::changes::literal(self.driver, &v))
                    })
                    .collect::<Result<Vec<_>>>()?;
                out.write_all(
                    format!(
                        "INSERT INTO {} ({names}) VALUES ({});\n",
                        quote_identifier(self.driver, name),
                        values.join(", ")
                    )
                    .as_bytes(),
                )
                .await?;
            }
        }
        if ddl {
            // Triggers follow data so restoring does not fire them twice.
            for (kind, _, definition) in &objects {
                if kind != "table" {
                    out.write_all(format!("{definition};\n").as_bytes()).await?;
                }
            }
            if let Some(table) = target {
                if !ddl_only
                    && objects
                        .iter()
                        .any(|(_, _, sql)| sql.to_ascii_uppercase().contains("AUTOINCREMENT"))
                {
                    let literal =
                        crate::changes::literal(self.driver, &DbValue::Text(table.into()))?;
                    if let Some(seq) = sqlx::query_scalar::<_, i64>(&format!(
                        "SELECT seq FROM {q}.sqlite_sequence WHERE name=?"
                    ))
                    .bind(table)
                    .fetch_optional(&mut *transaction)
                    .await?
                    {
                        out.write_all(format!("DELETE FROM sqlite_sequence WHERE name={literal};\nINSERT INTO sqlite_sequence(name,seq) VALUES({literal},{seq});\n").as_bytes()).await?;
                    }
                }
            } else {
                sqlite_restore_state(&mut transaction, &mut out, &q, !ddl_only).await?;
            }
        }
        out.write_all(b"COMMIT;\nPRAGMA foreign_keys=ON;\n").await?;
        out.flush().await?;
        transaction.commit().await?;
        finish(file).await
    }
    async fn csv_export(&self, schema: &str, options: &ExportOptions) -> Result<Artifact> {
        if options.table.is_empty() || ![',', ';', '\t', '|'].contains(&options.delimiter) {
            return Err(Error::Invalid("Select a table and CSV delimiter".into()));
        }
        let file = temporary().await?;
        let mut out = tokio::io::BufWriter::new(tokio::fs::File::from_std(file.reopen()?));
        let q = |s: &str| quote_identifier(self.driver, s);
        let mut matrix: Vec<Vec<String>> = Vec::new();
        let mut retained = 0usize;
        macro_rules! stream {
            ($pool:expr, $columns:expr, $projection:expr, $decode:path) => {{
                let mut connection = $pool.acquire().await?;
                let columns: Vec<String> = $columns;
                if columns.is_empty() {
                    return Err(Error::Invalid("No exportable columns".into()));
                }
                if options.column_header {
                    let mut header = columns.clone();
                    if options.row_header {
                        header.insert(0, "#".into());
                    }
                    csv_write(&mut out, &header, options, &mut matrix, &mut retained).await?;
                }
                let query = format!(
                    "SELECT {} FROM {}.{}",
                    $projection,
                    q(schema),
                    q(&options.table)
                );
                let mut rows = sqlx::raw_sql(&query).fetch(&mut *connection);
                let mut index = 0u64;
                while let Some(row) = rows.try_next().await? {
                    index += 1;
                    let mut values = (0..columns.len())
                        .map(|i| {
                            $decode(&row, i).map(|v| {
                                if v == DbValue::Null {
                                    options.null.clone()
                                } else {
                                    v.to_string()
                                }
                            })
                        })
                        .collect::<Result<Vec<_>>>()?;
                    if options.row_header {
                        values.insert(0, index.to_string());
                    }
                    csv_write(&mut out, &values, options, &mut matrix, &mut retained).await?;
                }
            }};
        }
        match &self.pool {
            Pool::ClickHouse(client) => {
                return self.clickhouse_csv_export(client, schema, options).await;
            }
            Pool::Mysql(pool) => {
                let columns: Vec<(String,String)> = sqlx::query_as("SELECT CAST(COLUMN_NAME AS CHAR), CAST(DATA_TYPE AS CHAR) FROM information_schema.COLUMNS WHERE TABLE_SCHEMA=? AND TABLE_NAME=? AND (? OR COALESCE(GENERATION_EXPRESSION,'')='') ORDER BY ORDINAL_POSITION").bind(schema).bind(&options.table).bind(options.generated).fetch_all(pool).await?;
                let projection = columns
                    .iter()
                    .map(|(n, t)| mysql::csv_projection(&q(n), t))
                    .collect::<Vec<_>>()
                    .join(", ");
                let names = columns.into_iter().map(|(n, _)| n).collect::<Vec<_>>();
                stream!(pool, names, projection, crate::worker::mysql_value);
            }
            Pool::Postgres(pool) => {
                let columns = sqlx::query_scalar::<_, String>("SELECT a.attname::text FROM pg_attribute a JOIN pg_class c ON c.oid=a.attrelid JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=$1 AND c.relname=$2 AND a.attnum>0 AND NOT a.attisdropped AND ($3 OR a.attgenerated='') ORDER BY a.attnum").bind(schema).bind(&options.table).bind(options.generated).fetch_all(pool).await?;
                let projection = columns
                    .iter()
                    .map(|n| format!("CAST({} AS text)", q(n)))
                    .collect::<Vec<_>>()
                    .join(", ");
                stream!(pool, columns, projection, crate::worker::pg_value);
            }
            Pool::Sqlite(pool) => {
                {
                    let mut connection = pool.acquire().await?;
                    connection.lock_handle().await?.remove_progress_handler();
                }
                let columns = {
                    let mut c = pool.acquire().await?;
                    sqlite_columns(&mut c, schema, &options.table, options.generated).await?
                };
                let projection = columns.iter().map(|n| q(n)).collect::<Vec<_>>().join(", ");
                stream!(pool, columns, projection, crate::worker::sqlite_value);
            }
        }
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

async fn sqlite_restore_state(
    connection: &mut sqlx::SqliteConnection,
    out: &mut tokio::io::BufWriter<tokio::fs::File>,
    q: &str,
    data: bool,
) -> Result<()> {
    for pragma in ["user_version", "application_id"] {
        let value: i32 = sqlx::query_scalar(&format!("PRAGMA {q}.{pragma}"))
            .fetch_one(&mut *connection)
            .await?;
        out.write_all(format!("PRAGMA {pragma}={value};\n").as_bytes())
            .await?;
    }
    // AUTOINCREMENT high-water marks can exceed the current maximum ID.
    let has_sequence: i64 = sqlx::query_scalar(&format!(
        "SELECT count(*) FROM {q}.sqlite_master WHERE name='sqlite_sequence'"
    ))
    .fetch_one(&mut *connection)
    .await?;
    if has_sequence > 0 && data {
        out.write_all(b"DELETE FROM sqlite_sequence;\n").await?;
        for (name, seq) in
            sqlx::query_as::<_, (String, i64)>(&format!("SELECT name,seq FROM {q}.sqlite_sequence"))
                .fetch_all(&mut *connection)
                .await?
        {
            out.write_all(
                format!(
                    "INSERT INTO sqlite_sequence VALUES ({}, {seq});\n",
                    crate::changes::literal(DbDriverType::Sqlite, &DbValue::Text(name))?
                )
                .as_bytes(),
            )
            .await?;
        }
    }
    Ok(())
}
type SqliteObjects = Vec<(String, String, String)>;
async fn sqlite_dump_catalog(
    connection: &mut sqlx::SqliteConnection,
    q: &str,
    ddl_only: bool,
    target: Option<&str>,
) -> Result<(SqliteObjects, std::collections::HashSet<String>)> {
    let objects: Vec<(String, String, String)> = sqlx::query_as(&format!("SELECT type,name,sql FROM {q}.sqlite_master WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%' AND (? IS NULL OR tbl_name=?) ORDER BY CASE type WHEN 'table' THEN 0 WHEN 'view' THEN 1 ELSE 2 END,name")).bind(target).bind(target).fetch_all(&mut *connection).await?;
    let tables = sqlx::query(&format!("PRAGMA {q}.table_list"))
        .fetch_all(&mut *connection)
        .await?;
    let mut shadows = std::collections::HashSet::new();
    let mut rowid_tables = std::collections::HashSet::new();
    for table in tables {
        let kind: String = table.try_get("type")?;
        let name: String = table.try_get("name")?;
        if kind == "shadow" {
            shadows.insert(name);
        } else if kind == "table" && table.try_get::<i64, _>("wr")? == 0 {
            rowid_tables.insert(name);
        }
    }
    let objects: Vec<_> = objects
        .into_iter()
        .filter(|(_, name, _)| !shadows.contains(name))
        .collect();
    if !ddl_only
        && objects.iter().any(|(_, _, definition)| {
            definition
                .to_ascii_uppercase()
                .starts_with("CREATE VIRTUAL TABLE")
        })
    {
        return Err(Error::Invalid("SQLite virtual tables need module-specific data restoration; use CSV for their visible rows or Generate DDL for definitions".into()));
    }
    Ok((objects, rowid_tables))
}
async fn sqlite_columns(
    connection: &mut sqlx::SqliteConnection,
    schema: &str,
    table: &str,
    generated: bool,
) -> Result<Vec<String>> {
    let rows = sqlx::query(&format!(
        "PRAGMA {}.table_xinfo({})",
        quote_identifier(DbDriverType::Sqlite, schema),
        quote_identifier(DbDriverType::Sqlite, table)
    ))
    .fetch_all(connection)
    .await?;
    let mut columns = Vec::new();
    for row in rows {
        let hidden: i64 = row.try_get("hidden")?;
        if hidden == 0 || (generated && hidden > 1) {
            columns.push(row.try_get("name")?);
        }
    }
    Ok(columns)
}
async fn csv_write(
    out: &mut tokio::io::BufWriter<tokio::fs::File>,
    values: &[String],
    options: &ExportOptions,
    matrix: &mut Vec<Vec<String>>,
    retained: &mut usize,
) -> Result<()> {
    if options.transpose {
        *retained += values.iter().map(|s| s.len() + 24).sum::<usize>();
        if *retained > 16 * 1024 * 1024 {
            return Err(Error::Invalid(
                "Transposed CSV exceeds 16 MiB; disable Transpose for streaming export".into(),
            ));
        }
        matrix.push(values.to_vec());
    } else {
        out.write_all(csv_line(values, options.delimiter).as_bytes())
            .await?;
    }
    Ok(())
}
fn csv_line(values: &[String], delimiter: char) -> String {
    let delimiter_string = delimiter.to_string();
    let mut line = values
        .iter()
        .map(|s| {
            if s.contains([delimiter, '"', '\n', '\r']) {
                format!("\"{}\"", s.replace('"', "\"\""))
            } else {
                s.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(&delimiter_string);
    line.push_str("\r\n");
    line
}

#[cfg(test)]
mod tests;
