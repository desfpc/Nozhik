//! Driver-independent owned result models.
use serde::{Deserialize, Serialize};

/// Supported SQL dialect and transport.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DbDriverType {
    /// PostgreSQL.
    Postgres,
    /// `ClickHouse` over HTTP(S).
    ClickHouse,
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
    /// MySQL internal geometry: little-endian SRID followed by WKB.
    Geometry(Vec<u8>),
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
            Self::Geometry(v) => {
                if let Some(point) = geometry_point(v) {
                    write!(f, "POINT ({} {})", point.0, point.1)
                } else {
                    write!(f, "GEOMETRY {}", Self::Bytes(v.clone()))
                }
            }
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

// MySQL sends a 4-byte SRID prefix before standard WKB. Preserve the original
// bytes for every geometry type; only POINT receives a compact text preview.
fn geometry_point(bytes: &[u8]) -> Option<(f64, f64)> {
    let wkb = bytes.get(4..)?;
    let little = match wkb.first()? {
        0 => false,
        1 => true,
        _ => return None,
    };
    let kind = wkb.get(1..5)?.try_into().ok()?;
    if (if little {
        u32::from_le_bytes(kind)
    } else {
        u32::from_be_bytes(kind)
    }) != 1
        || wkb.len() != 21
    {
        return None;
    }
    let read = |range: std::ops::Range<usize>| {
        let value = wkb.get(range)?.try_into().ok()?;
        Some(if little {
            f64::from_le_bytes(value)
        } else {
            f64::from_be_bytes(value)
        })
    };
    Some((read(5..13)?, read(13..21)?))
}
