use crate::tool_icons::{self, Icon};
use crate::{
    LeftTool, NozhikUi, SAVE_ACTION, SEARCH_ACTION, SETTINGS_ACTION,
    search_everywhere::SearchTarget, theme,
};
use egui::{Context, Key};
use nozhik_event_bus::{Event, FileEvent};
use nozhik_fs_model::Command;
use tokio::sync::broadcast::error::TryRecvError;

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
                if ctx.input_mut(|i| i.consume_key(modifiers, key))
                    && let Err(error) = keymap.dispatch(chord, context, &actions)
                {
                    self.status = error.to_string();
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
                        "ui.plugins" => self.plugins_open = true,
                        "ui.git" => {
                            self.left = if self.left == Some(LeftTool::Commit) {
                                None
                            } else {
                                Some(LeftTool::Commit)
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
                        if self
                            .project
                            .as_ref()
                            .is_some_and(|p| path.starts_with(&p.root_path))
                        {
                            self.status =
                                format!("Disk changed: {} (open buffers retained)", path.display());
                            self.refresh_project();
                            self.vcs.invalidate();
                        }
                    }
                    Event::File(FileEvent::Error(error)) => {
                        self.status.clone_from(error);
                        eprintln!("Watcher: {error}");
                    }
                    Event::ConfigChanged(_) => match self.settings.snapshot() {
                        Ok(config) => self.config = config,
                        Err(error) => self.status = error.to_string(),
                    },
                    _ => {}
                },
                Err(TryRecvError::Lagged(count)) => {
                    self.status = format!("Missed {count} notifications; current buffers retained");
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
                ui.horizontal_centered(|ui| {
                    if cfg!(target_os = "macos") {
                        ui.add_space(72.0);
                    }
                    ui.menu_button("Nozhik v", |ui| {
                        ui.label("Nozhik · project workspace");
                    });
                    self.vcs.toolbar(ui);
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
                        ui.menu_button("Current File v", |ui| {
                            ui.weak("Run configurations are planned");
                        });
                    });
                });
            });
    }

    #[allow(
        clippy::too_many_lines,
        reason = "Dock panels share sizing and terminal lifecycle state"
    )]
    fn panels(&mut self, ctx: &Context) {
        egui::TopBottomPanel::bottom("status")
            .exact_height(26.0)
            .frame(egui::Frame::new().fill(theme::CHROME).inner_margin(4))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Nozhik").on_hover_text("Nozhik v0.1.0-RC1");
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
                    ui.weak(&self.status);
                    if let Some(status) = &self.editor.ai.status {
                        ui.weak(status);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label("0.1.0-RC1")
                            .on_hover_text("Release candidate · Help → About Nozhik");
                        ui.label(self.vcs.branch())
                            .on_hover_text("Current Git branch");
                        ui.label("UTF-8").on_hover_text("Editor text encoding");
                    });
                });
            });
        egui::TopBottomPanel::bottom("bottom_strip")
            .exact_height(30.0)
            .frame(theme::rail_frame())
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    for label in [crate::terminal_panel::TERMINAL_LABEL, "! Problems"] {
                        if ui
                            .selectable_label(self.bottom == Some(label), label)
                            .clicked()
                        {
                            self.bottom = if self.bottom == Some(label) {
                                None
                            } else {
                                Some(label)
                            };
                            if self.bottom == Some(crate::terminal_panel::TERMINAL_LABEL)
                                && let Some(panel) = &mut self.terminal
                            {
                                panel.focus = true;
                            }
                        }
                    }
                });
            });
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
                .default_height(viewport.height() * self.layout.value.bottom)
                .show_separator_line(false)
                .frame(theme::dock_frame(theme::PANEL))
                .show(ctx, |ui| {
                    if label == crate::terminal_panel::TERMINAL_LABEL {
                        let panel = self
                            .terminal
                            .get_or_insert_with(|| crate::terminal_panel::TerminalPanel::new(ctx));
                        let root = self.project.as_ref().map(|p| p.root_path.as_path());
                        let enabled = !self.search.open
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
                            self.status = error;
                        }
                    } else {
                        ui.label(label);
                        ui.separator();
                        ui.weak("Tool window placeholder");
                    }
                });
            let mut layout = self.layout.value;
            layout.bottom = (panel.response.rect.height() / viewport.height()).clamp(0.1, 0.8);
            if ctx.input(|i| i.pointer.any_down()) {
                self.layout.set(layout);
            }
        }
        self.side_panels(ctx);
    }

    #[allow(
        clippy::too_many_lines,
        reason = "Left and right splitters share viewport geometry"
    )]
    fn side_panels(&mut self, ctx: &Context) {
        egui::SidePanel::left("left_strip")
            .exact_width(36.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(theme::rail_frame())
            .show(ctx, |ui| {
                for (icon, title, tool) in [
                    (Icon::Project, "Project", LeftTool::Project),
                    (Icon::Git, "Git · Changes / History", LeftTool::Commit),
                    (Icon::Structure, "Structure", LeftTool::Structure),
                    (Icon::Remote, "Remote Host / SSH", LeftTool::Remote),
                ] {
                    if tool_icons::button(ui, icon, self.left == Some(tool), title).clicked() {
                        self.left = if self.left == Some(tool) {
                            None
                        } else {
                            Some(tool)
                        };
                    }
                }
            });
        egui::SidePanel::right("right_strip")
            .exact_width(36.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(theme::rail_frame())
            .show(ctx, |ui| {
                for (icon, title) in [(Icon::Database, "Database"), (Icon::Bell, "Notifications")] {
                    if tool_icons::button(ui, icon, self.right == Some(title), title).clicked() {
                        self.right = if self.right == Some(title) {
                            None
                        } else {
                            Some(title)
                        };
                    }
                }
            });
        let viewport = ctx
            .input(|i| i.viewport().inner_rect)
            .unwrap_or_else(|| ctx.screen_rect());
        if let Some(title) = self.right {
            crate::layout::seed(
                ctx,
                "right_dock",
                viewport.width() * self.layout.value.right,
                viewport.height(),
            );
            let panel = egui::SidePanel::right("right_dock")
                .default_width(viewport.width() * self.layout.value.right)
                .width_range(150.0..=viewport.width() * 0.4)
                .resizable(true)
                .show_separator_line(false)
                .frame(theme::dock_frame(theme::PANEL))
                .show(ctx, |ui| {
                    ui.label(title);
                    ui.separator();
                    if title == "Database" {
                        self.database_panel(ui);
                    } else {
                        ui.weak("Tool window placeholder");
                    }
                });
            let mut layout = self.layout.value;
            layout.right = (panel.response.rect.width() / viewport.width()).clamp(0.1, 0.4);
            if ctx.input(|i| i.pointer.any_down()) {
                self.layout.set(layout);
            }
        }
        if let Some(tool) = self.left {
            crate::layout::seed(
                ctx,
                "left_dock",
                viewport.width() * self.layout.value.left,
                viewport.height(),
            );
            let panel = egui::SidePanel::left("left_dock")
                .resizable(true)
                .default_width(viewport.width() * self.layout.value.left)
                .width_range(150.0..=viewport.width() * 0.4)
                .show_separator_line(false)
                .frame(theme::dock_frame(theme::PANEL))
                .show(ctx, |ui| {
                    ui.label(match tool {
                        LeftTool::Project => "Project v",
                        LeftTool::Commit => "Commit",
                        LeftTool::Structure => "Structure",
                        LeftTool::Remote => "Remote Host / SSH",
                    });
                    ui.separator();
                    if tool == LeftTool::Project {
                        self.project_panel(ui);
                    } else if tool == LeftTool::Commit {
                        self.vcs.panel(ui);
                    } else if tool == LeftTool::Remote {
                        self.remote_panel(ui);
                    } else {
                        ui.weak("Tool window placeholder");
                    }
                });
            let mut layout = self.layout.value;
            layout.left = (panel.response.rect.width() / viewport.width()).clamp(0.1, 0.4);
            if ctx.input(|i| i.pointer.any_down()) {
                self.layout.set(layout);
            }
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "Routes existing file and database tabs through one central panel"
    )]
    pub(crate) fn editor_area(&mut self, ctx: &Context) {
        if self.left == Some(LeftTool::Commit) {
            egui::CentralPanel::default()
                .frame(theme::dock_frame(theme::BACKGROUND))
                .show(ctx, |ui| {
                    self.vcs.center(ui, f32::from(self.config.font_size));
                });
            return;
        }
        egui::CentralPanel::default()
            .frame(theme::dock_frame(theme::BACKGROUND))
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    self.database_tabs(ui);
                    let mut close = None;
                    for (index, document) in
                        self.documents.iter().enumerate().filter(|(_, d)| d.open)
                    {
                        if ui
                            .selectable_label(
                                self.active == Some(index) && self.database.active.is_none(),
                                format!(
                                    "{}{}",
                                    document.name,
                                    if document.buffer.is_dirty() {
                                        " ●"
                                    } else {
                                        ""
                                    }
                                ),
                            )
                            .clicked()
                        {
                            self.active = Some(index);
                            self.editor.focus_buffer = Some(document.buffer.snapshot().buffer_id);
                            self.database.active = None;
                        }
                        if ui
                            .small_button("×")
                            .on_hover_text("Close tab (asks before discarding changes)")
                            .clicked()
                        {
                            close = Some(index);
                        }
                    }
                    if let Some(index) = close {
                        self.request_close(crate::integration::CloseTarget::File(index), ctx);
                    }
                });
                ui.separator();
                self.find_bar(ui);
                if self.database.active.is_some() {
                    let enabled = !self.search.open
                        && !self.settings_open
                        && !self.plugins_open
                        && !self.remote.modal_open()
                        && !self.database.modal_open()
                        && !self.integration.blocking();
                    self.database_editor(ui, enabled);
                    return;
                }
                if let Some(index) = self.active {
                    let enabled = !self.search.open
                        && !self.settings_open
                        && !self.plugins_open
                        && !self.remote.modal_open()
                        && !self.database.modal_open()
                        && !self.integration.blocking();
                    if self.documents[index].binary {
                        ui.label("Binary file not supported");
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
                    self.editor.code_style = self
                        .settings_dialog
                        .styles
                        .selected(&extension, &self.config.file_types)
                        .cloned();
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
                        self.status = error.to_string();
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
    pub(crate) fn update_frame(&mut self, ctx: &Context) {
        let wake_context = ctx.clone();
        self.fs.set_wakeup(move || wake_context.request_repaint());
        self.shortcuts(ctx);
        self.receive_events();
        self.receive_files();
        self.integration_tick(ctx);
        self.database_tick(ctx);
        self.receive_remote(ctx);
        self.language_tick(ctx);
        if let Some(panel) = &mut self.terminal
            && let Some(message) = panel.tick()
        {
            self.status = message;
        }
        self.vcs.tick(ctx, self.left == Some(LeftTool::Commit));
        self.settings_tick(ctx);
        self.menu_bar(ctx);
        self.top_bar(ctx);
        self.panels(ctx);
        if self.active.is_none() || self.database.active.is_some() {
            self.editor.ai.cancel();
        }
        self.editor_area(ctx);
        if let Some(manager) = &self.plugins {
            let context = self.action_context();
            if let Err(error) =
                self.search
                    .set_actions(&manager.actions(), &manager.keymap(), context)
            {
                self.status = error.to_string();
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
    fn terminal_keymap_opens_panel_without_spawning_outside_a_project()
    -> Result<(), Box<dyn std::error::Error>> {
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
