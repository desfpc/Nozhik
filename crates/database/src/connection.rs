//! Validated public profiles and owned SQL pools with optional SSH forwarding.
use crate::{DbDriverType, Error, Result};
use nozhik_remote_ssh::{SshSession, credentials::Secret, tunnel::TunnelHandle};
use serde::{Deserialize, Serialize};
use sqlx::{mysql::MySqlConnectOptions, postgres::PgConnectOptions, sqlite::SqliteConnectOptions};
use std::{path::Path, time::Duration};

/// Persistent configuration. Passwords live in memory or the OS credential store.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectionProfile {
    /// Unique stable connection name.
    pub name: String,
    /// SQL driver.
    pub driver: DbDriverType,
    /// Server hostname as resolved by the SSH host when tunneled.
    pub host: String,
    /// Server TCP port (ignored for SQLite).
    pub port: u16,
    /// Database login.
    pub user: String,
    /// Database name, SQLite native filename or :memory:.
    pub database: String,
    /// Existing SSH profile name. Requires a verified, authenticated session.
    pub ssh_profile_id: Option<String>,
    /// Explicit opt-in to OS-protected password persistence.
    #[serde(default)]
    pub save_password: bool,
}
impl Default for ConnectionProfile {
    fn default() -> Self {
        Self {
            name: String::new(),
            driver: DbDriverType::Sqlite,
            host: "localhost".into(),
            port: 5432,
            user: String::new(),
            database: ":memory:".into(),
            ssh_profile_id: None,
            save_password: false,
        }
    }
}
impl ConnectionProfile {
    /// Validate before connecting or saving.
    /// # Errors
    /// Rejects missing fields, control characters and SQLite tunnels.
    pub fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty()
            || self.database.trim().is_empty()
            || [&self.name, &self.host, &self.user, &self.database]
                .iter()
                .any(|s| s.chars().any(char::is_control))
        {
            return Err(Error::Invalid(
                "Connection name and database are required; control characters are forbidden"
                    .into(),
            ));
        }
        if self.driver != DbDriverType::Sqlite
            && (self.host.trim().is_empty() || self.port == 0 || self.user.trim().is_empty())
        {
            return Err(Error::Invalid("Host, port and user are required".into()));
        }
        if self
            .ssh_profile_id
            .as_ref()
            .is_some_and(|id| id.trim().is_empty())
            || (self.driver == DbDriverType::Sqlite && self.ssh_profile_id.is_some())
        {
            return Err(Error::Invalid(
                "SQLite is local; SSH profile must be nonempty for server databases".into(),
            ));
        }
        Ok(())
    }
}
fn validate_profiles(profiles: &[ConnectionProfile]) -> Result<()> {
    let mut names = std::collections::HashSet::new();
    for p in profiles {
        p.validate()?;
        if !names.insert(&p.name) {
            return Err(Error::Invalid("Duplicate connection name".into()));
        }
    }
    Ok(())
}
/// Load profiles; missing files are empty, malformed files are errors.
/// # Errors
/// Returns I/O, JSON or validation failures.
pub fn load_profiles(path: &Path) -> Result<Vec<ConnectionProfile>> {
    let bytes = match std::fs::read(path) {
        Ok(v) => v,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let profiles: Vec<ConnectionProfile> = serde_json::from_slice(&bytes)?;
    validate_profiles(&profiles)?;
    Ok(profiles)
}
/// Atomically save profiles, refusing to overwrite an unreadable existing file.
/// # Errors
/// Returns existing file errors, validation or persistence failure.
pub fn save_profiles(path: &Path, profiles: &[ConnectionProfile]) -> Result<()> {
    use std::io::Write;
    load_profiles(path)?;
    validate_profiles(profiles)?;
    let parent = path
        .parent()
        .ok_or_else(|| Error::Invalid("Missing profile directory".into()))?;
    std::fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(&serde_json::to_vec_pretty(profiles)?)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|e| e.error)?;
    Ok(())
}
pub(crate) enum Pool {
    Postgres(sqlx::PgPool),
    Mysql(sqlx::MySqlPool),
    Sqlite(sqlx::SqlitePool),
}
/// A pool and its tunnel share a lifetime; queries never create an implicit SSH login.
pub struct Connection {
    pub(crate) pool: Pool,
    // Declared after pool: transport survives while the pool is being dropped.
    tunnel: Option<TunnelHandle>,
    pub(crate) driver: DbDriverType,
    pub(crate) database: String,
}
impl Connection {
    /// Connect with a deadline. Password is never embedded in a URL or profile.
    /// # Errors
    /// Returns validation, tunnel setup, authentication or driver failures.
    pub async fn connect(
        profile: &ConnectionProfile,
        password: Option<&Secret>,
        ssh: Option<&SshSession>,
    ) -> Result<Self> {
        profile.validate()?;
        let tunnel = if profile.ssh_profile_id.is_some() {
            Some(
                ssh.ok_or_else(|| Error::Invalid("SSH session required".into()))?
                    .start_tunnel(0, &profile.host, profile.port)
                    .await?,
            )
        } else {
            None
        };
        let (host, port) = tunnel
            .as_ref()
            .map_or((profile.host.as_str(), profile.port), |t| {
                ("127.0.0.1", t.local_addr().port())
            });
        let password = password.map_or("", Secret::expose);
        let pool = match profile.driver {
            DbDriverType::Postgres => Pool::Postgres(
                sqlx::postgres::PgPoolOptions::new()
                    .max_connections(4)
                    .acquire_timeout(Duration::from_secs(15))
                    .connect_with(
                        PgConnectOptions::new()
                            .host(host)
                            .port(port)
                            .username(&profile.user)
                            .password(password)
                            .database(&profile.database),
                    )
                    .await?,
            ),
            DbDriverType::Mysql => Pool::Mysql(
                sqlx::mysql::MySqlPoolOptions::new()
                    .max_connections(4)
                    .acquire_timeout(Duration::from_secs(15))
                    .connect_with(
                        MySqlConnectOptions::new()
                            .host(host)
                            .port(port)
                            .username(&profile.user)
                            .password(password)
                            .database(&profile.database),
                    )
                    .await?,
            ),
            DbDriverType::Sqlite => Pool::Sqlite(
                sqlx::sqlite::SqlitePoolOptions::new()
                    .max_connections(1)
                    .acquire_timeout(Duration::from_secs(15))
                    .connect_with(
                        SqliteConnectOptions::new()
                            .filename(&profile.database)
                            .in_memory(profile.database == ":memory:")
                            .create_if_missing(true)
                            .foreign_keys(true),
                    )
                    .await?,
            ),
        };
        Ok(Self {
            pool,
            tunnel,
            driver: profile.driver,
            database: profile.database.clone(),
        })
    }
    pub(crate) async fn healthy(&self) -> bool {
        use sqlx::Connection as _;
        if self.tunnel.as_ref().is_some_and(|t| !t.is_running()) {
            return false;
        }
        // A busy pool is not a transport failure; avoid competing with active queries.
        let ping = async {
            match &self.pool {
                Pool::Postgres(p) => {
                    if let Some(mut c) = p.try_acquire() {
                        c.ping().await
                    } else {
                        Ok(())
                    }
                }
                Pool::Mysql(p) => {
                    if let Some(mut c) = p.try_acquire() {
                        c.ping().await
                    } else {
                        Ok(())
                    }
                }
                Pool::Sqlite(_) => Ok(()),
            }
        };
        matches!(
            tokio::time::timeout(Duration::from_secs(2), ping).await,
            Ok(Ok(()))
        )
    }
    /// Dialect for identifier quoting and schema controls.
    #[must_use]
    pub const fn driver(&self) -> DbDriverType {
        self.driver
    }
}
/// Quote a single identifier; metadata names never become executable SQL.
#[must_use]
pub fn quote_identifier(driver: DbDriverType, name: &str) -> String {
    let quote = if driver == DbDriverType::Mysql {
        '`'
    } else {
        '"'
    };
    format!(
        "{quote}{}{quote}",
        name.replace(quote, &format!("{quote}{quote}"))
    )
}
/// Safely construct a qualified table preview limited to 1000 rows.
#[must_use]
pub fn preview_sql(driver: DbDriverType, schema: &str, table: &str) -> String {
    format!(
        "SELECT * FROM {}.{} LIMIT 1000",
        quote_identifier(driver, schema),
        quote_identifier(driver, table)
    )
}
