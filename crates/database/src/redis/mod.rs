//! Redis profiles, bounded key browsing and optimistic structured editing.
mod transport;
pub mod worker;

use nozhik_remote_ssh::credentials::Secret;
use serde::{Deserialize, Serialize};
use std::path::Path;
pub use transport::Connection;

/// Redis-specific errors; credentials are never included in endpoint URLs.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Redis command or connection error.
    #[error("Redis: {0}")]
    Redis(#[from] ::redis::RedisError),
    /// Profile or editor validation error.
    #[error("{0}")]
    Invalid(String),
    /// Local storage error.
    #[error("{0}")]
    Io(#[from] std::io::Error),
    /// JSON syntax error.
    #[error("{0}")]
    Json(#[from] serde_json::Error),
    /// Verified SSH transport error.
    #[error("{0}")]
    Ssh(#[from] nozhik_remote_ssh::Error),
    /// Protected credential store failure.
    #[error("Credential store: {0}")]
    Credential(#[from] keyring::Error),
}
/// Redis operation result.
pub type Result<T> = std::result::Result<T, Error>;
/// Public connection settings. Passwords are separate secrets.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    /// Unique connection name.
    pub name: String,
    /// Endpoint as resolved by the SSH server when tunneled.
    pub host: String,
    /// Redis TCP port.
    pub port: u16,
    /// Optional Redis ACL username; empty uses default authentication.
    pub user: String,
    /// Logical database, validated by SELECT on connection.
    pub database: u32,
    /// Existing verified SSH profile.
    pub ssh_profile_id: Option<String>,
    /// Explicit opt-in to native protected password storage.
    pub save_password: bool,
}
impl Default for Profile {
    fn default() -> Self {
        Self {
            name: String::new(),
            host: "localhost".into(),
            port: 6379,
            user: String::new(),
            database: 0,
            ssh_profile_id: None,
            save_password: false,
        }
    }
}
impl Profile {
    /// Validate public settings without connecting.
    /// # Errors
    /// Rejects missing endpoints and control characters.
    pub fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty()
            || self.host.trim().is_empty()
            || self.port == 0
            || [&self.name, &self.host, &self.user]
                .iter()
                .any(|v| v.chars().any(char::is_control))
            || self
                .ssh_profile_id
                .as_ref()
                .is_some_and(|s| s.trim().is_empty())
        {
            return Err(Error::Invalid(
                "Redis name, host and port are required; control characters are forbidden".into(),
            ));
        }
        Ok(())
    }
    pub(super) fn credential(&self) -> Result<keyring::Entry> {
        let identity =
            serde_json::to_string(&(&self.name, &self.host, self.port, &self.user, self.database))?;
        Ok(keyring::Entry::new("dev.Nozhik.Nozhik.Redis", &identity)?)
    }
}
/// Read profiles, treating only a missing file as an empty list.
/// # Errors
/// Reports malformed, duplicate or invalid profiles.
pub fn load_profiles(path: &Path) -> Result<Vec<Profile>> {
    let bytes = match std::fs::read(path) {
        Ok(v) => v,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let profiles: Vec<Profile> = serde_json::from_slice(&bytes)?;
    validate_profiles(&profiles)?;
    Ok(profiles)
}
fn validate_profiles(profiles: &[Profile]) -> Result<()> {
    let mut names = std::collections::HashSet::new();
    for p in profiles {
        p.validate()?;
        if !names.insert(&p.name) {
            return Err(Error::Invalid("Duplicate Redis connection name".into()));
        }
    }
    Ok(())
}
/// Atomically save public profiles, preserving an existing malformed file.
/// # Errors
/// Reports validation and file I/O failures.
pub fn save_profiles(path: &Path, profiles: &[Profile]) -> Result<()> {
    use std::io::Write as _;
    load_profiles(path)?;
    validate_profiles(profiles)?;
    let parent = path
        .parent()
        .ok_or_else(|| Error::Invalid("Missing profile directory".into()))?;
    std::fs::create_dir_all(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(&serde_json::to_vec_pretty(profiles)?)?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|e| e.error)?;
    Ok(())
}
/// A lossless Redis byte string in the JSON editor.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum Atom {
    /// Valid UTF-8 bytes.
    Text(String),
    /// Non-UTF-8 bytes, encoded as hexadecimal.
    Hex {
        /// Two hexadecimal characters per byte.
        hex: String,
    },
}
impl Atom {
    /// Encode bytes without loss, including non-UTF-8 values.
    #[must_use]
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        String::from_utf8(bytes).map_or_else(
            |e| Self::Hex {
                hex: e
                    .into_bytes()
                    .iter()
                    .flat_map(|b| {
                        [
                            char::from(b"0123456789abcdef"[usize::from(b >> 4)]),
                            char::from(b"0123456789abcdef"[usize::from(b & 15)]),
                        ]
                    })
                    .collect(),
            },
            Self::Text,
        )
    }
    /// Decode editor contents.
    /// # Errors
    /// Rejects malformed hexadecimal input.
    pub fn bytes(&self) -> Result<Vec<u8>> {
        match self {
            Self::Text(s) => Ok(s.as_bytes().to_vec()),
            Self::Hex { hex } => {
                if !hex.len().is_multiple_of(2) || !hex.is_ascii() {
                    return Err(Error::Invalid(
                        "Hex requires pairs of ASCII hexadecimal digits".into(),
                    ));
                }
                hex.as_bytes()
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|pair| {
                        let digit = |v: u8| {
                            char::from(v)
                                .to_digit(16)
                                .ok_or_else(|| Error::Invalid("Invalid hexadecimal digit".into()))
                        };
                        u8::try_from(digit(pair[0])? * 16 + digit(pair[1])?)
                            .map_err(|e| Error::Invalid(e.to_string()))
                    })
                    .collect()
            }
        }
    }
    /// Compact printable preview.
    #[must_use]
    pub fn preview(&self) -> String {
        match self {
            Self::Text(s) => s.chars().take(256).flat_map(char::escape_debug).collect(),
            Self::Hex { hex } => format!("0x{}", hex.chars().take(256).collect::<String>()),
        }
    }
}
/// Supported general-purpose Redis structures.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "lowercase",
    deny_unknown_fields
)]
pub enum Data {
    /// Binary-safe string.
    String(Atom),
    /// Ordered list.
    List(Vec<Atom>),
    /// Unordered unique members.
    Set(Vec<Atom>),
    /// Field/value pairs.
    Hash(Vec<(Atom, Atom)>),
    /// Member/score pairs; scores use lossless server text.
    Zset(Vec<(Atom, String)>),
    /// Immutable ID and field/value pairs.
    Stream(Vec<(String, Vec<(Atom, Atom)>)>),
    /// Unsupported server or module type, shown read-only.
    Unsupported(String),
}
impl Data {
    /// Redis native TYPE name.
    #[must_use]
    pub fn kind(&self) -> &str {
        match self {
            Self::String(_) => "string",
            Self::List(_) => "list",
            Self::Set(_) => "set",
            Self::Hash(_) => "hash",
            Self::Zset(_) => "zset",
            Self::Stream(_) => "stream",
            Self::Unsupported(kind) => kind,
        }
    }
    /// Parse editor JSON and validate all arguments before any writes.
    /// # Errors
    /// Rejects invalid bytes, duplicate identities and invalid scores.
    pub fn parse(text: &str) -> Result<Self> {
        if text.len() > transport::MAX_BYTES {
            return Err(Error::Invalid("Editor limit: 1 MiB".into()));
        }
        let value: Self = serde_json::from_str(text)?;
        value.arguments()?;
        Ok(value)
    }
    pub(super) fn arguments(&self) -> Result<Vec<Vec<u8>>> {
        let mut out = Vec::new();
        let mut unique = std::collections::HashSet::new();
        match self {
            Self::String(v) => out.push(v.bytes()?),
            Self::List(values) | Self::Set(values) => {
                for v in values {
                    let bytes = v.bytes()?;
                    if matches!(self, Self::Set(_)) && !unique.insert(bytes.clone()) {
                        return Err(Error::Invalid("Duplicate set member".into()));
                    }
                    out.push(bytes);
                }
            }
            Self::Hash(values) => {
                for (k, v) in values {
                    let key = k.bytes()?;
                    if !unique.insert(key.clone()) {
                        return Err(Error::Invalid("Duplicate hash field".into()));
                    }
                    out.push(key);
                    out.push(v.bytes()?);
                }
            }
            Self::Zset(values) => {
                for (member, score) in values {
                    let bytes = member.bytes()?;
                    if !unique.insert(bytes.clone())
                        || score.parse::<f64>().map_or(true, f64::is_nan)
                    {
                        return Err(Error::Invalid(
                            "Invalid score or duplicate sorted set member".into(),
                        ));
                    }
                    out.push(score.as_bytes().to_vec());
                    out.push(bytes);
                }
            }
            Self::Stream(_) | Self::Unsupported(_) => {
                return Err(Error::Invalid(
                    "Use Append / Delete entry for streams; unsupported types are read-only".into(),
                ));
            }
        }
        if out.is_empty() {
            return Err(Error::Invalid(
                "A collection needs at least one element; use Delete Key to remove it".into(),
            ));
        }
        let count = match self {
            Self::String(_) => 1,
            Self::List(values) | Self::Set(values) => values.len(),
            Self::Hash(values) => values.len(),
            Self::Zset(values) => values.len(),
            Self::Stream(values) => values.len(),
            Self::Unsupported(_) => 0,
        };
        if count > 1000 || out.iter().map(Vec::len).sum::<usize>() > transport::MAX_BYTES {
            return Err(Error::Invalid("Editor limit: 1000 elements / 1 MiB".into()));
        }
        Ok(out)
    }
}
/// Key listing row; values are bounded previews, not the complete editor snapshot.
#[derive(Clone, Debug)]
pub struct KeyRow {
    /// Original key bytes.
    pub key: Atom,
    /// Native TYPE name.
    pub kind: String,
    /// Bounded value or structure preview.
    pub preview: String,
    /// Remaining milliseconds; -1 means persistent, -2 means expired/missing.
    pub ttl_ms: i64,
}
/// One cursor step; empty pages may have a nonzero continuation.
#[derive(Clone, Debug)]
pub struct ScanPage {
    /// Next SCAN cursor, zero means complete.
    pub cursor: u64,
    /// Keys from this step.
    pub rows: Vec<KeyRow>,
}
/// Consistent key editor baseline.
#[derive(Clone, Debug)]
pub struct Snapshot {
    /// Original key bytes.
    pub key: Atom,
    /// Structure contents.
    pub data: Data,
    /// Serialized server value used for conflict detection.
    pub dump: Vec<u8>,
    /// TTL at load, in milliseconds.
    pub ttl_ms: i64,
    /// Partial or oversized values cannot be replaced.
    pub truncated: bool,
}
/// A single explicit editor operation.
#[derive(Clone, Debug)]
pub enum Mutation {
    /// Replace a supported structure, preserving its current expiry.
    Replace(Data),
    /// Delete exactly the loaded key.
    Delete,
    /// Set a positive TTL in seconds; None removes expiry.
    Expire(Option<u64>),
    /// Append fields under a server-generated Stream ID.
    Append(Vec<(Atom, Atom)>),
    /// Remove an existing Stream record by ID.
    DeleteEntry(String),
}
fn copy_secret(secret: &Secret) -> Secret {
    Secret::new(secret.expose().into())
}

#[cfg(test)]
mod tests;
