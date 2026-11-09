//! Terminal commands for project test runners.
//!
//! `PHPUnit` is the only runner. The IDE opens `docker exec -it` in a console and
//! does not parse the tool's output.

use std::ffi::OsString;

use crate::linter::{prompt_workdir, shell_join, split_docker_exec, tokenize};

/// A `PHPUnit` command found in one terminal line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Detected {
    /// Binary token as the user typed it.
    pub binary: String,
    /// Arguments to run again. A version or help command is stored empty.
    pub args: String,
    /// Container named by `docker exec`, when the line itself names one.
    pub container: Option<String>,
    /// Working directory from `-w` or from a shell prompt on the same line.
    pub workdir: Option<String>,
}

/// One `PHPUnit` console inside a container.
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

/// Finds `PHPUnit` in one submitted terminal line.
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
    let position = command.iter().position(|token| is_phpunit(token))?;
    let binary = command.get(position)?.clone();
    let rest = command.get(position + 1..).unwrap_or(&[]);
    let args = if rest.first().is_some_and(|token| is_meta(token)) {
        String::new()
    } else {
        shell_join(rest)
    };
    Some(Detected {
        binary: binary.clone(),
        args,
        container,
        workdir: flag_workdir.or_else(|| prompt_workdir(line, &binary)),
    })
}

/// Docker CLI arguments that run `PHPUnit` in an interactive console.
///
/// # Errors
/// Returns when the container or binary is empty.
pub fn console_args(command: &ConsoleCommand) -> crate::Result<Vec<OsString>> {
    let container = command.container.trim();
    let binary = command.binary.trim();
    if container.is_empty() || binary.is_empty() {
        return Err(crate::Error::Message(
            "PHPUnit container and binary are required".into(),
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
