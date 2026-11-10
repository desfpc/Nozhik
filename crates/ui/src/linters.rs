//! Project linter runs, settings and problem tabs.
//!
//! A discovered command is stored only when that linter kind is not already
//! configured. The run uses its own thread: a linter can take minutes and exits
//! nonzero when it finds issues, so it must not occupy the Compose worker.

use nozhik_buffer::{Cursor, TextBuffer};
use nozhik_config::{Config, languages::LanguageId, linters::LinterTool};
use nozhik_docker::linter::{
    DiagnosticItem, LinterJob, LinterKind, Severity, detect, format_context, infer_workdir,
};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};

const PHP_KINDS: [&str; 4] = ["phpstan", "phpcs", "psalm", "phpmd"];
const GO_KINDS: [&str; 1] = ["golangci-lint"];

const fn kinds_for(language: LanguageId) -> &'static [&'static str] {
    match language {
        LanguageId::Php => &PHP_KINDS,
        LanguageId::Go => &GO_KINDS,
        _ => &[],
    }
}

/// One Compose container the linter can target.
#[derive(Clone, Debug)]
pub struct KnownContainer {
    /// Full container id.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Mount source and destination.
    pub mounts: Vec<(String, String)>,
}

/// One gutter mark for a host line. `line` is zero-based; `column` is one-based.
#[derive(Clone, Debug)]
pub struct LinterMark {
    pub line: usize,
    pub column: Option<usize>,
    pub linter: String,
    pub message: String,
    pub rule: Option<String>,
    pub severity: Severity,
}

/// Preformatted diagnostics for the active agent draft.
#[derive(Clone, Debug, Default)]
pub struct Attach {
    pub file: Option<String>,
    pub all: Option<String>,
}

/// Failures from one finished run. Diagnostics stay on [`LinterUi`].
pub struct RunFinished {
    pub failures: Vec<(String, String)>,
}

struct Report {
    generation: u64,
    items: Vec<DiagnosticItem>,
    failures: Vec<(String, String)>,
}

#[derive(Clone)]
struct Jump {
    path: PathBuf,
    line: usize,
    column: Option<usize>,
    opened: bool,
}

/// Per-project linter results and the background run.
pub struct LinterUi {
    running: bool,
    items: Vec<DiagnosticItem>,
    ran: bool,
    tab: String,
    jump: Option<Jump>,
    cancel: Option<Arc<AtomicBool>>,
    generation: u64,
    inbox: mpsc::Receiver<Report>,
    sender: mpsc::Sender<Report>,
}

impl LinterUi {
    pub(crate) fn new() -> Self {
        let (sender, inbox) = mpsc::channel();
        Self {
            running: false,
            items: Vec::new(),
            ran: false,
            tab: String::new(),
            jump: None,
            cancel: None,
            generation: 0,
            inbox,
            sender,
        }
    }

    pub(crate) fn shutdown(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
    }

    pub(crate) const fn running(&self) -> bool {
        self.running
    }

    pub(crate) const fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub(crate) const fn len(&self) -> usize {
        self.items.len()
    }

    /// Starts every configured linter. A newer click cancels the previous run.
    ///
    /// # Errors
    /// Returns when the operating system refuses the worker thread.
    pub(crate) fn start(
        &mut self,
        ctx: &egui::Context,
        config: &Config,
        root: &Path,
        containers: &[KnownContainer],
    ) -> Result<(), String> {
        if self.running || config.linters.is_empty() {
            return Ok(());
        }
        let jobs = jobs(config, root, containers);
        if jobs.is_empty() {
            return Err("Configure a supported PHP linter before running it".into());
        }
        self.generation = self.generation.saturating_add(1);
        let generation = self.generation;
        let cancel = Arc::new(AtomicBool::new(false));
        if let Some(previous) = self.cancel.replace(Arc::clone(&cancel)) {
            previous.store(true, Ordering::Relaxed);
        }
        let sender = self.sender.clone();
        let context = ctx.clone();
        let program = nozhik_docker::docker_program();
        let root = root.to_path_buf();
        std::thread::Builder::new()
            .name("nozhik-linter".into())
            .spawn(move || {
                let mut items = Vec::new();
                let mut failures = Vec::new();
                for job in jobs {
                    if cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    match nozhik_docker::linter::run(&program, &root, &job, &cancel) {
                        Ok(found) => items.extend(found.into_iter().map(canonicalize_item)),
                        Err(error) => failures.push((job.kind.id().to_owned(), error.to_string())),
                    }
                }
                let _sent = sender.send(Report {
                    generation,
                    items,
                    failures,
                });
                context.request_repaint();
            })
            .map_err(|error| error.to_string())?;
        self.running = true;
        Ok(())
    }

    /// Applies the newest report for the current click and drops a stale one.
    pub(crate) fn poll(&mut self) -> Option<RunFinished> {
        let mut finished = None;
        while let Ok(report) = self.inbox.try_recv() {
            if report.generation != self.generation {
                continue;
            }
            self.items = report.items;
            self.ran = true;
            self.running = false;
            self.cancel = None;
            finished = Some(RunFinished {
                failures: report.failures,
            });
        }
        finished
    }

    pub(crate) fn marks(&self, path: &Path) -> Vec<LinterMark> {
        self.items
            .iter()
            .filter(|item| item.file_path == path)
            .map(|item| LinterMark {
                line: item.line.saturating_sub(1),
                column: item.column,
                linter: item.linter.clone(),
                message: item.message.clone(),
                rule: item.rule.clone(),
                severity: item.severity,
            })
            .collect()
    }

    pub(crate) fn attach(&self, root: Option<&Path>, file: Option<&Path>) -> Attach {
        let Some(root) = root else {
            return Attach::default();
        };
        let all = (!self.items.is_empty()).then(|| format_context(&self.items, root));
        let file = file.and_then(|path| {
            let rows: Vec<_> = self
                .items
                .iter()
                .filter(|item| item.file_path == path)
                .cloned()
                .collect();
            (!rows.is_empty()).then(|| format_context(&rows, root))
        });
        Attach { file, all }
    }

    pub(crate) fn queue_jump(&mut self, item: &DiagnosticItem) {
        self.jump = Some(Jump {
            path: item.file_path.clone(),
            line: item.line,
            column: item.column,
            opened: false,
        });
    }

    /// Places the cursor when the target document is already open.
    ///
    /// Returns the zero-based line and the buffer to focus.
    pub(crate) fn take_jump(
        &mut self,
        documents: &mut [crate::Document],
    ) -> Result<Option<(usize, nozhik_core::BufferId)>, String> {
        let Some(jump) = self.jump.clone() else {
            return Ok(None);
        };
        let Some(document) = documents
            .iter_mut()
            .find(|document| document.path == jump.path)
        else {
            return Ok(None);
        };
        if document.binary {
            self.jump = None;
            return Err("Linter target is a binary file".into());
        }
        let line = match place_cursor(&mut document.buffer, jump.line, jump.column) {
            Ok(line) => line,
            Err(error) => {
                self.jump = None;
                return Err(error);
            }
        };
        let buffer = document.buffer.snapshot().buffer_id;
        self.jump = None;
        Ok(Some((line, buffer)))
    }

    pub(crate) fn jump_request(&self) -> Option<&Path> {
        self.jump
            .as_ref()
            .filter(|jump| !jump.opened)
            .map(|jump| jump.path.as_path())
    }

    pub(crate) const fn mark_jump_opened(&mut self) {
        if let Some(jump) = &mut self.jump {
            jump.opened = true;
        }
    }

    /// Drops a jump whose host path is not a file and returns that path.
    pub(crate) fn take_missing_jump(&mut self) -> Option<PathBuf> {
        if self.jump.as_ref().is_some_and(|jump| !jump.path.is_file()) {
            self.jump.take().map(|jump| jump.path)
        } else {
            None
        }
    }

    pub(crate) fn show_tabs(
        &mut self,
        ui: &mut egui::Ui,
        tools: &[LinterTool],
        root: Option<&Path>,
    ) -> Option<DiagnosticItem> {
        if tools.is_empty() && self.items.is_empty() {
            return None;
        }
        ui.add_space(8.0);
        ui.heading("Linters");
        if self.running {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Running PHP linters…");
            });
        }
        if tools.is_empty() {
            ui.weak("No PHP linter is configured.");
            return None;
        }
        if !tools.iter().any(|tool| tool.kind == self.tab)
            && let Some(tool) = tools.first()
        {
            self.tab.clone_from(&tool.kind);
        }
        ui.horizontal_wrapped(|ui| {
            for tool in tools {
                let label = kind_label(&tool.kind);
                let count = self
                    .items
                    .iter()
                    .filter(|item| item.linter == label)
                    .count();
                if ui
                    .selectable_label(self.tab == tool.kind, format!("{label} ({count})"))
                    .clicked()
                {
                    self.tab.clone_from(&tool.kind);
                }
            }
        });
        let label = kind_label(&self.tab);
        let rows: Vec<DiagnosticItem> = self
            .items
            .iter()
            .filter(|item| item.linter == label)
            .cloned()
            .collect();
        if !self.ran {
            ui.weak("Not run yet.");
            return None;
        }
        if rows.is_empty() {
            ui.weak("No problems.");
            return None;
        }
        let mut clicked = None;
        let mut files = Vec::new();
        for item in &rows {
            if !files.contains(&item.file_path) {
                files.push(item.file_path.clone());
            }
        }
        for path in files {
            let group: Vec<_> = rows
                .iter()
                .filter(|item| item.file_path == path)
                .cloned()
                .collect();
            ui.strong(format!("{} ({})", display_path(&path, root), group.len()));
            for item in group {
                ui.horizontal(|ui| {
                    ui.add_space(12.0);
                    let response = ui.selectable_label(false, row_label(&item));
                    if response.clicked() {
                        clicked = Some(item.clone());
                    }
                    if let Some(rule) = &item.rule {
                        response.on_hover_text(rule);
                    }
                });
            }
            ui.add_space(4.0);
        }
        clicked
    }
}

impl Drop for LinterUi {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Remembers a terminal command when that linter kind is not configured yet.
pub fn remember(
    config: &mut Config,
    root: &Path,
    line: &str,
    session: Option<(&str, &str)>,
    containers: &[KnownContainer],
) -> bool {
    let Some(found) = detect(line) else {
        return false;
    };
    if config
        .linters
        .iter()
        .any(|tool| tool.kind == found.kind.id())
    {
        return false;
    }
    let Some(container) = attached_container(found.container.as_deref(), session, containers)
    else {
        return false;
    };
    let workdir = found
        .workdir
        .clone()
        .unwrap_or_else(|| infer_workdir(root, &container.mounts));
    let tool = LinterTool {
        kind: found.kind.id().to_owned(),
        container_id: container.id,
        container_name: container.name,
        binary_path: found.binary,
        workdir,
        args: found.args,
    };
    let mut tools = config.linters.clone();
    tools.push(tool);
    if nozhik_config::linters::validate(&tools).is_err() {
        return false;
    }
    config.linters = tools;
    true
}

pub fn blank_draft(kind: &str) -> LinterTool {
    LinterTool {
        kind: kind.to_owned(),
        container_id: String::new(),
        container_name: String::new(),
        binary_path: LinterKind::parse(kind)
            .map_or(String::new(), |kind| kind.default_binary().to_owned()),
        workdir: String::new(),
        args: String::new(),
    }
}

pub fn show_settings(
    ui: &mut egui::Ui,
    config: &mut Config,
    containers: &[(String, String)],
    language: LanguageId,
    draft: &mut LinterTool,
) {
    let kinds = kinds_for(language);
    if kinds.is_empty() {
        ui.label("No linters are available for this language.");
        return;
    }
    ui.label(if language == LanguageId::Go {
        "golangci-lint runs inside a Docker container when you press the bug button."
    } else {
        "PHP linters run inside a Docker container when you press the bug button."
    });
    let mut remove = None;
    for index in 0..config.linters.len() {
        let kind = config.linters[index].kind.clone();
        if !kinds.contains(&kind.as_str()) {
            continue;
        }
        ui.separator();
        ui.strong(kind_label(&kind));
        {
            let tool = &mut config.linters[index];
            container_picker(
                ui,
                &mut tool.container_id,
                &mut tool.container_name,
                containers,
                ("linter_container", index),
            );
        }
        ui.label("Binary");
        ui.add(
            crate::form::singleline(&mut config.linters[index].binary_path)
                .hint_text("vendor/bin/phpstan")
                .desired_width(ui.available_width()),
        );
        ui.label("Container workdir");
        ui.add(
            crate::form::singleline(&mut config.linters[index].workdir)
                .hint_text("/var/www/html")
                .desired_width(ui.available_width()),
        );
        ui.label("Arguments");
        ui.add(
            crate::form::singleline(&mut config.linters[index].args)
                .hint_text(args_hint(&kind))
                .desired_width(ui.available_width()),
        );
        if ui.button("Remove").clicked() {
            remove = Some(index);
        }
    }
    if let Some(index) = remove {
        config.linters.remove(index);
    }
    show_new_linter(ui, config, containers, kinds, draft);
}

fn show_new_linter(
    ui: &mut egui::Ui,
    config: &mut Config,
    containers: &[(String, String)],
    kinds: &'static [&'static str],
    draft: &mut LinterTool,
) {
    let available = unused(config, kinds);
    if available.is_empty() {
        return;
    }
    if !available.contains(&draft.kind.as_str())
        && let Some(kind) = available.first()
    {
        *draft = blank_draft(kind);
    }
    ui.separator();
    ui.strong("Add linter");
    egui::ComboBox::from_id_salt("linter_draft_kind")
        .selected_text(kind_label(&draft.kind))
        .show_ui(ui, |ui| {
            for kind in &available {
                if ui
                    .selectable_label(draft.kind == *kind, kind_label(kind))
                    .clicked()
                {
                    let keep_binary = LinterKind::parse(&draft.kind)
                        .is_some_and(|parsed| draft.binary_path != parsed.default_binary())
                        && !draft.binary_path.trim().is_empty();
                    (*kind).clone_into(&mut draft.kind);
                    if !keep_binary && let Some(parsed) = LinterKind::parse(kind) {
                        parsed.default_binary().clone_into(&mut draft.binary_path);
                    }
                }
            }
        });
    container_picker(
        ui,
        &mut draft.container_id,
        &mut draft.container_name,
        containers,
        "linter_draft_container",
    );
    ui.label("Binary");
    ui.add(crate::form::singleline(&mut draft.binary_path).desired_width(ui.available_width()));
    ui.label("Container workdir");
    ui.add(
        crate::form::singleline(&mut draft.workdir)
            .hint_text("/var/www/html")
            .desired_width(ui.available_width()),
    );
    ui.label("Arguments");
    ui.add(
        crate::form::singleline(&mut draft.args)
            .hint_text(args_hint(&draft.kind))
            .desired_width(ui.available_width()),
    );
    let ready = !draft.container_id.trim().is_empty() && !draft.binary_path.trim().is_empty();
    if ui
        .add_enabled(ready, egui::Button::new("Add"))
        .on_hover_text("Save this linter for the project")
        .clicked()
    {
        let mut tools = config.linters.clone();
        tools.push(draft.clone());
        if nozhik_config::linters::validate(&tools).is_ok() {
            config.linters = tools;
            if let Some(kind) = unused(config, kinds).first() {
                *draft = blank_draft(kind);
            }
        }
    }
}

/// Places a one-based linter location. The returned line is zero-based.
///
/// # Errors
/// Returns when the line or column does not exist in the buffer.
pub fn place_cursor(
    buffer: &mut TextBuffer,
    line: usize,
    column: Option<usize>,
) -> Result<usize, String> {
    if line == 0 {
        return Err("Linter line must be 1-based".into());
    }
    let index = line - 1;
    let rope = buffer.rope();
    if index >= rope.len_lines() {
        return Err(format!("Line {line} is past the end of the file"));
    }
    let slice = rope.line(index);
    let mut limit = slice.len_chars();
    if limit > 0 && matches!(slice.char(limit - 1), '\n' | '\r') {
        limit -= 1;
        if limit > 0 && slice.char(limit - 1) == '\r' {
            limit -= 1;
        }
    }
    let column = column.unwrap_or(1).saturating_sub(1).min(limit);
    let char_index = rope.line_to_char(index) + column;
    let byte = rope
        .try_char_to_byte(char_index)
        .map_err(|_| "Linter column is outside the line".to_owned())?;
    let head = buffer
        .position_at(byte)
        .map_err(|error| error.to_string())?;
    buffer
        .set_cursors(vec![Cursor { anchor: head, head }])
        .map_err(|error| error.to_string())?;
    Ok(index)
}

fn jobs(config: &Config, root: &Path, containers: &[KnownContainer]) -> Vec<LinterJob> {
    config
        .linters
        .iter()
        .filter_map(|tool| {
            let kind = LinterKind::parse(&tool.kind)?;
            let mounts = containers
                .iter()
                .find(|container| {
                    container.id == tool.container_id || container.name == tool.container_name
                })
                .map(|container| container.mounts.clone())
                .unwrap_or_default();
            let mut workdir = tool.workdir.clone();
            if workdir.trim().is_empty() {
                workdir = infer_workdir(root, &mounts);
            }
            Some(LinterJob {
                kind,
                container: tool.container_id.clone(),
                binary: tool.binary_path.clone(),
                workdir,
                args: tool.args.clone(),
                project_root: root.to_path_buf(),
                mounts,
            })
        })
        .collect()
}

fn canonicalize_item(mut item: DiagnosticItem) -> DiagnosticItem {
    if let Ok(path) = item.file_path.canonicalize() {
        item.file_path = path;
    }
    item
}

pub fn attached_container(
    named: Option<&str>,
    session: Option<(&str, &str)>,
    containers: &[KnownContainer],
) -> Option<KnownContainer> {
    if let Some(token) = named.map(str::trim).filter(|token| !token.is_empty()) {
        return Some(resolve_container(token, containers));
    }
    let (id, name) = session.filter(|(id, _)| !id.trim().is_empty())?;
    let mounts = containers
        .iter()
        .find(|container| container.id == id || container.name == name)
        .map(|container| container.mounts.clone())
        .unwrap_or_default();
    Some(KnownContainer {
        id: id.to_owned(),
        name: name.to_owned(),
        mounts,
    })
}

fn resolve_container(token: &str, containers: &[KnownContainer]) -> KnownContainer {
    let token = token.trim();
    if let Some(container) = containers
        .iter()
        .find(|container| container.id == token || container.name == token)
    {
        return container.clone();
    }
    if token.len() >= 12
        && let Some(container) = containers
            .iter()
            .find(|container| container.id.starts_with(token))
    {
        return container.clone();
    }
    KnownContainer {
        id: token.to_owned(),
        name: token.to_owned(),
        mounts: Vec::new(),
    }
}

fn unused(config: &Config, kinds: &'static [&'static str]) -> Vec<&'static str> {
    kinds
        .iter()
        .copied()
        .filter(|kind| config.linters.iter().all(|tool| tool.kind != *kind))
        .collect()
}

fn kind_label(kind: &str) -> &'static str {
    LinterKind::parse(kind).map_or("Linter", LinterKind::label)
}

fn args_hint(kind: &str) -> &'static str {
    match kind {
        "phpmd" => "classes,controllers ruleset.xml",
        "golangci-lint" => "./...",
        _ => "Optional paths and flags",
    }
}

fn display_path(path: &Path, root: Option<&Path>) -> String {
    root.and_then(|root| path.strip_prefix(root).ok())
        .unwrap_or(path)
        .display()
        .to_string()
}

fn row_label(item: &DiagnosticItem) -> String {
    item.column.map_or_else(
        || format!("[Line {}] {}", item.line, item.message),
        |column| format!("[Line {}:{}] {}", item.line, column, item.message),
    )
}

pub fn container_picker(
    ui: &mut egui::Ui,
    id: &mut String,
    name: &mut String,
    containers: &[(String, String)],
    salt: impl std::hash::Hash,
) {
    let shown = if name.is_empty() {
        if id.is_empty() {
            "Choose container".to_owned()
        } else {
            id.clone()
        }
    } else {
        name.clone()
    };
    egui::ComboBox::from_id_salt(salt)
        .selected_text(shown)
        .show_ui(ui, |ui| {
            if !id.is_empty()
                && !containers
                    .iter()
                    .any(|(container_id, _)| container_id == id)
            {
                let stored = if name.is_empty() {
                    id.clone()
                } else {
                    name.clone()
                };
                let _ = ui.selectable_label(true, stored);
            }
            for (container_id, container_name) in containers {
                let text = if container_name.is_empty() {
                    container_id.clone()
                } else {
                    container_name.clone()
                };
                if ui.selectable_label(id == container_id, text).clicked() {
                    id.clone_from(container_id);
                    name.clone_from(container_name);
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::{blank_draft, place_cursor, remember};
    use nozhik_buffer::TextBuffer;
    use nozhik_config::Config;
    use nozhik_core::BufferId;

    #[test]
    fn discovery_keeps_a_saved_linter_and_adds_a_new_kind() {
        let mut config = Config {
            linters: vec![blank_draft("phpstan")],
            ..Config::default()
        };
        config.linters[0].container_id = "saved".into();
        config.linters[0].binary_path = "vendor/bin/phpstan".into();
        let containers = vec![super::KnownContainer {
            id: "abc".into(),
            name: "php-fpm-nozhovecom".into(),
            mounts: vec![("/work/nozhove".into(), "/var/www/html".into())],
        }];
        let root = std::path::Path::new("/work/nozhove");
        assert!(!remember(
            &mut config,
            root,
            "vendor/bin/phpstan analyse src",
            Some(("abc", "php-fpm-nozhovecom")),
            &containers,
        ));
        assert_eq!(config.linters[0].container_id, "saved");
        assert!(remember(
            &mut config,
            root,
            "vendor/bin/phpmd classes,controllers text ruleset.xml",
            Some(("abc", "php-fpm-nozhovecom")),
            &containers,
        ));
        assert_eq!(config.linters.len(), 2);
        assert_eq!(config.linters[1].kind, "phpmd");
        assert_eq!(config.linters[1].container_id, "abc");
        assert_eq!(config.linters[1].binary_path, "vendor/bin/phpmd");
        assert_eq!(config.linters[1].args, "classes,controllers ruleset.xml");
        assert_eq!(config.linters[1].workdir, "/var/www/html");
    }

    #[test]
    fn cursor_lands_on_the_linter_line_and_stops_before_the_newline() -> Result<(), String> {
        let mut buffer = TextBuffer::load_from_str(BufferId(7), "<?php\nfunction charge() {\n}\n");
        let line = place_cursor(&mut buffer, 2, Some(10))?;
        assert_eq!(line, 1);
        let head = buffer.cursors().as_slice().first().ok_or("cursor")?.head;
        assert_eq!(head.line, 1);
        assert_eq!(head.column, 9);
        let end = place_cursor(&mut buffer, 2, Some(400))?;
        assert_eq!(end, 1);
        let head = buffer.cursors().as_slice().first().ok_or("cursor")?.head;
        let expected =
            u32::try_from("function charge() {".len()).map_err(|error| error.to_string())?;
        assert_eq!(head.column, expected);
        Ok(())
    }
}
