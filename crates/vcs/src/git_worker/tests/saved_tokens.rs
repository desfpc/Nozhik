//! Isolated restart and storage-failure regressions; never access the real keychain.
use super::*;
use crate::{clone::Authentication, connection::GitSettings, credentials::Store};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use zeroize::Zeroizing;
type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

#[derive(Default)]
struct MemoryStore {
    entries: Mutex<std::collections::BTreeMap<String, Zeroizing<String>>>,
    denied: AtomicBool,
}
impl MemoryStore {
    fn check(&self) -> std::result::Result<(), VcsError> {
        if self.denied.load(Ordering::Relaxed) {
            Err(keyring::Error::NoStorageAccess(Box::new(std::io::Error::other("locked"))).into())
        } else {
            Ok(())
        }
    }
}
impl Store for MemoryStore {
    fn load(&self, account: &str) -> std::result::Result<Option<Zeroizing<String>>, VcsError> {
        self.check()?;
        Ok(self
            .entries
            .lock()
            .map_err(|_| invalid("poisoned test store"))?
            .get(account)
            .cloned())
    }
    fn save(&self, account: &str, token: &str) -> std::result::Result<(), VcsError> {
        self.check()?;
        self.entries
            .lock()
            .map_err(|_| invalid("poisoned test store"))?
            .insert(account.into(), Zeroizing::new(token.into()));
        Ok(())
    }
    fn delete(&self, account: &str) -> std::result::Result<(), VcsError> {
        self.check()?;
        self.entries
            .lock()
            .map_err(|_| invalid("poisoned test store"))?
            .remove(account);
        Ok(())
    }
}
fn worker(path: &Path, store: &Arc<MemoryStore>) -> std::result::Result<GitWorker, VcsError> {
    let mut worker = GitWorker {
        credentials: store.clone(),
        ..GitWorker::default()
    };
    worker.execute(Task::Open(path.into()))?;
    Ok(worker)
}
fn token(text: &str) -> Authentication {
    Authentication::Token(Zeroizing::new(text.into()))
}
fn settings(worker: &mut GitWorker) -> std::result::Result<GitSettings, VcsError> {
    match worker.execute(Task::GitSettings)? {
        Outcome::GitSettings(value) => Ok(value),
        _ => Err(invalid("missing test settings")),
    }
}
fn save(
    worker: &mut GitWorker,
    mut new: GitSettings,
    text: &str,
) -> std::result::Result<GitSettings, VcsError> {
    new.auth = token(text).profile();
    let expected = settings(worker)?;
    worker.execute(Task::SaveGitSettings {
        expected,
        settings: new,
        authentication: token(text),
    })?;
    settings(worker)
}
fn resolved(
    worker: &GitWorker,
    settings: &GitSettings,
) -> std::result::Result<Authentication, VcsError> {
    crate::credentials::resolve(
        worker.credentials.as_ref(),
        worker
            .repo
            .as_ref()
            .ok_or_else(|| invalid("missing repo"))?,
        settings,
        worker.session.authentication(settings),
    )
}
fn assert_token(auth: Authentication, text: &str) {
    assert!(matches!(auth, Authentication::Token(value) if value.as_str() == text));
}
#[test]
fn token_survives_restart_blank_saves_and_is_removed_on_opt_out() -> TestResult {
    let (dir, repo) = repository()?;
    let store = Arc::new(MemoryStore::default());
    let mut first = worker(dir.path(), &store)?;
    let mut new = settings(&mut first)?;
    new.remote_url = "https://github.com/owner/repo".into();
    new.save_token = true;
    let saved = save(&mut first, new, "persistent-test-token")?;
    drop(first);
    let mut restarted = worker(dir.path(), &store)?;
    assert_token(resolved(&restarted, &saved)?, "persistent-test-token");
    let mut edited = saved;
    edited.author_name = "Changed author".into();
    let edited = save(&mut restarted, edited, "")?;
    assert_token(resolved(&restarted, &edited)?, "persistent-test-token");
    save(&mut restarted, edited.clone(), "replacement-test-token")?;
    let mut fresh = worker(dir.path(), &store)?;
    assert_token(resolved(&fresh, &edited)?, "replacement-test-token");
    let config = fs::read_to_string(repo.commondir().join("config"))?;
    assert!(
        !config.contains("persistent-test-token") && !config.contains("replacement-test-token")
    );
    let mut disabled = edited;
    disabled.save_token = false;
    let disabled = save(&mut fresh, disabled, "")?;
    let fresh = worker(dir.path(), &store)?;
    assert_token(resolved(&fresh, &disabled)?, "");
    assert!(store.entries.lock().map_err(|_| "poisoned")?.is_empty());
    Ok(())
}
#[test]
fn tokens_are_bound_to_repository_and_both_urls() -> TestResult {
    let (dir, repo) = repository()?;
    let store = Arc::new(MemoryStore::default());
    let mut first = worker(dir.path(), &store)?;
    let mut new = settings(&mut first)?;
    new.remote_url = "https://github.com/owner/repo".into();
    new.save_token = true;
    let saved = save(&mut first, new, "scoped-test-token")?;
    for push in [false, true] {
        let mut changed = saved.clone();
        if push {
            changed.push_url = "https://github.com/other/push".into();
        } else {
            changed.remote_url = "https://github.com/other/fetch".into();
        }
        assert_token(resolved(&first, &changed)?, "");
        assert!(save(&mut first, changed, "").is_err());
        assert_eq!(settings(&mut first)?, saved);
    }
    let (other_dir, other_repo) = repository()?;
    let old = crate::connection::read(&other_repo)?;
    crate::connection::save(&other_repo, &old, saved.clone())?;
    let other = worker(other_dir.path(), &store)?;
    assert_token(resolved(&other, &saved)?, "");
    repo.remote_set_url("origin", "https://github.com/external/edit")?;
    assert_token(resolved(&first, &crate::connection::read(&repo)?)?, "");
    Ok(())
}
#[test]
fn storage_errors_and_config_locks_preserve_settings_without_plaintext_fallback() -> TestResult {
    let (dir, repo) = repository()?;
    let store = Arc::new(MemoryStore::default());
    let mut first = worker(dir.path(), &store)?;
    let before = settings(&mut first)?;
    let mut new = before.clone();
    new.remote_url = "https://github.com/owner/repo".into();
    new.save_token = true;
    fs::write(repo.commondir().join("config.lock"), "busy")?;
    assert!(save(&mut first, new.clone(), "test-token").is_err());
    assert!(store.entries.lock().map_err(|_| "poisoned")?.is_empty());
    fs::remove_file(repo.commondir().join("config.lock"))?;
    store.denied.store(true, Ordering::Relaxed);
    assert!(save(&mut first, new.clone(), "test-token").is_err());
    assert_eq!(settings(&mut first)?, before);
    assert!(!repo.commondir().join("config.lock").exists());
    store.denied.store(false, Ordering::Relaxed);
    let saved = save(&mut first, new, "test-token")?;
    let mut fresh = worker(dir.path(), &store)?;
    store.denied.store(true, Ordering::Relaxed);
    assert!(resolved(&fresh, &saved).is_err());
    for task in [
        Task::Fetch,
        Task::Push,
        Task::Pull,
        Task::PullPush,
        Task::TestGitConnection {
            settings: saved.clone(),
            authentication: token(""),
        },
    ] {
        let error = fresh
            .execute(task)
            .err()
            .ok_or("expected locked store error")?;
        assert!(matches!(error, VcsError::Credential(_)));
    }
    store.denied.store(false, Ordering::Relaxed);
    store.entries.lock().map_err(|_| "poisoned")?.clear();
    assert_token(resolved(&fresh, &saved)?, "");
    assert!(!fs::read_to_string(repo.commondir().join("config"))?.contains("test-token"));
    Ok(())
}
#[test]
fn clone_opt_in_saves_token_for_reopened_project() -> TestResult {
    let (dir, repo) = repository()?;
    let store = Arc::new(MemoryStore::default());
    let url = "https://github.com/owner/repo";
    repo.remote("origin", url)?;
    crate::connection::remember_clone(&repo, &token("cloned-test-token"))?;
    let mut first = worker(dir.path(), &store)?;
    first.execute(Task::Authenticate(crate::clone::Credentials {
        url: url.into(),
        authentication: token("cloned-test-token"),
        save_token: true,
    }))?;
    let saved = settings(&mut first)?;
    assert!(saved.save_token);
    let fresh = worker(dir.path(), &store)?;
    assert_token(resolved(&fresh, &saved)?, "cloned-test-token");
    Ok(())
}
