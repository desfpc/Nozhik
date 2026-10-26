//! Driver-independent owned result models.
use serde::{Deserialize, Serialize};

/// Supported SQL dialect and transport.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DbDriverType {
    /// PostgreSQL.
    Postgres,
    /// MySQL and MariaDB.
    Mysql,
    /// Local SQLite file or :memory:.
    #[default]
    Sqlite,
}
/// Lossless common cell values. Decimal and unsigned overflow use Text.
#[derive(Clone, Debug, PartialEq)]
pub enum DbValue {
    /// SQL NULL.
    Null,
    /// Boolean.
    Bool(bool),
    /// Signed integer.
    Int(i64),
    /// Floating point.
    Float(f64),
    /// UTF-8 or exact decimal representation.
    Text(String),
    /// Binary payload.
    Bytes(Vec<u8>),
    /// ISO date/time representation.
    DateTime(String),
    /// Serialized JSON.
    Json(String),
}
impl std::fmt::Display for DbValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Null => f.write_str("NULL"),
            Self::Bool(v) => v.fmt(f),
            Self::Int(v) => v.fmt(f),
            Self::Float(v) => v.fmt(f),
            Self::Text(v) | Self::DateTime(v) | Self::Json(v) => f.write_str(v),
            Self::Bytes(v) => {
                f.write_str("0x")?;
                for byte in v {
                    write!(f, "{byte:02x}")?;
                }
                Ok(())
            }
        }
    }
}
/// Column label and native SQL type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnInfo {
    /// Returned label (including aliases).
    pub name: String,
    /// Driver-native type name.
    pub data_type: String,
}
/// One result set; console scripts retain each result set separately.
#[derive(Clone, Debug, Default)]
pub struct QueryResult {
    /// Ordered column descriptors, including for empty SELECTs.
    pub columns: Vec<ColumnInfo>,
    /// Rows matching the column order.
    pub rows: Vec<Vec<DbValue>>,
    /// Elapsed wall time in milliseconds.
    pub execution_time_ms: u64,
    /// Driver-reported affected rows.
    pub rows_affected: u64,
    /// Result exceeded the retained row or byte limit.
    pub truncated: bool,
}
