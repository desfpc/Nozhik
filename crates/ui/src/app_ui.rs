use crate::tool_icons::{self, Icon};
use crate::{
    LeftTool, NozhikUi, SAVE_ACTION, SEARCH_ACTION, SETTINGS_ACTION,
    search_everywhere::SearchTarget, theme,
};
use egui::{Context, Key};
use nozhik_event_bus::{Event, FileEvent};
use nozhik_fs_model::Command;
use tokio::sync::broadcast::error::TryRecvError;

const fn dock_title(tool: LeftTool) -> Option<&'static str> {
    match tool {
        LeftTool::Project | LeftTool::Commit | LeftTool::Snippets => None,
        LeftTool::Database => Some("Database"),
        LeftTool::Docker => Some("Docker"),
        LeftTool::Remote => Some("Remote Host / SSH"),
        LeftTool::Tools => Some("Tools"),
    }
}

impl NozhikUi {
    pub(crate) fn shortcuts(&mut self, ctx: &Context) {
        let (shift, time, used) = ctx.input(|i| {
            (
                i.modifiers.shift,
                i.time,
                i.events.iter().any(|e| {
                    matches!(
                        e,
                        egui::Event::Key { pressed: true, .. } | egui::Event::Text(_)
                    )
                }),
            )
        });
        if shift && !self.shift_down {
            if self.last_shift.is_some_and(|last| time - last < 0.4) {
                self.action(SEARCH_ACTION);
                self.last_shift = None;
            } else {
                self.last_shift = Some(time);
            }
        }
        if used {
            self.last_shift = None;
        }
        self.shift_down = shift;
        if self.global_modal() {
            return;
        }
        if !self.search.open
            && !self.settings_open
            && !self.plugins_open
            && let Some(manager) = &self.plugins
        {
            let mut context = self.action_context();
            context.editor_focus = self
                .editor_focus
                .is_some_and(|id| ctx.memory(|m| m.has_focus(id)));
            let actions = manager.actions();
            let keymap = manager.keymap();
            let available = actions.available(context).unwrap_or_default();
            for binding in keymap.bindings().unwrap_or_default() {
                if !binding.when.matches(context)
                    || !available.iter().any(|a| a.id == binding.action)
                {
                    continue;
                }
                let Some(key) = Key::from_name(&binding.chord.key) else {
                    continue;
                };
                let chord = &binding.chord;
                let modifiers = egui::Modifiers {
                    ctrl: chord.ctrl,
                    mac_cmd: chord.cmd,
                    command: chord.primary
                        || if cfg!(target_os = "macos") {
                            chord.cmd
                        } else {
                            chord.ctrl
                        },
                    alt: chord.alt,
                    shift: chord.shift,
                };
                if self.left == Some(LeftTool::Snippets) {
                    let editing = matches!(
                        binding.action.as_str(),
                        "ui.undo" | "ui.redo" | "ui.cut" | "ui.copy" | "ui.paste"
                    );
                    let find = matches!(binding.action.as_str(), "ui.find" | "ui.replace");
                    if find || (editing && self.snippets.editor_focused(ctx)) {
                        continue;
                    }
                }
                if ctx.input_mut(|i| i.consume_key(modifiers, key))
                    && let Err(error) = keymap.dispatch(chord, context, &actions)
                {
                    self.note_status(
                        "keymap",
                        "Keymap",
                        crate::problems::Severity::Error,
                        error.to_string(),
                    );
                }
            }
        }
        if self.left == Some(LeftTool::Commit)
            && !self.search.open
            && !self.settings_open
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::Enter))
        {
            self.vcs.commit_selected();
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "One frame drains the project bus, including snippet file reloads"
    )]
    pub(crate) fn receive_events(&mut self) {
        // Bound per-frame work so continuous producers cannot starve rendering.
        for _ in 0..1024 {
            match self.events.try_recv() {
                Ok(event) => match event.as_ref() {
                    Event::ActionTriggered(action) => match action.as_str() {
                        SAVE_ACTION => self.save_file(),
                        SEARCH_ACTION => {
                            self.settings_open = false;
                            self.search.open();
                        }
                        SETTINGS_ACTION => {
                            self.search.open = false;
                            self.settings_open = true;
                        }
                        "ui.terminal" => {
                            self.bottom =
                                if self.bottom == Some(crate::terminal_panel::TERMINAL_LABEL) {
                                    None
                                } else {
                                    Some(crate::terminal_panel::TERMINAL_LABEL)
                                };
                            if let Some(panel) = &mut self.terminal {
                                panel.focus = true;
                            }
                        }
                        "ui.tools" => {
                            self.left = Some(LeftTool::Tools);
                            self.tools.catalog();
                        }
                        "ui.plugins" => self.plugins_open = true,
                        "ui.git" => {
                            self.left = if self.left == Some(LeftTool::Commit) {
                                None
                            } else {
                                Some(LeftTool::Commit)
                            };
                        }
                        "ui.snippets" => {
                            self.left = if self.left == Some(LeftTool::Snippets) {
                                None
                            } else {
                                Some(LeftTool::Snippets)
                            };
                        }
                        "ui.project" => {
                            self.left = if self.left == Some(LeftTool::Project) {
                                None
                            } else {
                                Some(LeftTool::Project)
                            }
                        }
                        _ => self.integration.actions.push(action.clone()),
                    },
                    Event::File(FileEvent::Changed(path)) => {
                        self.index_changed(path.clone());
                        if self
                            .project
                            .as_ref()
                            .is_some_and(|p| path.starts_with(&p.root_path))
                        {
                            let snippet_file = self.snippets.is_store_path(path);
                            if snippet_file {
                                self.snippets.note_disk_change();
                            } else {
                                self.status = format!(
                                    "Disk changed: {} (open buffers retained)",
                                    path.display()
                                );
                            }
                            self.refresh_project();
                            self.vcs.invalidate();
                        }
                    }
                    Event::File(FileEvent::Error(error)) => {
                        self.note_status(
                            "watcher",
                            "File watcher",
                            crate::problems::severity_of(error),
                            error.clone(),
                        );
                        eprintln!("Watcher: {error}");
                    }
                    Event::ConfigChanged(_) => match self.settings.snapshot() {
                        Ok(config) => self.config = config,
                        Err(error) => self.note_status(
                            "settings",
                            "Settings",
                            crate::problems::Severity::Error,
                            error.to_string(),
                        ),
                    },
                    _ => {}
                },
                Err(TryRecvError::Lagged(count)) => {
                    if let Some(indexer) = &self.language.indexer {
                        let _sent = indexer.send(
                            nozhik_language_support::project_indexer::IndexCommand::Reconcile,
                        );
                    }
                    self.note_status(
                        "events",
                        "Events",
                        crate::problems::Severity::Warning,
                        format!("Missed {count} notifications; current buffers retained"),
                    );
                    self.refresh_project();
                    self.vcs.invalidate();
                    if let Ok(config) = self.settings.snapshot() {
                        self.config = config;
                    }
                }
                Err(TryRecvError::Empty | TryRecvError::Closed) => break,
            }
        }
    }

    fn top_bar(&mut self, ctx: &Context) {
        egui::TopBottomPanel::top("toolbar")
            .exact_height(36.0)
            .frame(theme::rail_frame())
            .show(ctx, |ui| {
                if cfg!(target_os = "macos") {
                    let background = ui.interact(
                        ui.max_rect(),
                        ui.id().with("window_drag"),
                        egui::Sense::drag(),
                    );
                    if background.drag_started() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                    }
                }
                ui.horizontal_centered(|ui| {
                    if cfg!(target_os = "macos") {
                        ui.add_space(72.0);
                    }
                    self.project_switcher(ui);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if tool_icons::button(ui, Icon::Settings, false, "Settings")
                            .on_hover_text("Settings")
                            .clicked()
                        {
                            self.action(SETTINGS_ACTION);
                        }
                        if ui
                            .add(Icon::Search)
                            .on_hover_text("Search Everywhere · Double Shift")
                            .clicked()
                        {
                            self.action(SEARCH_ACTION);
                        }
                        ui.add_enabled(false, Icon::Debug)
                            .on_hover_text("Debug · planned");
                        ui.add_enabled(false, Icon::Run)
                            .on_hover_text("Run · planned");
                    });
                });
            });
    }

    #[allow(
        clippy::too_many_lines,
        reason = "Dock panels share sizing and terminal lifecycle state"
    )]
    pub(crate) fn panels(&mut self, ctx: &Context) {
        egui::TopBottomPanel::bottom("status")
            .exact_height(26.0)
            .frame(egui::Frame::new().fill(theme::CHROME).inner_margin(4))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Nozhik")
                        .on_hover_text(concat!("Nozhik v", env!("CARGO_PKG_VERSION")));
                    let errors = self
                        .language
                        .views
                        .values()
                        .flat_map(|v| &v.diagnostics)
                        .filter(|d| {
                            d.severity
                                == Some(
                                    nozhik_language_support::lsp_types::DiagnosticSeverity::ERROR,
                                )
                        })
                        .count();
                    let warnings = self
                        .language
                        .views
                        .values()
                        .flat_map(|v| &v.diagnostics)
                        .filter(|d| {
                            d.severity
                                == Some(
                                    nozhik_language_support::lsp_types::DiagnosticSeverity::WARNING,
                                )
                        })
                        .count();
                    ui.label(format!("Errors: {errors}  Warnings: {warnings}"))
                        .on_hover_text("Diagnostics in open language buffers");
                    if !self.problems.hides_status(&self.status) {
                        ui.weak(&self.status);
                    }
                    if let Some(status) = &self.editor.ai.status
                        && self.editor.ai.problem().is_none()
                    {
                        ui.weak(status);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if let Some((done, total)) = self.language.indexing {
                            ui.spinner();
                            let percent = (done * 100).checked_div(total).unwrap_or(0);
                            ui.weak(format!("Indexing project ({percent}%)..."));
                        }
                        ui.label(env!("CARGO_PKG_VERSION"))
                            .on_hover_text("Help → About Nozhik");
                        ui.label(self.vcs.branch())
                            .on_hover_text("Current Git branch");
                        ui.label("UTF-8").on_hover_text("Editor text encoding");
                    });
                });
            });
        self.tool_rail(ctx);
        self.vcs.configure_deployment(
            self.config.sftp_deployment.is_some(),
            self.deployment.busy(),
        );
        if self.left == Some(LeftTool::Commit) {
            egui::TopBottomPanel::top("git_toolbar")
                .exact_height(50.0)
                .frame(egui::Frame::new().fill(egui::Color32::from_rgb(29, 34, 38)))
                .show(ctx, |ui| self.vcs.toolbar(ui));
            if self.deployment.visible() {
                egui::TopBottomPanel::top("git_deployment_progress")
                    .frame(theme::rail_frame())
                    .show(ctx, |ui| self.deployment.show(ui));
            }
            if self.vcs.is_merging() {
                egui::TopBottomPanel::top("git_merge_banner")
                    .frame(theme::rail_frame())
                    .show(ctx, |ui| self.vcs.merge_banner(ui));
            }
        }
        let viewport = ctx
            .input(|i| i.viewport().inner_rect)
            .unwrap_or_else(|| ctx.screen_rect());
        if let Some(label) = self.bottom {
            crate::layout::seed(
                ctx,
                "bottom_dock",
                viewport.width(),
                viewport.height() * self.layout.value.bottom,
            );
            let panel = egui::TopBottomPanel::bottom("bottom_dock")
                .resizable(true)
                .height_range(70.0..=viewport.height())
                .default_height(viewport.height() * self.layout.value.bottom)
                .show_separator_line(false)
                .frame(theme::dock_frame(theme::PANEL))
                .show(ctx, |ui| {
                    // Keep sparse tool contents anchored at the chosen dock height.
                    ui.set_min_height(ui.available_height());
                    if label == crate::llm_panel::LLM_LABEL {
                        let enabled = !self.global_modal() && !self.search.open;
                        let panel = self
                            .llm
                            .get_or_insert_with(|| crate::llm_panel::LlmPanel::new(ctx));
                        let root = self.project.as_ref().map(|p| p.root_path.as_path());
                        if panel.show(
                            ui,
                            root,
                            f32::from(self.config.font_size),
                            enabled,
                            &self.documents,
                            self.active,
                            &self.remote.documents,
                        ) {
                            self.bottom = None;
                        }
                    } else if label == crate::terminal_panel::TERMINAL_LABEL {
                        let panel = self
                            .terminal
                            .get_or_insert_with(|| crate::terminal_panel::TerminalPanel::new(ctx));
                        let root = self.project.as_ref().map(|p| p.root_path.as_path());
                        let enabled = !self.language.invalidate_dialog
                            && self.language.usages.is_none()
                            && !self.search.open
                            && !self.settings_open
                            && !self.plugins_open
                            && !self.remote.modal_open()
                            && !self.database.modal_open()
                            && !self.integration.blocking();
                        let (collapse, error) =
                            panel.show(ui, root, f32::from(self.config.font_size), enabled);
                        if collapse {
                            self.bottom = None;
                        }
                        if let Some(error) = error {
                            self.note_status(
                                "terminal",
                                "Terminal",
                                crate::problems::severity_of(&error),
                                error,
                            );
                        }
                    } else if label == "! Problems" {
                        self.problems.show_panel(ui);
                    } else {
                        ui.label(label);
                        ui.separator();
                        ui.weak("Tool window placeholder");
                    }
                });
            let mut layout = self.layout.value;
            layout.bottom = (panel.response.rect.height() / viewport.height()).clamp(0.05, 1.0);
            if ctx.input(|i| i.pointer.any_down() || i.pointer.any_released()) {
                self.layout.set(layout);
            }
        }
        if self.bottom != Some(crate::terminal_panel::TERMINAL_LABEL)
            && let Some(panel) = &mut self.terminal
        {
            panel.hidden();
        }
        if self.bottom != Some(crate::llm_panel::LLM_LABEL)
            && let Some(panel) = &mut self.llm
        {
            panel.hidden();
        }
        self.side_panels(ctx);
    }

    fn tool_rail(&mut self, ctx: &Context) {
        egui::SidePanel::left("left_strip")
            .exact_width(36.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(theme::rail_frame())
            .show(ctx, |ui| {
                for (icon, title, tool) in [
                    (Icon::Project, "Project", LeftTool::Project),
                    (Icon::Git, "Git · Changes / History", LeftTool::Commit),
                    (Icon::Remote, "Remote Host / SSH", LeftTool::Remote),
                    (Icon::Database, "Database", LeftTool::Database),
                    (Icon::Docker, "Docker", LeftTool::Docker),
                    (Icon::Snippets, "Snippets", LeftTool::Snippets),
                    (Icon::Tools, "Tools", LeftTool::Tools),
                ] {
                    if ui
                        .add_enabled_ui(
                            tool != LeftTool::Docker || !self.docker.files.is_empty(),
                            |ui| tool_icons::button(ui, icon, self.left == Some(tool), title),
                        )
                        .inner
                        .clicked()
                    {
                        if tool == LeftTool::Tools {
                            self.tools.catalog();
                        }
                        self.left = if self.left == Some(tool) {
                            None
                        } else {
                            Some(tool)
                        };
                    }
                }
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
                    for (icon, label, title) in [
                        (
                            Icon::Terminal,
                            crate::terminal_panel::TERMINAL_LABEL,
                            "Terminal",
                        ),
                        (Icon::Llm, crate::llm_panel::LLM_LABEL, "LLM console"),
                        (Icon::Problems, "! Problems", "Problems"),
                    ] {
                        if tool_icons::button(ui, icon, self.bottom == Some(label), title).clicked()
                        {
                            self.bottom = if self.bottom == Some(label) {
                                None
                            } else {
                                Some(label)
                            };
                            if self.bottom == Some(crate::llm_panel::LLM_LABEL)
                                && let Some(panel) = &mut self.llm
                            {
                                panel.focus();
                            }
                            if self.bottom == Some(crate::terminal_panel::TERMINAL_LABEL)
                                && let Some(panel) = &mut self.terminal
                            {
                                panel.focus = true;
                            }
                        }
                    }
                });
            });
    }

    fn side_panels(&mut self, ctx: &Context) {
        let viewport = ctx
            .input(|i| i.viewport().inner_rect)
            .unwrap_or_else(|| ctx.screen_rect());
        if let Some(tool) = self.left.filter(|tool| *tool != LeftTool::Tools) {
            let dock_id = match tool {
                LeftTool::Commit => "git_dock",
                LeftTool::Snippets => "snippets_dock",
                _ => "left_dock",
            };
            let proportion = match tool {
                LeftTool::Commit => self.layout.value.git,
                LeftTool::Snippets => self.layout.value.snippets,
                _ => self.layout.value.left,
            };
            crate::layout::seed(
                ctx,
                dock_id,
                viewport.width() * proportion,
                viewport.height(),
            );
            let panel = egui::SidePanel::left(dock_id)
                .resizable(true)
                .default_width(viewport.width() * proportion)
                .width_range(
                    (match tool {
                        LeftTool::Project => 180.0,
                        LeftTool::Snippets => 200.0,
                        _ => 150.0,
                    })..=viewport.width() * 0.4,
                )
                .show_separator_line(false)
                .frame(match tool {
                    LeftTool::Commit => egui::Frame::new()
                        .fill(egui::Color32::from_rgb(35, 42, 48))
                        .inner_margin(0),
                    LeftTool::Snippets => crate::snippets::sidebar_frame(),
                    _ => theme::dock_frame(theme::PANEL),
                })
                .show(ctx, |ui| {
                    if tool == LeftTool::Snippets {
                        let enabled = !self.global_modal();
                        let root = self
                            .project
                            .as_ref()
                            .map(|project| project.root_path.as_path());
                        self.snippets.show_nav(ui, root, enabled);
                    } else if let Some(title) = dock_title(tool) {
                        ui.label(title);
                        ui.separator();
                    }
                    if tool == LeftTool::Project {
                        self.project_panel(ui);
                    } else if tool == LeftTool::Commit {
                        self.vcs.panel(ui);
                    } else if tool == LeftTool::Remote {
                        self.remote_panel(ui);
                    } else if tool == LeftTool::Docker {
                        self.docker_panel(ui);
                    } else if tool == LeftTool::Database {
                        self.database_panel(ui);
                    }
                });
            let mut layout = self.layout.value;
            let width = (panel.response.rect.width() / viewport.width()).clamp(0.1, 0.4);
            match tool {
                LeftTool::Commit => layout.git = width,
                LeftTool::Snippets => layout.snippets = width,
                _ => layout.left = width,
            }
            if ctx.input(|i| i.pointer.any_down() || i.pointer.any_released()) {
                self.layout.set(layout);
            }
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "Routes existing file and database tabs through one central panel"
    )]
    pub(crate) fn editor_area(&mut self, ctx: &Context) {
        if self.left == Some(LeftTool::Tools) {
            egui::CentralPanel::default()
                .frame(theme::dock_frame(theme::BACKGROUND))
                .show(ctx, |ui| {
                    let enabled = !self.global_modal();
                    ui.add_enabled_ui(enabled, |ui| self.tools.show(ui));
                });
            self.editor_focus = None;
            return;
        }
        if self.left == Some(LeftTool::Commit) {
            egui::CentralPanel::default()
                .frame(
                    egui::Frame::new()
                        .fill(egui::Color32::from_rgb(35, 42, 48))
                        .inner_margin(0),
                )
                .show(ctx, |ui| {
                    self.vcs.configure_diff(&self.config);
                    self.vcs.center(ui, f32::from(self.config.font_size));
                });
            return;
        }
        if self.left == Some(LeftTool::Snippets) {
            egui::CentralPanel::default()
                .frame(crate::snippets::editor_frame())
                .show(ctx, |ui| {
                    let enabled = !self.global_modal();
                    let root = self
                        .project
                        .as_ref()
                        .map(|project| project.root_path.as_path());
                    if let Some(crate::snippets::SnippetCommand::OpenInConsole(text)) =
                        self.snippets.show_main(ui, root, enabled)
                    {
                        self.open_snippet_in_console(ctx, &text);
                    }
                });
            self.editor_focus = None;
            return;
        }
        egui::CentralPanel::default()
            .frame(theme::dock_frame(theme::BACKGROUND))
            .show(ctx, |ui| {
                self.editor_tabs(ui);
                self.find_bar(ui);
                if self.database.active.is_some() {
                    let enabled = !self.language.invalidate_dialog
                        && self.language.usages.is_none()
                        && !self.search.open
                        && !self.settings_open
                        && !self.plugins_open
                        && !self.remote.modal_open()
                        && !self.database.modal_open()
                        && !self.integration.blocking();
                    self.database_editor(ui, enabled);
                    return;
                }
                if let Some(index) = self.active {
                    let enabled = !self.language.invalidate_dialog
                        && self.language.usages.is_none()
                        && !self.search.open
                        && !self.settings_open
                        && !self.plugins_open
                        && !self.remote.modal_open()
                        && !self.database.modal_open()
                        && !self.integration.blocking();
                    if self.documents[index].binary {
                        ui.label("Binary file not supported");
                        return;
                    }
                    if self.openapi_toolbar(ui, ctx, index, enabled) {
                        return;
                    }
                    let editor_id = ui
                        .id()
                        .with(("editor", self.documents[index].buffer.snapshot().buffer_id));
                    self.integration.last_editor = Some(editor_id);
                    self.editor_focus = ui.memory(|m| m.has_focus(editor_id)).then_some(editor_id);
                    let extension = self.documents[index]
                        .path
                        .extension()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .to_lowercase();
                    self.editor.code_style = if nozhik_config::languages::enabled_for_path(
                        &self.config.enabled_languages,
                        &self.documents[index].path,
                    ) {
                        self.settings_dialog
                            .styles
                            .selected(&extension, &self.config.file_types)
                            .cloned()
                    } else {
                        None
                    };
                    self.editor.ai.configure(&self.config);
                    self.editor.proportional_font = self.config.editor_font == "proportional";
                    if let Some(language) = self.language.views.get_mut(&self.documents[index].path)
                    {
                        language.syntax_theme = self
                            .config
                            .file_types
                            .get(&extension)
                            .map_or_else(|| "default".into(), |s| s.syntax_theme.clone());
                    }
                    let document = &mut self.documents[index];
                    if let Err(error) = self.editor.show_language(
                        ui,
                        &mut document.buffer,
                        f32::from(self.config.font_size),
                        self.config.tab_width,
                        enabled,
                        self.language.views.get_mut(&document.path),
                    ) {
                        self.note_status(
                            "editor",
                            "Editor",
                            crate::problems::Severity::Error,
                            error.to_string(),
                        );
                    }
                    if let Some(target) = self.editor.documentation_target.take() {
                        use nozhik_language_support::doc_provider::DefinitionTarget;
                        match target {
                            DefinitionTarget::ExternalUrl(url) => {
                                crate::editor_view::open_documentation(url);
                            }
                            DefinitionTarget::LocalFile { path, line, column } => {
                                // Convert UTF-8 source columns to UTF-16 after asynchronous open.
                                self.language.local_navigation = Some((path, line, column));
                            }
                        }
                    }
                } else {
                    ui.add_space((ui.available_height() * 0.35).max(0.0));
                    ui.vertical_centered(|ui| {
                        ui.heading("Nozhik");
                        ui.add_space(20.0);
                        for label in [
                            "Search Everywhere    Double Shift",
                            "Go to File    Ctrl/Cmd + Shift + O",
                            "Recent Files    Ctrl/Cmd + E",
                        ] {
                            if ui.button(label).clicked() {
                                self.action(SEARCH_ACTION);
                            }
                        }
                    });
                }
            });
    }

    fn open_snippet_in_console(&mut self, ctx: &Context, text: &str) {
        ctx.copy_text(text.to_owned());
        let Some(root) = self
            .project
            .as_ref()
            .map(|project| project.root_path.clone())
        else {
            self.status = "Snippet copied".into();
            self.note_status(
                "snippets",
                "Snippets",
                crate::problems::Severity::Error,
                "Open a project before sending a snippet to the terminal",
            );
            return;
        };
        self.bottom = Some(crate::terminal_panel::TERMINAL_LABEL);
        let outcome = {
            let panel = self
                .terminal
                .get_or_insert_with(|| crate::terminal_panel::TerminalPanel::new(ctx));
            panel.focus = true;
            let needs_session = !panel.has_running_session();
            if needs_session && let Some(error) = panel.create(&root) {
                ConsolePaste::Failed(error)
            } else {
                match panel.queue_paste(text) {
                    Ok(crate::terminal_panel::PasteDelivery::Sent) => ConsolePaste::Sent,
                    Ok(crate::terminal_panel::PasteDelivery::Waiting) => ConsolePaste::Waiting,
                    Err(error) => ConsolePaste::Failed(error),
                }
            }
        };
        match outcome {
            ConsolePaste::Sent => {
                self.status = "Snippet copied into the terminal".into();
            }
            ConsolePaste::Waiting => {
                self.status =
                    "Snippet copied. Waiting for the terminal to accept the paste.".into();
            }
            ConsolePaste::Failed(error) => {
                self.status = "Snippet copied".into();
                self.note_status(
                    "snippets",
                    "Snippets",
                    crate::problems::Severity::Warning,
                    error,
                );
            }
        }
    }
}

enum ConsolePaste {
    Sent,
    Waiting,
    Failed(String),
}

impl eframe::App for NozhikUi {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        theme::CHROME.to_normalized_gamma_f32()
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.shutdown_systems();
    }

    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        self.update_frame(ctx);
    }
}
impl NozhikUi {
    #[allow(
        clippy::too_many_lines,
        reason = "One frame routes project services, tools and session notifications"
    )]
    pub(crate) fn update_frame(&mut self, ctx: &Context) {
        let wake_context = ctx.clone();
        self.fs.set_wakeup(move || wake_context.request_repaint());
        self.receive_files();
        if self.project.is_none() {
            self.snippets.finish();
            self.receive_events();
            self.integration_tick(ctx);
            #[cfg(target_os = "macos")]
            if self.native_menu.is_some() {
                self.menu_bar(ctx);
            }
            self.observe_session_problems();
            self.welcome_screen(ctx);
            self.integration_dialogs(ctx);
            self.problems.show_toasts(ctx);
            return;
        }
        self.welcome_transition(ctx);
        self.shortcuts(ctx);
        self.receive_events();
        self.integration_tick(ctx);
        self.database_tick(ctx);
        self.receive_remote(ctx);
        self.deployment_tick();
        self.language_tick(ctx);
        self.docker_tick(ctx);
        if let Some(panel) = &mut self.terminal
            && let Some(message) = panel.tick()
        {
            if let Some(detail) = message.strip_prefix("snippet-paste:") {
                self.note_status(
                    "snippets",
                    "Snippets",
                    crate::problems::Severity::Warning,
                    detail,
                );
            } else if message == "snippet-pasted" {
                self.status = "Snippet copied into the terminal".into();
            } else if message.contains("exited with code 0") {
                self.status = message;
            } else {
                let severity = if message.contains("exited with code") {
                    crate::problems::Severity::Warning
                } else {
                    crate::problems::severity_of(&message)
                };
                self.note_status("terminal", "Terminal", severity, message);
            }
        }
        if let Some(panel) = &mut self.llm {
            panel.tick();
        }
        self.vcs.configure_commit_ai(&self.config);
        self.vcs.tick(ctx, self.left == Some(LeftTool::Commit));
        self.snippets.tick(ctx);
        if let Some((severity, message)) = self.snippets.take_problem() {
            self.note_problem("snippets", "Snippets", severity, &message);
        }
        if let Some(paths) = self.vcs.take_deploy_paths() {
            self.deploy_files(paths);
        }
        let git_files = self.vcs.take_open_paths();
        for path in git_files {
            self.open_file(path);
            self.left = Some(LeftTool::Project);
        }
        self.settings_tick(ctx);
        self.observe_session_problems();
        self.menu_bar(ctx);
        self.top_bar(ctx);
        self.project_tabs(ctx);
        self.panels(ctx);
        if self.active.is_none() || self.database.active.is_some() {
            self.editor.ai.cancel();
        }
        self.editor_area(ctx);
        self.docker_window(ctx);
        if let Some(manager) = &self.plugins {
            let context = self.action_context();
            if let Err(error) =
                self.search
                    .set_actions(&manager.actions(), &manager.keymap(), context)
            {
                self.note_status(
                    "actions",
                    "Actions",
                    crate::problems::Severity::Error,
                    error.to_string(),
                );
            }
        }
        if let Some(target) = self.search.show(ctx, &self.documents) {
            match target {
                SearchTarget::File(path) => self.open_file(path),
                SearchTarget::Document(index) => self.open_document(index),
                SearchTarget::Action(action) => self.action(&action),
            }
        }
        if let Some((token, query)) = self.search.take_request() {
            self.queue(Command::Search { token, query });
        }
        self.settings_window(ctx);
        self.plugins_window(ctx);
        self.integration_dialogs(ctx);
        if let Some((path, auth)) = self.welcome.clone.tick() {
            if self.workspace.managed {
                self.workspace.clone_auth = Some(auth);
            } else {
                self.vcs.clone_auth = Some(auth);
            }
            self.open_project(path);
        }
        self.welcome.clone.show(ctx);
        self.index_dialogs(ctx);
        self.project_entry_dialog(ctx);
        self.observe_session_problems();
        self.problems.show_toasts(ctx);
        if !self.integration.actions.is_empty() {
            ctx.request_repaint();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nozhik_config::{Config, ConfigManager};
    use nozhik_event_bus::EventBus;

    #[test]
    fn terminal_keymap_opens_panel_without_a_project() -> Result<(), Box<dyn std::error::Error>> {
        let bus = EventBus::new(32)?;
        let dir = tempfile::tempdir()?;
        let settings = ConfigManager::from_path(dir.path().join("settings.toml"), bus.clone())?;
        let mut shell = NozhikUi::new(bus, settings, Config::default());
        let ctx = egui::Context::default();
        for expected_open in [true, true] {
            let input = egui::RawInput {
                modifiers: egui::Modifiers::ALT,
                events: vec![egui::Event::Key {
                    key: Key::F12,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::ALT,
                }],
                ..Default::default()
            };
            let _frame = ctx.run(input, |ctx| {
                shell.shortcuts(ctx);
                shell.receive_events();
                shell.global_actions(ctx);
                shell.panels(ctx);
                shell.editor_area(ctx);
            });
            assert_eq!(
                shell.bottom == Some(crate::terminal_panel::TERMINAL_LABEL),
                expected_open
            );
        }
        assert!(shell.terminal.is_some());
        if let Some(panel) = &mut shell.terminal {
            panel.shutdown();
        }
        Ok(())
    }

    #[test]
    fn git_toolbar_stays_right_of_rail_in_one_row() -> Result<(), Box<dyn std::error::Error>> {
        use egui::{Pos2, Rect, vec2};
        let bus = EventBus::new(32)?;
        let dir = tempfile::tempdir()?;
        let settings = ConfigManager::from_path(dir.path().join("settings.toml"), bus.clone())?;
        let mut shell = NozhikUi::new(bus, settings, Config::default());
        shell.left = Some(LeftTool::Commit);
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        for size in [vec2(1000.0, 800.0), vec2(800.0, 600.0)] {
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
                    ..Default::default()
                },
                |ctx| {
                    shell.top_bar(ctx);
                    shell.project_tabs(ctx);
                    shell.panels(ctx);
                    shell.editor_area(ctx);
                },
            );
            let rect = |name| {
                egui::containers::panel::PanelState::load(&ctx, egui::Id::new(name))
                    .map(|state| state.rect)
                    .ok_or("missing panel")
            };
            let rail = rect("left_strip")?;
            let toolbar = rect("git_toolbar")?;
            assert_eq!(rail.top(), toolbar.top());
            assert_eq!(toolbar.left(), rail.right());
            assert_eq!(toolbar.height(), 50.0);
            assert_eq!(rect("git_dock")?.top(), toolbar.bottom());
            for label in ["Current Branch", "Fetch origin"] {
                let text = output
                    .shapes
                    .iter()
                    .find_map(|shape| match &shape.shape {
                        egui::Shape::Text(text) if text.galley.text() == label => Some(text),
                        _ => None,
                    })
                    .ok_or("missing Git control")?;
                assert!(toolbar.contains_rect(Rect::from_min_size(text.pos, text.galley.size())));
            }
            assert!(!output.shapes.iter().any(|shape| {
                matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text() == "Current Repository")
            }));
        }
        Ok(())
    }

    #[test]
    fn bottom_tools_stay_in_full_height_rail() -> Result<(), Box<dyn std::error::Error>> {
        use egui::{Event as InputEvent, Pos2, Rect, vec2};
        let bus = EventBus::new(32)?;
        let dir = tempfile::tempdir()?;
        let settings = ConfigManager::from_path(dir.path().join("settings.toml"), bus.clone())?;
        let mut shell = NozhikUi::new(bus, settings, Config::default());
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let frame = |shell: &mut NozhikUi, size, events| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    shell.panels(ctx);
                    shell.editor_area(ctx);
                },
            )
        };
        let rect = |name| {
            egui::containers::panel::PanelState::load(&ctx, egui::Id::new(name))
                .map(|state| state.rect)
                .ok_or("missing panel")
        };
        for size in [vec2(1000.0, 800.0), vec2(800.0, 600.0)] {
            shell.bottom = None;
            frame(&mut shell, size, vec![]);
            let rail = rect("left_strip")?;
            assert_eq!(rail.width(), 36.0);
            assert_eq!(rail.bottom(), rect("status")?.top());
            let terminal = rail.left_bottom() + vec2(18.0, -18.0);
            let llm = terminal - vec2(0.0, 32.0);
            let problems = terminal - vec2(0.0, 64.0);
            // Actual pointer input exercises open, switch, collapse and reopen.
            for (position, expected) in [
                (terminal, Some(crate::terminal_panel::TERMINAL_LABEL)),
                (llm, Some(crate::llm_panel::LLM_LABEL)),
                (llm, None),
                (problems, Some("! Problems")),
                (problems, None),
                (terminal, Some(crate::terminal_panel::TERMINAL_LABEL)),
                (terminal, None),
            ] {
                let pointer = |pressed| InputEvent::PointerButton {
                    pos: position,
                    pressed,
                    button: egui::PointerButton::Primary,
                    modifiers: egui::Modifiers::NONE,
                };
                frame(
                    &mut shell,
                    size,
                    vec![InputEvent::PointerMoved(position), pointer(true)],
                );
                frame(&mut shell, size, vec![pointer(false)]);
                assert_eq!(shell.bottom, expected);
                assert_eq!(rect("left_strip")?, rail);
                assert!(rect("left_dock")?.left() >= rail.right());
                if expected.is_some() {
                    let dock = rect("bottom_dock")?;
                    assert!(dock.left() >= rail.right());
                    assert_eq!(dock.bottom(), rail.bottom());
                    assert!(
                        rect("left_dock")?.bottom() <= dock.top(),
                        "left={:?}, bottom={dock:?}, size={size:?}",
                        rect("left_dock")?
                    );
                }
            }
            // The top group retains its original position and click action.
            let project = rail.left_top() + vec2(18.0, 18.0);
            let before = shell.left;
            for pressed in [true, false] {
                frame(
                    &mut shell,
                    size,
                    vec![
                        InputEvent::PointerMoved(project),
                        InputEvent::PointerButton {
                            pos: project,
                            pressed,
                            button: egui::PointerButton::Primary,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                );
            }
            assert!(shell.left != before);
            shell.left = Some(LeftTool::Project);
        }
        if let Some(panel) = &mut shell.terminal {
            panel.shutdown();
        }
        Ok(())
    }

    #[test]
    fn notifications_rail_and_panel_are_removed() -> Result<(), Box<dyn std::error::Error>> {
        use egui::{Event as InputEvent, Pos2, Rect, vec2};
        let bus = EventBus::new(32)?;
        let dir = tempfile::tempdir()?;
        let settings = ConfigManager::from_path(dir.path().join("settings.toml"), bus.clone())?;
        let mut shell = NozhikUi::new(bus, settings, Config::default());
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let frame = |shell: &mut NozhikUi, events| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 800.0))),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    shell.panels(ctx);
                    shell.editor_area(ctx);
                },
            )
        };
        let shows = |output: &egui::FullOutput, label: &str| {
            output.shapes.iter().any(|shape| {
                matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text() == label)
            })
        };
        let output = frame(&mut shell, vec![]);
        assert!(!shows(&output, "Notifications"));
        assert!(!shows(&output, "Tool window placeholder"));
        let rail = egui::containers::panel::PanelState::load(&ctx, egui::Id::new("left_strip"))
            .ok_or("missing rail")?
            .rect;
        // Sixth top button is Snippets. Seventh is Tools. The next slot is empty.
        for (index, position) in [
            rail.left_top() + vec2(18.0, 178.0),
            rail.left_top() + vec2(18.0, 210.0),
            rail.left_top() + vec2(18.0, 242.0),
        ]
        .into_iter()
        .enumerate()
        {
            let before = shell.left;
            for pressed in [true, false] {
                frame(
                    &mut shell,
                    vec![
                        InputEvent::PointerMoved(position),
                        InputEvent::PointerButton {
                            pos: position,
                            pressed,
                            button: egui::PointerButton::Primary,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                );
            }
            if index == 0 {
                assert!(shell.left == Some(LeftTool::Snippets));
            } else if index == 1 {
                assert!(shell.left == Some(LeftTool::Tools));
            } else {
                assert!(shell.left == before);
            }
            let output = frame(&mut shell, vec![]);
            assert!(!shows(&output, "Notifications"));
            assert!(!shows(&output, "Tool window placeholder"));
        }
        Ok(())
    }

    #[test]
    fn terminal_resize_survives_hide_and_reopen() -> Result<(), Box<dyn std::error::Error>> {
        use egui::{Event as InputEvent, Pos2, Rect, vec2};
        let bus = EventBus::new(32)?;
        let dir = tempfile::tempdir()?;
        let settings = ConfigManager::from_path(dir.path().join("settings.toml"), bus.clone())?;
        let mut shell = NozhikUi::new(bus, settings, Config::default());
        let ctx = egui::Context::default();
        shell.bottom = Some(crate::terminal_panel::TERMINAL_LABEL);
        let frame = |shell: &mut NozhikUi, events| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 800.0))),
                    events,
                    ..Default::default()
                },
                |ctx| shell.panels(ctx),
            )
        };
        let panel_rect = || {
            egui::containers::panel::PanelState::load(&ctx, egui::Id::new("bottom_dock"))
                .map(|s| s.rect)
                .ok_or("missing bottom dock")
        };
        frame(&mut shell, vec![]);
        let initial = panel_rect()?;
        assert!(initial.height() > 280.0 && initial.height() < 400.0);
        let start = initial.center_top() + vec2(0.0, 2.0);
        let end = start - vec2(0.0, 140.0);
        let pointer = |pos, pressed| InputEvent::PointerButton {
            pos,
            pressed,
            button: egui::PointerButton::Primary,
            modifiers: egui::Modifiers::NONE,
        };
        frame(
            &mut shell,
            vec![InputEvent::PointerMoved(start), pointer(start, true)],
        );
        frame(&mut shell, vec![InputEvent::PointerMoved(end)]);
        frame(&mut shell, vec![pointer(end, false)]);
        let resized = panel_rect()?;
        assert!(
            resized.height() > initial.height() + 100.0,
            "initial={initial:?}, resized={resized:?}"
        );
        shell.bottom = None;
        frame(&mut shell, vec![]);
        shell.bottom = Some(crate::terminal_panel::TERMINAL_LABEL);
        frame(&mut shell, vec![]);
        assert!((panel_rect()?.height() - resized.height()).abs() < 2.0);
        if let Some(panel) = &mut shell.terminal {
            panel.shutdown();
        }
        Ok(())
    }

    #[test]
    fn modal_bus_shortcuts_and_empty_workspace_render() -> Result<(), Box<dyn std::error::Error>> {
        use eframe::App;
        fn is_app<T: App>(_: &T) {}
        let bus = EventBus::new(32)?;
        let settings = ConfigManager::from_path(
            std::env::temp_dir().join("nozhik-ui-test-missing/settings.toml"),
            bus.clone(),
        )?;
        let mut shell = NozhikUi::new(bus.clone(), settings, Config::default());
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        bus.publish(Event::ActionTriggered(SEARCH_ACTION.into()));
        shell.receive_events();
        assert!(shell.search.open);
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            shell.top_bar(ctx);
            shell.panels(ctx);
            shell.editor_area(ctx);
            shell.search.show(ctx, &shell.documents);
        });
        shell.search.open = false;
        for (time, shift) in [(1.0, true), (1.1, false), (1.2, true)] {
            let input = egui::RawInput {
                time: Some(time),
                modifiers: egui::Modifiers {
                    shift,
                    ..Default::default()
                },
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| shell.shortcuts(ctx));
        }
        shell.receive_events();
        assert!(shell.search.open);
        shell.search.open = false;
        shell.active = None;
        for document in &mut shell.documents {
            document.open = false;
        }
        let _ = ctx.run(egui::RawInput::default(), |ctx| shell.editor_area(ctx));
        let input = egui::RawInput {
            events: vec![egui::Event::Key {
                key: Key::G,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
            }],
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| shell.shortcuts(ctx));
        shell.receive_events();
        assert!(shell.left == Some(LeftTool::Commit));
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            shell.panels(ctx);
            shell.editor_area(ctx);
        });
        // Assert the eframe integration remains implemented without creating a native window.
        is_app(&shell);
        Ok(())
    }
}
