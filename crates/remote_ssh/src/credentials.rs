//! OS-protected credential persistence. No plaintext file fallback is permitted.
use crate::{Result, SshProfile};

/// Ephemeral password/passphrase; debug output is redacted and drop zeroes its buffer.
pub struct Secret(zeroize::Zeroizing<String>);
impl Secret {
    /// Take ownership of an entered password.
    #[must_use]
    pub fn new(value: String) -> Self {
        Self(zeroize::Zeroizing::new(value))
    }
    /// Borrow a secret only for authentication or the OS credential API.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}
impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret([redacted])")
    }
}
/// Storage boundary for native stores and isolated behavioral test doubles.
pub trait CredentialStore: Send + Sync {
    /// Load the saved secret; absence is distinct from a locked/unavailable store.
    /// # Errors
    /// Returns access/OS store failures.
    fn load(&self, profile: &SshProfile) -> Result<Option<Secret>>;
    /// Save a secret in the operating system's protected credential store.
    /// # Errors
    /// Returns access/OS store failures, never falls back to files.
    fn save(&self, profile: &SshProfile, secret: &Secret) -> Result<()>;
    /// Remove the stored secret; a missing credential is already removed.
    /// # Errors
    /// Returns access/OS store failures.
    fn delete(&self, profile: &SshProfile) -> Result<()>;
}
/// macOS Keychain / Windows Credential Manager / Linux Secret Service.
pub struct OsCredentialStore;
/// Credential identity includes profile name, endpoint, user and authentication method.
/// Password preference, forwarding rules and legacy pins do not change the identity.
#[must_use]
pub fn same_identity(a: &SshProfile, b: &SshProfile) -> bool {
    a.name == b.name
        && a.host.eq_ignore_ascii_case(&b.host)
        && a.port == b.port
        && a.username == b.username
        && a.auth == b.auth
}
fn entry(profile: &SshProfile) -> Result<keyring::Entry> {
    profile.validate()?;
    let account = serde_json::to_string(&(
        &profile.name,
        profile.host.to_lowercase(),
        profile.port,
        &profile.username,
        &profile.auth,
    ))?;
    Ok(keyring::Entry::new("dev.Nozhik.Nozhik.SSH", &account)?)
}
fn load_entry(entry: &keyring::Entry) -> Result<Option<Secret>> {
    match entry.get_password() {
        Ok(value) => Ok(Some(Secret::new(value))),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(error.into()),
    }
}
fn delete_entry(entry: &keyring::Entry) -> Result<()> {
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(error.into()),
    }
}
impl CredentialStore for OsCredentialStore {
    fn load(&self, profile: &SshProfile) -> Result<Option<Secret>> {
        load_entry(&entry(profile)?)
    }
    fn save(&self, profile: &SshProfile, secret: &Secret) -> Result<()> {
        entry(profile)?.set_password(secret.expose())?;
        Ok(())
    }
    fn delete(&self, profile: &SshProfile) -> Result<()> {
        delete_entry(&entry(profile)?)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn protected_entry_roundtrip_delete_and_error_no_fallback() -> Result<()> {
        let entry =
            keyring::Entry::new_with_credential(Box::<keyring::mock::MockCredential>::default());
        assert!(load_entry(&entry)?.is_none());
        let secret = Secret::new("unique-password".into());
        entry.set_password(secret.expose())?;
        assert_eq!(
            load_entry(&entry)?
                .ok_or_else(|| crate::Error::Invalid("missing secret".into()))?
                .expose(),
            "unique-password"
        );
        assert!(!format!("{secret:?}").contains("unique-password"));
        let mock = entry
            .get_credential()
            .downcast_ref::<keyring::mock::MockCredential>()
            .ok_or_else(|| crate::Error::Invalid("mock missing".into()))?;
        mock.set_error(keyring::Error::Invalid("store".into(), "locked".into()));
        assert!(load_entry(&entry).is_err());
        delete_entry(&entry)?;
        delete_entry(&entry)?;
        assert!(load_entry(&entry)?.is_none());
        Ok(())
    }
}
