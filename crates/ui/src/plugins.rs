//! Generic installed-plugin settings and declarative tool-tab renderer.
use crate::NozhikUi;
impl NozhikUi {
    pub(crate) fn plugins_window(&mut self, ctx: &egui::Context) {
        if !self.plugins_open {
            return;
        }
        egui::Window::new("Plugins")
            .open(&mut self.plugins_open)
            .default_width(600.0)
            .show(ctx, |ui| {
                let Some(manager) = &self.plugins else {
                    ui.label("Plugin host unavailable; see stderr");
                    return;
                };
                if ui.button("Rescan installed plugins").clicked()
                    && let Err(error) =
                        manager.scan(self.project.as_ref().map(|p| p.root_path.clone()))
                {
                    self.status = error.to_string();
                }
                let snapshot = manager.snapshot();
                if snapshot.plugins.is_empty() {
                    ui.weak("No plugins installed in ~/.nozhik/plugins or .nozhik/plugins");
                }
                egui::ScrollArea::vertical()
                    .max_height(400.0)
                    .show(ui, |ui| {
                        for plugin in snapshot.plugins {
                            ui.group(|ui| {
                                ui.horizontal(|ui| {
                                    ui.strong(&plugin.manifest.name);
                                    ui.label(plugin.manifest.version.to_string());
                                    let mut enabled = plugin.enabled();
                                    if ui.toggle_value(&mut enabled, "Enabled").changed()
                                        && let Err(error) =
                                            manager.set_enabled(&plugin.manifest.id, enabled)
                                    {
                                        self.status = error.to_string();
                                    }
                                });
                                ui.label(format!(
                                    "{} · {}",
                                    plugin.manifest.id, plugin.manifest.author
                                ));
                                ui.label(plugin.manifest.description);
                                ui.weak(format!("{:?}", plugin.status));
                            });
                        }
                        for diagnostic in snapshot.diagnostics {
                            ui.colored_label(crate::theme::WARNING, diagnostic);
                        }
                        ui.separator();
                        ui.label("Extension tabs");
                        for (id, tab) in &snapshot.tabs {
                            if ui
                                .selectable_label(self.plugin_tab.as_ref() == Some(id), &tab.title)
                                .clicked()
                            {
                                self.plugin_tab = Some(id.clone());
                            }
                        }
                        if let Some(tab) = self
                            .plugin_tab
                            .as_ref()
                            .and_then(|id| snapshot.tabs.get(id))
                        {
                            ui.group(|ui| {
                                ui.heading(&tab.title);
                                ui.label(&tab.description);
                            });
                        }
                    });
            });
    }
}
