//! UI-independent PTY sessions, VT100 emulation and nonblocking session handles.
pub mod external;
pub mod grid;
pub mod manager;
pub mod pty;
pub mod session;

pub use manager::TerminalManager;
pub use session::{SessionSnapshot, SessionState, TerminalSession};
use std::sync::Arc;

/// Stable, monotonically allocated terminal identity; never reused by a manager.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TerminalSessionId(pub u64);
/// Callback waking a consumer's event loop; invoked only outside shared locks.
pub type Wakeup = Arc<dyn Fn() + Send + Sync>;
/// Session lifecycle events. Screen data lives in authoritative Arc snapshots.
#[derive(Clone, Debug)]
pub enum TerminalEvent {
    /// Shell successfully spawned.
    Started {
        /// Session identity.
        id: TerminalSessionId,
        /// OS child process identifier, if available.
        pid: Option<u32>,
    },
    /// Child reaped; all normal exit codes are represented, including nonzero.
    Exited {
        /// Session identity.
        id: TerminalSessionId,
        /// Shell exit code.
        code: u32,
    },
    /// Creation or I/O failure; no implicit retry or replacement shell execution.
    Error {
        /// Session identity.
        id: TerminalSessionId,
        /// Human-readable operation failure.
        message: String,
    },
}
/// Terminal command/session failure.
#[derive(Debug, thiserror::Error)]
pub enum TerminalError {
    /// Underlying PTY library failure; conversion occurs at the platform boundary.
    #[error("PTY: {0}")]
    Pty(String),
    /// OS pipe or process error.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// Backpressure; the caller must keep or retry its input.
    #[error("terminal input queue is full")]
    Busy,
    /// Closed session or unavailable worker.
    #[error("terminal session is closed")]
    Closed,
    /// Invalid input, terminal geometry or session count.
    #[error("{0}")]
    Invalid(String),
}
/// Terminal operation result.
pub type Result<T> = std::result::Result<T, TerminalError>;

#[cfg(test)]
mod tests;
