//! Token storage scoped to the repository and exact transport preferences.
use crate::{VcsError, clone::Authentication, connection::GitSettings};
use zeroize::Zeroizing;

type Result<T> = std::result::Result<T, VcsError>;
pub trait Store: Send + Sync {
    fn load(&self, account: &str) -> Result<Option<Zeroizing<String>>>;
    fn save(&self, account: &str, token: &str) -> Result<()>;
    fn delete(&self, account: &str) -> Result<()>;
}
pub struct OsStore;
fn entry(account: &str) -> Result<keyring::Entry> {
    Ok(keyring::Entry::new("dev.Nozhik.Nozhik.Git", account)?)
}
impl Store for OsStore {
    fn load(&self, account: &str) -> Result<Option<Zeroizing<String>>> {
        match entry(account)?.get_password() {
            Ok(token) => Ok(Some(Zeroizing::new(token))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
    fn save(&self, account: &str, token: &str) -> Result<()> {
        entry(account)?.set_password(token)?;
        Ok(())
    }
    fn delete(&self, account: &str) -> Result<()> {
        match entry(account)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}
fn identity(repo: &git2::Repository, settings: &GitSettings) -> Result<String> {
    serde_json::to_string(&(
        repo.commondir().canonicalize()?,
        &settings.remote_url,
        &settings.push_url,
        &settings.auth.username,
        &settings.auth.key_path,
    ))
    .map_err(|_| VcsError::Invalid("Cannot identify Git token storage".into()))
}
pub fn resolve(
    store: &dyn Store,
    repo: &git2::Repository,
    settings: &GitSettings,
    auth: Authentication,
) -> Result<Authentication> {
    if settings.save_token
        && matches!(&auth, Authentication::Token(token) if token.is_empty())
        && let Some(token) = store.load(&identity(repo, settings)?)?
    {
        return Ok(Authentication::Token(token));
    }
    Ok(auth)
}
pub fn update(
    store: &dyn Store,
    repo: &git2::Repository,
    old: &GitSettings,
    new: &GitSettings,
    auth: &Authentication,
) -> Result<()> {
    if new.save_token {
        let Authentication::Token(token) = auth else {
            return Err(VcsError::Invalid("Only HTTPS tokens can be saved".into()));
        };
        if token.trim().is_empty() {
            return Err(VcsError::Invalid(
                "Enter a token before enabling system storage".into(),
            ));
        }
        store.save(&identity(repo, new)?, token)?;
    }
    if old.save_token && (!new.save_token || identity(repo, old)? != identity(repo, new)?) {
        store.delete(&identity(repo, old)?)?;
    }
    Ok(())
}
