//! Settings categories and background profile discovery/persistence.
use nozhik_config::{
    Config, ConfigManager,
    code_style::{CodeStyles, FileTypeSettings},
};
use std::{path::PathBuf, sync::mpsc};

enum Request {
    Scan,
    Save(Config),
}
enum Reply {
    Catalog(Result<CodeStyles, String>),
    Saved(Result<(), String>),
}
pub struct SettingsDialog {
    pub styles: CodeStyles,
    category: usize,
    extension: String,
    requests: mpsc::Sender<Request>,
    replies: mpsc::Receiver<Reply>,
    pending: usize,
}
impl SettingsDialog {
    pub fn start(path: PathBuf, bus: nozhik_event_bus::EventBus) -> Self {
        let (requests, input) = mpsc::channel();
        let (output, replies) = mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(request) = input.recv() {
                let reply = match request {
                    Request::Scan => Reply::Catalog((|| {
                        let mut styles = CodeStyles::builtin().map_err(|e| e.to_string())?;
                        for root in nozhik_config::code_style::search_roots(&path) {
                            styles
                                .scan(&root)
                                .map_err(|e| format!("{}: {e}", root.display()))?;
                        }
                        Ok(styles)
                    })()),
                    Request::Save(config) => Reply::Saved(
                        (|| {
                            let manager = ConfigManager::from_path(path.clone(), bus.clone())?;
                            manager.replace(config)?;
                            manager.save()
                        })()
                        .map_err(|e| e.to_string()),
                    ),
                };
                if output.send(reply).is_err() {
                    break;
                }
            }
        });
        let mut dialog = Self {
            styles: CodeStyles::builtin().unwrap_or_default(),
            category: 0,
            extension: "php".into(),
            requests,
            replies,
            pending: 0,
        };
        dialog.scan();
        dialog
    }
    fn navigation(&mut self, ui: &mut egui::Ui) -> bool {
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
        ui.selectable_value(&mut self.category, 0, "General");
        ui.selectable_value(&mut self.category, 1, "File Types & Code Styles");
        ui.button("Plugins…").clicked()
    }

    fn options(&mut self, ui: &mut egui::Ui, config: &mut Config, size: egui::Vec2) {
        egui::ScrollArea::both()
            .id_salt(("settings_options", self.category))
            .auto_shrink([false, false])
            .min_scrolled_width(0.0)
            .min_scrolled_height(0.0)
            .max_width(size.x)
            .max_height(size.y)
            .show(ui, |ui| {
                ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                    if self.category == 0 {
                        Self::general(ui, config);
                    } else {
                        self.file_types(ui, config);
                    }
                });
            });
    }

    fn content(&mut self, ui: &mut egui::Ui, config: &mut Config, size: egui::Vec2) -> bool {
        let compact = size.x < 560.0;
        let height = (size.y - if compact { 126.0 } else { 86.0 }).max(0.0);
        if compact {
            let plugins = ui.horizontal_wrapped(|ui| self.navigation(ui)).inner;
            ui.separator();
            self.options(ui, config, egui::vec2(size.x, height));
            plugins
        } else {
            ui.horizontal_top(|ui| {
                let sidebar_width = 184.0;
                let gap = ui.spacing().item_spacing.x;
                let plugins = ui
                    .allocate_ui_with_layout(
                        egui::vec2(sidebar_width, height),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            egui::ScrollArea::vertical()
                                .id_salt("settings_categories")
                                .auto_shrink([false, false])
                                .min_scrolled_height(0.0)
                                .max_height(height)
                                .show(ui, |ui| self.navigation(ui))
                                .inner
                        },
                    )
                    .inner;
                // Allocate the divider explicitly: Separator in an unbounded Area
                // consumes the full available height and can enlarge the modal.
                let (divider, _) =
                    ui.allocate_exact_size(egui::vec2(1.0, height), egui::Sense::hover());
                ui.painter().vline(
                    divider.center().x,
                    divider.y_range(),
                    ui.visuals().widgets.noninteractive.bg_stroke,
                );
                self.options(
                    ui,
                    config,
                    egui::vec2(
                        (2.0_f32.mul_add(-gap, size.x - sidebar_width) - 1.0).max(0.0),
                        height,
                    ),
                );
                plugins
            })
            .inner
        }
    }

    fn general(ui: &mut egui::Ui, config: &mut Config) {
        ui.heading("General");
        egui::ComboBox::from_label("Editor font")
            .selected_text(&config.editor_font)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut config.editor_font, "monospace".into(), "Monospace");
                ui.selectable_value(
                    &mut config.editor_font,
                    "proportional".into(),
                    "Proportional",
                );
            });
        ui.add(egui::Slider::new(&mut config.font_size, 6..=96).text("Editor font size"));
        ui.add(egui::Slider::new(&mut config.tab_width, 1..=16).text("Plain text tab width"));
        egui::ComboBox::from_label("Theme")
            .selected_text("New UI Dark")
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut config.theme, "dark".into(), "New UI Dark");
            });
        ui.weak("Keymap: ~/.nozhik/keymap.json");
    }
    fn file_types(&mut self, ui: &mut egui::Ui, config: &mut Config) {
        ui.heading("File Types & Code Styles");
        let mut extensions = vec![
            "php".to_owned(),
            "rs".into(),
            "sql".into(),
            "json".into(),
            "phtml".into(),
        ];
        extensions.extend(self.styles.0.keys().cloned());
        extensions.sort();
        extensions.dedup();
        egui::ComboBox::from_label("File type")
            .selected_text(format!("*.{}", self.extension))
            .show_ui(ui, |ui| {
                for extension in extensions {
                    ui.selectable_value(
                        &mut self.extension,
                        extension.clone(),
                        format!("*.{extension}"),
                    );
                }
            });
        let extension = &self.extension;
        let mut selection = config
            .file_types
            .get(extension)
            .cloned()
            .unwrap_or_else(|| FileTypeSettings {
                formatting: matches!(extension.as_str(), "php" | "phtml")
                    .then(|| "php_psr12.json".into()),
                ..FileTypeSettings::default()
            });
        let previous = selection.clone();
        let styles = self.styles.0.get(extension);
        let label = selection.formatting.as_ref().map_or_else(
            || "None".into(),
            |name| {
                styles
                    .and_then(|s| s.get(name))
                    .map_or_else(|| format!("Missing: {name}"), |s| s.name.clone())
            },
        );
        egui::ComboBox::from_label("Formatting Standard")
            .selected_text(label)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut selection.formatting, None, "None");
                if let Some(styles) = styles {
                    for (name, style) in styles {
                        ui.selectable_value(
                            &mut selection.formatting,
                            Some(name.clone()),
                            format!("{} ({name})", style.name),
                        );
                    }
                }
            });
        egui::ComboBox::from_label("Syntax Highlighting Theme")
            .selected_text(&selection.syntax_theme)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut selection.syntax_theme, "default".into(), "New UI Dark");
                ui.selectable_value(&mut selection.syntax_theme, "muted".into(), "Muted");
                ui.selectable_value(&mut selection.syntax_theme, "plain".into(), "Plain Text");
            });
        if selection != previous {
            config.file_types.insert(extension.clone(), selection);
        }
        ui.add_space(12.0);
        ui.weak("JSON profiles: ~/.nozhik/code_styles/<extension>/");
        ui.weak("Also reads code_styles/ beside settings.toml");
        if ui.button("Reload profiles").clicked() {
            self.scan();
        }
    }
    pub fn finish(&mut self) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while self.pending > 0 {
            match self
                .replies
                .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
            {
                Ok(reply) => {
                    self.pending -= 1;
                    if let Reply::Saved(Err(error)) = reply {
                        eprintln!("Settings save: {error}");
                    }
                }
                Err(error) => {
                    eprintln!("Settings flush: {error}");
                    break;
                }
            }
        }
    }
    fn scan(&mut self) {
        if self.requests.send(Request::Scan).is_ok() {
            self.pending += 1;
        }
    }
}
impl crate::NozhikUi {
    pub(crate) fn settings_tick(&mut self, ctx: &egui::Context) {
        for reply in self.settings_dialog.replies.try_iter() {
            self.settings_dialog.pending = self.settings_dialog.pending.saturating_sub(1);
            match reply {
                Reply::Catalog(Ok(styles)) => self.settings_dialog.styles = styles,
                Reply::Saved(Ok(())) => self.status = "Settings saved".into(),
                Reply::Catalog(Err(e)) | Reply::Saved(Err(e)) => self.status = e,
            }
        }
        if self.settings_dialog.pending > 0 {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }
    pub(crate) fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.settings_open {
            return;
        }
        let mut config = self.config.clone();
        let modal = egui::Modal::new(egui::Id::new("settings"))
            .frame(
                egui::Frame::new()
                    .fill(egui::Color32::from_rgb(30, 31, 34))
                    .corner_radius(8)
                    .inner_margin(16),
            )
            .show(ctx, |ui| {
                let size = (ctx.screen_rect().size() - egui::vec2(64.0, 64.0))
                    .min(egui::vec2(690.0, 380.0))
                    .max(egui::Vec2::ZERO);
                ui.set_width(size.x);
                ui.visuals_mut().window_corner_radius = egui::CornerRadius::same(8);
                ui.visuals_mut().widgets.inactive.weak_bg_fill =
                    egui::Color32::from_rgb(43, 45, 48);
                ui.heading("Settings");
                ui.separator();
                if self.settings_dialog.content(ui, &mut config, size) {
                    self.plugins_open = true;
                    self.settings_open = false;
                }
                ui.separator();
                if ui.button("Close").clicked() {
                    self.settings_open = false;
                }
            });
        if config != self.config {
            match self.settings.replace(config.clone()) {
                Ok(()) => {
                    self.config = config.clone();
                    if self
                        .settings_dialog
                        .requests
                        .send(Request::Save(config))
                        .is_ok()
                    {
                        self.settings_dialog.pending += 1;
                    }
                }
                Err(e) => self.status = e.to_string(),
            }
        }
        if self.settings_dialog.pending > 0 {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        if modal.should_close() {
            self.settings_open = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_fit_viewport_and_close_is_accessible() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let bus = nozhik_event_bus::EventBus::new(32)?;
        let settings = ConfigManager::from_path(dir.path().join("settings.toml"), bus.clone())?;
        let mut shell = crate::NozhikUi::new(bus, settings, Config::default());
        for size in [
            egui::vec2(1440.0, 900.0),
            egui::vec2(800.0, 600.0),
            egui::vec2(420.0, 280.0),
        ] {
            for category in [0, 1] {
                let ctx = egui::Context::default();
                crate::theme::apply(&ctx);
                shell.settings_open = true;
                shell.settings_dialog.category = category;
                let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
                let mut close = None;
                let mut visible_control = false;
                for _ in 0..4 {
                    let output = ctx.run(
                        egui::RawInput {
                            screen_rect: Some(viewport),
                            ..Default::default()
                        },
                        |ctx| shell.settings_window(ctx),
                    );
                    for shape in output.shapes {
                        if let egui::Shape::Text(text) = &shape.shape {
                            let label = if category == 0 {
                                "Editor font"
                            } else {
                                "Formatting Standard"
                            };
                            if text.galley.job.text == label {
                                let rect = egui::Rect::from_min_size(text.pos, text.galley.size());
                                visible_control |= viewport.contains_rect(rect)
                                    && shape.clip_rect.contains_rect(rect);
                            }
                        }
                        if let egui::Shape::Text(text) = shape.shape
                            && text.galley.job.text == "Close"
                        {
                            let rect = egui::Rect::from_min_size(text.pos, text.galley.size());
                            assert!(
                                viewport.contains_rect(rect),
                                "Close outside {size:?}: {rect:?}"
                            );
                            assert!(shape.clip_rect.contains_rect(rect));
                            close = Some(rect.center());
                        }
                    }
                }
                let bounds = ctx
                    .memory(|m| m.area_rect(egui::Id::new("settings")))
                    .ok_or("missing Settings")?;
                assert!(
                    viewport.contains_rect(bounds),
                    "Settings outside {size:?}: {bounds:?}"
                );
                assert!(
                    bounds.height() <= 460.0,
                    "Unbounded Settings height: {bounds:?}"
                );
                assert!(
                    visible_control,
                    "Settings controls must be visible for category {category} at {size:?}"
                );
                let close = close.ok_or("missing Close button")?;
                for pressed in [true, false] {
                    let _frame = ctx.run(
                        egui::RawInput {
                            screen_rect: Some(viewport),
                            events: vec![
                                egui::Event::PointerMoved(close),
                                egui::Event::PointerButton {
                                    pos: close,
                                    button: egui::PointerButton::Primary,
                                    pressed,
                                    modifiers: egui::Modifiers::NONE,
                                },
                            ],
                            ..Default::default()
                        },
                        |ctx| shell.settings_window(ctx),
                    );
                }
                assert!(!shell.settings_open, "Close must respond to pointer input");
            }
        }
        Ok(())
    }
}
