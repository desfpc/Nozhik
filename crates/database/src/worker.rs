//! Bounded worker queues and cancellable typed SQL execution.
use crate::{
    Connection, Error, QueryResult, Result,
    connection::Pool,
    driver::{ColumnInfo, DbValue},
};
use futures_util::TryStreamExt;
use sqlx::{Column, Executor, Row, TypeInfo, ValueRef};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

macro_rules! common_value {
    ($row:expr, $i:expr, $kind:expr) => {{
        let row = $row;
        let i = $i;
        match $kind {
            "BOOL" | "BOOLEAN" | "TINYINT(1)" => DbValue::Bool(row.try_get::<bool, _>(i)?),
            "INT2" | "SMALLINT" | "SMALLINT UNSIGNED" => {
                DbValue::Int(i64::from(row.try_get::<i16, _>(i)?))
            }
            "INT4" | "INT" | "MEDIUMINT" | "TINYINT" => {
                DbValue::Int(i64::from(row.try_get::<i32, _>(i)?))
            }
            "INT8" | "BIGINT" | "INTEGER" => DbValue::Int(row.try_get::<i64, _>(i)?),
            "REAL" | "FLOAT4" | "FLOAT" => DbValue::Float(f64::from(row.try_get::<f32, _>(i)?)),
            "FLOAT8" | "DOUBLE" | "DOUBLE PRECISION" => DbValue::Float(row.try_get::<f64, _>(i)?),
            "BYTEA" | "BLOB" | "VARBINARY" | "BINARY" => {
                DbValue::Bytes(row.try_get::<Vec<u8>, _>(i)?)
            }
            "JSON" | "JSONB" => DbValue::Json(row.try_get::<serde_json::Value, _>(i)?.to_string()),
            "DATE" => DbValue::DateTime(row.try_get::<chrono::NaiveDate, _>(i)?.to_string()),
            "TIME" => DbValue::DateTime(row.try_get::<chrono::NaiveTime, _>(i)?.to_string()),
            "TIMESTAMP" | "DATETIME" => {
                DbValue::DateTime(row.try_get::<chrono::NaiveDateTime, _>(i)?.to_string())
            }
            _ => DbValue::Text(row.try_get::<String, _>(i)?),
        }
    }};
}
fn pg_value(row: &sqlx::postgres::PgRow, i: usize) -> Result<DbValue> {
    let raw = row.try_get_raw(i)?;
    if raw.is_null() {
        return Ok(DbValue::Null);
    }
    let info = raw.type_info();
    let kind = info.name();
    Ok(match kind {
        "NUMERIC" => DbValue::Text(row.try_get::<rust_decimal::Decimal, _>(i)?.to_string()),
        "UUID" => DbValue::Text(row.try_get::<uuid::Uuid, _>(i)?.to_string()),
        "TIMESTAMPTZ" => DbValue::DateTime(
            row.try_get::<chrono::DateTime<chrono::Utc>, _>(i)?
                .to_rfc3339(),
        ),
        _ => common_value!(row, i, kind),
    })
}
fn mysql_value(row: &sqlx::mysql::MySqlRow, i: usize) -> Result<DbValue> {
    let raw = row.try_get_raw(i)?;
    if raw.is_null() {
        return Ok(DbValue::Null);
    }
    let info = raw.type_info();
    let kind = info.name();
    Ok(match kind {
        "TINYINT UNSIGNED" | "SMALLINT UNSIGNED" | "MEDIUMINT UNSIGNED" | "INT UNSIGNED"
        | "BIGINT UNSIGNED" => {
            let value: u64 = row.try_get(i)?;
            i64::try_from(value).map_or_else(|_| DbValue::Text(value.to_string()), DbValue::Int)
        }
        "DECIMAL" | "NEWDECIMAL" => {
            DbValue::Text(row.try_get::<rust_decimal::Decimal, _>(i)?.to_string())
        }
        _ => common_value!(row, i, kind),
    })
}
fn sqlite_value(row: &sqlx::sqlite::SqliteRow, i: usize) -> Result<DbValue> {
    let raw = row.try_get_raw(i)?;
    if raw.is_null() {
        return Ok(DbValue::Null);
    }
    if let Some(column) = row.columns().get(i) {
        match column.type_info().name() {
            "BOOLEAN" => return Ok(DbValue::Bool(row.try_get(i)?)),
            "DATE" | "TIME" | "DATETIME" => {
                return Ok(DbValue::DateTime(row.try_get::<String, _>(i)?));
            }
            _ => {}
        }
    }
    let info = raw.type_info();
    // SQLite expressions use runtime storage classes, declared types remain in columns.
    let kind = info.name();
    Ok(match kind {
        "BOOLEAN" => DbValue::Bool(row.try_get(i)?),
        "INTEGER" => DbValue::Int(row.try_get(i)?),
        "REAL" => DbValue::Float(row.try_get(i)?),
        "BLOB" => DbValue::Bytes(row.try_get(i)?),
        "DATE" | "TIME" | "DATETIME" => DbValue::DateTime(row.try_get(i)?),
        _ => DbValue::Text(row.try_get(i)?),
    })
}
macro_rules! install_cancel {
    ($connection:expr, $cancel:expr, sqlite) => {
        let token = $cancel.clone();
        $connection
            .lock_handle()
            .await?
            .set_progress_handler(1000, move || !token.is_cancelled());
    };
    ($connection:expr, $cancel:expr, server) => {};
}
macro_rules! retire_connection {
    ($connection:expr, $result:expr, server) => {
        if !matches!(&$result, Ok((_, false))) {
            $connection.close_on_drop();
        }
    };
    ($connection:expr, $result:expr, sqlite) => {};
}
macro_rules! execute_pool {
    ($pool:expr, $decode:ident, $sql:expr, $cancel:expr, $schema_sql:expr, $backend:ident) => {{
        let mut connection = tokio::select! {
            biased;
            () = $cancel.cancelled() => return Err(Error::Cancelled),
            connection = $pool.acquire() => connection?,
        };
        install_cancel!(connection, $cancel, $backend);
        let run = async {
            if let Some(schema_sql) = $schema_sql {
                sqlx::raw_sql(&schema_sql).execute(&mut *connection).await?;
            }
            // Describe before execution preserves columns of empty SELECTs. DDL scripts
            // may not be describable until executed; their rows supply descriptors.
            let described = (&mut *connection).describe($sql).await.ok().map(|d| {
                d.columns()
                    .iter()
                    .map(|c| ColumnInfo {
                        name: c.name().into(),
                        data_type: c.type_info().name().into(),
                    })
                    .collect::<Vec<_>>()
            });
            let mut stream = sqlx::raw_sql($sql).fetch_many(&mut *connection);
            let mut results = Vec::new();
            let mut result = QueryResult::default();
            let mut total_rows = 0usize;
            let mut bytes = 0usize;
            while let Some(item) = stream.try_next().await? {
                match item {
                    sqlx::Either::Left(done) => {
                        result.rows_affected = done.rows_affected();
                        results.push(std::mem::take(&mut result));
                    }
                    sqlx::Either::Right(row) => {
                        if result.columns.is_empty() {
                            result.columns = row
                                .columns()
                                .iter()
                                .map(|c| ColumnInfo {
                                    name: c.name().into(),
                                    data_type: c.type_info().name().into(),
                                })
                                .collect();
                        }
                        let values: Vec<DbValue> = (0..row.len())
                            .map(|i| $decode(&row, i))
                            .collect::<Result<_>>()?;
                        bytes += values
                            .iter()
                            .map(|v| match v {
                                DbValue::Text(s) | DbValue::Json(s) | DbValue::DateTime(s) => {
                                    s.len()
                                }
                                DbValue::Bytes(b) => b.len(),
                                _ => 8,
                            })
                            .sum::<usize>();
                        if total_rows >= 10_000 || bytes > 16 * 1024 * 1024 {
                            result.truncated = true;
                            results.push(result);
                            return Ok((results, true));
                        }
                        total_rows += 1;
                        result.rows.push(values);
                    }
                }
            }
            if results.len() == 1
                && let Some(first) = results.first_mut()
                && first.columns.is_empty()
            {
                first.columns = described.unwrap_or_default();
            }
            Ok::<_, Error>((results, false))
        };
        let result = tokio::select! {
            biased;
            () = $cancel.cancelled() => Err(Error::Cancelled),
            result = run => result,
        };
        retire_connection!(connection, result, $backend);
        result.map(|(results, _)| results)
    }};
}
impl Connection {
    /// Execute a script, retaining separate result sets, up to 10,000 rows / 16 MiB.
    /// Schema is applied on the same checked-out connection as the query.
    /// # Errors
    /// Returns SQL/decode errors or cancellation. Committed effects may remain.
    pub async fn query(
        &self,
        sql: &str,
        schema: Option<&str>,
        cancel: &CancellationToken,
    ) -> Result<Vec<QueryResult>> {
        let start = Instant::now();
        let schema_sql = match self.driver {
            crate::DbDriverType::Postgres => Some(schema.filter(|s| !s.is_empty()).map_or_else(
                || "SET search_path TO DEFAULT".into(),
                |s| {
                    format!(
                        "SET search_path TO {}",
                        crate::connection::quote_identifier(self.driver, s)
                    )
                },
            )),
            crate::DbDriverType::Mysql => Some(format!(
                "USE {}",
                crate::connection::quote_identifier(
                    self.driver,
                    schema.filter(|s| !s.is_empty()).unwrap_or(&self.database)
                )
            )),
            crate::DbDriverType::Sqlite => None,
        };
        let mut results = match &self.pool {
            Pool::Postgres(p) => execute_pool!(p, pg_value, sql, cancel, schema_sql, server),
            Pool::Mysql(p) => execute_pool!(p, mysql_value, sql, cancel, schema_sql, server),
            Pool::Sqlite(p) => execute_pool!(p, sqlite_value, sql, cancel, schema_sql, sqlite),
        }?;
        let elapsed = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
        for result in &mut results {
            result.execution_time_ms = elapsed;
        }
        Ok(results)
    }
}
/// Background command; request IDs are supplied by the UI and never reused.
pub enum Command {
    /// Load public profiles.
    Load,
    /// Save a new profile and optionally its password after testing successfully.
    Save(
        crate::ConnectionProfile,
        Option<nozhik_remote_ssh::credentials::Secret>,
    ),
    /// Open a pool, or test temporarily without retaining it.
    Connect {
        /// Request identity.
        id: u64,
        /// Profile.
        profile: crate::ConnectionProfile,
        /// Session-only password.
        password: Option<nozhik_remote_ssh::credentials::Secret>,
        /// Authenticated SSH session.
        ssh: Option<nozhik_remote_ssh::SshSession>,
        /// Drop pool after test.
        test: bool,
    },
    /// Drop a connection and cancel its active work.
    Disconnect(String),
    /// Execute query on a connected pool.
    Query {
        /// Request identity.
        id: u64,
        /// Connection name.
        name: String,
        /// SQL source.
        sql: String,
        /// Active schema.
        schema: Option<String>,
        /// Independent UI cancellation.
        cancel: CancellationToken,
    },
    /// Submit staged edits in a transaction.
    Apply {
        /// Request identity.
        id: u64,
        /// Connection name.
        name: String,
        /// Local snapshot to submit.
        changes: crate::changes::TableChanges,
        /// Independent cancellation.
        cancel: CancellationToken,
    },
    /// Lazily load metadata for one relation.
    Describe {
        /// Request identity.
        id: u64,
        /// Connection name.
        name: String,
        /// Namespace.
        schema: String,
        /// Relation name.
        table: String,
        /// Whether the relation is a view.
        is_view: bool,
    },
    /// Replace an existing profile; the original must still match the persisted version.
    Update {
        /// Previously loaded profile.
        original: crate::ConnectionProfile,
        /// Edited profile.
        profile: crate::ConnectionProfile,
        /// Replacement password, absent to preserve the existing credential.
        password: Option<nozhik_remote_ssh::credentials::Secret>,
    },
    /// Reload schema names and relation names.
    Inspect(String),
}
/// Typed completions.
pub enum Reply {
    /// Loaded or saved profiles.
    Profiles(Vec<crate::ConnectionProfile>),
    /// Profile replaced successfully; old connection has been invalidated.
    Updated(String, Vec<crate::ConnectionProfile>),
    /// One lazily loaded relation's metadata.
    Table(u64, Result<crate::introspection::TableInfo>),
    /// Profile persistence failed; form remains open.
    SaveError(String),
    /// Connection test/open result.
    Connected(u64, String, bool, Result<crate::DatabaseSchemaTree>),
    /// Connection closed; all active queries have received cancellation.
    Disconnected(String),
    /// Schema refresh result.
    Schema(String, Result<crate::DatabaseSchemaTree>),
    /// Query result, including errors and cancellation.
    Query(u64, Result<Vec<QueryResult>>),
    /// Transactional edit completion.
    Applied(u64, Result<()>),
    /// Worker-level failure.
    Error(String),
}
/// Nonblocking UI handle. Drop cancels tasks and releases runtime-owned pools.
pub struct Worker {
    input: mpsc::Sender<Command>,
    /// Bounded completions; drain without waiting on the UI thread.
    pub output: mpsc::Receiver<Reply>,
    stop: CancellationToken,
    done: std::sync::mpsc::Receiver<()>,
}
impl Worker {
    /// Start an isolated Tokio worker; profile and credential I/O use blocking pool.
    #[must_use]
    #[allow(
        clippy::too_many_lines,
        reason = "One bounded actor dispatches queries, metadata tasks and connection lifecycle commands"
    )]
    pub fn start(path: PathBuf, wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        let (input, mut commands) = mpsc::channel(32);
        let (sender, output) = mpsc::channel(32);
        let stop = CancellationToken::new();
        let stopped = stop.clone();
        let (finished, done) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(r) => r,
                Err(e) => {
                    let _sent = sender.blocking_send(Reply::Error(e.to_string()));
                    wake();
                    return;
                }
            };
            runtime.block_on(tokio::task::LocalSet::new().run_until(async {
                let mut connections: HashMap<String, (Arc<Connection>, CancellationToken)> = HashMap::new();
                let mut tasks = tokio::task::JoinSet::new();
                let mut tested_passwords = HashMap::new();
                let mut active_tokens: HashMap<u64, CancellationToken> = HashMap::new();
                let mut health = tokio::time::interval(Duration::from_secs(5));
                loop {
                    tokio::select! {
                        biased;
                        () = stopped.cancelled() => break,
                        Some(done) = tasks.join_next(), if !tasks.is_empty() => {
                            match done { Ok(id) => { active_tokens.remove(&id); }, Err(e) => eprintln!("Database task: {e}") }
                        }
                        _ = health.tick() => {
                            let mut lost = Vec::new();
                            for (name, (connection, _)) in &connections {
                                if !connection.healthy().await { lost.push(name.clone()); }
                            }
                            for name in lost {
                                if let Some((_, token)) = connections.remove(&name) { token.cancel(); }
                                if sender.send(Reply::Disconnected(name)).await.is_err() { break; } wake();
                            }
                        }
                        command = commands.recv() => {
                            let Some(command) = command else { break; };
                            if let Command::Describe {id,name,schema,table,is_view}=command {
                                let connection=connections.get(&name).cloned();let sender=sender.clone();let wake=wake.clone();
                                if tasks.len()>=16 {let _sent=sender.send(Reply::Table(id,Err(Error::Invalid("Too many active database requests".into())))).await;wake();continue;}
                                tasks.spawn_local(async move {
                                    let result=if let Some((connection,disconnected))=connection {
                                        tokio::select! { biased; ()=disconnected.cancelled()=>Err(Error::Cancelled), result=tokio::time::timeout(Duration::from_secs(20),connection.describe_table(&schema,&table,is_view))=>result.unwrap_or_else(|_|Err(Error::Invalid(format!("Metadata timed out for {schema}.{table}; retry this table")))) }
                                    } else {Err(Error::Invalid("Connection offline".into()))};
                                    let _sent=sender.send(Reply::Table(id,result)).await;wake();id
                                });continue;
                            }
                            if let Command::Apply { id, name, changes, cancel } = command {
                                let connection = connections.get(&name).cloned(); let sender = sender.clone(); let wake = wake.clone();
                                if tasks.len() >= 16 { let _sent = sender.send(Reply::Applied(id, Err(Error::Invalid("Too many active queries".into())))).await; wake(); continue; }
                                active_tokens.insert(id, cancel.clone());
                                tasks.spawn_local(async move {
                                    let result = if let Some((connection, disconnected)) = connection {
                                        let apply=connection.apply_changes(&changes,&cancel); tokio::pin!(apply);
                                        tokio::select! { biased; () = disconnected.cancelled() => {cancel.cancel();apply.await}, result = &mut apply => result }
                                    } else { Err(Error::Invalid("Connection offline".into())) };
                                    let _sent = sender.send(Reply::Applied(id, result)).await; wake(); id
                                });
                                continue;
                            }
                            if let Command::Query { id, name, sql, schema, cancel } = command {
                                let connection = connections.get(&name).cloned(); let sender = sender.clone(); let wake = wake.clone();
                                if tasks.len() >= 16 { let _sent = sender.send(Reply::Query(id, Err(Error::Invalid("Too many active queries".into())))).await; wake(); continue; }
                                active_tokens.insert(id, cancel.clone());
                                tasks.spawn_local(async move {
                                    let result = if let Some((connection, disconnected)) = connection {
                                        let query = connection.query(&sql, schema.as_deref(), &cancel);
                                        tokio::pin!(query);
                                        tokio::select! { biased; () = disconnected.cancelled() => { cancel.cancel(); query.await }, result = &mut query => result }
                                    } else { Err(Error::Invalid("Connection offline".into())) };
                                    let _sent = sender.send(Reply::Query(id, result)).await; wake();
                                    id
                                });
                                continue;
                            }
                            let reply = tokio::select! {
                                () = stopped.cancelled() => break,
                                reply = handle(command, &path, &mut connections, &mut tested_passwords) => reply,
                            };
                            if let Some(reply) = reply { if sender.send(reply).await.is_err() { break; } wake(); }
                        }
                    }
                }
                for token in active_tokens.values() { token.cancel(); }
                tasks.abort_all();
                while tasks.join_next().await.is_some() {}
                let closes=connections.values().map(|(connection,_)|connection.close());
                let _closed=tokio::time::timeout(Duration::from_secs(2),futures_util::future::join_all(closes)).await;
            }));
            runtime.shutdown_timeout(Duration::from_secs(1));
            let _finished = finished.send(());
        });
        Self {
            input,
            output,
            stop,
            done,
        }
    }
    /// Cancel requests, close pools and wait at most four seconds for worker teardown.
    pub fn shutdown(mut self) {
        self.stop.cancel();
        self.output.close();
        let _finished = self.done.recv_timeout(Duration::from_secs(4));
    }
    /// Enqueue without blocking; false means the queue is full or worker stopped.
    #[must_use]
    pub fn send(&self, command: Command) -> bool {
        self.input.try_send(command).is_ok()
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}
fn credential(profile: &crate::ConnectionProfile) -> Result<keyring::Entry> {
    let account = serde_json::to_string(&(
        profile.name.as_str(),
        profile.driver,
        &profile.host,
        profile.port,
        &profile.user,
        &profile.database,
    ))?;
    Ok(keyring::Entry::new("dev.Nozhik.Nozhik.Database", &account)?)
}
#[allow(
    clippy::too_many_lines,
    reason = "Typed worker commands keep credential gating, persistence and connection lifetime in one actor"
)]
async fn handle(
    command: Command,
    path: &std::path::Path,
    connections: &mut HashMap<String, (Arc<Connection>, CancellationToken)>,
    tested_passwords: &mut HashMap<
        String,
        (
            crate::ConnectionProfile,
            nozhik_remote_ssh::credentials::Secret,
        ),
    >,
) -> Option<Reply> {
    use nozhik_remote_ssh::credentials::Secret;
    match command {
        Command::Load => {
            let path = path.to_owned();
            Some(
                match tokio::task::spawn_blocking(move || crate::load_profiles(&path)).await {
                    Ok(Ok(p)) => Reply::Profiles(p),
                    Ok(Err(e)) => Reply::Error(e.to_string()),
                    Err(e) => Reply::Error(e.to_string()),
                },
            )
        }
        Command::Save(profile, password) => {
            if let Err(e) = profile.validate() {
                return Some(Reply::SaveError(e.to_string()));
            }
            if profile.save_password
                && !tested_passwords
                    .get(&profile.name)
                    .is_some_and(|(tested, secret)| {
                        tested == &profile
                            && password
                                .as_ref()
                                .is_some_and(|p| p.expose() == secret.expose())
                    })
            {
                return Some(Reply::SaveError(
                    "Test this connection and password successfully before saving credentials"
                        .into(),
                ));
            }
            let path = path.to_owned();
            Some(
                match tokio::task::spawn_blocking(move || {
                    let mut profiles = crate::load_profiles(&path)?;
                    if profiles.iter().any(|p| p.name == profile.name) {
                        return Err(Error::Invalid("Connection name already exists".into()));
                    }
                    if profile.save_password {
                        credential(&profile)?
                            .set_password(password.as_ref().map_or("", Secret::expose))?;
                    }
                    profiles.push(profile);
                    crate::save_profiles(&path, &profiles)?;
                    Ok::<_, Error>(profiles)
                })
                .await
                {
                    Ok(Ok(p)) => Reply::Profiles(p),
                    Ok(Err(e)) => Reply::SaveError(e.to_string()),
                    Err(e) => Reply::SaveError(e.to_string()),
                },
            )
        }
        Command::Update {
            original,
            profile,
            password,
        } => {
            let old_name = original.name.clone();
            if let Err(e) =
                validate_update(&original, &profile, password.as_ref(), tested_passwords)
            {
                return Some(Reply::SaveError(e.to_string()));
            }
            let path = path.to_owned();
            let saved = tokio::task::spawn_blocking(move || {
                let mut profiles = crate::load_profiles(&path)?;
                replace_profile(&mut profiles, &original, profile.clone())?;
                if profile.save_password {
                    if let Some(password) = password {
                        credential(&profile)?.set_password(password.expose())?;
                    } else if credential_identity(&original) != credential_identity(&profile) {
                        let password = Secret::new(credential(&original)?.get_password()?);
                        credential(&profile)?.set_password(password.expose())?;
                    }
                }
                crate::save_profiles(&path, &profiles)?;
                Ok::<_, Error>(profiles)
            })
            .await;
            match saved {
                Ok(Ok(profiles)) => {
                    if let Some((_, token)) = connections.remove(&old_name) {
                        token.cancel();
                    }
                    tested_passwords.remove(&old_name);
                    Some(Reply::Updated(old_name, profiles))
                }
                Ok(Err(e)) => Some(Reply::SaveError(e.to_string())),
                Err(e) => Some(Reply::SaveError(e.to_string())),
            }
        }
        Command::Connect {
            id,
            profile,
            password,
            ssh,
            test,
        } => {
            let name = profile.name.clone();
            let open = async {
                let password = if password.is_none() && profile.save_password {
                    let profile = profile.clone();
                    tokio::task::spawn_blocking(move || {
                        credential(&profile)?
                            .get_password()
                            .map(Secret::new)
                            .map_err(Error::from)
                    })
                    .await
                    .map_err(|e| Error::Invalid(e.to_string()))??
                    .into()
                } else {
                    password
                };
                let connection = Arc::new(
                    tokio::time::timeout(
                        Duration::from_secs(45),
                        Connection::connect(&profile, password.as_ref(), ssh.as_ref()),
                    )
                    .await
                    .map_err(|_| {
                        Error::Invalid("Database connection/authentication timed out".into())
                    })??,
                );
                let schema = if test {
                    crate::DatabaseSchemaTree::default()
                } else {
                    tokio::time::timeout(Duration::from_secs(30), connection.catalog())
                        .await
                        .map_err(|_| {
                            Error::Invalid(
                                "Database connected, but loading namespace/table names timed out"
                                    .into(),
                            )
                        })??
                };
                if test
                    && profile.save_password
                    && let Some(password) = password
                {
                    if tested_passwords.len() >= 32 {
                        tested_passwords.clear();
                    }
                    tested_passwords.insert(name.clone(), (profile.clone(), password));
                }
                if !test {
                    if let Some((_, old)) = connections.remove(&name) {
                        old.cancel();
                    }
                    connections.insert(name.clone(), (connection, CancellationToken::new()));
                }
                Ok(schema)
            };
            let result = tokio::time::timeout(Duration::from_secs(80), open)
                .await
                .unwrap_or_else(|_| {
                    Err(Error::Invalid(
                        "Database connection request timed out".into(),
                    ))
                });
            Some(Reply::Connected(id, name, test, result))
        }
        Command::Disconnect(name) => {
            if let Some((_, token)) = connections.remove(&name) {
                token.cancel();
            }
            Some(Reply::Disconnected(name))
        }
        Command::Inspect(name) => {
            let result = if let Some((connection, _)) = connections.get(&name) {
                tokio::time::timeout(Duration::from_secs(30), connection.catalog())
                    .await
                    .unwrap_or_else(|_| {
                        Err(Error::Invalid(
                            "Loading database/table names timed out".into(),
                        ))
                    })
            } else {
                Err(Error::Invalid("Connection offline".into()))
            };
            Some(Reply::Schema(name, result))
        }
        Command::Query { .. } | Command::Apply { .. } | Command::Describe { .. } => None,
    }
}

fn credential_identity(
    profile: &crate::ConnectionProfile,
) -> (&str, crate::DbDriverType, &str, u16, &str, &str) {
    (
        &profile.name,
        profile.driver,
        &profile.host,
        profile.port,
        &profile.user,
        &profile.database,
    )
}
fn same_endpoint(original: &crate::ConnectionProfile, profile: &crate::ConnectionProfile) -> bool {
    let mut previous = original.clone();
    previous.name.clone_from(&profile.name);
    previous.save_password = profile.save_password;
    previous == *profile
}
fn validate_update(
    original: &crate::ConnectionProfile,
    profile: &crate::ConnectionProfile,
    password: Option<&nozhik_remote_ssh::credentials::Secret>,
    tested: &HashMap<
        String,
        (
            crate::ConnectionProfile,
            nozhik_remote_ssh::credentials::Secret,
        ),
    >,
) -> Result<()> {
    profile.validate()?;
    if profile.save_password {
        let preserve =
            password.is_none() && original.save_password && same_endpoint(original, profile);
        let verified = tested.get(&profile.name).is_some_and(|(p, secret)| {
            p == profile && password.is_some_and(|s| s.expose() == secret.expose())
        });
        if !preserve && !verified {
            return Err(Error::Invalid(
                "Test the edited connection and replacement password before saving credentials"
                    .into(),
            ));
        }
    }
    Ok(())
}
/// Replace exactly the expected persisted profile, preserving ordering and unrelated entries.
pub(crate) fn replace_profile(
    profiles: &mut [crate::ConnectionProfile],
    original: &crate::ConnectionProfile,
    profile: crate::ConnectionProfile,
) -> Result<()> {
    profile.validate()?;
    let index = profiles.iter().position(|p| p == original).ok_or_else(|| {
        Error::Invalid("Connection changed or disappeared; reopen Edit Connection".into())
    })?;
    if profiles
        .iter()
        .enumerate()
        .any(|(i, p)| i != index && p.name == profile.name)
    {
        return Err(Error::Invalid("Connection name already exists".into()));
    }
    profiles[index] = profile;
    Ok(())
}
