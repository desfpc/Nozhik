//! Repository-local connection preferences; secrets remain in the owning Git session.
use crate::{VcsError, clone::Authentication};
use std::{io::Write, path::PathBuf};

/// Supported authentication methods, shared by cloning and existing repositories.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AuthMethod {
    /// Public access, repository/system credential helper or SSH agent.
    #[default]
    Automatic,
    /// HTTPS personal access token.
    Token,
    /// HTTPS account name and password/token.
    Password,
    /// Explicit SSH private key.
    SshKey,
}
impl AuthMethod {
    const fn key(self) -> &'static str {
        match self {
            Self::Automatic => "automatic",
            Self::Token => "token",
            Self::Password => "password",
            Self::SshKey => "ssh",
        }
    }
}
/// Non-secret authentication preferences stored in the repository's local Git config.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuthProfile {
    /// Authentication mechanism.
    pub method: AuthMethod,
    /// HTTPS or SSH account name.
    pub username: String,
    /// SSH private-key path, never the key contents.
    pub key_path: PathBuf,
}
impl Authentication {
    /// Non-secret preferences corresponding to these session credentials.
    #[must_use]
    pub fn profile(&self) -> AuthProfile {
        match self {
            Self::Automatic => AuthProfile::default(),
            Self::Token(_) => AuthProfile {
                method: AuthMethod::Token,
                ..AuthProfile::default()
            },
            Self::Password { username, .. } => AuthProfile {
                method: AuthMethod::Password,
                username: username.clone(),
                ..AuthProfile::default()
            },
            Self::SshKey { username, path, .. } => AuthProfile {
                method: AuthMethod::SshKey,
                username: username.clone(),
                key_path: path.clone(),
            },
        }
    }
    pub(crate) fn has_secret(&self) -> bool {
        match self {
            Self::Automatic => false,
            Self::Token(secret) => !secret.is_empty(),
            Self::Password { password, .. } => !password.is_empty(),
            Self::SshKey { passphrase, .. } => !passphrase.is_empty(),
        }
    }
}
/// Editable repository settings. No tokens, passwords or passphrases are included.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitSettings {
    /// Fetch URL of origin; empty for a local-only repository.
    pub remote_url: String,
    /// Optional distinct origin push URL; empty uses the fetch URL.
    pub push_url: String,
    /// Effective author name; empty removes the repository override.
    pub author_name: String,
    /// Effective author email; empty removes the repository override.
    pub author_email: String,
    /// Effective credential helper; empty uses the system configuration.
    pub credential_helper: String,
    /// Non-secret authentication mechanism.
    pub auth: AuthProfile,
}
fn value(config: &git2::Config, key: &str) -> Result<String, VcsError> {
    match config.get_string(key) {
        Ok(value) => Ok(value),
        Err(error) if error.code() == git2::ErrorCode::NotFound => Ok(String::new()),
        Err(error) => Err(error.into()),
    }
}
pub(crate) fn read(repo: &git2::Repository) -> Result<GitSettings, VcsError> {
    let config = repo.config()?;
    let method = match value(&config, "nozhik.auth.method")?.as_str() {
        "" | "automatic" => AuthMethod::Automatic,
        "token" => AuthMethod::Token,
        "password" => AuthMethod::Password,
        "ssh" => AuthMethod::SshKey,
        _ => {
            return Err(crate::git_worker::invalid(
                "Unknown Git authentication method in repository config",
            ));
        }
    };
    Ok(GitSettings {
        remote_url: value(&config, "remote.origin.url")?,
        push_url: value(&config, "remote.origin.pushurl")?,
        author_name: value(&config, "user.name")?,
        author_email: value(&config, "user.email")?,
        credential_helper: value(&config, "credential.helper")?,
        auth: AuthProfile {
            method,
            username: value(&config, "nozhik.auth.username")?,
            key_path: value(&config, "nozhik.auth.keypath")?.into(),
        },
    })
}
fn set(config: &mut git2::Config, key: &str, value: &str) -> Result<(), VcsError> {
    if value.is_empty() {
        match config.remove(key) {
            Ok(()) => {}
            Err(error) if error.code() == git2::ErrorCode::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    } else {
        config.set_str(key, value)?;
    }
    Ok(())
}
/// Validates and normalizes the URLs and non-secret fields without changing a repository.
/// # Errors
/// Returns invalid URLs, control characters, incompatible authentication or SSH key paths.
pub fn validate(settings: &mut GitSettings) -> Result<(), VcsError> {
    for url in [&mut settings.remote_url, &mut settings.push_url] {
        if url.chars().any(char::is_control) {
            return Err(crate::git_worker::invalid(
                "Remote URLs cannot contain control characters",
            ));
        }
        if url.trim().is_empty() {
            url.clear();
        } else {
            *url = crate::clone::normalize_url(url)?;
        }
    }
    if settings.remote_url.is_empty() && !settings.push_url.is_empty() {
        return Err(crate::git_worker::invalid(
            "A push URL requires an origin fetch URL",
        ));
    }
    if [
        &settings.author_name,
        &settings.author_email,
        &settings.credential_helper,
        &settings.auth.username,
    ]
    .iter()
    .any(|text| text.chars().any(char::is_control))
    {
        return Err(crate::git_worker::invalid(
            "Git settings cannot contain control characters",
        ));
    }
    if settings.auth.method == AuthMethod::Password && settings.auth.username.trim().is_empty() {
        return Err(crate::git_worker::invalid(
            "Enter the Git login for password authentication",
        ));
    }
    if !settings.author_name.trim().is_empty() && !settings.author_email.trim().is_empty() {
        git2::Signature::now(settings.author_name.trim(), settings.author_email.trim())
            .map_err(|_| crate::git_worker::invalid("Enter a valid Git author name and email"))?;
    }
    for url in [&settings.remote_url, &settings.push_url]
        .into_iter()
        .filter(|url| !url.is_empty())
    {
        let compatible = match settings.auth.method {
            AuthMethod::Automatic => true,
            AuthMethod::Token | AuthMethod::Password => url.starts_with("https://"),
            AuthMethod::SshKey => {
                url.starts_with("ssh://")
                    || !url.contains("://") && url.contains('@') && url.contains(':')
            }
        };
        if !compatible {
            return Err(crate::git_worker::invalid(
                "Authentication method does not match the remote URL protocol",
            ));
        }
    }
    if settings.auth.method == AuthMethod::SshKey
        && (!settings.auth.key_path.is_absolute() || !settings.auth.key_path.is_file())
    {
        return Err(crate::git_worker::invalid(
            "Choose an existing absolute SSH private-key file",
        ));
    }
    Ok(())
}
pub(crate) fn save(
    repo: &git2::Repository,
    expected: &GitSettings,
    mut settings: GitSettings,
) -> Result<GitSettings, VcsError> {
    validate(&mut settings)?;
    let destination = repo.commondir().join("config");
    let metadata = std::fs::symlink_metadata(&destination)?;
    if !metadata.file_type().is_file() {
        return Err(crate::git_worker::invalid(
            "Repository Git config must be a regular file",
        ));
    }
    // Use Git's conventional lock name and atomically replace the config on every platform.
    let mut lock = tempfile::Builder::new()
        .prefix("config.lock")
        .rand_bytes(0)
        .tempfile_in(repo.commondir())?;
    if &read(repo)? != expected {
        return Err(crate::git_worker::invalid(
            "Git settings changed externally. Reload before saving.",
        ));
    }
    lock.write_all(&std::fs::read(&destination)?)?;
    lock.flush()?;
    {
        let mut config = git2::Config::open(lock.path())?;
        set(&mut config, "remote.origin.url", &settings.remote_url)?;
        set(&mut config, "remote.origin.pushurl", &settings.push_url)?;
        if expected.remote_url.is_empty() && !settings.remote_url.is_empty() {
            config.set_str("remote.origin.fetch", "+refs/heads/*:refs/remotes/origin/*")?;
        }
        if settings.remote_url.is_empty() {
            set(&mut config, "remote.origin.fetch", "")?;
        }
        if settings.author_name != expected.author_name {
            set(&mut config, "user.name", settings.author_name.trim())?;
        }
        if settings.author_email != expected.author_email {
            set(&mut config, "user.email", settings.author_email.trim())?;
        }
        if settings.credential_helper != expected.credential_helper {
            set(
                &mut config,
                "credential.helper",
                &settings.credential_helper,
            )?;
        }
        config.set_str("nozhik.auth.method", settings.auth.method.key())?;
        set(&mut config, "nozhik.auth.username", &settings.auth.username)?;
        set(
            &mut config,
            "nozhik.auth.keypath",
            settings
                .auth
                .key_path
                .to_str()
                .ok_or_else(|| crate::git_worker::invalid("SSH key path must be UTF-8"))?,
        )?;
    }
    std::fs::set_permissions(lock.path(), metadata.permissions())?;
    std::fs::File::open(lock.path())?.sync_all()?;
    lock.persist(destination).map_err(|error| error.error)?;
    read(repo)
}
/// Records clone authentication preferences without storing any credentials.
/// # Errors
/// Returns invalid preferences or a repository config write error.
pub fn remember_clone(repo: &git2::Repository, auth: &Authentication) -> Result<(), VcsError> {
    let expected = read(repo)?;
    let mut settings = expected.clone();
    settings.auth = auth.profile();
    save(repo, &expected, settings)?;
    Ok(())
}

/// Session credentials are bound to the exact saved endpoints and preferences.
#[derive(Default)]
pub(crate) struct Session {
    auth: Option<(GitSettings, Authentication)>,
}
impl Session {
    pub fn replace(&mut self, settings: GitSettings, auth: Authentication) {
        let keep = self.auth.as_ref().is_some_and(|(old, _)| {
            old.remote_url == settings.remote_url
                && old.push_url == settings.push_url
                && old.auth == settings.auth
        });
        if !auth.has_secret() && keep {
            return;
        }
        self.auth = Some((settings, auth));
    }
    pub fn authentication(&self, settings: &GitSettings) -> Authentication {
        if let Some((old, auth)) = &self.auth
            && old.remote_url == settings.remote_url
            && old.push_url == settings.push_url
            && old.auth == settings.auth
        {
            return auth.clone();
        }
        match settings.auth.method {
            AuthMethod::Automatic => Authentication::Automatic,
            AuthMethod::Token => Authentication::Token(zeroize::Zeroizing::default()),
            AuthMethod::Password => Authentication::Password {
                username: settings.auth.username.clone(),
                password: zeroize::Zeroizing::default(),
            },
            AuthMethod::SshKey => Authentication::SshKey {
                username: settings.auth.username.clone(),
                path: settings.auth.key_path.clone(),
                passphrase: zeroize::Zeroizing::default(),
            },
        }
    }
}
