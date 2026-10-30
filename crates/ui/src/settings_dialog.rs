//! Settings categories and background profile discovery/persistence.
use nozhik_config::{
    Config, ConfigManager,
    code_style::{CodeStyles, FileTypeSettings},
};
use std::{path::PathBuf, sync::mpsc};

enum Request {
    Scan,
    Save(Config),
    Php(String),
}
enum Reply {
    Catalog(Result<CodeStyles, String>),
    Saved(Result<(), String>),
    Php(
        String,
        Result<nozhik_language_support::php_stubs::PhpVersionStubs, String>,
    ),
}
pub struct SettingsDialog {
    pub(crate) ai: crate::ai_inline::Models,
    pub styles: CodeStyles,
    pub php_stubs: Option<std::sync::Arc<nozhik_language_support::php_stubs::PhpVersionStubs>>,
    php_requested: String,
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
                    Request::Php(version) => {
                        let result = std::env::var_os("HOME")
                            .or_else(|| std::env::var_os("USERPROFILE"))
                            .map(PathBuf::from)
                            .ok_or_else(|| "PHP catalog: user home unavailable".to_owned())
                            .and_then(|home| {
                                nozhik_language_support::php_stubs::PhpVersionStubs::load(
                                    &home.join(".nozhik/languages/php/versions"),
                                    &version,
                                )
                                .map_err(|e| e.to_string())
                            });
                        Reply::Php(version, result)
                    }
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
            ai: crate::ai_inline::Models::default(),
            styles: CodeStyles::builtin().unwrap_or_default(),
            // Large version catalogs are parsed only by the worker on demand.
            php_stubs: None,
            php_requested: String::new(),
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
        ui.selectable_value(&mut self.category, 2, "AI Assistant (Ollama)");
        ui.selectable_value(&mut self.category, 3, "Languages");
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
                    } else if self.category == 1 {
                        self.file_types(ui, config);
                    } else if self.category == 2 {
                        self.ai.show(ui, config);
                    } else {
                        Self::languages(ui, config);
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
    fn languages(ui: &mut egui::Ui, config: &mut Config) {
        ui.heading("Languages");
        ui.separator();
        for language in nozhik_config::languages::LANGUAGES {
            let mut enabled = config.enabled_languages.contains(&language.id);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        language.available,
                        egui::Checkbox::new(&mut enabled, language.label),
                    )
                    .changed()
                {
                    if enabled {
                        config.enabled_languages.insert(language.id);
                    } else {
                        config.enabled_languages.remove(&language.id);
                    }
                }
                ui.weak(format!(".{}", language.extensions.join(", .")));
                if !language.available {
                    ui.weak("Coming soon");
                }
            });
        }
        ui.add_space(8.0);
        ui.label("Changes apply immediately.");
        ui.weak("Disabled files remain editable as plain text and searchable by name.");
        ui.weak("Disabling a language stops its indexing, diagnostics, syntax highlighting and completion. Its disk cache is retained for later use.");
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
        if matches!(self.extension.as_str(), "php" | "phtml") {
            egui::ComboBox::from_label("PHP Language Level")
                .selected_text(&config.php_language_level)
                .show_ui(ui, |ui| {
                    for version in nozhik_language_support::php_stubs::VERSIONS {
                        ui.selectable_value(
                            &mut config.php_language_level,
                            (*version).into(),
                            *version,
                        );
                    }
                });
            let actual = self.php_stubs.as_ref().map_or_else(
                || {
                    if nozhik_config::languages::enabled(
                        &config.enabled_languages,
                        nozhik_config::languages::LanguageId::Php,
                    ) {
                        "loading…"
                    } else {
                        "disabled"
                    }
                },
                |s| s.version.as_str(),
            );
            ui.weak(format!("Offline catalog: PHP {actual}"));
        }
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
        let php_enabled = nozhik_config::languages::enabled(
            &self.config.enabled_languages,
            nozhik_config::languages::LanguageId::Php,
        );
        if !php_enabled {
            self.settings_dialog.php_stubs = None;
            self.settings_dialog.php_requested.clear();
        }
        let needs_php = php_enabled
            && (self.settings_open
                || self.documents.iter().any(|document| {
                    document.open
                        && !document.binary
                        && nozhik_language_support::lsp::manager::is_php(&document.path)
                }));
        if needs_php
            && self.settings_dialog.php_requested != self.config.php_language_level
            && self
                .settings_dialog
                .requests
                .send(Request::Php(self.config.php_language_level.clone()))
                .is_ok()
        {
            self.settings_dialog
                .php_requested
                .clone_from(&self.config.php_language_level);
            self.settings_dialog.pending += 1;
        }
        for reply in self.settings_dialog.replies.try_iter() {
            self.settings_dialog.pending = self.settings_dialog.pending.saturating_sub(1);
            match reply {
                Reply::Catalog(Ok(styles)) => self.settings_dialog.styles = styles,
                Reply::Php(version, result) => {
                    if php_enabled && version == self.config.php_language_level {
                        match result {
                            Ok(stubs) => {
                                self.settings_dialog.php_stubs = Some(std::sync::Arc::new(stubs));
                                for view in self.language.views.values_mut() {
                                    view.completions.clear();
                                    view.token = 0;
                                    view.request = None;
                                }
                            }
                            Err(e) => self.status = e,
                        }
                    }
                }
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
                let scope_height = if self.project.is_some() { 42.0 } else { 0.0 };
                let size = (ctx.screen_rect().size() - egui::vec2(64.0, 64.0 + scope_height))
                    .min(egui::vec2(690.0, 380.0 - scope_height))
                    .max(egui::Vec2::ZERO);
                ui.set_width(size.x);
                ui.visuals_mut().window_corner_radius = egui::CornerRadius::same(8);
                ui.visuals_mut().widgets.inactive.weak_bg_fill =
                    egui::Color32::from_rgb(43, 45, 48);
                ui.heading("Settings");
                if let Some(project) = &self.project {
                    ui.label(format!(
                        "Project: {}",
                        crate::workspace::project_name(&project.root_path)
                    ));
                    ui.small(project.root_path.display().to_string());
                }
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

    fn language_settings_frame(
        shell: &mut crate::NozhikUi,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 600.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                shell.language_tick(ctx);
                shell.settings_tick(ctx);
                shell.settings_window(ctx);
            },
        )
    }
    fn click_php_setting(
        shell: &mut crate::NozhikUi,
        ctx: &egui::Context,
    ) -> Result<(), Box<dyn std::error::Error>> {
        language_settings_frame(shell, ctx, vec![]);
        let output = language_settings_frame(shell, ctx, vec![]);
        let pos = output
            .shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.job.text == "PHP" => {
                    Some(t.pos + t.galley.size() * 0.5)
                }
                _ => None,
            })
            .ok_or("PHP toggle not visible")?;
        for pressed in [true, false] {
            language_settings_frame(
                shell,
                ctx,
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
        Ok(())
    }
    fn wait_language_settings(
        shell: &mut crate::NozhikUi,
        ctx: &egui::Context,
        php_enabled: bool,
        path: &std::path::Path,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let until = std::time::Instant::now() + std::time::Duration::from_secs(8);
        loop {
            language_settings_frame(shell, ctx, vec![]);
            let ready = if php_enabled {
                shell.language.indexer.is_some()
                    && shell
                        .language
                        .views
                        .get(path)
                        .is_some_and(|v| v.project_index.is_some())
            } else {
                shell.language.indexer.is_none()
                    && shell.language.views.is_empty()
                    && shell.settings_dialog.php_stubs.is_none()
            };
            if ready && shell.settings_dialog.pending == 0 {
                return Ok(());
            }
            if std::time::Instant::now() > until {
                return Err(format!("language policy/save timeout: {}", shell.status).into());
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
    #[test]
    fn language_checkboxes_persist_and_stop_restart_services_with_open_buffers()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let root = dir.path().canonicalize()?;
        let path = root.join("sample.php");
        let source = "<?php class Sample { public function run() {} }";
        std::fs::write(&path, source)?;
        let bus = nozhik_event_bus::EventBus::new(32)?;
        let settings_path = root.join("settings.toml");
        let settings = ConfigManager::from_path(settings_path.clone(), bus.clone())?;
        let config = Config {
            ai_enabled: false,
            enabled_languages: std::collections::BTreeSet::new(),
            ..Default::default()
        };
        settings.replace(config.clone())?;
        settings.save()?;
        let mut shell = crate::NozhikUi::new(bus, settings, config);
        shell.project = Some(nozhik_fs_model::tree::ProjectTree::new(&root)?);
        shell.documents.push(crate::Document {
            path: path.clone(),
            name: "sample.php".into(),
            binary: false,
            open: true,
            buffer: nozhik_buffer::TextBuffer::load_from_str(nozhik_core::BufferId(1901), source),
        });
        shell.active = Some(0);
        shell.settings_open = true;
        shell.settings_dialog.category = 3;
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        wait_language_settings(&mut shell, &ctx, false, &path)?;
        assert!(
            !root.join(".nozhik/cache").exists(),
            "disabled startup cannot scan/cache PHP"
        );
        click_php_setting(&mut shell, &ctx)?;
        wait_language_settings(&mut shell, &ctx, true, &path)?;
        assert!(
            shell
                .config
                .enabled_languages
                .contains(&nozhik_config::languages::LanguageId::Php)
        );
        let cache_path = root.join(".nozhik/cache/index_php.json");
        let modified = cache_path.metadata()?.modified()?;
        click_php_setting(&mut shell, &ctx)?;
        wait_language_settings(&mut shell, &ctx, false, &path)?;
        let loaded = ConfigManager::from_path(settings_path, nozhik_event_bus::EventBus::new(8)?)?
            .snapshot()?;
        assert!(loaded.enabled_languages.is_empty());
        shell.reformat_code(&ctx);
        assert!(shell.status.contains("disabled"));
        assert_eq!(shell.documents[0].buffer.rope().to_string(), source);
        assert!(
            cache_path.exists(),
            "disabled languages retain their disk cache"
        );
        click_php_setting(&mut shell, &ctx)?;
        wait_language_settings(&mut shell, &ctx, true, &path)?;
        assert_eq!(
            cache_path.metadata()?.modified()?,
            modified,
            "re-enable reuses unchanged disk facts instead of rewriting cache"
        );
        Ok(())
    }

    #[test]
    fn php_level_combo_changes_global_setting() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let mut dialog = SettingsDialog::start(
            dir.path().join("settings.toml"),
            nozhik_event_bus::EventBus::new(8)?,
        );
        let ctx = egui::Context::default();
        let mut config = Config::default();
        let mut frame = |events| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(900.0, 600.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default()
                        .show(ctx, |ui| dialog.file_types(ui, &mut config));
                },
            )
        };
        let output = frame(vec![]);
        let text_pos = |output: &egui::FullOutput, label: &str| {
            output.shapes.iter().find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.job.text == label => {
                    Some(t.pos + t.galley.size() / 2.0)
                }
                _ => None,
            })
        };
        assert!(text_pos(&output, "PHP Language Level").is_some());
        let pos = text_pos(&output, "8.5").ok_or("version selector")?;
        for pressed in [true, false] {
            frame(vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ]);
        }
        let output = frame(vec![]);
        let pos = text_pos(&output, "7.4").ok_or("version option")?;
        for pressed in [true, false] {
            frame(vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ]);
        }
        assert_eq!(config.php_language_level, "7.4");
        config.validate()?;
        dialog.finish();
        Ok(())
    }

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
            for category in [0, 1, 3] {
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
                            let label = match category {
                                0 => "Editor font",
                                1 => "Formatting Standard",
                                _ => "PHP",
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
