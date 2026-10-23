//! Isolated process plugins, declarative contributions and portable action/keymap APIs.
pub mod keymap;
mod manager;
pub mod manifest;
mod process;
pub mod registry;
#[cfg(test)]
mod tests;

pub use manager::{HostSnapshot, PluginInfo, PluginManager, PluginStatus};
use std::path::PathBuf;
use thiserror::Error;

/// Host operations fail explicitly without unwinding the UI.
#[derive(Debug, Error)]
pub enum Error {
    /// Invalid manifest, predicate, RPC or dependency graph.
    #[error("{0}")]
    Invalid(String),
    /// File or child-process error.
    #[error("plugin I/O: {0}")]
    Io(#[from] std::io::Error),
    /// Invalid JSON.
    #[error("plugin JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// Shared registry poisoned by another thread.
    #[error("plugin registry lock poisoned")]
    Poisoned,
    /// Worker stopped or its bounded queue is full.
    #[error("plugin worker stopped or queue full")]
    Queue,
}
/// Plugin host result.
pub type Result<T> = std::result::Result<T, Error>;

pub(crate) enum Request {
    Scan(Option<PathBuf>),
    Toggle { id: String, enabled: bool },
    Execute { owner: String, command: String },
}
