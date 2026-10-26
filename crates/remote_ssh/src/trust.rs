//! Trust-on-first-use store keyed by host and port; changed keys require manual review.
use crate::{Error, Result, SshProfile};
use std::{collections::BTreeMap, path::Path};

/// Validate an OpenSSH SHA256 fingerprint (unpadded base64, 32-byte digest).
/// # Errors
/// Returns an invalid fingerprint error.
pub fn validate_fingerprint(fingerprint: &str) -> Result<()> {
    let parsed = fingerprint
        .parse::<russh::keys::ssh_key::Fingerprint>()
        .map_err(|_| Error::Invalid("invalid SHA256 host fingerprint".into()))?;
    if parsed.algorithm() != russh::keys::HashAlg::Sha256 {
        return Err(Error::Invalid("host fingerprint must use SHA256".into()));
    }
    Ok(())
}
fn address(profile: &SshProfile) -> String {
    format!("[{}]:{}", profile.host.to_lowercase(), profile.port)
}
fn load(path: &Path) -> Result<BTreeMap<String, String>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(e) => return Err(e.into()),
    };
    let entries: BTreeMap<String, String> = serde_json::from_slice(&bytes)?;
    for value in entries.values() {
        validate_fingerprint(value)?;
    }
    Ok(entries)
}
/// Fill an unpinned profile with the approved host identity, retaining legacy pins.
/// # Errors
/// Returns malformed trust file, conflicting legacy pin or filesystem errors.
pub fn resolve(path: &Path, mut profile: SshProfile) -> Result<SshProfile> {
    profile.validate()?;
    if let Some(pin) = load(path)?.get(&address(&profile)) {
        if !profile.host_key_sha256.is_empty() && profile.host_key_sha256 != *pin {
            return Err(Error::HostKeyChanged {
                expected: pin.clone(),
                received: profile.host_key_sha256,
            });
        }
        profile.host_key_sha256.clone_from(pin);
    }
    Ok(profile)
}
/// Persist a user's explicit first-use approval. Never replace a different trusted key.
/// # Errors
/// Returns conflict, malformed trust file, validation or persistence errors.
pub fn remember(path: &Path, profile: &SshProfile, fingerprint: &str) -> Result<()> {
    use std::io::Write;
    profile.validate()?;
    validate_fingerprint(fingerprint)?;
    let mut entries = load(path)?;
    let key = address(profile);
    if let Some(expected) = entries.get(&key)
        && expected != fingerprint
    {
        return Err(Error::HostKeyChanged {
            expected: expected.clone(),
            received: fingerprint.into(),
        });
    }
    entries.insert(key, fingerprint.into());
    let parent = path
        .parent()
        .ok_or_else(|| Error::Invalid("trust path has no parent".into()))?;
    std::fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(&serde_json::to_vec_pretty(&entries)?)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|e| e.error)?;
    Ok(())
}
