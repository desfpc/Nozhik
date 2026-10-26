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
        if !self.search.open
            && !self.settings_open
            && !self.plugins_open
            && let Some(manager) = &self.plugins
        {
            let context = self.action_context();
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
                        _ => {}
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
            .exact_height(44.0)
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
                        if ui.button("⚙").on_hover_text("Settings").clicked() {
                            self.action(SETTINGS_ACTION);
                        }
                        if ui
                            .button("⌕")
                            .on_hover_text("Search Everywhere · Double Shift")
                            .clicked()
                        {
                            self.action(SEARCH_ACTION);
                        }
                        ui.add_enabled(false, egui::Button::new("♧"))
                            .on_hover_text("Debug · planned");
                        ui.add_enabled(false, egui::Button::new("▶"))
                            .on_hover_text("Run · planned");
                        ui.menu_button("Current File v", |ui| {
                            ui.weak("Run configurations are planned");
                        });
                    });
                });
            });
    }

    fn panels(&mut self, ctx: &Context) {
        egui::TopBottomPanel::bottom("status")
            .exact_height(26.0)
            .frame(egui::Frame::new().fill(theme::PANEL).inner_margin(4))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Nozhik");
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
                    ui.label(format!("Errors: {errors}  Warnings: {warnings}"));
                    ui.weak(&self.status);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label("Rust 2024");
                        ui.label(self.vcs.branch());
                        ui.label("UTF-8");
                    });
                });
            });
        egui::TopBottomPanel::bottom("bottom_strip")
            .exact_height(32.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    for label in ["⌘ Terminal", "! Problems"] {
                        if ui
                            .selectable_label(self.bottom == Some(label), label)
                            .clicked()
                        {
                            self.bottom = if self.bottom == Some(label) {
                                None
                            } else {
                                Some(label)
                            };
                        }
                    }
                });
            });
        if let Some(label) = self.bottom {
            egui::TopBottomPanel::bottom("bottom_dock")
                .resizable(true)
                .default_height(160.0)
                .show(ctx, |ui| {
                    ui.label(label);
                    ui.separator();
                    ui.weak("Tool window placeholder");
                });
        }
        self.side_panels(ctx);
    }

    fn side_panels(&mut self, ctx: &Context) {
        egui::SidePanel::left("left_strip")
            .exact_width(42.0)
            .resizable(false)
            .show(ctx, |ui| {
                for (icon, title, tool) in [
                    ("▣", "Project", LeftTool::Project),
                    ("⑂", "Git · Changes / History", LeftTool::Commit),
                    ("☷", "Structure", LeftTool::Structure),
                ] {
                    if ui
                        .selectable_label(self.left == Some(tool), icon)
                        .on_hover_text(title)
                        .clicked()
                    {
                        self.left = if self.left == Some(tool) {
                            None
                        } else {
                            Some(tool)
                        };
                    }
                }
            });
        egui::SidePanel::right("right_strip")
            .exact_width(42.0)
            .resizable(false)
            .show(ctx, |ui| {
                for (icon, title) in [("▤", "Database"), ("♧", "Notifications")] {
                    if ui
                        .selectable_label(self.right == Some(title), icon)
                        .on_hover_text(title)
                        .clicked()
                    {
                        self.right = if self.right == Some(title) {
                            None
                        } else {
                            Some(title)
                        };
                    }
                }
            });
        if let Some(title) = self.right {
            egui::SidePanel::right("right_dock")
                .default_width(240.0)
                .resizable(true)
                .show(ctx, |ui| {
                    ui.label(title);
                    ui.separator();
                    ui.weak("Tool window placeholder");
                });
        }
        if let Some(tool) = self.left {
            egui::SidePanel::left("left_dock")
                .resizable(true)
                .default_width(if tool == LeftTool::Commit {
                    340.0
                } else {
                    245.0
                })
                .frame(egui::Frame::new().fill(theme::PANEL).inner_margin(8))
                .show(ctx, |ui| {
                    ui.label(match tool {
                        LeftTool::Project => "Project v",
                        LeftTool::Commit => "Commit",
                        LeftTool::Structure => "Structure",
                    });
                    ui.separator();
                    if tool == LeftTool::Project {
                        self.project_panel(ui);
                    } else if tool == LeftTool::Commit {
                        self.vcs.panel(ui);
                    } else {
                        ui.weak("Tool window placeholder");
                    }
                });
        }
    }

    pub(crate) fn editor_area(&mut self, ctx: &Context) {
        if self.left == Some(LeftTool::Commit) {
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(theme::BACKGROUND))
                .show(ctx, |ui| {
                    self.vcs.center(ui, f32::from(self.config.font_size));
                });
            return;
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::BACKGROUND))
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    let mut close = None;
                    for (index, document) in
                        self.documents.iter().enumerate().filter(|(_, d)| d.open)
                    {
                        if ui
                            .selectable_label(
                                self.active == Some(index),
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
                        }
                        if ui
                            .small_button("×")
                            .on_hover_text("Close tab (unsaved buffer is retained)")
                            .clicked()
                        {
                            close = Some(index);
                        }
                    }
                    if let Some(index) = close {
                        self.documents[index].open = false;
                        if self.active == Some(index) {
                            self.active = self.documents.iter().position(|d| d.open);
                        }
                    }
                });
                ui.separator();
                if let Some(index) = self.active {
                    let enabled = !self.search.open && !self.settings_open && !self.plugins_open;
                    if self.documents[index].binary {
                        ui.label("Binary file not supported");
                        return;
                    }
                    let editor_id = ui
                        .id()
                        .with(("editor", self.documents[index].buffer.snapshot().buffer_id));
                    self.editor_focus = ui.memory(|m| m.has_focus(editor_id)).then_some(editor_id);
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
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.shutdown_language();
    }

    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        let wake_context = ctx.clone();
        self.fs.set_wakeup(move || wake_context.request_repaint());
        self.shortcuts(ctx);
        self.receive_events();
        self.receive_files();
        self.language_tick(ctx);
        self.vcs.tick(ctx, self.left == Some(LeftTool::Commit));
        self.top_bar(ctx);
        self.panels(ctx);
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
        if self.settings_open {
            let modal = egui::Modal::new(egui::Id::new("settings")).show(ctx, |ui| {
                ui.heading("Settings · current session");
                if ui.button("Plugins…").clicked() { self.plugins_open = true; self.settings_open = false; }
                ui.weak("Keymap: ~/.nozhik/keymap.json · JetBrains / VSCode / Custom · reloads every second");
                let mut config = self.config.clone();
                ui.add(egui::Slider::new(&mut config.font_size, 6..=96).text("Editor font size"));
                ui.add(egui::Slider::new(&mut config.tab_width, 1..=16).text("Tab width"));
                if config != self.config {
                    match self.settings.replace(config.clone()) {
                        Ok(()) => self.config = config,
                        Err(error) => self.status = error.to_string(),
                    }
                }
                ui.weak("New UI Dark · settings are kept in memory for this session");
                if ui.button("Close").clicked() {
                    self.settings_open = false;
                }
            });
            if modal.should_close() {
                self.settings_open = false;
            }
        }
        self.plugins_window(ctx);
        // Bounds latency of best-effort bus requests from producers without a UI handle.
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nozhik_config::{Config, ConfigManager};
    use nozhik_event_bus::EventBus;

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
