//! Git engine with one serial worker. All repository and network I/O stays there.
mod git_worker;
pub mod model;
pub mod project_paths;

use model::{CommitInfo, DiffTarget, FileChange, FileDiff, GitRepoStatus};
use std::{
    path::PathBuf,
    sync::{Arc, OnceLock, mpsc},
    thread,
};
use thiserror::Error;

/// Recoverable Git or filesystem failure.
#[derive(Debug, Error)]
pub enum VcsError {
    /// libgit2 failure, including credentials or repository locks.
    #[error(transparent)]
    Git(#[from] git2::Error),
    /// Filesystem failure.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// Operation rejected before applying it.
    #[error("{0}")]
    Invalid(String),
}
/// Commands are serialized; commit never silently restages working-tree contents.
pub enum Task {
    /// Discover a repository from an IDE project. A non-repository is a normal state.
    Open(PathBuf),
    /// Recompute status and local branches.
    Refresh,
    /// Stage entire files or reset them to HEAD in the index (worktree is retained).
    Stage {
        /// Selected repository-relative files.
        paths: Vec<PathBuf>,
        /// True to stage; false to unstage.
        staged: bool,
    },
    /// Generate a bounded patch.
    Diff(DiffTarget),
    /// Read newest history entries in topological/time order.
    History(usize),
    /// Read commit metadata and affected files.
    Details(String),
    /// Commit the current index with the configured Git identity.
    Commit {
        /// Required single-line title.
        summary: String,
        /// Optional body.
        body: String,
    },
    /// Fetch origin, preserving local working files.
    Fetch,
    /// Non-forced push to origin; establishes tracking for an untracked branch.
    Push,
    /// Fetch and fast-forward a clean tracked branch; divergence is an error.
    Pull,
    /// Safely switch to an existing local branch, requiring a clean worktree.
    Checkout(String),
    /// Create and switch to a new local branch from HEAD.
    NewBranch(String),
    /// Restore one file's index/worktree to HEAD, or delete an untracked file.
    /// Callers must obtain explicit confirmation from the user first.
    Discard(PathBuf),
    /// Append an escaped anchored file pattern to the root .gitignore.
    Ignore(PathBuf),
}
/// Successful completion.
#[derive(Debug)]
pub enum Outcome {
    /// None means the project is outside a non-bare repository.
    Status(Option<GitRepoStatus>),
    /// Patch data for the requested target.
    Diff(FileDiff),
    /// Commit list.
    History(Vec<CommitInfo>),
    /// Commit details and first-parent changed files.
    Details {
        /// Selected commit.
        commit: CommitInfo,
        /// Changed files.
        files: Vec<FileChange>,
    },
    /// Commit succeeded. Status/history must be refreshed from the repository.
    Committed(CommitInfo),
    /// Mutation completed; refresh status/history afterwards.
    Done(String),
}
/// One completion, correlated by project generation and request identity.
#[derive(Debug)]
pub struct Reply {
    /// IDE project generation; stale-project replies must be ignored.
    pub generation: u64,
    /// Caller-issued request identity, also used to reject obsolete diff/details.
    pub id: u64,
    /// Result of exactly one command.
    pub result: Result<Outcome, VcsError>,
}
struct Request {
    generation: u64,
    id: u64,
    task: Task,
}
#[derive(Default)]
struct Wakeup(OnceLock<Box<dyn Fn() + Send + Sync>>);
/// UI-independent channel handle. Dropping it stops the worker after its current task.
pub struct Vcs {
    requests: mpsc::Sender<Request>,
    replies: mpsc::Receiver<Reply>,
    wakeup: Arc<Wakeup>,
}
impl Default for Vcs {
    fn default() -> Self {
        Self::start()
    }
}
impl Vcs {
    /// Starts an idle worker without touching any repository on the caller thread.
    #[must_use]
    pub fn start() -> Self {
        let (requests, commands) = mpsc::channel::<Request>();
        let (results, replies) = mpsc::channel();
        let wakeup = Arc::new(Wakeup::default());
        let signal = wakeup.clone();
        thread::spawn(move || {
            let mut worker = git_worker::GitWorker::default();
            while let Ok(request) = commands.recv() {
                let reply = Reply {
                    generation: request.generation,
                    id: request.id,
                    result: worker.execute(request.task),
                };
                if results.send(reply).is_err() {
                    break;
                }
                if let Some(callback) = signal.0.get() {
                    callback();
                }
            }
        });
        Self {
            requests,
            replies,
            wakeup,
        }
    }
    /// Installs a nonblocking event-loop wake callback once; runs on the worker.
    pub fn set_wakeup(&self, callback: impl Fn() + Send + Sync + 'static) {
        self.wakeup.0.get_or_init(|| Box::new(callback));
    }
    /// Queues a task. False means the worker has stopped.
    #[must_use]
    pub fn send(&self, generation: u64, id: u64, task: Task) -> bool {
        self.requests
            .send(Request {
                generation,
                id,
                task,
            })
            .is_ok()
    }
    /// Drains bounded completions without blocking.
    pub fn drain(&self) -> impl Iterator<Item = Reply> + '_ {
        self.replies.try_iter().take(128)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[test]
    fn worker_preserves_request_identity_and_wakes_ui() -> Result<(), Box<dyn std::error::Error>> {
        let first = tempfile::tempdir()?;
        let second = tempfile::tempdir()?;
        let vcs = Vcs::start();
        let count = Arc::new(AtomicUsize::new(0));
        let callbacks = count.clone();
        vcs.set_wakeup(move || {
            callbacks.fetch_add(1, Ordering::SeqCst);
        });
        assert!(vcs.send(10, 1, Task::Open(first.path().into())));
        assert!(vcs.send(11, 2, Task::Open(second.path().into())));
        let deadline = std::time::Duration::from_secs(5);
        let first = vcs.replies.recv_timeout(deadline)?;
        let second = vcs.replies.recv_timeout(deadline)?;
        assert_eq!((first.generation, first.id), (10, 1));
        assert_eq!((second.generation, second.id), (11, 2));
        assert!(matches!(first.result?, Outcome::Status(None)));
        assert!(matches!(second.result?, Outcome::Status(None)));
        // First callback precedes processing the second request.
        assert!(count.load(Ordering::SeqCst) >= 1);
        Ok(())
    }
}
