//! SSH connections, ephemeral credentials, cached SFTP and loopback tunnels.
pub mod client;
pub mod credentials;
pub mod sftp;
pub mod trust;
pub mod tunnel;

pub use client::{SshSession, SshShellStream};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Serializable authentication selection. Secrets never belong to profiles.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum AuthMethod {
    /// Ask for a password for each new connection.
    Password,
    /// OpenSSH private key; passphrase supplied separately in memory.
    PrivateKeyFile {
        /// Native private key path; a leading ~/ is expanded on the worker.
        path: PathBuf,
    },
    /// `SSH_AUTH_SOCK` on Unix, OpenSSH named pipe on Windows.
    Agent,
}
/// Local listener and remote target, resolved by the SSH server.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PortForward {
    /// Loopback listener port; zero allocates an available port.
    pub local_port: u16,
    /// Target as seen from the remote machine.
    pub remote_host: String,
    /// Target TCP port.
    pub remote_port: u16,
}
/// Persisted connection configuration; passwords/passphrases are never serialized.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SshProfile {
    /// Unique display name.
    pub name: String,
    /// DNS hostname or IP literal.
    pub host: String,
    /// SSH port, defaults to 22.
    #[serde(default = "default_port")]
    pub port: u16,
    /// Remote login name.
    pub username: String,
    /// Authentication method.
    pub auth: AuthMethod,
    /// Optional legacy host key pin. New profiles use the separate trust store.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub host_key_sha256: String,
    /// User opted into OS-protected credential storage; no secret is serialized.
    #[serde(default)]
    pub save_password: bool,
    /// Tunnels started explicitly by the UI.
    #[serde(default)]
    pub forwards: Vec<PortForward>,
}
const fn default_port() -> u16 {
    22
}
impl SshProfile {
    /// Checks fields before persistence or any connection attempt.
    /// # Errors
    /// Returns invalid field/port/fingerprint errors.
    pub fn validate(&self) -> Result<()> {
        for field in [&self.name, &self.host, &self.username] {
            if field.trim().is_empty() || field.chars().any(char::is_control) {
                return Err(Error::Invalid(
                    "name, host and user must be nonempty and contain no control characters".into(),
                ));
            }
        }
        if self.port == 0 {
            return Err(Error::Invalid("SSH port must be nonzero".into()));
        }
        if !self.host_key_sha256.is_empty() {
            trust::validate_fingerprint(&self.host_key_sha256)?;
        }
        if let AuthMethod::PrivateKeyFile { path } = &self.auth
            && path.as_os_str().is_empty()
        {
            return Err(Error::Invalid("private key path is empty".into()));
        }
        for forward in &self.forwards {
            if forward.remote_host.trim().is_empty()
                || forward.remote_host.chars().any(char::is_control)
                || forward.remote_port == 0
            {
                return Err(Error::Invalid("invalid tunnel destination".into()));
            }
        }
        Ok(())
    }
}
/// Load profiles, preserving malformed files rather than overwriting them.
/// # Errors
/// Returns I/O, JSON, duplicate name or validation failures.
pub fn load_profiles(path: &Path) -> Result<Vec<SshProfile>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let profiles: Vec<SshProfile> = serde_json::from_slice(&bytes)?;
    validate_profiles(&profiles)?;
    Ok(profiles)
}
fn validate_profiles(profiles: &[SshProfile]) -> Result<()> {
    let mut names = std::collections::HashSet::new();
    for profile in profiles {
        profile.validate()?;
        if !names.insert(&profile.name) {
            return Err(Error::Invalid("duplicate SSH profile name".into()));
        }
    }
    Ok(())
}
/// Atomically persist public configuration via a temporary file in the same folder.
/// # Errors
/// Returns validation, serialization or filesystem errors.
pub fn save_profiles(path: &Path, profiles: &[SshProfile]) -> Result<()> {
    use std::io::Write;
    validate_profiles(profiles)?;
    let parent = path
        .parent()
        .ok_or_else(|| Error::Invalid("profile path has no parent".into()))?;
    std::fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(&serde_json::to_vec_pretty(profiles)?)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|e| e.error)?;
    Ok(())
}
/// SSH subsystem failures, including bounded operation deadlines.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// SSH authentication completed unsuccessfully.
    #[error("SSH authentication rejected")]
    AuthenticationRejected,
    /// Native credential store failure; no plaintext fallback is used.
    #[error("system credential store: {0}")]
    Credential(#[from] keyring::Error),
    /// Server presented a key that has not yet been approved by the user.
    #[error("SSH server key requires trust: {0}")]
    UnknownHostKey(String),
    /// A previously trusted host changed its identity; never accepted automatically.
    #[error("SSH HOST KEY CHANGED: expected {expected}, received {received}. Connection blocked.")]
    HostKeyChanged {
        /// Previously trusted fingerprint.
        expected: String,
        /// Fingerprint presented by this connection.
        received: String,
    },
    /// Invalid configuration or rejected authentication.
    #[error("{0}")]
    Invalid(String),
    /// SSH protocol/transport failure.
    #[error(transparent)]
    Ssh(#[from] russh::Error),
    /// SFTP protocol failure.
    #[error(transparent)]
    Sftp(#[from] russh_sftp::client::error::Error),
    /// OS I/O failure.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// Malformed profiles.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// Deadline elapsed.
    #[error("SSH operation timed out")]
    Timeout(#[from] tokio::time::error::Elapsed),
    /// Key loading or agent error.
    #[error(transparent)]
    Keys(#[from] russh::keys::Error),
}
/// SSH operation result.
pub type Result<T> = std::result::Result<T, Error>;
/// Bound for connect, authentication and individual channel operations.
pub const OPERATION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

#[cfg(test)]
mod tests;
