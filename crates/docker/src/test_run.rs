//! Terminal commands for project test runners.
//!
//! `PHPUnit` and `go test` open `docker exec -it` in a console. The IDE does not
//! parse the tool's output.

use std::ffi::OsString;

use crate::linter::{prompt_workdir, shell_join, split_docker_exec, tokenize};

/// Which test command a terminal line ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Runner {
    /// `PHPUnit`.
    PhpUnit,
    /// `go test`.
    GoTest,
}

impl Runner {
    /// Settings language id.
    #[must_use]
    pub const fn language(self) -> &'static str {
        match self {
            Self::PhpUnit => "php",
            Self::GoTest => "go",
        }
    }

    /// Settings runner id.
    #[must_use]
    pub const fn kind(self) -> &'static str {
        match self {
            Self::PhpUnit => "phpunit",
            Self::GoTest => "gotest",
        }
    }
}

/// A test command found in one terminal line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Detected {
    /// `PHPUnit` or `go test`.
    pub runner: Runner,
    /// Binary token as the user typed it.
    pub binary: String,
    /// Arguments to run again. A version or help command is stored empty.
    pub args: String,
    /// Container named by `docker exec`, when the line itself names one.
    pub container: Option<String>,
    /// Working directory from `-w` or from a shell prompt on the same line.
    pub workdir: Option<String>,
}

/// One test console inside a container.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsoleCommand {
    /// Container id or name.
    pub container: String,
    /// Binary inside the container.
    pub binary: String,
    /// Working directory inside the container. Empty keeps the image default.
    pub workdir: String,
    /// Arguments after the binary.
    pub args: String,
}

/// Finds `PHPUnit` or `go test` in one submitted terminal line.
///
/// The line may include a shell prompt or a `docker exec` prefix.
#[must_use]
pub fn detect(line: &str) -> Option<Detected> {
    let tokens = tokenize(line);
    if tokens.is_empty() {
        return None;
    }
    let (container, flag_workdir, command) =
        split_docker_exec(&tokens).unwrap_or((None, None, tokens));
    let (position, runner) =
        if let Some(position) = command.iter().position(|token| is_phpunit(token)) {
            (position, Runner::PhpUnit)
        } else {
            (go_test_at(&command)?, Runner::GoTest)
        };
    let binary = command.get(position)?.clone();
    let rest = command.get(position + 1..).unwrap_or(&[]);
    let args = match runner {
        Runner::PhpUnit => {
            if rest.first().is_some_and(|token| is_meta(token)) {
                String::new()
            } else {
                shell_join(rest)
            }
        }
        Runner::GoTest => {
            // Empty arguments would run bare `go`. Keep the `test` subcommand.
            if rest.get(1).is_some_and(|token| is_go_help(token)) {
                "test".to_owned()
            } else {
                shell_join(rest)
            }
        }
    };
    Some(Detected {
        runner,
        binary: binary.clone(),
        args,
        container,
        workdir: flag_workdir.or_else(|| prompt_workdir(line, &binary)),
    })
}

/// Docker CLI arguments that run a test command in an interactive console.
///
/// # Errors
/// Returns when the container or binary is empty.
pub fn console_args(command: &ConsoleCommand) -> crate::Result<Vec<OsString>> {
    let container = command.container.trim();
    let binary = command.binary.trim();
    if container.is_empty() || binary.is_empty() {
        return Err(crate::Error::Message(
            "Test container and binary are required".into(),
        ));
    }
    let mut args = vec![OsString::from("exec"), OsString::from("-it")];
    let workdir = command.workdir.trim();
    if !workdir.is_empty() {
        args.push(OsString::from("-w"));
        args.push(OsString::from(workdir));
    }
    args.push(OsString::from("--"));
    args.push(OsString::from(container));
    args.push(OsString::from(binary));
    args.extend(tokenize(&command.args).into_iter().map(OsString::from));
    Ok(args)
}

fn is_phpunit(token: &str) -> bool {
    let name = token.rsplit(['/', '\\']).next().unwrap_or(token);
    name.strip_suffix(".phar").unwrap_or(name) == "phpunit"
}

fn is_meta(token: &str) -> bool {
    matches!(token, "--version" | "-V" | "--help" | "-h" | "help")
}

fn go_test_at(command: &[String]) -> Option<usize> {
    command
        .iter()
        .position(|token| is_go(token))
        .filter(|index| {
            command
                .get(index.saturating_add(1))
                .is_some_and(|token| token == "test")
        })
}

fn is_go(token: &str) -> bool {
    token.rsplit(['/', '\\']).next().unwrap_or(token) == "go"
}

fn is_go_help(token: &str) -> bool {
    matches!(token, "-h" | "--help")
}

#[cfg(test)]
mod tests {
    use super::{ConsoleCommand, console_args, detect};

    #[test]
    fn detection_keeps_filters_and_a_container_prompt() -> Result<(), &'static str> {
        let detected = detect("root@box:/var/www/html# vendor/bin/phpunit --filter PaymentTest")
            .ok_or("phpunit")?;
        assert_eq!(detected.binary, "vendor/bin/phpunit");
        assert_eq!(detected.args, "--filter PaymentTest");
        assert_eq!(detected.workdir.as_deref(), Some("/var/www/html"));
        assert!(detected.container.is_none());
        let exec = detect(
            "docker exec -it -w /var/www/html php-fpm-nozhovecom vendor/bin/phpunit tests/Unit",
        )
        .ok_or("exec")?;
        assert_eq!(exec.container.as_deref(), Some("php-fpm-nozhovecom"));
        assert_eq!(exec.workdir.as_deref(), Some("/var/www/html"));
        assert_eq!(exec.args, "tests/Unit");
        assert!(detect("vendor/bin/phpunit --version").is_some_and(|found| found.args.is_empty()));
        assert!(detect("composer test").is_none());
        assert!(detect("vendor/bin/phpstan analyse").is_none());
        let go = detect("docker exec -it -w /src app go test ./... -count=1").ok_or("go")?;
        assert_eq!(go.runner, super::Runner::GoTest);
        assert_eq!(go.binary, "go");
        assert_eq!(go.args, "test ./... -count=1");
        assert_eq!(go.container.as_deref(), Some("app"));
        assert_eq!(go.workdir.as_deref(), Some("/src"));
        assert!(detect("go test -h").is_some_and(|found| found.args == "test"));
        assert!(detect("go version").is_none());
        assert!(detect("go help").is_none());
        assert!(detect("golangci-lint run ./...").is_none());
        Ok(())
    }

    #[test]
    fn console_uses_an_interactive_exec() -> Result<(), String> {
        let args = console_args(&ConsoleCommand {
            container: "abc".into(),
            binary: "vendor/bin/phpunit".into(),
            workdir: "/var/www/html".into(),
            args: "--filter PaymentTest".into(),
        })
        .map_err(|error| error.to_string())?;
        let text: Vec<String> = args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            text,
            vec![
                "exec",
                "-it",
                "-w",
                "/var/www/html",
                "--",
                "abc",
                "vendor/bin/phpunit",
                "--filter",
                "PaymentTest",
            ]
        );
        let bare = console_args(&ConsoleCommand {
            container: "abc".into(),
            binary: "vendor/bin/phpunit".into(),
            workdir: String::new(),
            args: String::new(),
        })
        .map_err(|error| error.to_string())?;
        assert_eq!(bare.len(), 5);
        assert!(
            console_args(&ConsoleCommand {
                container: "  ".into(),
                binary: "vendor/bin/phpunit".into(),
                workdir: String::new(),
                args: String::new(),
            })
            .is_err()
        );
        Ok(())
    }
}
