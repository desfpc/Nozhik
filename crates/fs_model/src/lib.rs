//! Filesystem model and a serial background command worker. No UI dependencies.
mod content_search;
mod entries;
pub mod fuzzy;
pub mod tree;
pub mod watcher;

pub use content_search::TextHit;

use fuzzy::{FileIndex, FileMatch};
use nozhik_event_bus::EventBus;
use ropey::Rope;
use std::{
    fs::OpenOptions,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, OnceLock, mpsc},
    thread,
};
use thiserror::Error;
use tree::ProjectTree;
use watcher::ProjectWatcher;

/// Filesystem failures, transported to the UI as error messages.
#[derive(Debug, Error)]
pub enum FsError {
    /// Disk operation failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// Ignore-aware traversal failed.
    #[error(transparent)]
    Walk(#[from] ignore::Error),
    /// Invalid root.
    #[error("Not a directory: {0}")]
    NotDirectory(PathBuf),
    /// Unsupported binary contents.
    #[error("Binary file not supported")]
    Binary,
    /// Text is not UTF-8.
    #[error("File is not valid UTF-8: {0}")]
    Encoding(#[from] std::string::FromUtf8Error),
    /// Directory request escaped the tree.
    #[error("Path is outside the project")]
    OutsideProject,
}
/// Validates a single project directory name on every supported OS.
///
/// # Errors
/// Rejects empty names, traversal, separators and Windows reserved names.
pub fn validate_project_name(name: &str) -> Result<(), FsError> {
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem
                .as_bytes()
                .get(3)
                .is_some_and(|b| (b'1'..=b'9').contains(b)));
    if name.trim().is_empty()
        || name != name.trim()
        || name.ends_with('.')
        || name
            .chars()
            .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c))
        || matches!(name, "." | "..")
        || reserved
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Enter a project name without separators or reserved characters",
        )
        .into());
    }
    Ok(())
}
/// UI-to-worker commands, processed serially so saves cannot overtake one another.
pub enum Command {
    /// Replace the open project.
    OpenProject(PathBuf),
    /// Create a project directory exclusively, then open it.
    CreateProject {
        /// Existing parent directory chosen by the user.
        parent: PathBuf,
        /// A single portable directory name.
        name: String,
    },
    /// Create one named child inside an existing project directory.
    CreateEntry {
        /// Existing destination directory.
        parent: PathBuf,
        /// Single file or directory name.
        name: String,
        /// Whether to create a directory instead of an empty file.
        directory: bool,
    },
    /// Reveal a project entry in the OS file manager.
    ShowInFileManager(PathBuf),
    /// Load one directory's children.
    LoadDir(PathBuf),
    /// Refresh index and previously loaded directories.
    Refresh,
    /// Detect file or folder on the worker, then open through existing I/O paths.
    OpenPath(PathBuf),
    /// Create a new empty file exclusively; existing files are never truncated.
    Create(PathBuf),
    /// Read and canonicalize a file.
    Read(PathBuf),
    /// Persist a shared Rope snapshot; revision identifies the save point.
    Save {
        /// Canonical destination.
        path: PathBuf,
        /// Buffer revision being written.
        revision: u64,
        /// Shared text snapshot.
        text: Rope,
    },
    /// Search newest index; token rejects stale query responses.
    Search {
        /// Query generation.
        token: u64,
        /// Relative fuzzy query.
        query: String,
    },
    /// Literal text search across every project file. Indexed extensions are reported first.
    SearchContent {
        /// Query generation. Stale scans are ignored by the UI.
        token: u64,
        /// Case-sensitive literal. Empty queries do not read files.
        query: String,
        /// Lowercase extensions that belong to the supported-language index.
        indexed_extensions: Vec<String>,
    },
    /// Drop cached text hits for a changed or deleted path and its descendants.
    ForgetSearch(PathBuf),
    /// Drop the in-memory text search cache. Called on shutdown and project switch.
    ClearSearchCache,
}
/// Reliable command completions, separate from best-effort bus notifications.
pub enum Reply {
    /// A project was successfully opened (distinct from loading tree children).
    ProjectOpened(ProjectTree),
    /// Successfully created and opened a new project.
    ProjectCreated(ProjectTree),
    /// Project creation failed; the UI may keep its form open.
    ProjectCreationError(String),
    /// Entry creation completed; files should open in the editor.
    EntryCreated {
        /// Created absolute path.
        path: PathBuf,
        /// Whether the entry is a directory.
        directory: bool,
    },
    /// Entry creation failed, retaining the input dialog.
    EntryCreationError(String),
    /// New tree snapshot.
    Tree(ProjectTree),
    /// Refreshed index and loaded directories after filesystem invalidation.
    Refreshed(ProjectTree),
    /// Canonical path and validated UTF-8 contents.
    Read {
        /// Canonical file identity.
        path: PathBuf,
        /// Validated text.
        text: String,
    },
    /// Binary placeholder, never editable/savable.
    Binary(PathBuf),
    /// Successfully persisted revision.
    Saved {
        /// Canonical destination.
        path: PathBuf,
        /// Successfully persisted revision.
        revision: u64,
    },
    /// Fuzzy results for one query token.
    Matches {
        /// Query generation.
        token: u64,
        /// Ranked paths.
        files: Vec<FileMatch>,
    },
    /// A batch of literal text matches. `done` is the last batch for `token`.
    TextMatches {
        /// Query generation.
        token: u64,
        /// New matches. Indexed files are sent before the rest.
        hits: Vec<TextHit>,
        /// True while project files are still being read.
        scanning: bool,
        /// True on the final batch for this token.
        done: bool,
        /// Files read from disk in this batch. Cache hits stay at zero.
        files_read: usize,
    },
    /// Recoverable failure for the status/log.
    Error(String),
}
/// Channels to a dedicated worker. Dropping the handle requests shutdown.
pub struct FileSystem {
    commands: mpsc::Sender<Command>,
    replies: mpsc::Receiver<Reply>,
    wakeup: Arc<Wakeup>,
}
impl FileSystem {
    /// Spawns the worker without performing disk I/O on the caller.
    #[must_use]
    pub fn start(bus: EventBus) -> Self {
        let (commands, requests) = mpsc::channel();
        let (results, replies) = mpsc::channel();
        let wakeup = Arc::new(Wakeup::default());
        let notification = wakeup.clone();
        let search = content_search::SearchHub::spawn(results.clone(), {
            let wakeup = notification.clone();
            move || wakeup.wake()
        });
        thread::spawn(move || {
            let mut worker = Worker {
                tree: None,
                index: FileIndex::default(),
                watcher: None,
                bus,
                wakeup: notification.clone(),
                search: Arc::clone(&search),
            };
            let _search_shutdown = content_search::ShutdownGuard(search);
            while let Ok(command) = requests.recv() {
                let creating = matches!(command, Command::CreateProject { .. });
                let entry = matches!(command, Command::CreateEntry { .. });
                match worker.execute(command) {
                    Ok(replies) => {
                        for reply in replies {
                            if results.send(reply).is_err() {
                                return;
                            }
                        }
                    }
                    Err(error) => {
                        let reply = if creating {
                            Reply::ProjectCreationError(error.to_string())
                        } else if entry {
                            Reply::EntryCreationError(error.to_string())
                        } else {
                            Reply::Error(error.to_string())
                        };
                        if results.send(reply).is_err() {
                            return;
                        }
                    }
                }
                notification.wake();
            }
        });
        Self {
            commands,
            replies,
            wakeup,
        }
    }
    /// Registers a nonblocking event-loop wake callback once. It runs on workers.
    pub fn set_wakeup(&self, callback: impl Fn() + Send + Sync + 'static) {
        self.wakeup.callback.get_or_init(|| Box::new(callback));
    }
    /// Queues an operation. False means the worker has stopped.
    #[must_use]
    pub fn send(&self, command: Command) -> bool {
        self.commands.send(command).is_ok()
    }
    /// Drains at most 128 completions without blocking.
    pub fn drain(&self) -> impl Iterator<Item = Reply> + '_ {
        self.replies.try_iter().take(128)
    }
}
#[derive(Default)]
struct Wakeup {
    callback: OnceLock<Box<dyn Fn() + Send + Sync>>,
}
impl Wakeup {
    fn wake(&self) {
        if let Some(callback) = self.callback.get() {
            callback();
        }
    }
}
struct Worker {
    tree: Option<ProjectTree>,
    index: FileIndex,
    watcher: Option<ProjectWatcher>,
    wakeup: Arc<Wakeup>,
    bus: EventBus,
    search: Arc<content_search::SearchHub>,
}
impl Worker {
    fn create_project(&mut self, parent: &Path, name: &str) -> Result<Vec<Reply>, FsError> {
        validate_project_name(name)?;
        let parent = parent.canonicalize()?;
        if !parent.is_dir() {
            return Err(FsError::NotDirectory(parent));
        }
        let path = parent.join(name);
        std::fs::create_dir(&path)?;
        self.execute(Command::OpenProject(path)).map(|replies| {
            replies
                .into_iter()
                .map(|reply| {
                    if let Reply::ProjectOpened(tree) = reply {
                        Reply::ProjectCreated(tree)
                    } else {
                        reply
                    }
                })
                .collect()
        })
    }
    fn execute(&mut self, command: Command) -> Result<Vec<Reply>, FsError> {
        let reply = match command {
            Command::OpenProject(root) => self.open_project(&root)?,
            Command::Search { .. }
            | Command::SearchContent { .. }
            | Command::ForgetSearch(_)
            | Command::ClearSearchCache => return self.search_command(command),
            Command::CreateProject { parent, name } => return self.create_project(&parent, &name),
            Command::CreateEntry {
                parent,
                name,
                directory,
            } => {
                return self.create_entry(&parent, &name, directory);
            }
            Command::ShowInFileManager(path) => {
                entries::show_in_file_manager(&path)?;
                return Ok(Vec::new());
            }
            Command::LoadDir(path) => {
                let Some(tree) = &mut self.tree else {
                    return Ok(Vec::new());
                };
                tree.load(&path)?;
                Reply::Tree(tree.clone())
            }
            Command::Refresh => {
                let Some(tree) = &mut self.tree else {
                    return Ok(Vec::new());
                };
                self.index = FileIndex::scan(&tree.root_path)?;
                let loaded: Vec<_> = tree
                    .directories
                    .values()
                    .filter(|d| d.children.is_some())
                    .map(|d| d.path.clone())
                    .collect();
                for path in loaded {
                    if path.is_dir() {
                        tree.load(&path)?;
                    } else {
                        tree.directories.remove(&path);
                    }
                }
                Reply::Refreshed(tree.clone())
            }
            Command::OpenPath(path) => {
                return self.execute(if path.is_dir() {
                    Command::OpenProject(path)
                } else {
                    Command::Read(path)
                });
            }
            Command::Create(path) => {
                std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)?;
                return self.execute(Command::Read(path));
            }
            Command::Read(path) => {
                let path = path.canonicalize()?;
                match read_text(&path) {
                    Ok(text) => Reply::Read { path, text },
                    Err(FsError::Binary) => Reply::Binary(path),
                    Err(error) => return Err(error),
                }
            }
            Command::Save {
                path,
                revision,
                text,
            } => {
                write_text(&path, &text)?;
                Reply::Saved { path, revision }
            }
        };
        Ok(vec![reply])
    }

    fn open_project(&mut self, root: &Path) -> Result<Reply, FsError> {
        let mut tree = ProjectTree::new(root)?;
        let root = tree.root_path.clone();
        tree.load(&root)?;
        // Start observing before scanning so changes during the scan invalidate it.
        let watcher =
            ProjectWatcher::start_with_wakeup(root.clone(), self.bus.clone(), self.wakeup.clone());
        let index = FileIndex::scan(&root)?;
        self.search.clear()?;
        self.watcher = match watcher {
            Ok(watcher) => Some(watcher),
            Err(error) => {
                self.bus.publish(nozhik_event_bus::Event::File(
                    nozhik_event_bus::FileEvent::Error(error.to_string()),
                ));
                None
            }
        };
        self.index = index;
        self.tree = Some(tree.clone());
        Ok(Reply::ProjectOpened(tree))
    }

    fn search_command(&self, command: Command) -> Result<Vec<Reply>, FsError> {
        match command {
            Command::Search { token, query } => Ok(vec![Reply::Matches {
                token,
                files: self.index.search(&query, 500),
            }]),
            Command::SearchContent {
                token,
                query,
                indexed_extensions,
            } => {
                self.search.submit(content_search::SearchJob {
                    token,
                    query,
                    extensions: indexed_extensions,
                    files: self.index.snapshot(),
                });
                Ok(Vec::new())
            }
            Command::ForgetSearch(path) => {
                self.search.forget(&path)?;
                Ok(Vec::new())
            }
            Command::ClearSearchCache => {
                self.search.clear()?;
                Ok(Vec::new())
            }
            _ => Ok(Vec::new()),
        }
    }
}
fn write_text(path: &std::path::Path, text: &Rope) -> Result<(), FsError> {
    // Check existing-file write access before staging a replacement. No create on deletion.
    let original = OpenOptions::new().write(true).open(path)?;
    let permissions = original.metadata()?.permissions();
    drop(original);
    let parent = path.parent().ok_or(FsError::OutsideProject)?;
    let mut staged = tempfile::NamedTempFile::new_in(parent)?;
    staged.as_file().set_permissions(permissions)?;
    for chunk in text.chunks() {
        staged.write_all(chunk.as_bytes())?;
    }
    staged.as_file().sync_all()?;
    staged.persist(path).map_err(|error| error.error)?;
    Ok(())
}
fn read_text(path: &std::path::Path) -> Result<String, FsError> {
    let mut file = std::fs::File::open(path)?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut file).take(1024).read_to_end(&mut bytes)?;
    if bytes.contains(&0) {
        return Err(FsError::Binary);
    }
    file.read_to_end(&mut bytes)?;
    Ok(String::from_utf8(bytes)?)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn project_entries_are_exclusive_validated_and_refresh_lazy_parent()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let worker = FileSystem::start(EventBus::new(16)?);
        let next = || {
            worker
                .replies
                .recv_timeout(std::time::Duration::from_secs(5))
        };
        assert!(worker.send(Command::OpenProject(dir.path().into())));
        let Reply::ProjectOpened(tree) = next()? else {
            return Err("Expected project".into());
        };
        let root = tree.root_path;
        for (name, directory) in [("Новая папка", true), ("hello world.txt", false)] {
            assert!(worker.send(Command::CreateEntry {
                parent: root.clone(),
                name: name.into(),
                directory
            }));
            assert!(
                matches!(next()?, Reply::EntryCreated { path, directory: actual } if path == root.join(name) && actual == directory)
            );
            assert!(
                matches!(next()?, Reply::Tree(tree) if tree.directories[&root].children.as_ref().is_some_and(|children| children.iter().any(|node| node.path() == root.join(name))))
            );
        }
        let file = root.join("hello world.txt");
        std::fs::write(&file, "keep")?;
        for (parent, name, directory) in [
            (root.clone(), "hello world.txt", false),
            (root.clone(), "Новая папка", true),
            (root.clone(), "../escape", false),
            (root.clone(), "", true),
            (root.clone(), "CON.txt", false),
            (root.join("missing"), "x", false),
            (
                root.parent().ok_or("missing parent")?.to_path_buf(),
                "escape",
                true,
            ),
        ] {
            assert!(worker.send(Command::CreateEntry {
                parent,
                name: name.into(),
                directory
            }));
            assert!(matches!(next()?, Reply::EntryCreationError(_)));
        }
        assert_eq!(std::fs::read_to_string(file)?, "keep");
        assert!(worker.send(Command::CreateEntry {
            parent: root.join("Новая папка"),
            name: "nested.rs".into(),
            directory: false
        }));
        assert!(matches!(next()?, Reply::EntryCreated { .. }));
        assert!(matches!(next()?, Reply::Tree(_)));
        Ok(())
    }

    #[test]
    fn create_project_exclusive_and_open_path_modes() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let worker = FileSystem::start(EventBus::new(16)?);
        let next = || {
            worker
                .replies
                .recv_timeout(std::time::Duration::from_secs(5))
        };
        for name in [
            "",
            "..",
            "../escape",
            "a/b",
            "a\\b",
            "CON",
            "nul.txt",
            "x:",
            "bad.",
        ] {
            assert!(validate_project_name(name).is_err(), "{name}");
        }
        assert!(worker.send(Command::CreateProject {
            parent: dir.path().into(),
            name: "Новый проект".into()
        }));
        let Reply::ProjectCreated(tree) = next()? else {
            return Err("Expected opened project".into());
        };
        let project = dir.path().join("Новый проект").canonicalize()?;
        assert_eq!(tree.root_path, project);
        let file = project.join("keep.txt");
        std::fs::write(&file, "keep")?;
        assert!(worker.send(Command::CreateProject {
            parent: dir.path().into(),
            name: "Новый проект".into()
        }));
        assert!(matches!(next()?, Reply::ProjectCreationError(_)));
        assert_eq!(std::fs::read_to_string(&file)?, "keep");
        assert!(worker.send(Command::OpenPath(file)));
        assert!(matches!(next()?, Reply::Read { text, .. } if text == "keep"));
        assert!(worker.send(Command::OpenPath(project)));
        assert!(matches!(next()?, Reply::ProjectOpened(_)));
        assert!(worker.send(Command::OpenProject(dir.path().join("missing"))));
        assert!(matches!(next()?, Reply::Error(_)));
        Ok(())
    }
    #[test]
    fn background_index_refresh_and_read() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("main.rs");
        std::fs::write(&path, "hello")?;
        let worker = FileSystem::start(EventBus::new(16)?);
        let next = || {
            worker
                .replies
                .recv_timeout(std::time::Duration::from_secs(5))
        };
        assert!(worker.send(Command::OpenProject(dir.path().into())));
        assert!(matches!(next()?, Reply::ProjectOpened(_)));
        assert!(worker.send(Command::Search {
            token: 1,
            query: "mnrs".into()
        }));
        let Reply::Matches { token, files } = next()? else {
            return Err("expected matches".into());
        };
        assert_eq!(token, 1);
        assert_eq!(files.len(), 1);
        assert!(worker.send(Command::Read(files[0].path.clone())));
        assert!(matches!(next()?, Reply::Read { text, .. } if text == "hello"));
        std::fs::remove_file(path)?;
        std::fs::write(dir.path().join("new.rs"), "new")?;
        assert!(worker.send(Command::Refresh));
        assert!(matches!(next()?, Reply::Refreshed(_)));
        assert!(worker.send(Command::Search {
            token: 2,
            query: String::new()
        }));
        let Reply::Matches { files, .. } = next()? else {
            return Err("expected matches".into());
        };
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].relative, "new.rs");
        Ok(())
    }
    #[test]
    fn save_replaces_preserves_permissions_and_reports_errors()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("file.rs");
        std::fs::write(&path, "long old contents")?;
        let permissions = std::fs::metadata(&path)?.permissions();
        write_text(&path, &Rope::from_str("новое\r\n"))?;
        assert_eq!(std::fs::read_to_string(&path)?, "новое\r\n");
        assert_eq!(std::fs::metadata(&path)?.permissions(), permissions);
        assert!(write_text(&dir.path().join("missing"), &Rope::new()).is_err());
        assert!(write_text(dir.path(), &Rope::new()).is_err());
        Ok(())
    }
    #[test]
    fn io_binary_encoding_and_errors() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("file.rs");
        std::fs::write(&path, "привет\r\n")?;
        assert_eq!(read_text(&path)?, "привет\r\n");
        std::fs::write(&path, [0, 1, 2])?;
        assert!(matches!(read_text(&path), Err(FsError::Binary)));
        std::fs::write(&path, [255])?;
        assert!(matches!(read_text(&path), Err(FsError::Encoding(_))));
        assert!(read_text(&dir.path().join("missing")).is_err());
        Ok(())
    }

    #[test]
    fn content_search_covers_unindexed_files_and_caches_until_change_or_clear()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let root = dir.path();
        write_search_fixture(root)?;
        let worker = FileSystem::start(EventBus::new(32)?);
        assert!(worker.send(Command::OpenProject(root.into())));
        let opened = worker
            .replies
            .recv_timeout(std::time::Duration::from_secs(5))?;
        assert!(matches!(opened, Reply::ProjectOpened(_)));
        let extensions = ["php", "phtml", "css", "json", "yaml", "yml"]
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let shared = collect_text(&worker, 1, "SHARED_NEEDLE", &extensions)?;
        assert!(shared.files_read > 0, "cold scan reads project files");
        assert!(
            shared.saw_scanning,
            "results keep a spinner while unscanned files are still open"
        );
        let mut saw_plain = false;
        for hit in &shared.hits {
            if hit.indexed && saw_plain {
                return Err("indexed hit arrived after an unindexed file".into());
            }
            saw_plain |= !hit.indexed;
        }
        assert!(
            shared
                .hits
                .iter()
                .any(|hit| hit.relative.ends_with("Widget.php") && hit.indexed)
        );
        assert!(
            shared
                .hits
                .iter()
                .any(|hit| hit.relative == "notes/readme.txt" && !hit.indexed)
        );
        let plain = collect_text(&worker, 2, "PLAIN_NEEDLE", &extensions)?;
        let plain_paths: Vec<_> = plain.hits.iter().map(|hit| hit.relative.as_str()).collect();
        assert!(plain_paths.contains(&"notes/readme.txt"));
        assert!(plain_paths.contains(&"vendor/extra.md"));
        assert!(plain.hits.iter().all(|hit| !hit.relative.contains("blob")
            && !hit.path.components().any(|c| c.as_os_str() == ".git")));
        let script = collect_text(&worker, 3, "SCRIPT_NEEDLE", &extensions)?;
        assert_eq!(script.hits.len(), 1);
        assert!(!script.hits[0].indexed);
        assert_eq!(script.hits[0].relative, "app.js");
        let indexed = collect_text(&worker, 4, "INDEX_NEEDLE", &extensions)?;
        assert_eq!(indexed.hits.len(), 1);
        assert!(indexed.hits[0].indexed);
        let css = collect_text(&worker, 11, "CSS_NEEDLE", &extensions)?;
        assert_eq!(css.hits.len(), 1);
        assert!(css.hits[0].indexed);
        assert!(css.hits[0].relative.ends_with("theme.css"));
        assert_cache_and_names(&worker, root, &extensions, plain.hits.len())?;
        Ok(())
    }

    fn write_search_fixture(root: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
        std::fs::create_dir_all(root.join("src"))?;
        std::fs::create_dir_all(root.join("notes"))?;
        std::fs::create_dir_all(root.join("vendor"))?;
        std::fs::create_dir_all(root.join("data"))?;
        std::fs::create_dir_all(root.join(".git"))?;
        std::fs::write(root.join(".gitignore"), "notes/\n")?;
        std::fs::write(
            root.join("src/Widget.php"),
            "<?php\nclass Widget { const INDEX_NEEDLE = 1; const SHARED_NEEDLE = 2; }\n",
        )?;
        std::fs::write(root.join("src/theme.css"), ".box { color: CSS_NEEDLE; }\n")?;
        std::fs::write(
            root.join("notes/readme.txt"),
            "plain PLAIN_NEEDLE\nshared SHARED_NEEDLE\n",
        )?;
        std::fs::write(root.join("vendor/extra.md"), "markdown PLAIN_NEEDLE\n")?;
        std::fs::write(root.join("app.js"), "const SCRIPT_NEEDLE = true;\n")?;
        std::fs::write(root.join("data/blob.bin"), b"PLAIN_NEEDLE\0HIDDEN")?;
        std::fs::write(root.join(".git/config"), "PLAIN_NEEDLE\n")?;
        Ok(())
    }

    fn assert_cache_and_names(
        worker: &FileSystem,
        root: &std::path::Path,
        extensions: &[String],
        plain_len: usize,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let again = collect_text(worker, 5, "PLAIN_NEEDLE", extensions)?;
        assert_eq!(
            again.files_read, 0,
            "the same query must not reread the project"
        );
        assert_eq!(again.hits.len(), plain_len);
        std::fs::write(root.join("notes/readme.txt"), "plain edited\n")?;
        assert!(worker.send(Command::ForgetSearch(root.join("notes/readme.txt"))));
        let forgotten = collect_text(worker, 6, "PLAIN_NEEDLE", extensions)?;
        assert_eq!(
            forgotten.files_read, 1,
            "only the changed file is read again"
        );
        assert!(
            forgotten
                .hits
                .iter()
                .all(|hit| hit.relative != "notes/readme.txt")
        );
        assert!(
            forgotten
                .hits
                .iter()
                .any(|hit| hit.relative == "vendor/extra.md")
        );
        std::fs::write(
            root.join("src/Widget.php"),
            "<?php\nclass Widget { const FRESH = 1; }\n",
        )?;
        assert!(worker.send(Command::ClearSearchCache));
        let cleared = collect_text(worker, 7, "INDEX_NEEDLE", extensions)?;
        assert!(
            cleared.files_read > 1,
            "clearing the cache scans the project again"
        );
        assert_eq!(cleared.hits.len(), 0);
        let fresh = collect_text(worker, 8, "FRESH", extensions)?;
        assert!(
            fresh
                .hits
                .iter()
                .any(|hit| hit.relative.ends_with("Widget.php"))
        );
        assert_file_name(worker, 9, "readme", "notes/readme.txt")?;
        assert_file_name(worker, 10, "Widget", "Widget.php")?;
        Ok(())
    }

    fn assert_file_name(
        worker: &FileSystem,
        token: u64,
        query: &str,
        relative_suffix: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        assert!(worker.send(Command::Search {
            token,
            query: query.to_owned(),
        }));
        let names = worker
            .replies
            .recv_timeout(std::time::Duration::from_secs(5))?;
        let Reply::Matches { files, .. } = names else {
            return Err("expected file name matches".into());
        };
        assert!(
            files
                .iter()
                .any(|file| file.relative.ends_with(relative_suffix))
        );
        Ok(())
    }

    struct Collected {
        hits: Vec<TextHit>,
        files_read: usize,
        saw_scanning: bool,
    }

    fn collect_text(
        worker: &FileSystem,
        token: u64,
        query: &str,
        extensions: &[String],
    ) -> Result<Collected, Box<dyn std::error::Error>> {
        assert!(worker.send(Command::SearchContent {
            token,
            query: query.to_owned(),
            indexed_extensions: extensions.to_vec(),
        }));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
        let mut collected = Collected {
            hits: Vec::new(),
            files_read: 0,
            saw_scanning: false,
        };
        while std::time::Instant::now() < deadline {
            match worker
                .replies
                .recv_timeout(std::time::Duration::from_millis(200))
            {
                Ok(Reply::TextMatches {
                    token: reply_token,
                    hits,
                    scanning,
                    done,
                    files_read,
                }) => {
                    assert_eq!(reply_token, token);
                    collected.files_read += files_read;
                    collected.saw_scanning |= scanning;
                    collected.hits.extend(hits);
                    if done {
                        return Ok(collected);
                    }
                }
                Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("search worker stopped".into());
                }
            }
        }
        Err("timed out waiting for text search".into())
    }
}
