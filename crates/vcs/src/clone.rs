//! Blocking repository cloning; callers must use a background worker.
use crate::VcsError;
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use zeroize::Zeroizing;

/// Session-only authentication. Secrets are never stored in Git configuration.
#[derive(Clone, Default)]
pub enum Authentication {
    /// Public repository, SSH agent or Git credential helper.
    #[default]
    Automatic,
    /// HTTPS personal access token.
    Token(Zeroizing<String>),
    /// HTTPS basic authentication for servers that support it.
    Password {
        /// Account name.
        username: String,
        /// Password or server token.
        password: Zeroizing<String>,
    },
    /// SSH private key with an optional passphrase.
    SshKey {
        /// SSH user, commonly `git`.
        username: String,
        /// Local private key file; its contents stay owned by libgit2.
        path: PathBuf,
        /// Optional key passphrase.
        passphrase: Zeroizing<String>,
    },
}

/// Resolves GitHub shorthand and rejects embedded passwords before any connection.
///
/// # Errors
/// Returns an error for empty, unsupported or credential-bearing remote URLs.
pub fn normalize_url(input: &str) -> Result<String, VcsError> {
    let input = input.trim();
    if input.is_empty() || input.starts_with('-') || input.chars().any(char::is_control) {
        return Err(VcsError::Invalid("Enter a repository URL".into()));
    }
    if !input.contains(':') && !input.starts_with('/') {
        let parts: Vec<_> = input.split('/').collect();
        if parts.len() == 2
            && parts.iter().all(|s| {
                !s.is_empty()
                    && s.chars()
                        .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
            })
        {
            return Ok(format!("https://github.com/{input}"));
        }
    }
    if input.starts_with("https://") || input.starts_with("ssh://") || input.starts_with("file://")
    {
        if input.contains(['?', '#']) {
            return Err(VcsError::Invalid(
                "Remove query parameters and fragments; use the authentication fields".into(),
            ));
        }
        let authority = input
            .split_once("://")
            .map_or("", |(_, s)| s.split('/').next().unwrap_or(""));
        if let Some((userinfo, _)) = authority.rsplit_once('@')
            && (input.starts_with("https://") || userinfo.contains(':'))
        {
            return Err(VcsError::Invalid(
                "Use the authentication fields instead of credentials in the URL".into(),
            ));
        }
        return Ok(input.into());
    }
    if let Some((user, _)) = input.split_once('@')
        && !input.contains("://")
        && user.contains(':')
    {
        return Err(VcsError::Invalid(
            "Use the authentication fields instead of credentials in the URL".into(),
        ));
    }
    if input.contains('@') && input.contains(':') && !input.contains("://")
        || Path::new(input).is_absolute()
    {
        return Ok(input.into());
    }
    Err(VcsError::Invalid(
        "Use HTTPS, SSH, a local path, or owner/repository".into(),
    ))
}

fn credentials(
    auth: &Authentication,
    url: &str,
    user: Option<&str>,
    allowed: git2::CredentialType,
) -> Result<git2::Cred, git2::Error> {
    if allowed.contains(git2::CredentialType::USERNAME) {
        let name = match auth {
            Authentication::Password { username, .. } | Authentication::SshKey { username, .. } => {
                username.as_str()
            }
            _ => user.unwrap_or("git"),
        };
        return git2::Cred::username(name);
    }
    match auth {
        Authentication::Token(token)
            if allowed.contains(git2::CredentialType::USER_PASS_PLAINTEXT) =>
        {
            git2::Cred::userpass_plaintext("x-access-token", token)
        }
        Authentication::Password { username, password }
            if allowed.contains(git2::CredentialType::USER_PASS_PLAINTEXT) =>
        {
            git2::Cred::userpass_plaintext(username, password)
        }
        Authentication::SshKey {
            username,
            path,
            passphrase,
        } if allowed.contains(git2::CredentialType::SSH_KEY) => git2::Cred::ssh_key(
            username,
            None,
            path,
            (!passphrase.is_empty()).then_some(passphrase.as_str()),
        ),
        Authentication::Automatic if allowed.contains(git2::CredentialType::SSH_KEY) => {
            git2::Cred::ssh_key_from_agent(user.unwrap_or("git"))
        }
        Authentication::Automatic
            if allowed.contains(git2::CredentialType::USER_PASS_PLAINTEXT) =>
        {
            git2::Cred::credential_helper(&git2::Config::open_default()?, url, user)
        }
        _ => Err(git2::Error::from_str(
            "Selected authentication does not match this repository protocol",
        )),
    }
}

/// Clones into a temporary sibling and publishes only a complete checkout.
///
/// Existing destinations are refused. Cancellation cleans up only owned temporary files.
/// Default libgit2 certificate and SSH host-key verification remain enabled.
///
/// # Errors
/// Returns URL, destination, authentication, transport or cancellation errors.
pub fn repository(
    input: &str,
    destination: &Path,
    auth: &Authentication,
    cancel: &Arc<AtomicBool>,
) -> Result<PathBuf, VcsError> {
    let url = normalize_url(input)?;
    if !destination.is_absolute() || destination.file_name().is_none() || destination.exists() {
        return Err(VcsError::Invalid(
            "Choose a new, absolute destination directory".into(),
        ));
    }
    let parent = destination
        .parent()
        .ok_or_else(|| VcsError::Invalid("Destination has no parent".into()))?;
    let temporary = tempfile::Builder::new()
        .prefix(".nozhik-clone-")
        .tempdir_in(parent)?;
    let checkout = temporary.path().join("repository");
    let mut callbacks = git2::RemoteCallbacks::new();
    let mut attempts = 0;
    callbacks.credentials(|remote, user, allowed| {
        if remote != url {
            return Err(git2::Error::from_str(
                "Refusing credentials for a redirected repository URL",
            ));
        }
        attempts += 1;
        if cancel.load(Ordering::Relaxed) || attempts > 4 {
            return Err(git2::Error::from_str(
                "Authentication failed or cancelled; check your credentials",
            ));
        }
        credentials(auth, remote, user, allowed)
    });
    callbacks.transfer_progress(|_| !cancel.load(Ordering::Relaxed));
    let mut fetch = git2::FetchOptions::new();
    fetch.remote_callbacks(callbacks);
    let result = git2::build::RepoBuilder::new()
        .fetch_options(fetch)
        .clone(&url, &checkout);
    // Do not forward transport errors: libgit2 may include credentials supplied by a server.
    if result.is_err() {
        return Err(VcsError::Invalid(if cancel.load(Ordering::Relaxed) { "Clone cancelled" } else { "Clone failed. Check URL, credentials, network and host trust. GitHub requires a token or SSH key, not an account password." }.into()));
    }
    drop(result);
    if cancel.load(Ordering::Relaxed) {
        return Err(VcsError::Invalid("Clone cancelled".into()));
    }
    // Reserve the destination exclusively. Never replace an existing user directory.
    std::fs::create_dir(destination)?;
    // Move contents into the directory we own, retaining the original remote URL.
    for entry in std::fs::read_dir(&checkout)? {
        let entry = entry?;
        std::fs::rename(entry.path(), destination.join(entry.file_name()))?;
    }
    Ok(destination.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clone_checkout_origin_destination_and_cancel() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let source = root.path().join("source");
        let repo = git2::Repository::init(&source)?;
        std::fs::write(source.join("hello.txt"), "hello")?;
        let mut index = repo.index()?;
        index.add_path(Path::new("hello.txt"))?;
        let tree_id = index.write_tree()?;
        let tree = repo.find_tree(tree_id)?;
        let signature = git2::Signature::now("Test", "test@example.com")?;
        repo.commit(Some("HEAD"), &signature, &signature, "initial", &tree, &[])?;
        let target = root.path().join("cloned");
        let cancel = Arc::new(AtomicBool::new(false));
        repository(
            &source.to_string_lossy(),
            &target,
            &Authentication::Automatic,
            &cancel,
        )?;
        assert_eq!(std::fs::read_to_string(target.join("hello.txt"))?, "hello");
        assert_eq!(
            git2::Repository::open(&target)?
                .find_remote("origin")?
                .url()?,
            source.to_str().ok_or("Source path is not UTF-8")?
        );
        assert!(
            repository(
                &source.to_string_lossy(),
                &target,
                &Authentication::Automatic,
                &cancel
            )
            .is_err()
        );
        assert_eq!(std::fs::read_to_string(target.join("hello.txt"))?, "hello");
        cancel.store(true, Ordering::Relaxed);
        let cancelled = root.path().join("cancelled");
        assert!(
            repository(
                &source.to_string_lossy(),
                &cancelled,
                &Authentication::Automatic,
                &cancel
            )
            .is_err()
        );
        assert!(!cancelled.exists());
        assert_eq!(
            normalize_url("desfpc/Nozhik")?,
            "https://github.com/desfpc/Nozhik"
        );
        assert!(normalize_url("https://user:secret@github.com/a/b").is_err());
        assert!(normalize_url("http://example.com/a/b").is_err());
        assert!(normalize_url("https://github.com/a/b?token=secret").is_err());
        assert!(normalize_url("user:secret@host:repo").is_err());
        assert!(
            credentials(
                &Authentication::Password {
                    username: "test".into(),
                    password: Zeroizing::new("secret".into())
                },
                "https://example.com",
                None,
                git2::CredentialType::USER_PASS_PLAINTEXT
            )?
            .has_username()
        );
        Ok(())
    }
}
