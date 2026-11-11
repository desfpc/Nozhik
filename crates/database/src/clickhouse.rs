//! `ClickHouse` HTTP transport, bounded dynamic results and native catalogs.
use crate::catalog::{TableObject, TableObjects};
use crate::introspection::{DatabaseSchemaTree, SchemaColumn, SchemaInfo, TableInfo};
use crate::{
    CancellationToken, ColumnInfo, ConnectionProfile, DbValue, Error, QueryResult, Result,
};
use futures_util::TryStreamExt;
use nozhik_remote_ssh::credentials::Secret;
use std::{
    net::SocketAddr,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

const FORMAT: &str = "JSONCompactEachRowWithNamesAndTypes";
const MAX_BYTES: usize = 16 * 1024 * 1024;
static QUERY_ID: AtomicU64 = AtomicU64::new(0);

pub struct Client {
    http: reqwest::Client,
    url: reqwest::Url,
    user: String,
    password: Secret,
    database: String,
}
pub fn text(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
}
impl Client {
    pub(crate) async fn connect(
        profile: &ConnectionProfile,
        password: &str,
        tunnel: Option<SocketAddr>,
    ) -> Result<Self> {
        let mut builder = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(300));
        let mut url =
            reqwest::Url::parse("http://localhost/").map_err(|e| Error::Invalid(e.to_string()))?;
        url.set_scheme(if profile.tls { "https" } else { "http" })
            .map_err(|()| Error::Invalid("Invalid HTTP scheme".into()))?;
        let host = if tunnel.is_some() && !profile.tls {
            "127.0.0.1"
        } else {
            &profile.host
        };
        url.set_host(Some(host))
            .map_err(|e| Error::Invalid(e.to_string()))?;
        url.set_port(Some(tunnel.map_or(profile.port, |address| address.port())))
            .map_err(|()| Error::Invalid("Invalid HTTP port".into()))?;
        if let Some(address) = tunnel.filter(|_| profile.tls) {
            if profile.host.parse::<std::net::IpAddr>().is_ok() {
                return Err(Error::Invalid(
                    "HTTPS through SSH requires the certificate's DNS hostname".into(),
                ));
            }
            builder = builder.resolve(&profile.host, address);
        }
        let client = Self {
            http: builder.build()?,
            url,
            user: profile.user.clone(),
            password: Secret::new(password.into()),
            database: profile.database.clone(),
        };
        client
            .query("SELECT 1", &profile.database, &CancellationToken::new())
            .await?;
        Ok(client)
    }
    fn request(&self, sql: &str, database: &str, id: &str) -> reqwest::RequestBuilder {
        self.http
            .post(self.url.clone())
            .basic_auth(&self.user, Some(self.password.expose()))
            .query(&[
                ("database", database),
                ("default_format", FORMAT),
                ("query_id", id),
                ("output_format_json_quote_64bit_integers", "1"),
                ("output_format_json_quote_64bit_floats", "1"),
                ("output_format_json_quote_decimals", "1"),
                ("output_format_json_quote_denormals", "1"),
                ("output_format_json_validate_utf8", "0"),
                ("cancel_http_readonly_queries_on_client_close", "1"),
            ])
            .body(sql.to_owned())
    }
    async fn kill(&self, id: &str) {
        // Best effort: cancellation never claims rollback, and ACLs may deny KILL.
        let sql = format!("KILL QUERY WHERE query_id={} ASYNC", text(id));
        let _outcome = tokio::time::timeout(
            Duration::from_secs(2),
            self.request(&sql, &self.database, "").send(),
        )
        .await;
    }
    pub(crate) async fn query(
        &self,
        sql: &str,
        database: &str,
        cancel: &CancellationToken,
    ) -> Result<Vec<QueryResult>> {
        self.run(sql, database, cancel, true).await
    }
    pub(crate) async fn execute(
        &self,
        sql: &str,
        database: &str,
        cancel: &CancellationToken,
    ) -> Result<()> {
        self.run(sql, database, cancel, false).await.map(|_| ())
    }
    async fn run(
        &self,
        sql: &str,
        database: &str,
        cancel: &CancellationToken,
        retain: bool,
    ) -> Result<Vec<QueryResult>> {
        let mut results = Vec::new();
        let mut rows = 0;
        let mut bytes = 0;
        for statement in statements(sql)? {
            let command = statement
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .to_ascii_uppercase();
            if ["USE", "SET", "BEGIN", "COMMIT", "ROLLBACK", "START"].contains(&command.as_str()) {
                return Err(Error::Invalid("ClickHouse HTTP consoles have no persistent session: choose a database in the toolbar and use per-query SETTINGS; transactions are unsupported".into()));
            }
            let id = format!(
                "nozhik-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos(),
                QUERY_ID.fetch_add(1, Ordering::Relaxed)
            );
            let operation = async {
                let response = self
                    .request(&statement, database, &id)
                    .send()
                    .await
                    .map_err(|e| Error::Http(e.without_url()))?;
                let success = response.status().is_success();
                let mut stream = response.bytes_stream();
                let mut buffer = Vec::new();
                let mut decoder = Decoder::default();
                while let Some(chunk) = stream
                    .try_next()
                    .await
                    .map_err(|e| Error::Http(e.without_url()))?
                {
                    for part in chunk.split_inclusive(|b| *b == b'\n') {
                        if buffer.len().saturating_add(part.len()) > MAX_BYTES {
                            return Err(Error::Invalid(
                                "ClickHouse response record exceeds 16 MiB".into(),
                            ));
                        }
                        buffer.extend_from_slice(part);
                        if part.last() == Some(&b'\n') {
                            if !success {
                                return Err(server_error(&buffer));
                            }
                            if !retain {
                                rows = 0;
                                bytes = 0;
                            }
                            if !decoder.line(&buffer, &mut rows, &mut bytes)? {
                                return Ok(decoder.result);
                            }
                            if !retain {
                                decoder.result.rows.clear();
                            }
                            buffer.clear();
                        }
                    }
                }
                if !success {
                    return Err(server_error(&buffer));
                }
                if !buffer.is_empty() {
                    if !retain {
                        rows = 0;
                        bytes = 0;
                    }
                    decoder.line(&buffer, &mut rows, &mut bytes)?;
                    if !retain {
                        decoder.result.rows.clear();
                    }
                }
                if decoder.names.is_some() && decoder.result.columns.is_empty() {
                    return Err(Error::Invalid(
                        "Incomplete ClickHouse result metadata".into(),
                    ));
                }
                Ok(decoder.result)
            };
            let result = tokio::select! { biased; () = cancel.cancelled() => Err(Error::Cancelled), result = operation => result };
            if matches!(result, Err(Error::Cancelled)) || result.as_ref().is_ok_and(|r| r.truncated)
            {
                self.kill(&id).await;
            }
            let result = result?;
            let truncated = result.truncated;
            if retain {
                results.push(result);
            }
            if truncated {
                break;
            }
        }
        Ok(results)
    }
    pub(crate) async fn each_row(
        &self,
        sql: &str,
        database: &str,
        mut visit: impl AsyncFnMut(&[ColumnInfo], Vec<DbValue>) -> Result<()>,
    ) -> Result<()> {
        let response = self
            .request(sql, database, "")
            .send()
            .await
            .map_err(|e| Error::Http(e.without_url()))?;
        let success = response.status().is_success();
        let mut stream = response.bytes_stream();
        let mut buffer = Vec::new();
        let mut decoder = Decoder::default();
        while let Some(chunk) = stream
            .try_next()
            .await
            .map_err(|e| Error::Http(e.without_url()))?
        {
            for part in chunk.split_inclusive(|b| *b == b'\n') {
                if buffer.len().saturating_add(part.len()) > MAX_BYTES {
                    return Err(Error::Invalid(
                        "ClickHouse export record exceeds 16 MiB".into(),
                    ));
                }
                buffer.extend_from_slice(part);
                if part.last() == Some(&b'\n') {
                    if !success {
                        return Err(server_error(&buffer));
                    }
                    decoder.line(&buffer, &mut 0, &mut 0)?;
                    for row in std::mem::take(&mut decoder.result.rows) {
                        visit(&decoder.result.columns, row).await?;
                    }
                    buffer.clear();
                }
            }
        }
        if !success {
            return Err(server_error(&buffer));
        }
        if !buffer.is_empty() {
            decoder.line(&buffer, &mut 0, &mut 0)?;
            for row in std::mem::take(&mut decoder.result.rows) {
                visit(&decoder.result.columns, row).await?;
            }
        }
        Ok(())
    }
    async fn metadata(&self, sql: &str) -> Result<QueryResult> {
        let result = self
            .query(sql, &self.database, &CancellationToken::new())
            .await?
            .pop()
            .unwrap_or_default();
        if result.truncated {
            return Err(Error::Invalid(
                "ClickHouse catalog exceeds metadata limit; narrow server permissions".into(),
            ));
        }
        Ok(result)
    }
    pub(crate) async fn catalog(&self) -> Result<DatabaseSchemaTree> {
        let mut tree = DatabaseSchemaTree::default();
        for row in self
            .metadata("SELECT name FROM system.databases ORDER BY name")
            .await?
            .rows
        {
            tree.schemas.push(SchemaInfo {
                name: cell(&row, 0)?,
                tables: Vec::new(),
            });
        }
        for row in self.metadata("SELECT database, name, engine FROM system.tables WHERE is_temporary=0 ORDER BY database,name").await?.rows {
            let database = cell(&row, 0)?;
            if let Some(schema) = tree.schemas.iter_mut().find(|s| s.name == database) {
                let engine = cell(&row, 2)?;
                schema.tables.push(TableInfo { name: cell(&row, 1)?, is_view: engine.ends_with("View"), columns: Vec::new(), objects: TableObjects::default(), details_loaded: false });
            }
        }
        Ok(tree)
    }
    pub(crate) async fn describe(
        &self,
        schema: &str,
        table: &str,
        is_view: bool,
    ) -> Result<TableInfo> {
        let sql = format!(
            "SELECT name,type,default_kind,default_expression,is_in_primary_key FROM system.columns WHERE database={} AND table={} ORDER BY position",
            text(schema),
            text(table)
        );
        let mut columns = Vec::new();
        for row in self.metadata(&sql).await?.rows {
            let data_type = cell(&row, 1)?;
            let default = cell(&row, 3)?;
            columns.push(SchemaColumn {
                name: cell(&row, 0)?,
                nullable: data_type.contains("Nullable("),
                data_type,
                is_primary_key: cell(&row, 4)? == "1",
                is_foreign_key: false,
                default_value: (!default.is_empty())
                    .then(|| format!("{} {default}", cell(&row, 2).unwrap_or_default())),
            });
        }
        if columns.is_empty() {
            return Err(Error::Invalid(
                "ClickHouse table has no visible columns or no longer exists".into(),
            ));
        }
        let objects = self
            .objects(schema, table)
            .await
            .unwrap_or_else(|e| TableObjects {
                error: Some(e.to_string()),
                ..TableObjects::default()
            });
        Ok(TableInfo {
            name: table.into(),
            is_view,
            columns,
            objects,
            details_loaded: true,
        })
    }
    pub(crate) async fn objects(&self, schema: &str, table: &str) -> Result<TableObjects> {
        let mut objects = TableObjects::default();
        let sql = format!(
            "SELECT engine,partition_key,sorting_key,primary_key,sampling_key,create_table_query FROM system.tables WHERE database={} AND name={}",
            text(schema),
            text(table)
        );
        let result = self.metadata(&sql).await?;
        let row = result
            .rows
            .first()
            .ok_or_else(|| Error::Invalid("ClickHouse table disappeared".into()))?;
        for (i, name) in [
            "Engine",
            "PARTITION BY",
            "ORDER BY",
            "PRIMARY KEY",
            "SAMPLE BY",
        ]
        .iter()
        .enumerate()
        {
            let definition = cell(row, i)?;
            if !definition.is_empty() {
                objects.keys.push(TableObject {
                    name: (*name).into(),
                    definition,
                });
            }
        }
        // Native CREATE includes TTL, engine settings, projections and skipping indexes.
        objects.ddl = cell(row, 5)?;
        objects.ddl.push(';');
        Ok(objects)
    }
}
fn cell(row: &[DbValue], index: usize) -> Result<String> {
    row.get(index)
        .map(ToString::to_string)
        .ok_or_else(|| Error::Invalid("Invalid ClickHouse metadata row".into()))
}
fn server_error(bytes: &[u8]) -> Error {
    Error::Invalid(format!(
        "ClickHouse: {}",
        String::from_utf8_lossy(bytes.get(..bytes.len().min(8192)).unwrap_or_default()).trim()
    ))
}
#[derive(Default)]
struct Decoder {
    names: Option<Vec<String>>,
    result: QueryResult,
    header: bool,
}
impl Decoder {
    fn line(&mut self, line: &[u8], rows: &mut usize, bytes: &mut usize) -> Result<bool> {
        if line.iter().all(u8::is_ascii_whitespace) {
            return Ok(true);
        }
        if self.names.is_none() {
            self.names = Some(serde_json::from_slice(line).map_err(|_| server_error(line))?);
        } else if !self.header {
            let types: Vec<String> =
                serde_json::from_slice(line).map_err(|_| server_error(line))?;
            let names = self.names.as_ref().ok_or_else(|| server_error(line))?;
            if names.len() != types.len() {
                return Err(server_error(line));
            }
            self.result.columns = names
                .iter()
                .zip(types)
                .map(|(name, data_type)| ColumnInfo {
                    name: name.clone(),
                    data_type,
                })
                .collect();
            self.header = true;
        } else {
            let values: Vec<&serde_json::value::RawValue> =
                serde_json::from_slice(line).map_err(|_| server_error(line))?;
            if values.len() != self.result.columns.len() {
                return Err(server_error(line));
            }
            *bytes += line.len();
            if *rows >= 10_000 || *bytes > MAX_BYTES {
                self.result.truncated = true;
                return Ok(false);
            }
            *rows += 1;
            self.result.rows.push(
                values
                    .into_iter()
                    .zip(&self.result.columns)
                    .map(|(value, column)| decode(value, &column.data_type))
                    .collect::<Result<_>>()?,
            );
        }
        Ok(true)
    }
}
fn decode(raw: &serde_json::value::RawValue, kind: &str) -> Result<DbValue> {
    use serde_json::Value;
    let mut kind = kind;
    while let Some(inner) = kind
        .strip_prefix("Nullable(")
        .or_else(|| kind.strip_prefix("LowCardinality("))
        .and_then(|s| s.strip_suffix(')'))
    {
        kind = inner;
    }
    let source = raw.get();
    if source.starts_with(['[', '{']) {
        return Ok(DbValue::Json(source.into()));
    }
    // Preserve exact numeric lexemes even if SQL overrides the requested quoting settings.
    if source != "null"
        && !source.starts_with('"')
        && (kind.starts_with("Decimal") || kind.starts_with("Int") || kind.starts_with("UInt"))
    {
        return Ok(if kind.starts_with("Decimal") {
            DbValue::Text(source.into())
        } else {
            source
                .parse::<i64>()
                .map_or_else(|_| DbValue::Text(source.into()), DbValue::Int)
        });
    }
    Ok(match serde_json::from_str(source)? {
        Value::Null => DbValue::Null,
        Value::Bool(v) => DbValue::Bool(v),
        Value::Number(v) if kind.starts_with("Float") => v
            .as_f64()
            .map_or_else(|| DbValue::Text(v.to_string()), DbValue::Float),
        Value::Number(v) => v
            .as_i64()
            .map_or_else(|| DbValue::Text(v.to_string()), DbValue::Int),
        Value::String(v) => {
            if kind.starts_with("Int") || kind.starts_with("UInt") {
                v.parse::<i64>()
                    .map_or_else(|_| DbValue::Text(v), DbValue::Int)
            } else if kind.starts_with("Float") {
                v.parse::<f64>()
                    .map_or_else(|_| DbValue::Text(v), DbValue::Float)
            } else if kind.starts_with("Date") {
                DbValue::DateTime(v)
            } else {
                DbValue::Text(v)
            }
        }
        v => DbValue::Json(v.to_string()),
    })
}
/// Split `ClickHouse` scripts outside quotes and comments; no implicit HTTP session.
pub fn statements(sql: &str) -> Result<Vec<String>> {
    let mut output = Vec::new();
    let mut buffer = String::new();
    let mut chars = sql.chars().peekable();
    let mut quote = None;
    while let Some(c) = chars.next() {
        if let Some(end) = quote {
            buffer.push(c);
            if c == '\\' {
                if let Some(next) = chars.next() {
                    buffer.push(next);
                }
            } else if c == end {
                if chars.peek() == Some(&end) {
                    buffer.push(chars.next().unwrap_or(end));
                } else {
                    quote = None;
                }
            }
        } else if c == '-' && chars.peek() == Some(&'-') || c == '#' {
            for next in chars.by_ref() {
                if next == '\n' {
                    break;
                }
            }
            buffer.push('\n');
        } else if c == '/' && chars.peek() == Some(&'*') {
            chars.next();
            let mut closed = false;
            while let Some(next) = chars.next() {
                if next == '*' && chars.peek() == Some(&'/') {
                    chars.next();
                    closed = true;
                    break;
                }
            }
            if !closed {
                return Err(Error::Invalid("Unterminated ClickHouse comment".into()));
            }
            buffer.push(' ');
        } else if c == '$' {
            buffer.push_str(&heredoc(&mut chars)?.unwrap_or_else(|| "$".into()));
        } else if matches!(c, '\'' | '"' | '`') {
            quote = Some(c);
            buffer.push(c);
        } else if c == ';' {
            if !buffer.trim().is_empty() {
                output.push(buffer.trim().to_owned());
            }
            buffer.clear();
        } else {
            buffer.push(c);
        }
    }
    if quote.is_some() {
        return Err(Error::Invalid("Unterminated ClickHouse quote".into()));
    }
    if !buffer.trim().is_empty() {
        output.push(buffer.trim().to_owned());
    }
    Ok(output)
}

#[cfg(test)]
mod tests;

fn heredoc(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Result<Option<String>> {
    let mut delimiter = "$".to_owned();
    for c in chars.clone() {
        if c == '$' {
            delimiter.push(c);
            break;
        }
        if !c.is_ascii_alphanumeric() && c != '_' {
            return Ok(None);
        }
        delimiter.push(c);
    }
    if !delimiter.ends_with('$') || delimiter.len() == 1 {
        return Ok(None);
    }
    for _ in 1..delimiter.len() {
        chars.next();
    }
    let mut body = String::new();
    for c in chars.by_ref() {
        body.push(c);
        if body.ends_with(&delimiter) {
            return Ok(Some(delimiter + &body));
        }
    }
    Err(Error::Invalid("Unterminated ClickHouse heredoc".into()))
}
