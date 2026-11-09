//! Bounded CLI execution without a command shell; cancellation kills only our CLI child.
use std::{
    ffi::OsString,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
const OUTPUT_LIMIT: usize = 4 * 1024 * 1024;
fn collect(mut pipe: impl Read) -> std::io::Result<(Vec<u8>, bool)> {
    let mut result = Vec::new();
    let mut buffer = [0; 8192];
    let mut truncated = false;
    loop {
        let count = pipe.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        let keep = count.min(OUTPUT_LIMIT.saturating_sub(result.len()));
        result.extend_from_slice(&buffer[..keep]);
        truncated |= keep < count;
    }
    Ok((result, truncated))
}
/// Resolves the Docker CLI.
///
/// A matching file on `PATH` wins. Otherwise the function checks known install
/// locations and finally returns the bare name `docker`.
#[must_use]
pub fn docker_program() -> PathBuf {
    if let Some(paths) = std::env::var_os("PATH") {
        for path in std::env::split_paths(&paths) {
            let candidate = path.join(if cfg!(windows) {
                "docker.exe"
            } else {
                "docker"
            });
            if candidate.is_file() {
                return candidate.canonicalize().unwrap_or(candidate);
            }
        }
    }
    for path in [
        "/usr/local/bin/docker",
        "/opt/homebrew/bin/docker",
        "/Applications/Docker.app/Contents/Resources/bin/docker",
    ] {
        if Path::new(path).is_file() {
            return path.into();
        }
    }
    "docker".into()
}
struct RawOutput {
    code: i32,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    truncated: bool,
}

/// Stdout and stderr of one CLI child. A nonzero exit is still a successful capture.
pub struct Captured {
    /// Process exit code. `-1` means the child ended without a code.
    pub code: i32,
    /// Standard output, lossy-decoded as UTF-8.
    pub stdout: String,
    /// Standard error, lossy-decoded as UTF-8.
    pub stderr: String,
}

pub fn run(
    program: &Path,
    cwd: &Path,
    args: &[OsString],
    stop: &Arc<AtomicBool>,
    timeout: Duration,
) -> crate::Result<String> {
    let merge = args.first().is_some_and(|arg| arg == "logs");
    let raw = execute(program, cwd, args, stop, timeout, merge)?;
    if raw.code != 0 {
        return Err(crate::Error::Message(
            String::from_utf8_lossy(if merge { &raw.stdout } else { &raw.stderr })
                .chars()
                .take(4000)
                .collect(),
        ));
    }
    if raw.truncated {
        return Err(crate::Error::Message(
            "Docker output exceeds 4 MiB; narrow the requested output".into(),
        ));
    }
    Ok(String::from_utf8_lossy(&raw.stdout).into_owned())
}

/// Runs a CLI child and keeps stdout when the process exits nonzero.
///
/// # Errors
/// Returns cancellation, timeout, spawn, truncation, or reader failures.
pub fn capture(
    program: &Path,
    cwd: &Path,
    args: &[OsString],
    stop: &Arc<AtomicBool>,
    timeout: Duration,
) -> crate::Result<Captured> {
    let raw = execute(program, cwd, args, stop, timeout, false)?;
    if raw.truncated {
        return Err(crate::Error::Message(
            "Docker output exceeds 4 MiB; narrow the requested output".into(),
        ));
    }
    Ok(Captured {
        code: raw.code,
        stdout: String::from_utf8_lossy(&raw.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&raw.stderr).into_owned(),
    })
}

fn execute(
    program: &Path,
    cwd: &Path,
    args: &[OsString],
    stop: &Arc<AtomicBool>,
    timeout: Duration,
    merge: bool,
) -> crate::Result<RawOutput> {
    if stop.load(Ordering::Relaxed) {
        return Err(crate::Error::Message("Docker command cancelled".into()));
    }
    let (stdout, stdout_writer) = std::io::pipe()?;
    let (stderr, stderr_writer) = std::io::pipe()?;
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .env("NO_COLOR", "1")
        .stdin(Stdio::null());
    if merge {
        command
            .stdout(stdout_writer.try_clone()?)
            .stderr(stdout_writer);
        drop(stderr_writer);
    } else {
        command.stdout(stdout_writer).stderr(stderr_writer);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let mut child = command.spawn().map_err(|e| {
        crate::Error::Message(format!(
            "Cannot run Docker: {e}. Install Docker with the Compose plugin and start its engine."
        ))
    })?;
    // Release the parent writer handles so reader threads can observe EOF.
    drop(command);
    let out = std::thread::spawn(move || collect(stdout));
    let err = std::thread::spawn(move || collect(stderr));
    let deadline = Instant::now() + timeout;
    let status = loop {
        if stop.load(Ordering::Relaxed) || Instant::now() >= deadline {
            terminate(&mut child);
            let _reaped = child.wait();
            return Err(crate::Error::Message(
                "Docker command cancelled or timed out; refresh to inspect its current state"
                    .into(),
            ));
        }
        if let Some(status) = child.try_wait()? {
            break status;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let (stdout, stdout_truncated) = out
        .join()
        .map_err(|_| crate::Error::Message("Docker output reader failed".into()))??;
    let (stderr, stderr_truncated) = err
        .join()
        .map_err(|_| crate::Error::Message("Docker error reader failed".into()))??;
    Ok(RawOutput {
        code: status.code().unwrap_or(-1),
        stdout,
        stderr,
        truncated: stdout_truncated || stderr_truncated,
    })
}

// Compose is commonly a child plugin process. Cancel the whole owned CLI tree,
// without sending signals to Docker's daemon or any container process.
fn terminate(child: &mut std::process::Child) {
    #[cfg(unix)]
    if let Ok(pid) = i32::try_from(child.id()) {
        let _killed = nix::sys::signal::killpg(
            nix::unistd::Pid::from_raw(pid),
            nix::sys::signal::Signal::SIGKILL,
        );
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _killed = Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .creation_flags(0x0800_0000)
            .output();
    }
    let _killed = child.kill();
}
