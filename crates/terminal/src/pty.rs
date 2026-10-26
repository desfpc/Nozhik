//! Platform PTY boundary. All methods run outside the UI thread.
use crate::{Result, TerminalError};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize};
use std::{
    ffi::OsString,
    io::{Read, Write},
    path::PathBuf,
};

/// Shell invocation; explicit commands make tests independent of user shell files.
#[derive(Clone, Debug)]
pub struct ShellCommand {
    /// Executable path/name, passed without invoking an intermediary shell.
    pub program: OsString,
    /// Shell arguments.
    pub args: Vec<OsString>,
}
/// Resolves the platform shell. This function is called only in the PTY worker.
#[must_use]
pub fn default_shell() -> ShellCommand {
    #[cfg(unix)]
    {
        let program = std::env::var_os("SHELL")
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                if cfg!(target_os = "macos") {
                    "/bin/zsh".into()
                } else {
                    "/bin/bash".into()
                }
            });
        ShellCommand {
            program,
            args: vec!["-l".into()],
        }
    }
    #[cfg(windows)]
    {
        let program = std::env::var_os("COMSPEC")
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                let found = std::env::var_os("PATH").is_some_and(|paths| {
                    std::env::split_paths(&paths).any(|p| p.join("powershell.exe").is_file())
                });
                if found {
                    "powershell.exe".into()
                } else {
                    "cmd.exe".into()
                }
            });
        ShellCommand {
            program,
            args: Vec::new(),
        }
    }
}
/// Immutable options for starting one terminal.
#[derive(Clone, Debug)]
pub struct SessionOptions {
    /// Native project working directory.
    pub cwd: PathBuf,
    /// Explicit command, or platform shell discovery.
    pub shell: Option<ShellCommand>,
    /// Initial rows (1..=200).
    pub rows: u16,
    /// Initial columns (1..=500).
    pub cols: u16,
}
impl SessionOptions {
    /// Creates default 24×80 shell options for a project directory.
    #[must_use]
    pub const fn new(cwd: PathBuf) -> Self {
        Self {
            cwd,
            shell: None,
            rows: 24,
            cols: 80,
        }
    }
}
/// Spawned master/child plus split streams. Slave descriptors are dropped at spawn.
pub struct PtyProcess {
    /// Master geometry/control handle.
    pub master: Box<dyn MasterPty + Send>,
    /// Child process to poll, terminate and reap.
    pub child: Box<dyn Child + Send + Sync>,
    /// Output stream, moved to a reader thread.
    pub reader: Box<dyn Read + Send>,
    /// Input stream, moved to a writer thread.
    pub writer: Box<dyn Write + Send>,
}
impl PtyProcess {
    /// Opens a native Unix PTY or Windows `ConPTY` and spawns the selected shell.
    /// # Errors
    /// Returns invalid geometry, executable, cwd, PTY and pipe errors.
    pub fn spawn(options: &SessionOptions) -> Result<Self> {
        if !(1..=200).contains(&options.rows) || !(1..=500).contains(&options.cols) {
            return Err(TerminalError::Invalid("invalid terminal geometry".into()));
        }
        let pair = portable_pty::native_pty_system()
            .openpty(size(options.rows, options.cols))
            .map_err(error)?;
        // Prepare fallible stream handles before a process exists.
        let reader = pair.master.try_clone_reader().map_err(error)?;
        let writer = pair.master.take_writer().map_err(error)?;
        let shell = options.shell.clone().unwrap_or_else(default_shell);
        let mut command = CommandBuilder::new(&shell.program);
        command.args(&shell.args);
        command.cwd(&options.cwd);
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        let child = pair.slave.spawn_command(command).map_err(error)?;
        drop(pair.slave);
        Ok(Self {
            master: pair.master,
            child,
            reader,
            writer,
        })
    }
}
/// Converts validated geometry to portable-pty's row/column contract.
#[must_use]
pub const fn size(rows: u16, cols: u16) -> PtySize {
    PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}
pub(crate) fn error(error: impl std::fmt::Display) -> TerminalError {
    TerminalError::Pty(error.to_string())
}
