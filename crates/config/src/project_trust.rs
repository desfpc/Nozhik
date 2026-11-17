//! Host-owned project authorization; all filesystem work stays on workers.
use nozhik_core::ExecutionGuard;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    time::Duration,
};

/// Visible trust state, containing no project-controlled authorization booleans.
#[derive(Clone, Debug, Default)]
pub struct Status {
    /// Canonical project and execution inputs have been inspected.
    pub ready: bool,
    /// Human-readable reason for restricted mode or a failed persistence operation.
    pub detail: String,
    /// Fingerprint displayed when the user chose to trust; approval checks it again.
    pub fingerprint: Option<String>,
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Record {
    schema: u8,
    root: PathBuf,
    identity: String,
    fingerprint: String,
}
struct Shared {
    allowed: AtomicBool,
    epoch: AtomicU64,
    authorized_epoch: AtomicU64,
    approved: Mutex<Option<Record>>,
    status: Mutex<Status>,
}
enum Request {
    Approve(String, u64),
    Revoke,
    Stop,
}
/// Project-scoped authorization and its background monitor. Never stored in a repository.
pub struct ProjectTrust {
    shared: Arc<Shared>,
    input: mpsc::Sender<Request>,
    guard: ExecutionGuard,
    issued: Mutex<Option<(u64, ExecutionGuard)>>,
}
impl ProjectTrust {
    /// Starts a restricted session using the OS application configuration directory.
    /// # Errors
    /// Fails if no host-owned configuration directory exists or a worker cannot start.
    pub fn start(root: PathBuf, wake: impl Fn() + Send + Sync + 'static) -> io::Result<Self> {
        let dirs = directories::ProjectDirs::from("dev", "Nozhik", "Nozhik")
            .ok_or_else(|| io::Error::other("Host trust directory unavailable"))?;
        let extras = directories::BaseDirs::new()
            .map(|d| d.home_dir().join(".nozhik/plugins"))
            .into_iter()
            .collect();
        Self::with_sources(
            root,
            dirs.config_dir().join("project-trust"),
            extras,
            true,
            wake,
        )
    }
    /// Starts with injected host storage and extra plugin roots, for embedders/tests.
    /// Storage must resolve outside the project. Extra execution inputs must be supplied
    /// explicitly; unlike `start`, this does not inspect the host PATH or global plugins.
    /// No real user settings are needed in tests.
    /// # Errors
    /// Returns worker construction failures; inspection errors keep execution restricted.
    pub fn with_storage(
        root: PathBuf,
        storage: PathBuf,
        extras: Vec<PathBuf>,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> io::Result<Self> {
        Self::with_sources(root, storage, extras, false, wake)
    }
    fn with_sources(
        root: PathBuf,
        storage: PathBuf,
        extras: Vec<PathBuf>,
        include_environment: bool,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> io::Result<Self> {
        let shared = Arc::new(Shared {
            allowed: AtomicBool::new(false),
            epoch: AtomicU64::new(0),
            authorized_epoch: AtomicU64::new(u64::MAX),
            approved: Mutex::new(None),
            status: Mutex::new(Status::default()),
        });
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(wake);
        let check_shared = shared.clone();
        let check_root = root.clone();
        let check_storage = storage.clone();
        let check_extras = extras.clone();
        let check_wake = wake.clone();
        let current_shared = shared.clone();
        let guard = ExecutionGuard::with_status(
            move || {
                if !permitted(&check_shared) {
                    return false;
                }
                let expected = check_shared.approved.lock().ok().and_then(|r| r.clone());
                let valid = expected.as_ref().is_some_and(|record| {
                    inspect(&check_root, &check_extras, include_environment)
                        .is_ok_and(|current| &current == record)
                });
                if !valid {
                    check_shared.allowed.store(false, Ordering::Release);
                    if let Some(record) = &expected
                        && let Ok(file) = store_path(&check_storage, &record.root)
                    {
                        // Persist invalidation before returning a denied launch. A later
                        // restart must not resurrect the grant if old bytes are restored.
                        let _removed = fs::remove_file(file);
                    }
                    check_shared.epoch.fetch_add(1, Ordering::AcqRel);
                    if let Ok(mut saved) = check_shared.approved.lock() {
                        *saved = None;
                    }
                    check_wake();
                }
                valid && permitted(&check_shared)
            },
            move || permitted(&current_shared),
        );
        let (input, requests) = mpsc::channel();
        let state = shared.clone();
        std::thread::Builder::new()
            .name("nozhik-project-trust".into())
            .spawn(move || {
                monitor(
                    &root,
                    &storage,
                    &extras,
                    include_environment,
                    &state,
                    &requests,
                    &wake,
                );
            })?;
        Ok(Self {
            shared,
            input,
            guard,
            issued: Mutex::new(None),
        })
    }
    /// Cheap memory-only state for UI controls. Actual launches must also check the guard.
    #[must_use]
    pub fn allowed(&self) -> bool {
        permitted(&self.shared)
    }
    /// Memory-only display snapshot. A poisoned state always fails closed.
    #[must_use]
    pub fn status(&self) -> Status {
        self.shared
            .status
            .lock()
            .map_or_else(|_| Status::default(), |s| s.clone())
    }
    /// Guard for process workers. It remains revocable even after it has been cloned.
    #[must_use]
    pub fn guard(&self) -> ExecutionGuard {
        let epoch = self.shared.epoch.load(Ordering::Acquire);
        let Ok(mut issued) = self.issued.lock() else {
            return ExecutionGuard::restricted();
        };
        if let Some((generation, guard)) = issued.as_ref()
            && *generation == epoch
        {
            return guard.clone();
        }
        let verify = self.guard.clone();
        let current = self.guard.clone();
        let verify_shared = self.shared.clone();
        let current_shared = self.shared.clone();
        let guard = ExecutionGuard::with_status(
            move || {
                verify_shared.epoch.load(Ordering::Acquire) == epoch
                    && verify.check()
                    && verify_shared.epoch.load(Ordering::Acquire) == epoch
            },
            move || current_shared.epoch.load(Ordering::Acquire) == epoch && current.current(),
        );
        *issued = Some((epoch, guard.clone()));
        guard
    }
    /// Approves exactly the inspected snapshot, only after explicit user confirmation.
    pub fn approve(&self, fingerprint: String) {
        let _sent = self.input.send(Request::Approve(
            fingerprint,
            self.shared.epoch.load(Ordering::Acquire),
        ));
    }
    /// Immediately denies new launches, then removes the persisted grant on the worker.
    pub fn revoke(&self) {
        self.shared.epoch.fetch_add(1, Ordering::AcqRel);
        self.shared.allowed.store(false, Ordering::Release);
        let _sent = self.input.send(Request::Revoke);
    }
}
impl Drop for ProjectTrust {
    fn drop(&mut self) {
        self.shared.epoch.fetch_add(1, Ordering::AcqRel);
        self.shared.allowed.store(false, Ordering::Release);
        let _sent = self.input.send(Request::Stop);
    }
}

fn permitted(shared: &Shared) -> bool {
    shared.allowed.load(Ordering::Acquire)
        && shared.epoch.load(Ordering::Acquire) == shared.authorized_epoch.load(Ordering::Acquire)
}

#[allow(
    clippy::too_many_lines,
    clippy::too_many_arguments,
    reason = "One worker serializes trust inspection, consent, revocation and atomic persistence"
)]
fn monitor(
    root: &Path,
    storage: &Path,
    extras: &[PathBuf],
    include_environment: bool,
    shared: &Shared,
    requests: &mpsc::Receiver<Request>,
    wake: &Arc<dyn Fn() + Send + Sync>,
) {
    let mut cache = FileCache::default();
    let mut first = true;
    let mut record_file = None;
    let mut request = None;
    loop {
        if matches!(request, Some(Request::Stop)) {
            break;
        }
        let result = (|| -> io::Result<(Record, PathBuf)> {
            // Approval must hash the bytes again, even if mtime was preserved.
            if matches!(request, Some(Request::Approve(..))) {
                cache.clear();
            }
            let current = inspect_cached(root, extras, &mut cache, include_environment)?;
            let file = store_path(storage, &current.root)?;
            record_file = Some(file.clone());
            if matches!(request, Some(Request::Revoke)) {
                match fs::remove_file(&file) {
                    Ok(()) => {}
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e),
                }
                *shared
                    .approved
                    .lock()
                    .map_err(|_| io::Error::other("Trust state unavailable"))? = None;
            }
            if first && file.exists() && shared.epoch.load(Ordering::Acquire) == 0 {
                let saved: Record =
                    serde_json::from_slice(&fs::read(&file)?).map_err(io::Error::other)?;
                if saved == current {
                    shared.authorized_epoch.store(0, Ordering::Release);
                    *shared
                        .approved
                        .lock()
                        .map_err(|_| io::Error::other("Trust state unavailable"))? = Some(saved);
                }
            }
            if let Some(Request::Approve(expected, epoch)) = &request {
                if shared.epoch.load(Ordering::Acquire) != *epoch {
                    return Err(io::Error::other("Trust request superseded by revocation"));
                }
                if expected != &current.fingerprint {
                    return Err(io::Error::other(
                        "Execution inputs changed; review and trust again",
                    ));
                }
                fs::create_dir_all(storage)?;
                let file = store_path(storage, &current.root)?;
                let mut staged = tempfile::NamedTempFile::new_in(storage)?;
                staged.write_all(&serde_json::to_vec(&current).map_err(io::Error::other)?)?;
                staged.as_file().sync_all()?;
                staged.persist(file).map_err(|e| e.error)?;
                shared.authorized_epoch.store(*epoch, Ordering::Release);
                *shared
                    .approved
                    .lock()
                    .map_err(|_| io::Error::other("Trust state unavailable"))? =
                    Some(current.clone());
            }
            Ok((current, file))
        })();
        first = false;
        let (allowed, status) = match result {
            Ok((current, file)) => {
                let allowed = shared
                    .approved
                    .lock()
                    .is_ok_and(|saved| saved.as_ref() == Some(&current));
                if !allowed {
                    if let Ok(mut saved) = shared.approved.lock()
                        && saved.take().is_some()
                    {
                        shared.epoch.fetch_add(1, Ordering::AcqRel);
                    }
                    // Invalidation is permanent, even if an old executable is later restored.
                    match fs::remove_file(file) {
                        Ok(()) => {}
                        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                        Err(e) => {
                            if let Ok(mut status) = shared.status.lock() {
                                status.detail = e.to_string();
                            }
                        }
                    }
                }
                (
                    allowed,
                    Status {
                        ready: true,
                        detail: if allowed {
                            "Trusted execution inputs"
                        } else {
                            "Restricted Mode: external code is disabled"
                        }
                        .into(),
                        fingerprint: Some(current.fingerprint),
                    },
                )
            }
            Err(error) => {
                if let Some(file) = &record_file {
                    let _removed = fs::remove_file(file);
                }
                if let Ok(mut saved) = shared.approved.lock()
                    && saved.take().is_some()
                {
                    shared.epoch.fetch_add(1, Ordering::AcqRel);
                }
                (
                    false,
                    Status {
                        ready: false,
                        detail: error.to_string(),
                        fingerprint: None,
                    },
                )
            }
        };
        let changed = shared.allowed.swap(allowed, Ordering::AcqRel) != allowed;
        if let Ok(mut previous) = shared.status.lock() {
            let different =
                previous.detail != status.detail || previous.fingerprint != status.fingerprint;
            *previous = status;
            if different || changed {
                wake();
            }
        }
        request = match requests.recv_timeout(Duration::from_secs(1)) {
            Ok(request) => Some(request),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
    }
    shared.allowed.store(false, Ordering::Release);
}

fn store_path(storage: &Path, root: &Path) -> io::Result<PathBuf> {
    let mut existing = storage;
    while !existing.exists() {
        existing = existing
            .parent()
            .ok_or_else(|| io::Error::other("Invalid trust directory"))?;
    }
    if existing.canonicalize()?.starts_with(root) {
        return Err(io::Error::other("Trust must be stored outside the project"));
    }
    let name = format!(
        "{:x}.json",
        Sha256::digest(serde_json::to_vec(root).map_err(io::Error::other)?)
    );
    let path = storage.join(name);
    if path
        .symlink_metadata()
        .is_ok_and(|m| m.file_type().is_symlink())
    {
        return Err(io::Error::other("Trust record cannot be a symlink"));
    }
    Ok(path)
}
#[derive(Default)]
struct FileCache {
    files: std::collections::BTreeMap<PathBuf, (String, Vec<u8>)>,
    seen: std::collections::BTreeSet<PathBuf>,
}
impl FileCache {
    fn clear(&mut self) {
        self.files.clear();
    }
}
fn inspect(root: &Path, extras: &[PathBuf], include_environment: bool) -> io::Result<Record> {
    inspect_cached(root, extras, &mut FileCache::default(), include_environment)
}
fn inspect_cached(
    root: &Path,
    extras: &[PathBuf],
    cache: &mut FileCache,
    include_environment: bool,
) -> io::Result<Record> {
    cache.seen.clear();
    if cache.files.len() > 8192 {
        cache.clear();
    }
    let root = root.canonicalize()?;
    let metadata = root.metadata()?;
    if !metadata.is_dir() {
        return Err(io::Error::other("Project is not a directory"));
    }
    #[cfg(unix)]
    let identity = {
        use std::os::unix::fs::MetadataExt;
        format!(
            "{}:{}:{:?}",
            metadata.dev(),
            metadata.ino(),
            metadata.created().ok()
        )
    };
    #[cfg(not(unix))]
    let identity = format!("{:?}", metadata.created()?);
    let mut digest = Sha256::new();
    digest.update(serde_json::to_vec(&root).map_err(io::Error::other)?);
    digest.update(identity.as_bytes());
    let mut remaining = 512 * 1024 * 1024_u64;
    let mut entries = 0;
    for name in [
        ".nozhik/settings.toml",
        ".nozhik/settings.json",
        ".nozhik/plugins",
        "composer.json",
        "package.json",
        "go.mod",
        ".env",
        ".envrc",
    ] {
        hash_path(
            &root.join(name),
            &mut digest,
            &mut remaining,
            &mut entries,
            cache,
        )?;
    }
    git_inputs(&root, &mut digest, &mut remaining, &mut entries, cache)?;
    for path in extras {
        hash_path(path, &mut digest, &mut remaining, &mut entries, cache)?;
    }
    for path in project_execution_files(&root)? {
        hash_path(&path, &mut digest, &mut remaining, &mut entries, cache)?;
    }
    if include_environment {
        // PATH discovery and shell changes are executable inputs too; relative PATH
        // entries are ignored, rather than resolved against an attacker-controlled cwd.
        for name in ["phpactor", "intelephense", "docker", "ollama"] {
            if let Some(path) = find_program(&root, name) {
                hash_target(&path, &mut digest, &mut remaining, cache)?;
            }
        }
        for path in [
            "/bin/zsh",
            "/bin/bash",
            "/usr/local/bin/docker",
            "/opt/homebrew/bin/docker",
            "/Applications/Docker.app/Contents/Resources/bin/docker",
            "/opt/homebrew/bin/ollama",
            "/usr/local/bin/ollama",
            "/Applications/Ollama.app/Contents/Resources/ollama",
        ] {
            let path = Path::new(path);
            if path.is_file() {
                hash_target(path, &mut digest, &mut remaining, cache)?;
            }
        }
        #[cfg(windows)]
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            let path = PathBuf::from(local).join("Programs/Ollama/ollama.exe");
            if path.is_file() {
                hash_target(&path, &mut digest, &mut remaining, cache)?;
            }
        }
        for key in ["SHELL", "COMSPEC"] {
            if let Some(program) = std::env::var_os(key) {
                let path = PathBuf::from(program);
                let path = if path.is_absolute() {
                    path
                } else {
                    root.join(path)
                };
                hash_target(&path, &mut digest, &mut remaining, cache)?;
            }
        }
    }
    Ok(Record {
        schema: 1,
        root,
        identity,
        fingerprint: format!("{:x}", digest.finalize()),
    })
}
fn git_inputs(
    root: &Path,
    digest: &mut Sha256,
    remaining: &mut u64,
    entries: &mut usize,
    cache: &mut FileCache,
) -> io::Result<()> {
    let dot_git = root.join(".git");
    let git = if dot_git.is_file() {
        hash_path(&dot_git, digest, remaining, entries, cache)?;
        let text = fs::read_to_string(&dot_git)?;
        let path = text
            .trim()
            .strip_prefix("gitdir:")
            .ok_or_else(|| io::Error::other("Invalid Git worktree pointer"))?
            .trim();
        root.join(path).canonicalize()?
    } else {
        dot_git
    };
    let mut roots = vec![git.clone()];
    let common = git.join("commondir");
    hash_path(&common, digest, remaining, entries, cache)?;
    if common.is_file() {
        roots.push(
            git.join(fs::read_to_string(&common)?.trim())
                .canonicalize()?,
        );
    }
    for directory in roots {
        for name in ["config", "config.worktree"] {
            let path = directory.join(name);
            hash_path(&path, digest, remaining, entries, cache)?;
            if path.is_file() {
                let text = fs::read_to_string(&path)?;
                // Repository-local conditional includes cannot be safely attested by
                // a pathname-only approval. Keep them restricted instead of ignoring them.
                if text.lines().any(|line| {
                    line.trim_start()
                        .to_ascii_lowercase()
                        .starts_with("[include")
                }) {
                    return Err(io::Error::other(
                        "Repository Git config includes require review outside project execution trust",
                    ));
                }
            }
        }
        hash_path(&directory.join("hooks"), digest, remaining, entries, cache)?;
    }
    Ok(())
}

fn project_execution_files(root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut pending = vec![root.to_path_buf()];
    let mut found = Vec::new();
    let mut visited = 0;
    while let Some(directory) = pending.pop() {
        if directory.components().count() > 128 {
            return Err(io::Error::other("Project path is too deep to inspect"));
        }
        for entry in fs::read_dir(directory)? {
            visited += 1;
            if visited > 100_000 {
                return Err(io::Error::other(
                    "Project execution discovery exceeds 100000 entries",
                ));
            }
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let kind = entry.file_type()?;
            if kind.is_dir() {
                if !matches!(
                    name.as_ref(),
                    ".git" | ".nozhik" | "target" | "node_modules" | "vendor" | "binaries"
                ) {
                    pending.push(entry.path());
                }
            } else {
                let lower = name.to_ascii_lowercase();
                // Superset of supported Compose filenames, plus common execution inputs.
                if lower.starts_with("compose")
                    || lower.starts_with("docker-compose")
                    || lower.starts_with("dockerfile")
                    || matches!(
                        lower.as_str(),
                        ".env" | ".envrc" | "composer.json" | "package.json" | "go.mod"
                    )
                {
                    found.push(entry.path());
                }
            }
        }
    }
    found.sort();
    Ok(found)
}

fn find_program(_root: &Path, name: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    for directory in std::env::split_paths(&paths) {
        if !directory.is_absolute() {
            continue;
        }
        for suffix in if cfg!(windows) {
            &[".exe", ".cmd", ".bat"][..]
        } else {
            &[""][..]
        } {
            let candidate = directory.join(format!("{name}{suffix}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}
fn hash_target(
    path: &Path,
    digest: &mut Sha256,
    remaining: &mut u64,
    cache: &mut FileCache,
) -> io::Result<()> {
    let path = path.canonicalize()?;
    digest.update(serde_json::to_vec(&path).map_err(io::Error::other)?);
    hash_file(&path, digest, remaining, cache)
}
fn hash_file(
    path: &Path,
    digest: &mut Sha256,
    remaining: &mut u64,
    cache: &mut FileCache,
) -> io::Result<()> {
    let meta = path.metadata()?;
    let first = cache.seen.insert(path.to_path_buf());
    if !meta.is_file() || (first && meta.len() > *remaining) {
        return Err(io::Error::other(
            "Execution input exceeds inspection limit or is not a regular file",
        ));
    }
    if first {
        *remaining -= meta.len();
    }
    digest.update(meta.len().to_le_bytes());
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        digest.update(meta.mode().to_le_bytes());
    }
    let stamp = format!("{:?}:{}", meta.modified()?, meta.len());
    #[cfg(unix)]
    let stamp = {
        use std::os::unix::fs::MetadataExt;
        format!(
            "{stamp}:{}:{}:{}:{}:{}",
            meta.dev(),
            meta.ino(),
            meta.mode(),
            meta.ctime(),
            meta.ctime_nsec()
        )
    };
    if let Some((previous, bytes)) = cache.files.get(path)
        && previous == &stamp
    {
        digest.update(bytes);
        return Ok(());
    }
    let mut content = Sha256::new();
    let mut file = fs::File::open(path)?.take(meta.len() + 1);
    let mut chunk = [0; 16 * 1024];
    let mut length = 0_u64;
    loop {
        let n = file.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        length += n as u64;
        content.update(&chunk[..n]);
    }
    if length != meta.len() {
        return Err(io::Error::other(
            "Execution input changed during inspection",
        ));
    }
    let bytes = content.finalize().to_vec();
    digest.update(&bytes);
    cache.files.insert(path.to_path_buf(), (stamp, bytes));
    Ok(())
}
fn hash_path(
    path: &Path,
    digest: &mut Sha256,
    remaining: &mut u64,
    entries: &mut usize,
    cache: &mut FileCache,
) -> io::Result<()> {
    if path.components().count() > 128 {
        return Err(io::Error::other("Execution configuration path is too deep"));
    }
    *entries += 1;
    if *entries > 4096 {
        return Err(io::Error::other("Too many execution inputs"));
    }
    digest.update(serde_json::to_vec(path).map_err(io::Error::other)?);
    for ancestor in path.ancestors() {
        if ancestor
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink())
        {
            return Err(io::Error::other("Symlink in execution configuration path"));
        }
    }
    let meta = match path.symlink_metadata() {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            digest.update(b"missing");
            return Ok(());
        }
        Err(e) => return Err(e),
    };
    if meta.file_type().is_symlink() {
        return Err(io::Error::other(
            "Symlinks in project execution configuration are not trusted",
        ));
    }
    if meta.is_dir() {
        digest.update(b"directory");
        let mut paths = fs::read_dir(path)?
            .take(4097)
            .map(|e| e.map(|e| e.path()))
            .collect::<io::Result<Vec<_>>>()?;
        if paths.len() > 4096 {
            return Err(io::Error::other("Too many execution inputs"));
        }
        paths.sort();
        for child in paths {
            hash_path(&child, digest, remaining, entries, cache)?;
        }
    } else {
        digest.update(b"file");
        hash_file(path, digest, remaining, cache)?;
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if matches!(name, "plugin.json" | "settings.json" | "settings.toml") {
            if meta.len() > 1024 * 1024 {
                return Err(io::Error::other("Execution configuration exceeds 1 MiB"));
            }
            let bytes = fs::read(path)?;
            let value: serde_json::Value = if name == "settings.toml" {
                serde_json::to_value(
                    toml::from_str::<toml::Value>(
                        &String::from_utf8(bytes).map_err(io::Error::other)?,
                    )
                    .map_err(io::Error::other)?,
                )
                .map_err(io::Error::other)?
            } else {
                serde_json::from_slice(&bytes).map_err(io::Error::other)?
            };
            referenced_files(
                &value,
                path.parent()
                    .ok_or_else(|| io::Error::other("No configuration parent"))?,
                digest,
                remaining,
                cache,
            )?;
        }
    }
    Ok(())
}

fn referenced_files(
    value: &serde_json::Value,
    base: &Path,
    digest: &mut Sha256,
    remaining: &mut u64,
    cache: &mut FileCache,
) -> io::Result<()> {
    match value {
        serde_json::Value::Object(fields) => {
            for (key, value) in fields {
                if matches!(
                    key.as_str(),
                    "binary_path" | "executable" | "program" | "args"
                ) {
                    let strings: Vec<&str> = match value {
                        serde_json::Value::String(text) => std::iter::once(text.as_str())
                            .chain(text.split_whitespace())
                            .collect(),
                        serde_json::Value::Array(values) => values
                            .iter()
                            .filter_map(serde_json::Value::as_str)
                            .collect(),
                        _ => Vec::new(),
                    };
                    for text in strings.into_iter().filter(|text| text.len() <= 4096) {
                        let path = Path::new(text.trim_matches(['\'', '"']));
                        for parent in [
                            base,
                            base.parent().unwrap_or(base),
                            base.parent().and_then(Path::parent).unwrap_or(base),
                        ] {
                            let path = parent.join(path);
                            if path.is_file() {
                                hash_target(&path, digest, remaining, cache)?;
                            }
                        }
                    }
                }
                referenced_files(value, base, digest, remaining, cache)?;
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                referenced_files(value, base, digest, remaining, cache)?;
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    type TestResult = Result<(), Box<dyn std::error::Error>>;
    fn wait(session: &ProjectTrust, predicate: impl Fn(&ProjectTrust) -> bool) -> TestResult {
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        while !predicate(session) {
            if std::time::Instant::now() > deadline {
                return Err(format!("Trust timeout: {:?}", session.status()).into());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Ok(())
    }
    fn approve(session: &ProjectTrust) -> TestResult {
        wait(session, |s| s.status().ready)?;
        session.approve(session.status().fingerprint.ok_or("Missing fingerprint")?);
        wait(session, ProjectTrust::allowed)
    }
    #[test]
    fn persistent_trust_is_local_revocable_and_bound_to_execution_bytes() -> TestResult {
        let dir = tempfile::tempdir()?;
        let root = dir.path().join("project");
        let storage = dir.path().join("host-trust");
        fs::create_dir_all(root.join(".nozhik/plugins/example"))?;
        let input = root.join(".nozhik/plugins/example/plugin.json");
        fs::write(&input, r#"{"args":["one"]}"#)?;
        let session = ProjectTrust::with_storage(root.clone(), storage.clone(), vec![], || {})?;
        wait(&session, |s| s.status().ready)?;
        assert!(!session.allowed());
        assert!(!session.guard().check());
        approve(&session)?;
        assert!(session.guard().check());
        assert!(!root.join(".nozhik/project-trust").exists());
        let old_guard = session.guard();
        drop(session);
        assert!(!old_guard.current());
        let session = ProjectTrust::with_storage(root.clone(), storage.clone(), vec![], || {})?;
        wait(&session, ProjectTrust::allowed)?;
        // Same-length argument mutation must invalidate the byte-level launch check.
        fs::write(&input, r#"{"args":["two"]}"#)?;
        assert!(!session.guard().check());
        assert!(!session.allowed());
        wait(&session, |s| s.status().ready && !s.allowed())?;
        // Wait for the updated candidate, rather than approving the old display snapshot.
        let current = inspect(&root, &[], false)?.fingerprint;
        wait(&session, |s| {
            s.status().fingerprint.as_ref() == Some(&current)
        })?;
        approve(&session)?;
        let binary = root.join(".nozhik/plugins/example/tool");
        fs::write(&binary, b"version one")?;
        assert!(!session.guard().check());
        let current = inspect(&root, &[], false)?.fingerprint;
        wait(&session, |s| {
            s.status().fingerprint.as_ref() == Some(&current)
        })?;
        approve(&session)?;
        fs::write(&binary, b"version two")?;
        assert!(!session.guard().check());
        session.revoke();
        assert!(!session.guard().current());
        wait(&session, |_| {
            fs::read_dir(&storage).is_ok_and(|mut entries| entries.next().is_none())
        })?;
        drop(session);
        let reopened = ProjectTrust::with_storage(root, storage, vec![], || {})?;
        wait(&reopened, |s| s.status().ready)?;
        assert!(!reopened.allowed());
        Ok(())
    }
    #[test]
    fn project_cannot_supply_trust_and_grants_do_not_transfer() -> TestResult {
        let dir = tempfile::tempdir()?;
        let storage = dir.path().join("host");
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        fs::create_dir_all(a.join(".nozhik"))?;
        fs::create_dir_all(b.join(".nozhik"))?;
        fs::write(b.join(".nozhik/trust.json"), r#"{"trusted":true}"#)?;
        let first = ProjectTrust::with_storage(a.clone(), storage.clone(), vec![], || {})?;
        approve(&first)?;
        let second = ProjectTrust::with_storage(b, storage, vec![], || {})?;
        wait(&second, |s| s.status().ready)?;
        assert!(!second.allowed());
        let invalid = ProjectTrust::with_storage(a.clone(), a.join("host"), vec![], || {})?;
        wait(&invalid, |s| !s.status().detail.is_empty())?;
        assert!(!invalid.status().ready);
        assert!(!invalid.guard().check());
        // Replacing a folder at the same path does not inherit its identity.
        let old = dir.path().join("old");
        fs::rename(&a, &old)?;
        fs::create_dir_all(&a)?;
        assert!(!first.guard().check());
        Ok(())
    }
    #[cfg(unix)]
    #[test]
    fn canonical_aliases_match_but_symlinked_execution_inputs_fail_closed() -> TestResult {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir()?;
        let root = dir.path().join("root");
        let alias = dir.path().join("alias");
        fs::create_dir_all(&root)?;
        symlink(&root, &alias)?;
        let storage = dir.path().join("host");
        let first = ProjectTrust::with_storage(root.clone(), storage.clone(), vec![], || {})?;
        approve(&first)?;
        let same = ProjectTrust::with_storage(alias.clone(), storage, vec![], || {})?;
        wait(&same, ProjectTrust::allowed)?;
        let different = dir.path().join("different");
        fs::create_dir_all(&different)?;
        fs::remove_file(&alias)?;
        symlink(&different, &alias)?;
        assert!(!same.guard().check());
        assert!(first.guard().check());
        let external = dir.path().join("outside");
        fs::create_dir_all(&external)?;
        symlink(&external, root.join(".nozhik"))?;
        assert!(!first.guard().check());
        assert!(!same.guard().check());
        Ok(())
    }
    #[test]
    fn old_queued_guards_stay_revoked_after_a_new_grant() -> TestResult {
        let dir = tempfile::tempdir()?;
        let root = dir.path().join("root");
        fs::create_dir_all(&root)?;
        let session = ProjectTrust::with_storage(root, dir.path().join("host"), vec![], || {})?;
        approve(&session)?;
        let old = session.guard();
        assert!(old.same_authorization(&session.guard()));
        session.revoke();
        approve(&session)?;
        assert!(!old.current());
        assert!(!old.check());
        assert!(session.guard().check());
        Ok(())
    }
    #[test]
    fn stale_dialog_and_queued_approval_cannot_override_revocation() -> TestResult {
        let dir = tempfile::tempdir()?;
        let root = dir.path().join("root");
        fs::create_dir_all(root.join(".nozhik"))?;
        let session =
            ProjectTrust::with_storage(root.clone(), dir.path().join("host"), vec![], || {})?;
        wait(&session, |s| s.status().ready)?;
        let previous = session.status().fingerprint.ok_or("Missing candidate")?;
        fs::write(root.join(".nozhik/settings.toml"), "ai_enabled = false")?;
        session.approve(previous);
        wait(&session, |s| s.status().detail.contains("changed"))?;
        assert!(!session.allowed());
        wait(&session, |s| s.status().ready)?;
        session.approve(session.status().fingerprint.ok_or("Missing candidate")?);
        session.revoke();
        std::thread::sleep(Duration::from_millis(200));
        assert!(!session.guard().current());
        Ok(())
    }
}

#[cfg(test)]
mod execution_input_tests {
    use super::*;
    #[test]
    fn compose_nested_manifests_referenced_tools_and_git_worktrees_are_attested()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let root = dir.path().join("project");
        fs::create_dir_all(root.join(".nozhik"))?;
        fs::create_dir_all(root.join("services"))?;
        let tool = root.join("runner.sh");
        fs::write(&tool, "echo one")?;
        fs::write(
            root.join(".nozhik/settings.toml"),
            "[[tests]]\nbinary_path = 'runner.sh'\nargs = ''\n",
        )?;
        let before = inspect(&root, &[], false)?;
        fs::write(&tool, "echo two")?;
        assert!(inspect(&root, &[], false)? != before);
        let before = inspect(&root, &[], false)?;
        fs::write(root.join("services/compose.prod.yaml"), "services: {}")?;
        assert!(inspect(&root, &[], false)? != before);
        let git = dir.path().join("git");
        let worktree = git.join("worktrees/test");
        fs::create_dir_all(&worktree)?;
        fs::write(
            root.join(".git"),
            format!("gitdir: {}\n", worktree.display()),
        )?;
        fs::write(worktree.join("commondir"), "../..\n")?;
        fs::write(git.join("config"), "[core]\nrepositoryformatversion = 0\n")?;
        let before = inspect(&root, &[], false)?;
        fs::write(git.join("config"), "[credential]\nhelper = changed\n")?;
        assert!(inspect(&root, &[], false)? != before);
        fs::write(git.join("config"), "[include]\npath = injected\n")?;
        assert!(inspect(&root, &[], false).is_err());
        Ok(())
    }
}
