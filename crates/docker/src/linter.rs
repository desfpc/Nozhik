//! PHP and Go linter discovery, Docker exec arguments, JSON reports and host paths.
//!
//! A linter exits nonzero when it finds issues. That status is a report, not a
//! failure to launch. Invalid JSON and configuration failures become errors.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

use serde_json::Value;

/// How serious one reported issue is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    /// The linter treats the issue as an error.
    Error,
    /// The linter treats the issue as a warning.
    Warning,
    /// The linter treats the issue as a note.
    Info,
}

/// One issue mapped onto the host project.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiagnosticItem {
    /// Display name: `PHPStan`, `PHPCS`, `Psalm`, `PHPMD` or `golangci-lint`.
    pub linter: String,
    /// Absolute path on the host.
    pub file_path: PathBuf,
    /// 1-based line.
    pub line: usize,
    /// 1-based column when the tool reports one.
    pub column: Option<usize>,
    /// Human-readable explanation.
    pub message: String,
    /// Weight copied from the tool.
    pub severity: Severity,
    /// Rule, sniff or issue identifier when the tool reports one.
    pub rule: Option<String>,
}

/// Supported linter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LinterKind {
    /// `PHPStan`.
    PhpStan,
    /// `PHP_CodeSniffer` (`phpcs`).
    PhpCs,
    /// `Psalm`.
    Psalm,
    /// PHP Mess Detector (`phpmd`).
    PhpMd,
    /// `golangci-lint`.
    Golangci,
}

impl LinterKind {
    /// Stable settings id.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::PhpStan => "phpstan",
            Self::PhpCs => "phpcs",
            Self::Psalm => "psalm",
            Self::PhpMd => "phpmd",
            Self::Golangci => "golangci-lint",
        }
    }

    /// Short label used in the UI and in AI context.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::PhpStan => "PHPStan",
            Self::PhpCs => "PHPCS",
            Self::Psalm => "Psalm",
            Self::PhpMd => "PHPMD",
            Self::Golangci => "golangci-lint",
        }
    }

    /// Parses a settings id.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "phpstan" => Some(Self::PhpStan),
            "phpcs" => Some(Self::PhpCs),
            "psalm" => Some(Self::Psalm),
            "phpmd" => Some(Self::PhpMd),
            "golangci-lint" => Some(Self::Golangci),
            _ => None,
        }
    }

    /// Binary path suggested for a new project setting.
    #[must_use]
    pub const fn default_binary(self) -> &'static str {
        match self {
            Self::PhpStan => "vendor/bin/phpstan",
            Self::PhpCs => "vendor/bin/phpcs",
            Self::Psalm => "vendor/bin/psalm",
            Self::PhpMd => "vendor/bin/phpmd",
            Self::Golangci => "golangci-lint",
        }
    }
}

/// A linter command found in one terminal line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Detected {
    /// Which tool ran.
    pub kind: LinterKind,
    /// Binary token as the user typed it.
    pub binary: String,
    /// Arguments to reuse, without the JSON format flags the runner forces.
    pub args: String,
    /// Container named by `docker exec`, when the line itself names one.
    pub container: Option<String>,
    /// Working directory from `-w` or from a shell prompt on the same line.
    pub workdir: Option<String>,
}

/// One explicit linter run inside a container.
#[derive(Clone, Debug)]
pub struct LinterJob {
    /// Tool to run.
    pub kind: LinterKind,
    /// Container id or name. Docker accepts either; ids are preferred.
    pub container: String,
    /// Binary inside the container.
    pub binary: String,
    /// Working directory inside the container. Empty keeps the image default.
    pub workdir: String,
    /// Extra arguments, as typed in settings or remembered from the terminal.
    pub args: String,
    /// Host project root used to rewrite reported paths.
    pub project_root: PathBuf,
    /// Mount source/destination pairs for the chosen container.
    pub mounts: Vec<(String, String)>,
}

/// Finds a supported linter in one submitted terminal line.
///
/// The line may include a shell prompt or a `docker exec` prefix. Detection
/// does not require the binary to be the first token.
#[must_use]
pub fn detect(line: &str) -> Option<Detected> {
    let tokens = tokenize(line);
    if tokens.is_empty() {
        return None;
    }
    let (container, flag_workdir, command) =
        split_docker_exec(&tokens).unwrap_or((None, None, tokens));
    let mut found = None;
    for (index, token) in command.iter().enumerate() {
        if let Some(kind) = kind_of_token(token) {
            found = Some((index, kind));
            break;
        }
    }
    let (position, kind) = found?;
    let binary = command[position].clone();
    Some(Detected {
        kind,
        args: shell_join(&remembered_args(kind, &command[position + 1..])),
        binary: binary.clone(),
        container,
        workdir: flag_workdir.or_else(|| prompt_workdir(line, &binary)),
    })
}

/// Docker CLI arguments for one job, including `exec` and the JSON flags.
///
/// # Errors
/// Returns when the container, binary or `PHPMd` paths/ruleset are missing.
pub fn docker_args(job: &LinterJob) -> crate::Result<Vec<String>> {
    Ok(exec_os(job)?
        .into_iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect())
}

/// Runs one linter and maps its JSON onto host paths.
///
/// A nonzero exit with valid JSON is success. The call waits at most 10 minutes.
///
/// # Errors
/// Returns launch, timeout, truncation, configuration and JSON failures.
pub fn run(
    program: &Path,
    cwd: &Path,
    job: &LinterJob,
    stop: &Arc<AtomicBool>,
) -> crate::Result<Vec<DiagnosticItem>> {
    let args = exec_os(job)?;
    let captured = crate::process::capture(program, cwd, &args, stop, Duration::from_secs(600))?;
    interpret(job, captured.code, &captured.stdout, &captured.stderr)
}

/// Maps captured output. Nonzero status with valid JSON yields diagnostics.
///
/// # Errors
/// Returns when the tool produced no JSON or the JSON is not a known report.
pub fn interpret(
    job: &LinterJob,
    code: i32,
    stdout: &str,
    stderr: &str,
) -> crate::Result<Vec<DiagnosticItem>> {
    let label = job.kind.label();
    if stdout.trim().is_empty() {
        if code == 0 && stderr.trim().is_empty() {
            return Ok(Vec::new());
        }
        let detail = snippet(if stderr.trim().is_empty() {
            stdout
        } else {
            stderr
        });
        return Err(crate::Error::Message(format!(
            "{label} failed (exit {code}) before it returned JSON. {detail}"
        )));
    }
    match parse(
        job.kind,
        stdout,
        &job.project_root,
        &job.workdir,
        &job.mounts,
    ) {
        Ok(items) => Ok(items),
        Err(error) => {
            let detail = snippet(stderr);
            let extra = if detail.is_empty() {
                String::new()
            } else {
                format!(" {detail}")
            };
            Err(crate::Error::Message(format!(
                "{label} output could not be parsed (exit {code}): {error}.{extra}"
            )))
        }
    }
}

/// Rewrites a container path into a host path.
///
/// Relative paths join the project root. Absolute paths strip `workdir` first,
/// then the longest matching mount destination.
#[must_use]
pub fn host_path(
    container_path: &str,
    project_root: &Path,
    workdir: &str,
    mounts: &[(String, String)],
) -> PathBuf {
    let path = container_path.trim().replace('\\', "/");
    if path.is_empty() {
        return project_root.to_path_buf();
    }
    if !path.starts_with('/') {
        return join_relative(project_root, &path);
    }
    if let Some(relative) = strip_dir(&path, workdir) {
        return join_relative(project_root, relative);
    }
    let mut best: Option<(usize, &str, &str)> = None;
    for (source, destination) in mounts {
        if let Some(relative) = strip_dir(&path, destination)
            && best
                .as_ref()
                .is_none_or(|(length, _, _)| destination.len() > *length)
        {
            best = Some((destination.len(), source.as_str(), relative));
        }
    }
    best.map_or_else(
        || PathBuf::from(&path),
        |(_, source, relative)| join_relative(Path::new(source), relative),
    )
}

/// Container directory of the mount whose source is the project root.
#[must_use]
pub fn infer_workdir(project_root: &Path, mounts: &[(String, String)]) -> String {
    let mut best: Option<(usize, String)> = None;
    for (source, destination) in mounts {
        if !same_directory(source, project_root) {
            continue;
        }
        let destination = destination.trim().trim_end_matches('/');
        if destination.is_empty() {
            continue;
        }
        if best
            .as_ref()
            .is_none_or(|(length, _)| destination.len() < *length)
        {
            best = Some((destination.len(), destination.to_owned()));
        }
    }
    best.map_or_else(String::new, |(_, destination)| destination)
}

/// Formats diagnostics for an LLM draft. Long reports are truncated.
#[must_use]
pub fn format_context(items: &[DiagnosticItem], project_root: &Path) -> String {
    const MAX_ITEMS: usize = 200;
    const MAX_BYTES: usize = 48 * 1024;
    let mut text = String::from("### Linter Diagnostics Context:\n");
    let mut shown = 0_usize;
    for item in items {
        if shown == MAX_ITEMS || text.len() >= MAX_BYTES {
            break;
        }
        let path = item
            .file_path
            .strip_prefix(project_root)
            .unwrap_or(item.file_path.as_path());
        let location = item.column.map_or_else(
            || format!("Line {}", item.line),
            |column| format!("Line {}, Col {column}", item.line),
        );
        let mut row = format!(
            "- [{}] File: {} ({location}): {}",
            item.linter,
            path.display(),
            item.message
        );
        if let Some(rule) = &item.rule {
            row.push_str(" (");
            row.push_str(rule);
            row.push(')');
        }
        row.push('\n');
        if text.len() + row.len() > MAX_BYTES && shown > 0 {
            break;
        }
        text.push_str(&row);
        shown += 1;
    }
    let hidden = items.len().saturating_sub(shown);
    if hidden > 0 {
        let note = format!("- … {hidden} more diagnostics omitted\n");
        if text.len() + note.len() <= MAX_BYTES {
            text.push_str(&note);
        }
    }
    text
}

fn exec_os(job: &LinterJob) -> crate::Result<Vec<OsString>> {
    let container = job.container.trim();
    let binary = job.binary.trim();
    if container.is_empty() || binary.is_empty() {
        return Err(crate::Error::Message(
            "Linter container and binary are required".into(),
        ));
    }
    let command = command_args(job.kind, binary, &job.args)?;
    let mut args = vec![OsString::from("exec"), OsString::from("-i")];
    let workdir = job.workdir.trim();
    if !workdir.is_empty() {
        args.push(OsString::from("-w"));
        args.push(OsString::from(workdir));
    }
    args.push(OsString::from("--"));
    args.push(OsString::from(container));
    args.extend(command.into_iter().map(OsString::from));
    Ok(args)
}

fn command_args(kind: LinterKind, binary: &str, args: &str) -> crate::Result<Vec<String>> {
    let tokens = tokenize(args);
    let mut command = vec![binary.to_owned()];
    command.extend(match kind {
        LinterKind::PhpStan => phpstan_command(&tokens),
        LinterKind::PhpCs => phpcs_command(&tokens),
        LinterKind::Psalm => psalm_command(&tokens),
        LinterKind::PhpMd => phpmd_command(tokens)?,
        LinterKind::Golangci => golangci_command(&tokens),
    });
    Ok(command)
}

fn phpstan_command(args: &[String]) -> Vec<String> {
    let mut tokens = strip_options(args, &["--error-format"], &["--no-progress"]);
    if tokens.first().map(String::as_str) != Some("analyse") {
        tokens.insert(0, "analyse".into());
    }
    tokens.push("--error-format=json".into());
    tokens.push("--no-progress".into());
    tokens
}

fn phpcs_command(args: &[String]) -> Vec<String> {
    let mut tokens = strip_options(args, &["--report"], &["-q", "--quiet"]);
    tokens.push("--report=json".into());
    tokens.push("-q".into());
    tokens
}

fn psalm_command(args: &[String]) -> Vec<String> {
    let mut tokens = strip_options(args, &["--output-format"], &["--no-progress"]);
    tokens.push("--output-format=json".into());
    tokens.push("--no-progress".into());
    tokens
}

fn phpmd_command(mut tokens: Vec<String>) -> crate::Result<Vec<String>> {
    if tokens.is_empty() || tokens.first().is_some_and(|token| is_meta(token)) {
        return Err(crate::Error::Message(
            "PHPMd needs file paths and a ruleset. Set them in Settings → Linters.".into(),
        ));
    }
    if tokens.len() >= 2 {
        let last = tokens.len() - 1;
        tokens = tokens
            .into_iter()
            .enumerate()
            .filter(|(index, token)| *index == 0 || *index == last || !is_phpmd_format(token))
            .map(|(_, token)| token)
            .collect();
    }
    if tokens.len() < 2 {
        return Err(crate::Error::Message(
            "PHPMd needs file paths and a ruleset. Set them in Settings → Linters.".into(),
        ));
    }
    let Some(ruleset) = tokens.pop() else {
        return Err(crate::Error::Message(
            "PHPMd needs file paths and a ruleset. Set them in Settings → Linters.".into(),
        ));
    };
    tokens.push("json".into());
    tokens.push(ruleset);
    Ok(tokens)
}

fn golangci_command(args: &[String]) -> Vec<String> {
    let mut tokens = strip_options(
        args,
        &["--out-format", "--output.json.path", "--output.text.path"],
        &[],
    );
    if tokens.first().map(String::as_str) != Some("run")
        && !tokens.first().is_some_and(|token| is_meta(token))
    {
        tokens.insert(0, "run".into());
    }
    // v1 prints JSON with --out-format. v2 uses separate output paths.
    if args
        .iter()
        .any(|arg| arg == "--out-format" || arg.starts_with("--out-format="))
    {
        tokens.push("--out-format=json".into());
    } else {
        tokens.push("--output.json.path=stdout".into());
        tokens.push("--output.text.path=/dev/null".into());
    }
    tokens
}

fn remembered_args(kind: LinterKind, args: &[String]) -> Vec<String> {
    if kind == LinterKind::Golangci {
        return remembered_golangci(args);
    }
    if args.first().is_some_and(|token| is_meta(token)) {
        return Vec::new();
    }
    if kind == LinterKind::PhpMd {
        return remembered_phpmd(args);
    }
    let (valued, bare): (&[&str], &[&str]) = match kind {
        LinterKind::PhpStan => (&["--error-format"], &["--no-progress"]),
        LinterKind::PhpCs => (&["--report"], &["-q", "--quiet"]),
        LinterKind::Psalm => (&["--output-format"], &["--no-progress"]),
        LinterKind::PhpMd | LinterKind::Golangci => (&[], &[]),
    };
    strip_options(args, valued, bare)
}

fn remembered_golangci(args: &[String]) -> Vec<String> {
    let mut tokens = args.to_vec();
    if tokens.first().map(String::as_str) == Some("run") {
        tokens.remove(0);
    }
    if tokens.first().is_some_and(|token| is_meta(token)) {
        return Vec::new();
    }
    strip_options(
        &tokens,
        &["--out-format", "--output.json.path", "--output.text.path"],
        &[],
    )
}

fn remembered_phpmd(args: &[String]) -> Vec<String> {
    if args.is_empty() {
        return Vec::new();
    }
    let last = args.len() - 1;
    args.iter()
        .enumerate()
        .filter(|(index, token)| *index == 0 || *index == last || !is_phpmd_format(token))
        .map(|(_, token)| token.clone())
        .collect()
}

fn is_meta(token: &str) -> bool {
    matches!(
        token,
        "--version"
            | "-V"
            | "--help"
            | "-h"
            | "help"
            | "list"
            | "init"
            | "worker"
            | "diagnose"
            | "dump-deps"
            | "clear-result-cache"
    )
}

fn is_phpmd_format(token: &str) -> bool {
    matches!(
        token,
        "text" | "xml" | "json" | "html" | "ansi" | "github" | "sarif" | "checkstyle"
    )
}

fn strip_options(args: &[String], valued: &[&str], bare: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    let mut skip = false;
    for arg in args {
        if skip {
            skip = false;
            continue;
        }
        if bare.iter().any(|flag| arg == flag) {
            continue;
        }
        if valued.iter().any(|flag| arg == flag) {
            skip = true;
            continue;
        }
        if valued
            .iter()
            .any(|flag| arg.starts_with(&format!("{flag}=")))
        {
            continue;
        }
        out.push(arg.clone());
    }
    out
}

fn parse(
    kind: LinterKind,
    stdout: &str,
    project_root: &Path,
    workdir: &str,
    mounts: &[(String, String)],
) -> crate::Result<Vec<DiagnosticItem>> {
    let stdout = stdout.trim_start_matches('\u{feff}').trim();
    let value = json_value(stdout)?;
    let map = MapCtx {
        root: project_root,
        workdir,
        mounts,
    };
    match kind {
        LinterKind::PhpStan => parse_phpstan(&value, &map),
        LinterKind::PhpCs => parse_phpcs(&value, &map),
        LinterKind::Psalm => parse_psalm(&value, &map),
        LinterKind::PhpMd => parse_phpmd(&value, &map),
        LinterKind::Golangci => parse_golangci(&value, &map),
    }
}

struct MapCtx<'a> {
    root: &'a Path,
    workdir: &'a str,
    mounts: &'a [(String, String)],
}

fn parse_phpstan(value: &Value, map: &MapCtx<'_>) -> crate::Result<Vec<DiagnosticItem>> {
    let mut items = Vec::new();
    if let Some(files) = value.get("files") {
        if let Some(object) = files.as_object() {
            for (path, file) in object {
                push_messages(
                    &mut items,
                    LinterKind::PhpStan,
                    path,
                    file,
                    map,
                    phpstan_severity,
                    &["identifier"],
                );
            }
        } else if let Some(list) = files.as_array() {
            for file in list {
                let path = text(file, &["path", "file"]).unwrap_or("");
                push_messages(
                    &mut items,
                    LinterKind::PhpStan,
                    path,
                    file,
                    map,
                    phpstan_severity,
                    &["identifier"],
                );
            }
        }
    }
    if items.is_empty()
        && let Some(errors) = value.get("errors").and_then(Value::as_array)
    {
        let messages: Vec<&str> = errors.iter().filter_map(json_message).collect();
        if !messages.is_empty() {
            return Err(crate::Error::Message(messages.join("; ")));
        }
    }
    Ok(items)
}

fn parse_phpcs(value: &Value, map: &MapCtx<'_>) -> crate::Result<Vec<DiagnosticItem>> {
    let mut items = Vec::new();
    let Some(files) = value.get("files").and_then(Value::as_object) else {
        return Err(crate::Error::Message(
            "PHP_CodeSniffer JSON has no files object".into(),
        ));
    };
    for (path, file) in files {
        push_messages(
            &mut items,
            LinterKind::PhpCs,
            path,
            file,
            map,
            phpcs_severity,
            &["source"],
        );
    }
    Ok(items)
}

fn parse_psalm(value: &Value, map: &MapCtx<'_>) -> crate::Result<Vec<DiagnosticItem>> {
    let list = value
        .as_array()
        .or_else(|| value.get("results").and_then(Value::as_array))
        .or_else(|| value.get("issues").and_then(Value::as_array))
        .ok_or_else(|| crate::Error::Message("Psalm JSON has no issue list".into()))?;
    let mut items = Vec::new();
    for issue in list {
        let Some(path) = text(issue, &["file_path", "file_name", "file"]) else {
            continue;
        };
        push_one(
            &mut items,
            LinterKind::Psalm,
            path,
            issue,
            map,
            psalm_severity(issue),
            text(issue, &["type"]).map(str::to_owned),
        );
    }
    Ok(items)
}

fn parse_phpmd(value: &Value, map: &MapCtx<'_>) -> crate::Result<Vec<DiagnosticItem>> {
    let mut items = Vec::new();
    if let Some(files) = value.get("files").and_then(Value::as_array) {
        for file in files {
            let Some(path) = text(file, &["file"]) else {
                continue;
            };
            let Some(violations) = file.get("violations").and_then(Value::as_array) else {
                continue;
            };
            for violation in violations {
                let severity = phpmd_severity(usize_field(violation, &["priority"]));
                push_one(
                    &mut items,
                    LinterKind::PhpMd,
                    path,
                    violation,
                    map,
                    severity,
                    text(violation, &["rule"]).map(str::to_owned),
                );
            }
        }
    }
    if let Some(errors) = value.get("errors").and_then(Value::as_array) {
        for error in errors {
            let Some(path) = text(error, &["fileName", "file"]) else {
                continue;
            };
            push_one(
                &mut items,
                LinterKind::PhpMd,
                path,
                error,
                map,
                Severity::Error,
                None,
            );
        }
    }
    if items.is_empty() && value.get("files").is_none() {
        return Err(crate::Error::Message("PHPMD JSON has no files".into()));
    }
    Ok(items)
}

fn parse_golangci(value: &Value, map: &MapCtx<'_>) -> crate::Result<Vec<DiagnosticItem>> {
    let issues = value
        .get("Issues")
        .or_else(|| value.get("issues"))
        .and_then(Value::as_array);
    let Some(issues) = issues else {
        let report = value
            .get("Report")
            .or_else(|| value.get("report"))
            .and_then(|report| text(report, &["Error", "error"]))
            .filter(|message| !message.is_empty());
        return Err(crate::Error::Message(
            report
                .unwrap_or("golangci-lint JSON has no Issues")
                .to_owned(),
        ));
    };
    let mut items = Vec::new();
    for issue in issues {
        let pos = issue.get("Pos").or_else(|| issue.get("pos"));
        let filename = pos
            .and_then(|pos| text(pos, &["Filename", "filename"]))
            .or_else(|| text(issue, &["Filename", "filename"]))
            .unwrap_or("");
        if filename.is_empty() {
            continue;
        }
        let line = pos
            .and_then(|pos| usize_field(pos, &["Line", "line"]))
            .or_else(|| usize_field(issue, &["Line", "line"]))
            .filter(|line| *line > 0)
            .unwrap_or(1);
        let column = pos
            .and_then(|pos| usize_field(pos, &["Column", "column"]))
            .or_else(|| usize_field(issue, &["Column", "column"]))
            .filter(|column| *column > 0);
        let message = text(issue, &["Text", "text", "message"]).unwrap_or("golangci-lint issue");
        items.push(DiagnosticItem {
            linter: LinterKind::Golangci.label().to_owned(),
            file_path: host_path(filename, map.root, map.workdir, map.mounts),
            line,
            column,
            message: message.to_owned(),
            severity: golangci_severity(text(issue, &["Severity", "severity"]).unwrap_or("")),
            rule: text(issue, &["FromLinter", "fromLinter", "FromLinter"]).map(str::to_owned),
        });
    }
    Ok(items)
}

fn golangci_severity(text: &str) -> Severity {
    match text.to_ascii_lowercase().as_str() {
        "error" => Severity::Error,
        "info" | "information" => Severity::Info,
        _ => Severity::Warning,
    }
}

fn push_messages(
    items: &mut Vec<DiagnosticItem>,
    kind: LinterKind,
    path: &str,
    file: &Value,
    map: &MapCtx<'_>,
    severity: fn(&Value) -> Severity,
    rule_keys: &[&str],
) {
    let Some(messages) = file.get("messages").and_then(Value::as_array) else {
        return;
    };
    for message in messages {
        push_one(
            items,
            kind,
            path,
            message,
            map,
            severity(message),
            text(message, rule_keys).map(str::to_owned),
        );
    }
}

fn push_one(
    items: &mut Vec<DiagnosticItem>,
    kind: LinterKind,
    path: &str,
    value: &Value,
    map: &MapCtx<'_>,
    severity: Severity,
    rule: Option<String>,
) {
    let Some(message) = text(value, &["message", "description"]) else {
        return;
    };
    if path.is_empty() {
        return;
    }
    items.push(DiagnosticItem {
        linter: kind.label().to_owned(),
        file_path: host_path(path, map.root, map.workdir, map.mounts),
        line: line_of(value),
        column: column_of(value),
        message: message.to_owned(),
        severity,
        rule,
    });
}

const fn phpstan_severity(_value: &Value) -> Severity {
    Severity::Error
}

fn phpcs_severity(value: &Value) -> Severity {
    match text(value, &["type"]).unwrap_or("ERROR") {
        "WARNING" => Severity::Warning,
        "INFO" => Severity::Info,
        _ => Severity::Error,
    }
}

fn psalm_severity(value: &Value) -> Severity {
    match text(value, &["severity"]).unwrap_or("error") {
        "info" => Severity::Info,
        "error" => Severity::Error,
        _ => Severity::Warning,
    }
}

const fn phpmd_severity(priority: Option<usize>) -> Severity {
    match priority {
        Some(1) => Severity::Error,
        Some(2 | 3) | None => Severity::Warning,
        Some(_) => Severity::Info,
    }
}

fn line_of(value: &Value) -> usize {
    usize_field(value, &["line", "beginLine", "line_from"])
        .filter(|line| *line > 0)
        .unwrap_or(1)
}

fn column_of(value: &Value) -> Option<usize> {
    usize_field(value, &["column", "beginColumn", "column_from"]).filter(|column| *column > 0)
}

fn text<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|key| {
        value
            .get(*key)
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
    })
}

fn usize_field(value: &Value, keys: &[&str]) -> Option<usize> {
    keys.iter().find_map(|key| {
        value
            .get(*key)
            .and_then(Value::as_u64)
            .and_then(|number| usize::try_from(number).ok())
    })
}

fn json_message(value: &Value) -> Option<&str> {
    value.as_str().or_else(|| text(value, &["message"]))
}

fn json_value(stdout: &str) -> crate::Result<Value> {
    if let Ok(value) = serde_json::from_str(stdout) {
        return Ok(value);
    }
    let start = stdout
        .find(['{', '['])
        .ok_or_else(|| crate::Error::Message("output is not JSON".into()))?;
    serde_json::from_str(&stdout[start..])
        .map_err(|error| crate::Error::Message(format!("invalid JSON: {error}")))
}

fn snippet(text: &str) -> String {
    let trimmed = text.trim();
    let mut out: String = trimmed.chars().take(500).collect();
    if trimmed.chars().count() > 500 {
        out.push('…');
    }
    out
}

fn kind_of_token(token: &str) -> Option<LinterKind> {
    let name = token.rsplit(['/', '\\']).next().unwrap_or(token);
    let name = name
        .strip_suffix(".phar")
        .or_else(|| name.strip_suffix(".bat"))
        .or_else(|| name.strip_suffix(".cmd"))
        .unwrap_or(name);
    match name {
        "phpstan" => Some(LinterKind::PhpStan),
        "phpcs" => Some(LinterKind::PhpCs),
        "psalm" => Some(LinterKind::Psalm),
        "phpmd" => Some(LinterKind::PhpMd),
        "golangci-lint" => Some(LinterKind::Golangci),
        _ => None,
    }
}

pub(crate) fn split_docker_exec(
    tokens: &[String],
) -> Option<(Option<String>, Option<String>, Vec<String>)> {
    let docker_at = tokens.iter().position(|token| is_docker(token))?;
    let after = tokens.get(docker_at + 1..)?;
    if after.first().map(String::as_str) != Some("exec") {
        return None;
    }
    let mut rest = after.get(1..).unwrap_or(&[]);
    let mut workdir = None;
    let valued = [
        "-w",
        "--workdir",
        "-u",
        "--user",
        "-e",
        "--env",
        "--env-file",
        "--entrypoint",
    ];
    while let Some(token) = rest.first() {
        if token == "--" {
            rest = rest.get(1..).unwrap_or(&[]);
            break;
        }
        if let Some(path) = token.strip_prefix("--workdir=") {
            workdir = Some(path.to_owned());
            rest = rest.get(1..).unwrap_or(&[]);
            continue;
        }
        if let Some(path) = token.strip_prefix("-w").filter(|path| !path.is_empty()) {
            workdir = Some((*path).to_owned());
            rest = rest.get(1..).unwrap_or(&[]);
            continue;
        }
        if valued.iter().any(|flag| token == flag) {
            if token == "-w" || token == "--workdir" {
                workdir = rest.get(1).cloned();
            }
            rest = rest.get(2..).unwrap_or(&[]);
            continue;
        }
        if token.starts_with('-') {
            rest = rest.get(1..).unwrap_or(&[]);
            continue;
        }
        break;
    }
    let container = rest.first().cloned()?;
    let command = rest.get(1..).unwrap_or(&[]).to_vec();
    Some((Some(container), workdir, command))
}

fn is_docker(token: &str) -> bool {
    let name = token.rsplit(['/', '\\']).next().unwrap_or(token);
    name == "docker" || name == "docker.exe"
}

pub(crate) fn prompt_workdir(line: &str, binary: &str) -> Option<String> {
    let index = line.find(binary)?;
    let prefix = &line[..index];
    let mut found = None;
    for (offset, character) in prefix.char_indices() {
        if character != ':' {
            continue;
        }
        let rest = &prefix[offset + character.len_utf8()..];
        let Some(path) = rest.strip_prefix('/') else {
            continue;
        };
        let end = path.find([' ', '#', '$', '%', '\t']).unwrap_or(path.len());
        let full = format!("/{}", &path[..end])
            .trim_end_matches('/')
            .to_owned();
        if full != "/" {
            found = Some(full);
        }
    }
    found
}

pub(crate) fn tokenize(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    for character in line.chars() {
        if let Some(mark) = quote {
            if character == mark {
                quote = None;
            } else {
                current.push(character);
            }
            continue;
        }
        if character == '\'' || character == '"' {
            quote = Some(character);
            continue;
        }
        if character.is_whitespace() {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            continue;
        }
        current.push(character);
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

pub(crate) fn shell_join(args: &[String]) -> String {
    args.iter()
        .map(|arg| {
            if arg.chars().any(char::is_whitespace) {
                format!("\"{arg}\"")
            } else {
                arg.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn join_relative(root: &Path, relative: &str) -> PathBuf {
    let mut out = root.to_path_buf();
    for part in relative.split(['/', '\\']) {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            out.pop();
            continue;
        }
        out.push(part);
    }
    out
}

fn strip_dir<'a>(path: &'a str, directory: &str) -> Option<&'a str> {
    let directory = directory.trim().trim_end_matches('/');
    if directory.is_empty() {
        return None;
    }
    let path = path.trim_end_matches('/');
    if path == directory {
        return Some("");
    }
    path.strip_prefix(&format!("{directory}/"))
}

fn same_directory(source: &str, project_root: &Path) -> bool {
    let source_path = Path::new(source);
    if source_path == project_root {
        return true;
    }
    matches!(
        (source_path.canonicalize(), project_root.canonicalize()),
        (Ok(left), Ok(right)) if left == right
    )
}

#[cfg(test)]
mod tests {
    use super::{
        DiagnosticItem, LinterJob, LinterKind, Severity, detect, docker_args, format_context,
        host_path, infer_workdir, interpret,
    };
    use std::path::Path;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    fn job(kind: LinterKind, args: &str) -> LinterJob {
        LinterJob {
            kind,
            container: "php-fpm".into(),
            binary: kind.default_binary().into(),
            workdir: "/var/www/html".into(),
            args: args.into(),
            project_root: Path::new("/work/nozhove").into(),
            mounts: vec![("/work/nozhove".into(), "/var/www/html".into())],
        }
    }

    #[test]
    fn detection_keeps_analysis_arguments_and_container_prompt() -> Result<(), &'static str> {
        let detected = detect("root@box:/var/www/html# vendor/bin/phpstan analyse src --level=8")
            .ok_or("phpstan")?;
        assert_eq!(detected.kind, LinterKind::PhpStan);
        assert_eq!(detected.binary, "vendor/bin/phpstan");
        assert_eq!(detected.args, "analyse src --level=8");
        assert_eq!(detected.workdir.as_deref(), Some("/var/www/html"));
        assert!(detected.container.is_none());

        let exec = detect(
            "docker exec -i -w /var/www/html php-fpm-nozhovecom vendor/bin/phpcs src --standard=PSR12",
        )
        .ok_or("phpcs")?;
        assert_eq!(exec.kind, LinterKind::PhpCs);
        assert_eq!(exec.container.as_deref(), Some("php-fpm-nozhovecom"));
        assert_eq!(exec.workdir.as_deref(), Some("/var/www/html"));
        assert_eq!(exec.args, "src --standard=PSR12");

        let psalm = detect("vendor/bin/psalm --no-cache").ok_or("psalm")?;
        assert_eq!(psalm.kind, LinterKind::Psalm);
        assert_eq!(psalm.args, "--no-cache");

        let phpmd = detect(
            "docker exec -i -e ICECMS_ENV=local php-fpm-nozhovecom php vendor/bin/phpmd classes,controllers text ruleset.xml",
        )
        .ok_or("phpmd")?;
        assert_eq!(phpmd.kind, LinterKind::PhpMd);
        assert_eq!(phpmd.binary, "vendor/bin/phpmd");
        assert_eq!(phpmd.args, "classes,controllers ruleset.xml");
        assert_eq!(phpmd.container.as_deref(), Some("php-fpm-nozhovecom"));
        assert!(detect("composer install").is_none());
        assert!(detect("vendor/bin/phpstan --version").is_some_and(|d| d.args.is_empty()));
        Ok(())
    }

    #[test]
    fn docker_arguments_force_json_and_keep_user_paths() -> Result<(), String> {
        let stan = docker_args(&job(LinterKind::PhpStan, "analyse src --level=8"))
            .map_err(|error| error.to_string())?;
        assert_eq!(
            stan,
            vec![
                "exec",
                "-i",
                "-w",
                "/var/www/html",
                "--",
                "php-fpm",
                "vendor/bin/phpstan",
                "analyse",
                "src",
                "--level=8",
                "--error-format=json",
                "--no-progress",
            ]
        );
        let phpcs = docker_args(&job(LinterKind::PhpCs, "--report=checkstyle app"))
            .map_err(|error| error.to_string())?;
        assert!(phpcs.iter().any(|arg| arg == "--report=json"));
        assert!(!phpcs.iter().any(|arg| arg.contains("checkstyle")));
        assert!(phpcs.iter().any(|arg| arg == "app"));
        let psalm = docker_args(&job(LinterKind::Psalm, "")).map_err(|error| error.to_string())?;
        assert!(psalm.iter().any(|arg| arg == "--output-format=json"));
        let phpmd = docker_args(&job(LinterKind::PhpMd, "classes,controllers ruleset.xml"))
            .map_err(|error| error.to_string())?;
        assert_eq!(
            &phpmd[phpmd.len() - 4..],
            [
                "vendor/bin/phpmd",
                "classes,controllers",
                "json",
                "ruleset.xml",
            ]
        );
        assert!(docker_args(&job(LinterKind::PhpMd, "")).is_err());
        Ok(())
    }

    #[test]
    fn reports_map_paths_and_a_nonzero_exit_is_still_a_report() -> Result<(), String> {
        let root = Path::new("/work/nozhove");
        let stan = r#"{"totals":{"errors":0,"file_errors":1},"files":{"/var/www/html/app/User.php":{"errors":1,"messages":[{"message":"Call to an undefined method App\\Models\\User::charge()","line":48,"identifier":"method.notFound"}]}},"errors":[]}"#;
        let items = interpret(&job(LinterKind::PhpStan, ""), 1, stan, "")
            .map_err(|error| error.to_string())?;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].linter, "PHPStan");
        assert_eq!(items[0].file_path, root.join("app/User.php"));
        assert_eq!(items[0].line, 48);
        assert_eq!(items[0].rule.as_deref(), Some("method.notFound"));
        assert_eq!(items[0].severity, Severity::Error);

        let phpcs = r#"{"totals":{"errors":1,"warnings":0,"fixable":0},"files":{"/var/www/html/app/Services/PaymentService.php":{"errors":1,"warnings":0,"messages":[{"message":"Missing function doc comment","source":"PEAR.Commenting.FunctionComment.Missing","severity":5,"type":"ERROR","line":12,"column":1}]}}}"#;
        let items = interpret(&job(LinterKind::PhpCs, ""), 2, phpcs, "")
            .map_err(|error| error.to_string())?;
        assert_eq!(items[0].column, Some(1));
        assert_eq!(
            items[0].rule.as_deref(),
            Some("PEAR.Commenting.FunctionComment.Missing")
        );

        let psalm = r#"[{"severity":"error","line_from":10,"column_from":4,"type":"InvalidReturnType","message":"Bad return","file_path":"/var/www/html/src/Foo.php"}]"#;
        let items = interpret(&job(LinterKind::Psalm, ""), 2, psalm, "")
            .map_err(|error| error.to_string())?;
        assert_eq!(items[0].file_path, root.join("src/Foo.php"));
        assert_eq!(items[0].line, 10);
        assert_eq!(items[0].column, Some(4));

        let phpmd = r#"{"package":"phpmd","files":[{"file":"/var/www/html/classes/Foo.php","violations":[{"beginLine":9,"description":"Avoid unused private methods such as 'run'.","rule":"UnusedPrivateMethod","priority":3}]}],"errors":[{"fileName":"/var/www/html/classes/Bad.php","message":"Could not parse"}]}"#;
        let items = interpret(&job(LinterKind::PhpMd, ""), 2, phpmd, "")
            .map_err(|error| error.to_string())?;
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].severity, Severity::Warning);
        assert_eq!(items[0].file_path, root.join("classes/Foo.php"));
        assert_eq!(items[1].severity, Severity::Error);

        let fatal = r#"{"totals":{"errors":1,"file_errors":0},"files":{},"errors":["Invalid configuration"]}"#;
        let error = interpret(&job(LinterKind::PhpStan, ""), 1, fatal, "")
            .err()
            .ok_or("expected PHPStan configuration failure")?;
        assert!(
            error.to_string().contains("Invalid configuration"),
            "{error}"
        );
        let garbage = interpret(
            &job(LinterKind::PhpCs, ""),
            1,
            "not json",
            "ruleset missing",
        )
        .err()
        .ok_or("expected invalid JSON")?;
        assert!(
            garbage.to_string().contains("could not be parsed"),
            "{garbage}"
        );
        assert!(garbage.to_string().contains("ruleset missing"), "{garbage}");
        Ok(())
    }

    #[test]
    fn path_fallbacks_and_context_use_the_host_project() {
        let root = Path::new("/work/nozhove");
        assert_eq!(
            host_path("classes/Foo.php", root, "/var/www/html", &[]),
            root.join("classes/Foo.php")
        );
        assert_eq!(
            host_path(
                "/srv/app/classes/Foo.php",
                root,
                "/srv/app",
                &[("/work/nozhove".into(), "/srv/app".into())]
            ),
            root.join("classes/Foo.php")
        );
        assert_eq!(
            infer_workdir(root, &[("/work/nozhove".into(), "/var/www/html/".into())]),
            "/var/www/html"
        );
        let item = DiagnosticItem {
            linter: "PHPMD".into(),
            file_path: root.join("classes/Foo.php"),
            line: 9,
            column: None,
            message: "Avoid unused private methods".into(),
            severity: Severity::Warning,
            rule: Some("UnusedPrivateMethod".into()),
        };
        let text = format_context(&[item], root);
        assert!(text.contains("### Linter Diagnostics Context:"));
        assert!(text.contains("[PHPMD] File: classes/Foo.php (Line 9): Avoid unused private methods (UnusedPrivateMethod)"));
    }

    #[test]
    fn capture_keeps_stdout_when_the_child_exits_nonzero() -> Result<(), String> {
        let program = if cfg!(windows) { "cmd" } else { "sh" };
        let args: Vec<std::ffi::OsString> = if cfg!(windows) {
            ["/C", "echo hello&& exit /b 2"]
                .into_iter()
                .map(std::ffi::OsString::from)
                .collect()
        } else {
            ["-c", "echo hello; exit 2"]
                .into_iter()
                .map(std::ffi::OsString::from)
                .collect()
        };
        let stop = Arc::new(AtomicBool::new(false));
        let captured = crate::process::capture(
            Path::new(program),
            Path::new("."),
            &args,
            &stop,
            std::time::Duration::from_secs(10),
        )
        .map_err(|error| error.to_string())?;
        assert_eq!(captured.code, 2);
        assert!(captured.stdout.contains("hello"), "{}", captured.stdout);
        stop.store(true, Ordering::Relaxed);
        assert!(
            crate::process::capture(
                Path::new(program),
                Path::new("."),
                &args,
                &stop,
                std::time::Duration::from_secs(10),
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn golangci_lint_json_maps_container_paths() -> Result<(), String> {
        let detected = detect("root@box:/src# golangci-lint run ./... --output.json.path=stdout")
            .ok_or("detect")?;
        assert_eq!(detected.kind, LinterKind::Golangci);
        assert_eq!(detected.binary, "golangci-lint");
        assert_eq!(detected.args, "./...");
        assert_eq!(detected.workdir.as_deref(), Some("/src"));
        assert!(detect("golangci-lint --version").is_some_and(|found| found.args.is_empty()));
        assert!(detect("go test ./...").is_none());
        let args =
            docker_args(&job(LinterKind::Golangci, "./...")).map_err(|error| error.to_string())?;
        assert!(args.iter().any(|arg| arg == "run"));
        assert!(args.iter().any(|arg| arg == "--output.json.path=stdout"));
        assert!(args.iter().any(|arg| arg == "--output.text.path=/dev/null"));
        let v1 = docker_args(&job(LinterKind::Golangci, "--out-format json ./..."))
            .map_err(|error| error.to_string())?;
        assert!(v1.iter().any(|arg| arg == "--out-format=json"));
        assert!(!v1.iter().any(|arg| arg.contains("output.json.path")));
        let report = r#"{"Issues":[{"FromLinter":"govet","Text":"unused","Severity":"","Pos":{"Filename":"/var/www/html/main.go","Line":4,"Column":2}}]}"#;
        let items = interpret(&job(LinterKind::Golangci, "./..."), 1, report, "")
            .map_err(|error| error.to_string())?;
        let item = items.first().ok_or("issue")?;
        assert_eq!(item.file_path, Path::new("/work/nozhove/main.go"));
        assert_eq!(item.line, 4);
        assert_eq!(item.column, Some(2));
        assert_eq!(item.rule.as_deref(), Some("govet"));
        assert_eq!(item.severity, Severity::Warning);
        let empty = r#"{"Issues":[]}"#;
        assert_eq!(
            interpret(&job(LinterKind::Golangci, ""), 0, empty, "")
                .map_err(|error| error.to_string())?,
            Vec::<DiagnosticItem>::new()
        );
        let broken = r#"{"Report":{"Error":"config failed"}}"#;
        assert!(interpret(&job(LinterKind::Golangci, ""), 3, broken, "").is_err());
        Ok(())
    }
}
