//! Relational database connections, typed results, metadata and background execution.
pub mod connection;
pub mod driver;
pub mod introspection;
pub mod worker;

pub use connection::{Connection, ConnectionProfile, load_profiles, save_profiles};
pub use driver::{ColumnInfo, DbDriverType, DbValue, QueryResult};
pub use introspection::DatabaseSchemaTree;
pub use tokio_util::sync::CancellationToken;

/// Database subsystem failures. SQL errors retain their original cause.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Driver or server failure.
    #[error("{0}")]
    Sql(#[from] sqlx::Error),
    /// Local profile I/O.
    #[error("{0}")]
    Io(#[from] std::io::Error),
    /// Profile syntax failure.
    #[error("{0}")]
    Json(#[from] serde_json::Error),
    /// SSH transport failure.
    #[error("{0}")]
    Ssh(#[from] nozhik_remote_ssh::Error),
    /// Invalid configuration or unsupported result type.
    #[error("{0}")]
    Invalid(String),
    /// Query cancelled; already committed effects cannot be rolled back.
    #[error("Query cancelled; committed effects may remain")]
    Cancelled,
    /// Protected credential store failure.
    #[error("Credential store: {0}")]
    Credential(#[from] keyring::Error),
}
/// Result with a database error.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests;
