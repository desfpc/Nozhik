//! Local foreground-agent identification; never infer an agent from terminal output.
#[cfg(unix)]
use std::io::Read;
#[cfg(any(unix, test))]
use std::path::Path;
#[cfg(unix)]
use std::{
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// Supported local interactive coding agents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalAgent {
    /// `OpenAI` Codex CLI.
    Codex,
    /// xAI Grok Build CLI.
    Grok,
}
impl LocalAgent {
    /// Name displayed in the terminal toolbar.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Grok => "Grok Build",
        }
    }
    #[cfg(any(unix, test))]
    fn from_command(command: &str) -> Option<Self> {
        match Path::new(command.trim()).file_name()?.to_str()? {
            "codex" | "codex.exe" => Some(Self::Codex),
            "grok" | "grok-build" | "xai-grok-pager" | "grok.exe" | "grok-build.exe" => {
                Some(Self::Grok)
            }
            _ => None,
        }
    }
}

/// Identity of a recognized process occupying the foreground process group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ForegroundAgent {
    /// Recognized CLI.
    pub agent: LocalAgent,
    /// Native process identity, retained when preparing asynchronous context.
    pub pid: u32,
}

#[cfg(unix)]
pub(crate) fn foreground(master: &dyn portable_pty::MasterPty) -> Option<ForegroundAgent> {
    let pid = u32::try_from(master.process_group_leader()?).ok()?;
    if pid <= 1 {
        return None;
    }
    // Query only the foreground group (macOS includes npm/node wrappers). Bound both
    // the wait and read, and reap even on timeout. No shell command interpolation.
    let mut command = Command::new("/bin/ps");
    #[cfg(target_os = "macos")]
    command.args(["-g", &pid.to_string(), "-o", "pid=,comm="]);
    #[cfg(not(target_os = "macos"))]
    command.args(["-p", &pid.to_string(), "-o", "pid=,comm="]);
    parse_processes(&process_output(&mut command)?)
}

#[cfg(unix)]
fn process_output(command: &mut Command) -> Option<String> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let until = Instant::now() + Duration::from_millis(200);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(_)) => return None,
            Ok(None) if Instant::now() < until => std::thread::sleep(Duration::from_millis(2)),
            _ => {
                let _killed = child.kill();
                let _reaped = child.wait();
                return None;
            }
        }
    }
    let mut output = String::new();
    child
        .stdout
        .take()?
        .take(4096)
        .read_to_string(&mut output)
        .ok()?;
    Some(output)
}

#[cfg(any(unix, test))]
fn parse_processes(output: &str) -> Option<ForegroundAgent> {
    output
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let (pid, command) = line.split_once(char::is_whitespace)?;
            Some(ForegroundAgent {
                pid: pid.parse().ok()?,
                agent: LocalAgent::from_command(command)?,
            })
        })
        .min_by_key(|a| a.pid)
}

#[cfg(not(unix))]
pub(crate) fn foreground(_master: &dyn portable_pty::MasterPty) -> Option<ForegroundAgent> {
    None
}

// Recheck in the writer after earlier keyboard bytes have been flushed. A queued
// interrupt/exit must not turn an assisted paste into shell input.
#[cfg(unix)]
pub(crate) fn still_foreground(expected: ForegroundAgent) -> bool {
    let mut command = Command::new("/bin/ps");
    command.args(["-p", &expected.pid.to_string(), "-o", "tpgid=,pgid=,comm="]);
    let Some(output) = process_output(&mut command) else {
        return false;
    };
    let mut columns = output.split_whitespace();
    let foreground = columns.next().and_then(|s| s.parse::<u32>().ok());
    let group = columns.next().and_then(|s| s.parse::<u32>().ok());
    let command = columns.collect::<Vec<_>>().join(" ");
    foreground.is_some_and(|pid| pid > 1)
        && foreground == group
        && LocalAgent::from_command(&command) == Some(expected.agent)
}
#[cfg(not(unix))]
pub(crate) const fn still_foreground(_expected: ForegroundAgent) -> bool {
    false
}

/// Encodes one explicit paste; control bytes cannot escape the bracketed frame.
/// # Errors
/// Rejects terminal controls and payloads exceeding the atomic input batch limit.
pub fn paste_bytes(text: &str) -> crate::Result<Vec<u8>> {
    if text
        .chars()
        .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        return Err(crate::TerminalError::Invalid(
            "LLM context contains terminal control characters".into(),
        ));
    }
    let bytes = format!("\x1b[200~{text}\x1b[201~").into_bytes();
    if bytes.len() > 64 * 1024 {
        return Err(crate::TerminalError::Invalid(
            "LLM context exceeds 64 KiB; attach a file instead".into(),
        ));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_executable_identity_and_safe_paste() -> crate::Result<()> {
        assert_eq!(
            LocalAgent::from_command("/Users/me/bin/grok\n"),
            Some(LocalAgent::Grok)
        );
        assert_eq!(
            LocalAgent::from_command("/opt/bin/codex"),
            Some(LocalAgent::Codex)
        );
        assert_eq!(
            parse_processes(" 81 /usr/bin/node\n 92 /opt/bin/codex\n 95 /usr/bin/git\n"),
            Some(ForegroundAgent {
                pid: 92,
                agent: LocalAgent::Codex
            })
        );
        for command in ["zsh", "ssh", "codex-helper", "echo codex", "node"] {
            assert_eq!(LocalAgent::from_command(command), None);
        }
        assert_eq!(
            paste_bytes("a\nb\tЖ")?,
            "\x1b[200~a\nb\tЖ\x1b[201~".as_bytes()
        );
        assert!(paste_bytes("bad\x1b[201~\rcommand").is_err());
        assert!(paste_bytes(&"x".repeat(65536)).is_err());
        Ok(())
    }
}
