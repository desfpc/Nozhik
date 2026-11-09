//! `PHPUnit` settings and the consoles opened by the play button.
//!
//! Output stays in the container terminal. The IDE does not parse it.

use nozhik_config::{Config, languages::LanguageId, test_tools::TestTool};
use nozhik_docker::linter::infer_workdir;
use nozhik_docker::test_run::{ConsoleCommand, detect};
use std::ffi::OsString;
use std::path::Path;

use crate::linters::{KnownContainer, attached_container, container_picker};

/// One console the play button opens.
pub struct Launch {
    /// Container id passed to `docker exec`.
    pub container_id: String,
    /// Container name shown on the terminal tab.
    pub container_name: String,
    /// Short runner name, for example `PHPUnit`.
    pub title: &'static str,
    /// Arguments after the `docker` program, including `exec`.
    pub args: Vec<OsString>,
}

/// `PHPUnit` consoles for languages that are enabled in this project.
pub fn launches(config: &Config) -> Vec<Launch> {
    if !nozhik_config::languages::enabled(&config.enabled_languages, LanguageId::Php) {
        return Vec::new();
    }
    config
        .tests
        .iter()
        .filter(|tool| tool.language == "php" && tool.kind == "phpunit")
        .filter_map(|tool| {
            let command = ConsoleCommand {
                container: tool.container_id.clone(),
                binary: tool.binary_path.clone(),
                workdir: tool.workdir.clone(),
                args: tool.args.clone(),
            };
            let args = nozhik_docker::test_run::console_args(&command).ok()?;
            let container_name = if tool.container_name.trim().is_empty() {
                tool.container_id.clone()
            } else {
                tool.container_name.clone()
            };
            Some(Launch {
                container_id: tool.container_id.clone(),
                container_name,
                title: "PHPUnit",
                args,
            })
        })
        .collect()
}

/// True when the play button has at least one console to open.
#[must_use]
pub fn ready(config: &Config) -> bool {
    !launches(config).is_empty()
}

/// Remembers `PHPUnit` when it is not configured yet.
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
        .tests
        .iter()
        .any(|tool| tool.language == "php" && tool.kind == "phpunit")
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
    let tool = TestTool {
        language: "php".into(),
        kind: "phpunit".into(),
        container_id: container.id,
        container_name: container.name,
        binary_path: found.binary,
        workdir,
        args: found.args,
    };
    let mut tools = config.tests.clone();
    tools.push(tool);
    if nozhik_config::test_tools::validate(&tools).is_err() {
        return false;
    }
    config.tests = tools;
    true
}

pub fn blank_draft() -> TestTool {
    TestTool {
        language: "php".into(),
        kind: "phpunit".into(),
        container_id: String::new(),
        container_name: String::new(),
        binary_path: "vendor/bin/phpunit".into(),
        workdir: String::new(),
        args: String::new(),
    }
}

pub fn show_settings(
    ui: &mut egui::Ui,
    config: &mut Config,
    containers: &[(String, String)],
    draft: &mut TestTool,
) {
    ui.label("PHPUnit runs in the container console when you press the play button.");
    if let Some(index) = config
        .tests
        .iter()
        .position(|tool| tool.language == "php" && tool.kind == "phpunit")
    {
        show_phpunit(ui, config, containers, index);
        return;
    }
    show_new_phpunit(ui, config, containers, draft);
}

fn show_phpunit(
    ui: &mut egui::Ui,
    config: &mut Config,
    containers: &[(String, String)],
    index: usize,
) {
    ui.separator();
    ui.strong("PHPUnit");
    {
        let Some(tool) = config.tests.get_mut(index) else {
            return;
        };
        container_picker(
            ui,
            &mut tool.container_id,
            &mut tool.container_name,
            containers,
            ("phpunit_container", index),
        );
    }
    ui.label("Binary");
    if let Some(tool) = config.tests.get_mut(index) {
        ui.add(
            crate::form::singleline(&mut tool.binary_path)
                .hint_text("vendor/bin/phpunit")
                .desired_width(ui.available_width()),
        );
    }
    ui.label("Container workdir");
    if let Some(tool) = config.tests.get_mut(index) {
        ui.add(
            crate::form::singleline(&mut tool.workdir)
                .hint_text("/var/www/html")
                .desired_width(ui.available_width()),
        );
    }
    ui.label("Arguments");
    if let Some(tool) = config.tests.get_mut(index) {
        ui.add(
            crate::form::singleline(&mut tool.args)
                .hint_text("--filter PaymentTest")
                .desired_width(ui.available_width()),
        );
    }
    if ui.button("Remove").clicked() {
        config.tests.remove(index);
    }
}

fn show_new_phpunit(
    ui: &mut egui::Ui,
    config: &mut Config,
    containers: &[(String, String)],
    draft: &mut TestTool,
) {
    if draft.kind != "phpunit" {
        *draft = blank_draft();
    }
    ui.separator();
    ui.strong("PHPUnit");
    container_picker(
        ui,
        &mut draft.container_id,
        &mut draft.container_name,
        containers,
        "phpunit_draft_container",
    );
    ui.label("Binary");
    ui.add(
        crate::form::singleline(&mut draft.binary_path)
            .hint_text("vendor/bin/phpunit")
            .desired_width(ui.available_width()),
    );
    ui.label("Container workdir");
    ui.add(
        crate::form::singleline(&mut draft.workdir)
            .hint_text("/var/www/html")
            .desired_width(ui.available_width()),
    );
    ui.label("Arguments");
    ui.add(
        crate::form::singleline(&mut draft.args)
            .hint_text("--filter PaymentTest")
            .desired_width(ui.available_width()),
    );
    let ready = !draft.container_id.trim().is_empty() && !draft.binary_path.trim().is_empty();
    if ui
        .add_enabled(ready, egui::Button::new("Add"))
        .on_hover_text("Save PHPUnit for the project")
        .clicked()
    {
        let mut tools = config.tests.clone();
        tools.push(draft.clone());
        if nozhik_config::test_tools::validate(&tools).is_ok() {
            config.tests = tools;
            *draft = blank_draft();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{blank_draft, launches, ready, remember};
    use nozhik_config::{Config, languages::LanguageId};

    #[test]
    fn discovery_keeps_saved_phpunit_and_a_disabled_language_does_not_run()
    -> Result<(), &'static str> {
        let mut config = Config::default();
        let containers = vec![crate::linters::KnownContainer {
            id: "abc".into(),
            name: "php-fpm-nozhovecom".into(),
            mounts: vec![("/work/nozhove".into(), "/var/www/html".into())],
        }];
        let root = std::path::Path::new("/work/nozhove");
        assert!(remember(
            &mut config,
            root,
            "vendor/bin/phpunit --filter PaymentTest",
            Some(("abc", "php-fpm-nozhovecom")),
            &containers,
        ));
        let saved = config.tests.first().ok_or("phpunit")?;
        assert_eq!(saved.kind, "phpunit");
        assert_eq!(saved.container_id, "abc");
        assert_eq!(saved.binary_path, "vendor/bin/phpunit");
        assert_eq!(saved.args, "--filter PaymentTest");
        assert_eq!(saved.workdir, "/var/www/html");
        assert!(!remember(
            &mut config,
            root,
            "vendor/bin/phpunit tests",
            Some(("abc", "php-fpm-nozhovecom")),
            &containers,
        ));
        assert_eq!(config.tests.len(), 1);
        assert_eq!(
            config.tests.first().map(|tool| tool.args.as_str()),
            Some("--filter PaymentTest")
        );
        assert!(ready(&config));
        assert_eq!(launches(&config).len(), 1);
        config.enabled_languages.remove(&LanguageId::Php);
        assert!(!ready(&config));
        assert!(launches(&config).is_empty());
        assert_eq!(blank_draft().binary_path, "vendor/bin/phpunit");
        Ok(())
    }
}
