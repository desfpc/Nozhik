use crate::{
    LeftTool, NozhikUi, SEARCH_ACTION, SETTINGS_ACTION, search_everywhere::SearchTarget, theme,
};
use egui::{Context, Key};
use nozhik_event_bus::Event;
use tokio::sync::broadcast::error::TryRecvError;

impl NozhikUi {
    fn shortcuts(&mut self, ctx: &Context) {
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
        let search = ctx.input_mut(|i| {
            i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, Key::A)
                || i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, Key::O)
                || i.consume_key(egui::Modifiers::COMMAND, Key::E)
        });
        if search {
            self.action(SEARCH_ACTION);
        }
    }

    fn receive_events(&mut self) {
        // Bound per-frame work so continuous producers cannot starve rendering.
        for _ in 0..1024 {
            match self.events.try_recv() {
                Ok(event) => match event.as_ref() {
                    Event::ActionTriggered(action) => match action.as_str() {
                        SEARCH_ACTION => {
                            self.settings_open = false;
                            self.search.open();
                        }
                        SETTINGS_ACTION => {
                            self.search.open = false;
                            self.settings_open = true;
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
                    Event::ConfigChanged(_) => match self.settings.snapshot() {
                        Ok(config) => self.config = config,
                        Err(error) => self.status = error.to_string(),
                    },
                    _ => {}
                },
                Err(TryRecvError::Lagged(count)) => {
                    self.status = format!("Missed {count} notifications; current buffers retained");
                    if let Ok(config) = self.settings.snapshot() {
                        self.config = config;
                    }
                }
                Err(TryRecvError::Empty | TryRecvError::Closed) => break,
            }
        }
    }

    fn top_bar(&self, ctx: &Context) {
        egui::TopBottomPanel::top("toolbar")
            .exact_height(44.0)
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    if cfg!(target_os = "macos") {
                        ui.add_space(72.0);
                    }
                    ui.menu_button("Nozhik v", |ui| {
                        ui.label("Nozhik · sample workspace");
                    });
                    ui.menu_button("master v", |ui| {
                        ui.weak("Git integration is planned");
                    });
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
                    ui.weak(&self.status);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label("Rust 2024");
                        ui.label("master");
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
                    ("⑂", "Commit / VCS", LeftTool::Commit),
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
                .default_width(245.0)
                .frame(egui::Frame::new().fill(theme::PANEL).inner_margin(8))
                .show(ctx, |ui| {
                    ui.label(match tool {
                        LeftTool::Project => "Project v",
                        LeftTool::Commit => "Commit",
                        LeftTool::Structure => "Structure",
                    });
                    ui.separator();
                    if tool == LeftTool::Project {
                        let mut open = None;
                        egui::CollapsingHeader::new("Nozhik · samples")
                            .default_open(true)
                            .show(ui, |ui| {
                                egui::CollapsingHeader::new("src").default_open(true).show(
                                    ui,
                                    |ui| {
                                        for (index, document) in self.documents.iter().enumerate() {
                                            if ui
                                                .selectable_label(
                                                    self.active == Some(index),
                                                    &document.name,
                                                )
                                                .clicked()
                                            {
                                                open = Some(index);
                                            }
                                        }
                                    },
                                );
                            });
                        if let Some(index) = open {
                            self.open_document(index);
                        }
                    } else {
                        ui.weak("Tool window placeholder");
                    }
                });
        }
    }

    fn editor_area(&mut self, ctx: &Context) {
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
                            .on_hover_text("Close tab (sample buffer is retained)")
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
                    let enabled = !self.search.open && !self.settings_open;
                    if let Err(error) = self.editor.show(
                        ui,
                        &mut self.documents[index].buffer,
                        f32::from(self.config.font_size),
                        self.config.tab_width,
                        enabled,
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
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        self.shortcuts(ctx);
        self.receive_events();
        self.top_bar(ctx);
        self.panels(ctx);
        self.editor_area(ctx);
        if let Some(target) = self.search.show(ctx, &self.documents) {
            match target {
                SearchTarget::Document(index) => self.open_document(index),
                SearchTarget::Action(action) => self.action(action),
            }
        }
        if self.settings_open {
            let modal = egui::Modal::new(egui::Id::new("settings")).show(ctx, |ui| {
                ui.heading("Settings · current session");
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
        // Assert the eframe integration remains implemented without creating a native window.
        is_app(&shell);
        Ok(())
    }
}
