//! Project-scoped Docker Compose discovery and background CLI commands.
pub mod compose;
mod engine;
mod process;
pub use compose::{RunMarker, is_compose_file};
use std::{
    ffi::OsString,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

/// Docker operation failure.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Filesystem or child I/O failure.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// CLI, syntax, scope or protocol error.
    #[error("{0}")]
    Message(String),
    /// Unexpected Docker JSON.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
/// Docker operation result.
pub type Result<T> = std::result::Result<T, Error>;
/// One declared Compose service, including services not yet created.
#[derive(Clone, Debug)]
pub struct Service {
    /// Compose service key.
    pub name: String,
    /// Configured image, if supplied.
    pub image: String,
}
/// Immutable project container facts obtained from Docker inspect.
#[derive(Clone, Debug, Default)]
pub struct Container {
    /// Full immutable container ID; names are never command targets.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Compose service label.
    pub service: String,
    /// Configured image.
    pub image: String,
    /// Engine state: running, exited, created, paused, etc.
    pub state: String,
    /// Healthcheck status, if present.
    pub health: String,
    /// Exit code.
    pub exit_code: i64,
    /// Mount source/destination pairs.
    pub mounts: Vec<(String, String)>,
    /// Container labels, without environment variables.
    pub labels: Vec<(String, String)>,
}
/// Evaluated Compose project and its scoped containers.
#[derive(Clone, Debug)]
pub struct Snapshot {
    /// Evaluated project name (including .env/name overrides).
    pub project: String,
    /// Declared services.
    pub services: Vec<Service>,
    /// Only containers whose Compose labels belong to this configuration directory.
    pub containers: Vec<Container>,
}
/// Explicit user action; CLI arguments are always passed directly, without a shell.
#[derive(Clone, Debug)]
pub enum Action {
    /// Create/start all services, or one service and its dependencies.
    Up(Option<String>),
    /// Start an existing container.
    Start(String),
    /// Stop an existing container.
    Stop(String),
    /// Stop all scoped containers, retaining containers and volumes.
    StopAll,
    /// Fetch the most recent 500 timestamped log lines.
    Logs(String),
    /// Validate a running container and prepare an interactive /bin/sh console.
    Console(String),
}
/// Commands consumed on a dedicated worker thread.
pub enum Command {
    /// Parse an immutable editor snapshot for gutter markers.
    Document {
        /// Document path.
        path: PathBuf,
        /// Buffer identity, never reused by the UI.
        buffer: u64,
        /// Source revision.
        revision: u64,
        /// Complete editor text.
        source: String,
    },
    /// Evaluate a selected Compose configuration and list containers.
    Refresh {
        /// UI request identity.
        token: u64,
        /// Ordered Compose -f paths.
        files: Vec<PathBuf>,
    },
    /// Perform one explicit project-scoped operation.
    Act {
        /// UI request identity.
        token: u64,
        /// Ordered Compose -f paths.
        files: Vec<PathBuf>,
        /// Requested action.
        action: Action,
    },
    /// Reconcile Compose filenames now.
    Discover,
}
/// Worker events; no callback or I/O is performed on the UI thread.
pub enum Event {
    /// Changed file inventory, or a discovery failure.
    Files(Result<Vec<PathBuf>>),
    /// Source-bound editor run markers.
    Markers {
        /// Document path.
        path: PathBuf,
        /// Exact buffer identity.
        buffer: u64,
        /// Exact source revision.
        revision: u64,
        /// Parsed markers.
        markers: Vec<RunMarker>,
    },
    /// A completed request, including errors and cancellation.
    Completed(u64, Result<Response>),
}
/// Successful worker reply.
pub enum Response {
    /// Refreshed project/container facts after a query or mutation.
    Snapshot(Snapshot),
    /// Requested container log text.
    Logs(String),
    /// An explicitly requested PTY command, with validated container identity.
    Console {
        /// Native Docker CLI executable.
        program: PathBuf,
        /// Direct exec arguments, without a shell wrapper.
        args: Vec<OsString>,
        /// Container display name.
        name: String,
    },
}
/// One worker per IDE project; dropping it cancels only its CLI subprocess.
pub struct DockerWorker {
    input: mpsc::SyncSender<Command>,
    events: mpsc::Receiver<Event>,
    stop: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
}
impl DockerWorker {
    /// Starts filename discovery. Does not start Docker or any container.
    #[must_use]
    pub fn start(root: PathBuf, wake: impl Fn() + Send + 'static) -> Self {
        Self::with_program(root, None, wake)
    }
    /// Starts with a specific CLI executable, also used by process-isolated tests.
    #[must_use]
    pub fn with_program(
        root: PathBuf,
        program: Option<PathBuf>,
        wake: impl Fn() + Send + 'static,
    ) -> Self {
        let (input, commands) = mpsc::sync_channel(32);
        let (output, events) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let cancel = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let cancelling = cancel.clone();
        std::thread::spawn(move || {
            worker(
                &root,
                program,
                &commands,
                &output,
                &stopping,
                &cancelling,
                &wake,
            );
        });
        Self {
            input,
            events,
            stop,
            cancel,
        }
    }
    /// Enqueues without blocking; false means retain/retry the command later.
    #[must_use]
    pub fn send(&self, command: Command) -> bool {
        self.input.try_send(command).is_ok()
    }
    /// Drains a bounded batch of available replies.
    pub fn drain(&self) -> impl Iterator<Item = Event> + '_ {
        self.events.try_iter().take(128)
    }
    /// Requests cancellation of the active CLI operation. Containers are not removed.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
impl Drop for DockerWorker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.cancel.store(true, Ordering::Relaxed);
    }
}
fn worker(
    root: &std::path::Path,
    program: Option<PathBuf>,
    input: &mpsc::Receiver<Command>,
    output: &mpsc::Sender<Event>,
    stop: &Arc<AtomicBool>,
    cancel: &Arc<AtomicBool>,
    wake: &impl Fn(),
) {
    let engine = engine::Engine::new(
        root.to_path_buf(),
        program.unwrap_or_else(process::docker_program),
        cancel.clone(),
    );
    let mut known = Vec::new();
    let mut first = true;
    let discover = |known: &mut Vec<PathBuf>, first: &mut bool| {
        match compose::discover(root) {
            Ok(files) if *first || *known != files => {
                known.clone_from(&files);
                let _sent = output.send(Event::Files(Ok(files)));
                wake();
            }
            Ok(_) => {}
            Err(error) => {
                let _sent = output.send(Event::Files(Err(error)));
                wake();
            }
        }
        *first = false;
    };
    discover(&mut known, &mut first);
    while !stop.load(Ordering::Relaxed) {
        let command = match input.recv_timeout(Duration::from_secs(5)) {
            Ok(command) => command,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                discover(&mut known, &mut first);
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        if stop.load(Ordering::Relaxed) {
            break;
        }
        cancel.store(false, Ordering::Relaxed);
        let event = match command {
            Command::Discover => {
                discover(&mut known, &mut first);
                continue;
            }
            Command::Document {
                path,
                buffer,
                revision,
                source,
            } => Event::Markers {
                path,
                buffer,
                revision,
                markers: compose::run_markers(&source).unwrap_or_default(),
            },
            Command::Refresh { token, files } => {
                Event::Completed(token, engine.snapshot(&files).map(Response::Snapshot))
            }
            Command::Act {
                token,
                files,
                action,
            } => Event::Completed(token, engine.act(&files, &action)),
        };
        if output.send(event).is_err() {
            break;
        }
        wake();
    }
}
#[cfg(test)]
mod tests;
