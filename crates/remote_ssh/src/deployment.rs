//! Bounded local deployment planning and streamed SFTP uploads, without remote deletion.
use crate::{Error, OPERATION_TIMEOUT, Result, sftp::SftpClient};
use russh_sftp::protocol::{OpenFlags, StatusCode};
use std::{
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// One immutable entry in a validated local traversal.
#[derive(Debug)]
struct Entry {
    source: PathBuf,
    relative: String,
    directory: bool,
    size: u64,
}
/// Local selection mapped relative to the project root. Build off the UI thread.
#[derive(Debug)]
pub struct DeploymentPlan {
    entries: Vec<Entry>,
    skipped: usize,
    files: usize,
    bytes: u64,
}
/// Progress counters for a single deployment.
#[derive(Debug, Clone, Default)]
pub struct DeploymentProgress {
    /// Completed file count.
    pub files_done: usize,
    /// Number of files planned.
    pub files_total: usize,
    /// Bytes sent so far.
    pub bytes_done: u64,
    /// Planned total bytes.
    pub bytes_total: u64,
    /// Excluded symlinks and internal entries.
    pub skipped: usize,
    /// Project-relative path currently being sent.
    pub current: String,
}
fn invalid(message: impl Into<String>) -> Error {
    Error::Invalid(message.into())
}
/// Validate and normalize the absolute server project root.
/// # Errors
/// Rejects relative paths, controls, backslashes and dot traversal components.
pub fn remote_root(path: &str) -> Result<String> {
    if !path.starts_with('/')
        || path.chars().any(char::is_control)
        || path.contains('\\')
        || path.split('/').any(|p| matches!(p, "." | ".."))
    {
        return Err(invalid(
            "Deployment root must be an absolute POSIX path without . or ..",
        ));
    }
    let parts: Vec<_> = path.split('/').filter(|p| !p.is_empty()).collect();
    Ok(format!("/{}", parts.join("/")))
}
fn relative(root: &Path, path: &Path) -> Result<String> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| invalid("Deployment source is outside this project"))?;
    let mut names = Vec::new();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err(invalid("Invalid deployment path component"));
        };
        let name = name
            .to_str()
            .ok_or_else(|| invalid("Deployment paths must be UTF-8"))?;
        if matches!(name, ".git" | ".nozhik")
            || name.contains('\\')
            || name.chars().any(char::is_control)
        {
            return Err(invalid(
                "Project internals and ambiguous paths cannot be deployed",
            ));
        }
        names.push(name);
    }
    Ok(names.join("/"))
}
fn target(root: &str, relative: &str) -> String {
    if relative.is_empty() {
        root.to_owned()
    } else {
        format!("{}/{}", root.trim_end_matches('/'), relative)
    }
}
impl DeploymentPlan {
    /// Recursively inspect regular files/directories, excluding symlinks and project internals.
    /// # Errors
    /// Returns I/O, cancellation, path escape or more-than-100000-entry errors before upload.
    pub fn build(root: &Path, selection: &Path, cancelled: &AtomicBool) -> Result<Self> {
        let root = root.canonicalize()?;
        let rel = relative(&root, selection)?;
        let mut prefix = root.clone();
        for part in rel.split('/').filter(|p| !p.is_empty()) {
            prefix.push(part);
            if std::fs::symlink_metadata(&prefix)?.file_type().is_symlink() {
                return Err(invalid("Deployment does not follow local symlinks"));
            }
        }
        if !selection.canonicalize()?.starts_with(&root) {
            return Err(invalid("Deployment source escapes the project"));
        }
        let mut plan = Self {
            entries: Vec::new(),
            skipped: 0,
            files: 0,
            bytes: 0,
        };
        let mut pending = vec![selection.to_path_buf()];
        while let Some(path) = pending.pop() {
            if cancelled.load(Ordering::Acquire) {
                return Err(invalid("SFTP deployment cancelled"));
            }
            if plan.entries.len() >= 100_000 {
                return Err(invalid(
                    "Deployment exceeds 100000 entries; select a smaller directory",
                ));
            }
            let metadata = std::fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink() {
                plan.skipped += 1;
                continue;
            }
            if !metadata.is_dir() && !metadata.is_file() {
                return Err(invalid(format!(
                    "Not a regular deployment file: {}",
                    path.display()
                )));
            }
            let name = relative(&root, &path)?;
            plan.entries.push(Entry {
                source: path.clone(),
                relative: name,
                directory: metadata.is_dir(),
                size: metadata.len(),
            });
            if metadata.is_dir() {
                let mut children = Vec::new();
                for entry in std::fs::read_dir(&path)? {
                    let entry = entry?;
                    if matches!(entry.file_name().to_str(), Some(".git" | ".nozhik")) {
                        plan.skipped += 1;
                    } else {
                        children.push(entry.path());
                    }
                    if children
                        .len()
                        .saturating_add(pending.len())
                        .saturating_add(plan.entries.len())
                        > 100_000
                    {
                        return Err(invalid("Deployment exceeds 100000 entries"));
                    }
                }
                children.sort();
                pending.extend(children.into_iter().rev());
            } else {
                plan.files += 1;
                plan.bytes = plan
                    .bytes
                    .checked_add(metadata.len())
                    .ok_or_else(|| invalid("Deployment size overflow"))?;
            }
        }
        Ok(plan)
    }
}
impl DeploymentPlan {
    /// Plans exactly the current regular files in a supplied list. Missing paths,
    /// directories, symlinks, internals and paths outside this project are skipped.
    /// This never recursively expands a directory listed by Git.
    /// # Errors
    /// Rejects unsafe paths, local I/O failures, cancellation and more than 100000 paths.
    pub fn files(root: &Path, paths: &[PathBuf], cancelled: &AtomicBool) -> Result<Self> {
        if paths.len() > 100_000 {
            return Err(invalid("Deployment exceeds 100000 paths"));
        }
        let root = root.canonicalize()?;
        let mut plan = Self {
            entries: Vec::new(),
            skipped: 0,
            files: 0,
            bytes: 0,
        };
        let unique: std::collections::BTreeSet<_> = paths.iter().collect();
        for path in unique {
            if cancelled.load(Ordering::Acquire) {
                return Err(invalid("SFTP deployment cancelled"));
            }
            let Ok(rel) = path.strip_prefix(&root) else {
                plan.skipped += 1;
                continue;
            };
            if rel
                .components()
                .any(|c| matches!(c, Component::Normal(n) if n==".git" || n==".nozhik"))
            {
                plan.skipped += 1;
                continue;
            }
            let name = relative(&root, path)?;
            let metadata = match std::fs::symlink_metadata(path) {
                Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                    metadata
                }
                Ok(_) => {
                    plan.skipped += 1;
                    continue;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    plan.skipped += 1;
                    continue;
                }
                Err(e) => return Err(e.into()),
            };
            let mut prefix = root.clone();
            for part in name.split('/') {
                prefix.push(part);
                if std::fs::symlink_metadata(&prefix)?.file_type().is_symlink() {
                    return Err(invalid("Deployment does not follow local symlinks"));
                }
            }
            if path.canonicalize()? != *path {
                return Err(invalid("Deployment source changed or escapes the project"));
            }
            plan.files += 1;
            plan.bytes = plan
                .bytes
                .checked_add(metadata.len())
                .ok_or_else(|| invalid("Deployment size overflow"))?;
            plan.entries.push(Entry {
                source: path.clone(),
                relative: name,
                directory: false,
                size: metadata.len(),
            });
        }
        if plan.files == 0 {
            return Err(invalid(
                "No current regular project files from the selected commit are available to deploy",
            ));
        }
        Ok(plan)
    }
}

impl SftpClient {
    async fn deployment_directory(&self, path: &str, allow_link: bool) -> Result<()> {
        match self.session.symlink_metadata(path).await {
            Ok(metadata) if metadata.is_dir() => Ok(()),
            Ok(metadata)
                if allow_link
                    && metadata.file_type().is_symlink()
                    && self.session.metadata(path).await?.is_dir() =>
            {
                Ok(())
            }
            Ok(_) => Err(invalid(format!(
                "Deployment directory is not a directory or is a symlink: {path}"
            ))),
            Err(russh_sftp::client::error::Error::Status(status))
                if status.status_code == StatusCode::NoSuchFile =>
            {
                self.session.create_dir(path).await?;
                Ok(())
            }
            Err(error) => Err(error.into()),
        }
    }
    async fn deployment_parents(&self, root: &str, relative: &str) -> Result<()> {
        let mut path = root.to_owned();
        for part in relative.split('/').filter(|p| !p.is_empty()) {
            path = target(&path, part);
            self.deployment_directory(&path, false).await?;
        }
        Ok(())
    }
    /// Send a validated project selection. Existing regular files are overwritten;
    /// failures/cancellation can leave partial writes. Unrelated remote files are retained.
    /// # Errors
    /// Returns local changes, remote symlink/type conflicts, permissions or SFTP deadlines.
    pub async fn deploy(
        &mut self,
        plan: &DeploymentPlan,
        root: &str,
        mut progress: impl FnMut(DeploymentProgress),
    ) -> Result<DeploymentProgress> {
        let root = remote_root(root)?;
        let mut prefix = String::new();
        for part in root.split('/').filter(|p| !p.is_empty()) {
            prefix.push('/');
            prefix.push_str(part);
            self.deployment_directory(&prefix, true).await?;
        }
        let root = self.session.canonicalize(root).await?;
        let root = remote_root(&root)?;
        let mut state = DeploymentProgress {
            files_total: plan.files,
            bytes_total: plan.bytes,
            skipped: plan.skipped,
            ..DeploymentProgress::default()
        };
        let mut buffer = vec![0; 64 * 1024];
        self.invalidate();
        for entry in &plan.entries {
            state.current.clone_from(&entry.relative);
            progress(state.clone());
            let path = target(&root, &entry.relative);
            if entry.directory {
                self.deployment_parents(&root, &entry.relative).await?;
                continue;
            }
            if let Some((parent, _)) = entry.relative.rsplit_once('/') {
                self.deployment_parents(&root, parent).await?;
            }
            match self.session.symlink_metadata(&path).await {
                Ok(metadata) if !metadata.file_type().is_file() => {
                    return Err(invalid(format!(
                        "Deployment target is not a regular file: {path}"
                    )));
                }
                Ok(_) => {}
                Err(russh_sftp::client::error::Error::Status(status))
                    if status.status_code == StatusCode::NoSuchFile => {}
                Err(error) => return Err(error.into()),
            }
            let metadata = tokio::fs::symlink_metadata(&entry.source).await?;
            if !metadata.is_file()
                || metadata.file_type().is_symlink()
                || metadata.len() != entry.size
            {
                return Err(invalid("Local deployment file changed; retry after saving"));
            }
            if tokio::fs::canonicalize(&entry.source).await? != entry.source {
                return Err(invalid("Local deployment path changed or became a symlink"));
            }
            let mut local = tokio::fs::File::open(&entry.source).await?;
            let mut remote = self
                .session
                .open_with_flags(
                    &path,
                    OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::TRUNCATE,
                )
                .await?;
            let mut sent = 0u64;
            loop {
                let count =
                    tokio::time::timeout(OPERATION_TIMEOUT, local.read(&mut buffer)).await??;
                if count == 0 {
                    break;
                }
                tokio::time::timeout(OPERATION_TIMEOUT, remote.write_all(&buffer[..count]))
                    .await??;
                sent += count as u64;
                state.bytes_done += count as u64;
                progress(state.clone());
            }
            tokio::time::timeout(OPERATION_TIMEOUT, remote.close()).await??;
            if sent != entry.size {
                return Err(invalid(
                    "Local deployment file changed during upload; remote file may be partial",
                ));
            }
            state.files_done += 1;
        }
        progress(state.clone());
        self.invalidate();
        Ok(state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use russh_sftp::{
        client::SftpSession,
        protocol::{Attrs, File, FileAttributes, Handle, Name, Status},
    };
    use std::{
        collections::{BTreeMap, HashMap},
        sync::{Arc, Mutex},
    };
    type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;
    type Files = Arc<Mutex<BTreeMap<String, (u32, Vec<u8>)>>>;
    fn attributes(mode: u32) -> FileAttributes {
        let mut a = FileAttributes::empty();
        a.permissions = Some(mode);
        a
    }
    fn status(id: u32) -> Status {
        Status {
            id,
            status_code: StatusCode::Ok,
            error_message: String::new(),
            language_tag: String::new(),
        }
    }
    struct Memory(Files);
    #[allow(
        clippy::unused_async_trait_impl,
        reason = "In-memory protocol fixtures use asynchronous handler signatures with immediate replies"
    )]
    impl russh_sftp::server::Handler for Memory {
        type Error = StatusCode;
        fn unimplemented(&self) -> StatusCode {
            StatusCode::OpUnsupported
        }
        async fn lstat(&mut self, id: u32, path: String) -> std::result::Result<Attrs, StatusCode> {
            let files = self.0.lock().map_err(|_| StatusCode::Failure)?;
            let mode = files.get(&path).ok_or(StatusCode::NoSuchFile)?.0;
            drop(files);
            Ok(Attrs {
                id,
                attrs: attributes(mode),
            })
        }
        async fn stat(&mut self, id: u32, path: String) -> std::result::Result<Attrs, StatusCode> {
            self.lstat(id, path).await
        }
        async fn mkdir(
            &mut self,
            id: u32,
            path: String,
            _attrs: FileAttributes,
        ) -> std::result::Result<Status, StatusCode> {
            let mut files = self.0.lock().map_err(|_| StatusCode::Failure)?;
            if files.contains_key(&path) {
                return Err(StatusCode::Failure);
            }
            files.insert(path, (0o040_755, Vec::new()));
            drop(files);
            Ok(status(id))
        }
        async fn realpath(
            &mut self,
            id: u32,
            path: String,
        ) -> std::result::Result<Name, StatusCode> {
            Ok(Name {
                id,
                files: vec![File {
                    filename: path,
                    longname: String::new(),
                    attrs: attributes(0o040_755),
                }],
            })
        }
        async fn open(
            &mut self,
            id: u32,
            path: String,
            flags: OpenFlags,
            _attrs: FileAttributes,
        ) -> std::result::Result<Handle, StatusCode> {
            if path.ends_with("denied.txt") {
                return Err(StatusCode::PermissionDenied);
            }
            let mut files = self.0.lock().map_err(|_| StatusCode::Failure)?;
            if flags.contains(OpenFlags::TRUNCATE) {
                files.insert(path.clone(), (0o100_644, Vec::new()));
            }
            drop(files);
            Ok(Handle { id, handle: path })
        }
        async fn write(
            &mut self,
            id: u32,
            handle: String,
            offset: u64,
            data: Vec<u8>,
        ) -> std::result::Result<Status, StatusCode> {
            let mut files = self.0.lock().map_err(|_| StatusCode::Failure)?;
            let (_, bytes) = files.get_mut(&handle).ok_or(StatusCode::NoSuchFile)?;
            if offset != bytes.len() as u64 {
                return Err(StatusCode::Failure);
            }
            bytes.extend(data);
            drop(files);
            Ok(status(id))
        }
        async fn close(
            &mut self,
            id: u32,
            _handle: String,
        ) -> std::result::Result<Status, StatusCode> {
            Ok(status(id))
        }
    }
    #[test]
    fn local_mapping_exclusions_outside_paths_and_cancellation() -> TestResult {
        let dir = tempfile::tempdir()?;
        let root = dir.path().canonicalize()?;
        std::fs::create_dir(root.join("src"))?;
        std::fs::write(root.join("src/Ж file.txt"), b"source")?;
        for name in [".git", ".nozhik"] {
            std::fs::create_dir(root.join(name))?;
            std::fs::write(root.join(name).join("private"), b"not deployed")?;
        }
        let cancel = AtomicBool::new(false);
        let plan = DeploymentPlan::build(&root, &root, &cancel)?;
        assert_eq!(plan.files, 1);
        assert_eq!(plan.skipped, 2);
        assert!(plan.entries.iter().any(|e| e.relative == "src/Ж file.txt"));
        assert!(DeploymentPlan::build(&root, &root.join(".git"), &cancel).is_err());
        let outside = tempfile::tempdir()?;
        assert!(DeploymentPlan::build(&root, outside.path(), &cancel).is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(outside.path(), root.join("link"))?;
            assert!(DeploymentPlan::build(&root, &root.join("link"), &cancel).is_err());
            assert_eq!(DeploymentPlan::build(&root, &root, &cancel)?.skipped, 3);
        }
        cancel.store(true, Ordering::Release);
        assert!(DeploymentPlan::build(&root, &root, &cancel).is_err());
        assert_eq!(
            target("/srv/app", "src/Ж file.txt"),
            "/srv/app/src/Ж file.txt"
        );
        assert_eq!(target("/", "a"), "/a");
        assert_eq!(remote_root("//srv//app/")?, "/srv/app");
        Ok(())
    }
    #[test]
    fn exact_file_list_skips_missing_directories_and_unrelated_paths() -> TestResult {
        let dir = tempfile::tempdir()?;
        let root = dir.path().canonicalize()?;
        std::fs::create_dir(root.join("replaced"))?;
        std::fs::write(root.join("replaced/child"), b"not selected")?;
        std::fs::write(root.join("current.txt"), b"current working content")?;
        std::fs::create_dir(root.join(".git"))?;
        std::fs::write(root.join(".git/private"), b"private")?;
        let paths = vec![
            root.join("current.txt"),
            root.join("current.txt"),
            root.join("deleted.txt"),
            root.join("replaced"),
            root.join(".git/private"),
            root.parent().ok_or("parent")?.join("outside"),
        ];
        let cancel = AtomicBool::new(false);
        let plan = DeploymentPlan::files(&root, &paths, &cancel)?;
        assert_eq!(plan.files, 1);
        assert_eq!(plan.skipped, 4);
        assert_eq!(plan.entries.len(), 1);
        assert_eq!(plan.bytes, 23);
        assert_eq!(plan.entries.first().ok_or("entry")?.relative, "current.txt");
        assert!(DeploymentPlan::files(&root, &[root.join("missing")], &cancel).is_err());
        cancel.store(true, Ordering::Release);
        assert!(DeploymentPlan::files(&root, &paths, &cancel).is_err());
        Ok(())
    }
    #[tokio::test]
    async fn protocol_deploy_streams_binary_creates_directories_overwrites_and_rejects_links()
    -> TestResult {
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            let dir = tempfile::tempdir()?;
            let root = dir.path().canonicalize()?;
            std::fs::create_dir_all(root.join("src/empty"))?;
            let bytes = vec![173; 1024 * 1024 + 31];
            std::fs::write(root.join("src/Ж.bin"), &bytes)?;
            let files = Arc::new(Mutex::new(BTreeMap::from([
                ("/".into(), (0o040_755, Vec::new())),
                ("/unrelated".into(), (0o100_644, b"preserve".to_vec())),
            ])));
            let (client, server) = tokio::io::duplex(8192);
            let task = tokio::spawn(russh_sftp::server::run(server, Memory(files.clone())));
            let mut sftp = SftpClient {
                session: SftpSession::new(client).await?,
                cache: HashMap::new(),
            };
            let plan = DeploymentPlan::build(&root, &root, &AtomicBool::new(false))?;
            let mut updates = 0;
            let result = sftp.deploy(&plan, "/srv/app", |_| updates += 1).await?;
            assert_eq!(result.files_done, 1);
            assert_eq!(result.bytes_done, bytes.len() as u64);
            assert!(updates > 4);
            {
                let remote = files.lock().map_err(|e| e.to_string())?;
                assert_eq!(
                    remote.get("/srv/app/src/Ж.bin").map(|v| v.1.as_slice()),
                    Some(bytes.as_slice())
                );
                assert!(remote.contains_key("/srv/app/src/empty"));
                assert_eq!(
                    remote.get("/unrelated").map(|v| v.1.as_slice()),
                    Some(b"preserve".as_slice())
                );
            }
            std::fs::write(root.join("src/Ж.bin"), b"shorter")?;
            let plan = DeploymentPlan::files(
                &root,
                &[root.join("src/Ж.bin"), root.join("deleted")],
                &AtomicBool::new(false),
            )?;
            sftp.deploy(&plan, "/srv/app", |_| {}).await?;
            assert_eq!(
                files
                    .lock()
                    .map_err(|e| e.to_string())?
                    .get("/srv/app/src/Ж.bin")
                    .map(|v| v.1.clone()),
                Some(b"shorter".to_vec())
            );
            files
                .lock()
                .map_err(|e| e.to_string())?
                .insert("/srv/app/src".into(), (0o120_777, b"/outside".to_vec()));
            assert!(sftp.deploy(&plan, "/srv/app", |_| {}).await.is_err());
            files
                .lock()
                .map_err(|e| e.to_string())?
                .insert("/srv/app/src".into(), (0o040_755, Vec::new()));
            std::fs::write(root.join("denied.txt"), b"denied")?;
            let plan =
                DeploymentPlan::build(&root, &root.join("denied.txt"), &AtomicBool::new(false))?;
            assert!(sftp.deploy(&plan, "/srv/app", |_| {}).await.is_err());
            sftp.session.close().await?;
            task.await?;
            Ok::<_, Box<dyn std::error::Error>>(())
        })
        .await??;
        Ok(())
    }
}
