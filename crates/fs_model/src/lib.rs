//! Filesystem model and a serial background command worker. No UI dependencies.
pub mod fuzzy;
pub mod tree;
pub mod watcher;

use fuzzy::{FileIndex, FileMatch};
use nozhik_event_bus::EventBus;
use ropey::Rope;
use std::{
    fs::OpenOptions,
    io::{Read, Write},
    path::PathBuf,
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
/// UI-to-worker commands, processed serially so saves cannot overtake one another.
pub enum Command {
    /// Replace the open project.
    OpenProject(PathBuf),
    /// Load one directory's children.
    LoadDir(PathBuf),
    /// Refresh index and previously loaded directories.
    Refresh,
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
}
/// Reliable command completions, separate from best-effort bus notifications.
pub enum Reply {
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
        thread::spawn(move || {
            let mut worker = Worker {
                tree: None,
                index: FileIndex::default(),
                watcher: None,
                bus,
                wakeup: notification.clone(),
            };
            while let Ok(command) = requests.recv() {
                match worker.execute(command) {
                    Ok(replies) => {
                        for reply in replies {
                            if results.send(reply).is_err() {
                                return;
                            }
                        }
                    }
                    Err(error) => {
                        if results.send(Reply::Error(error.to_string())).is_err() {
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
}
impl Worker {
    fn execute(&mut self, command: Command) -> Result<Vec<Reply>, FsError> {
        let reply = match command {
            Command::OpenProject(root) => {
                let mut tree = ProjectTree::new(&root)?;
                let root = tree.root_path.clone();
                tree.load(&root)?;
                // Start observing before scanning so changes during the scan invalidate it.
                let watcher = ProjectWatcher::start_with_wakeup(
                    root.clone(),
                    self.bus.clone(),
                    self.wakeup.clone(),
                );
                let index = FileIndex::scan(&root)?;
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
                Reply::Tree(tree)
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
            Command::Search { token, query } => Reply::Matches {
                token,
                files: self.index.search(&query, 100),
            },
        };
        Ok(vec![reply])
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
        assert!(matches!(next()?, Reply::Tree(_)));
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
}
