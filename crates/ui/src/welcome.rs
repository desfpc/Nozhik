//! Project launcher, shown until a project has successfully opened.
use crate::{NozhikUi, repository_clone::CloneDialog};

#[derive(Default)]
pub struct Welcome {
    pub clone: CloneDialog,
    filter: String,
    about: bool,
    logo: Option<egui::TextureHandle>,
    transitioned: bool,
    #[cfg(test)]
    rows: Vec<egui::Rect>,
}

impl NozhikUi {
    pub(crate) fn welcome_transition(&mut self, ctx: &egui::Context) {
        if !self.welcome.transitioned {
            self.welcome.clone.leave_launcher();
            self.welcome.transitioned = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(1440.0, 900.0)));
            ctx.send_viewport_cmd(egui::ViewportCommand::Title("Nozhik".into()));
        }
    }
    pub(crate) fn welcome_screen(&mut self, ctx: &egui::Context) {
        if let Some(path) = self.welcome.clone.tick() {
            self.open_project(path);
        }
        if self.welcome.logo.is_none()
            && let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!(
                "../../app/assets/window-icon.png"
            ))
        {
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [icon.width as usize, icon.height as usize],
                &icon.rgba,
            );
            self.welcome.logo =
                Some(ctx.load_texture("nozhik_logo", image, egui::TextureOptions::LINEAR));
        }
        let blocked = self.welcome.clone.open;
        self.welcome_navigation(ctx, blocked);
        let mut selected = None;
        let mut open = false;
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(24.0);
            ui.add_enabled_ui(!blocked, |ui| {
                if self.welcome.about {
                    ui.heading("About Nozhik");
                    ui.add_space(16.0);
                    ui.hyperlink_to(
                        "github.com/desfpc/Nozhik",
                        "https://github.com/desfpc/Nozhik",
                    );
                    return;
                }
                ui.horizontal(|ui| {
                    let width = (ui.available_width() - 260.0).max(100.0);
                    ui.add(
                        egui::TextEdit::singleline(&mut self.welcome.filter)
                            .hint_text("Search projects")
                            .desired_width(width),
                    );
                    open = ui.button("Open Folder…").clicked();
                    if ui
                        .add_enabled(
                            !self.welcome.clone.busy(),
                            egui::Button::new("Clone Repository…"),
                        )
                        .clicked()
                    {
                        self.welcome.clone.open = true;
                    }
                });
                ui.add_space(20.0);
                ui.separator();
                if let Some(error) = &self.recent_projects.error {
                    ui.colored_label(crate::theme::ERROR, format!("Project history: {error}"));
                }
                let filter = self.welcome.filter.to_lowercase();
                #[cfg(test)]
                self.welcome.rows.clear();
                egui::ScrollArea::vertical()
                    .id_salt("welcome_projects")
                    .show(ui, |ui| {
                        let mut count = 0;
                        for entry in &self.recent_projects.entries {
                            let path = entry.path.display().to_string();
                            if !path.to_lowercase().contains(&filter) {
                                continue;
                            }
                            count += 1;
                            let response = project_row(ui, &entry.path);
                            #[cfg(test)]
                            self.welcome.rows.push(response.rect);
                            if response.clicked() {
                                selected = Some(entry.path.clone());
                            }
                        }
                        if count == 0 {
                            ui.add_space(28.0);
                            ui.weak(if filter.is_empty() {
                                "Open a local folder or clone a repository to get started."
                            } else {
                                "No matching projects"
                            });
                        }
                    });
            });
            ui.separator();
            ui.weak(&self.status);
        });
        if open {
            self.choose_project_folder(ctx);
        }
        if let Some(path) = selected {
            self.open_project(path);
        }
        self.welcome.clone.show(ctx);
    }
    fn welcome_navigation(&mut self, ctx: &egui::Context, blocked: bool) {
        egui::SidePanel::left("welcome_navigation")
            .exact_width(235.0)
            .resizable(false)
            .show(ctx, |ui| {
                ui.add_space(30.0);
                ui.horizontal(|ui| {
                    if let Some(logo) = &self.welcome.logo {
                        ui.image((logo.id(), egui::vec2(56.0, 56.0)));
                    }
                    ui.vertical(|ui| {
                        ui.heading("Nozhik");
                        ui.weak(env!("CARGO_PKG_VERSION"));
                    });
                });
                ui.add_space(32.0);
                ui.add_enabled_ui(!blocked, |ui| {
                    if ui
                        .add_sized(
                            [210.0, 40.0],
                            egui::Button::selectable(!self.welcome.about, "Projects"),
                        )
                        .clicked()
                    {
                        self.welcome.about = false;
                    }
                    if ui
                        .add_sized(
                            [210.0, 40.0],
                            egui::Button::selectable(self.welcome.about, "About"),
                        )
                        .clicked()
                    {
                        self.welcome.about = true;
                    }
                });
            });
    }
}

fn project_row(ui: &mut egui::Ui, path: &std::path::Path) -> egui::Response {
    let name = path
        .file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy();
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 76.0), egui::Sense::click());
    let painter = ui.painter().with_clip_rect(rect);
    if response.hovered() {
        painter.rect_filled(rect, 6.0, ui.visuals().widgets.hovered.bg_fill);
    }
    let badge = egui::Rect::from_center_size(
        rect.left_center() + egui::vec2(28.0, 0.0),
        egui::vec2(36.0, 36.0),
    );
    painter.rect_filled(badge, 8.0, egui::Color32::from_rgb(71, 100, 170));
    painter.text(
        badge.center(),
        egui::Align2::CENTER_CENTER,
        name.chars()
            .next()
            .unwrap_or('N')
            .to_uppercase()
            .to_string(),
        egui::FontId::proportional(20.0),
        egui::Color32::WHITE,
    );
    let origin = rect.left_top() + egui::vec2(60.0, 17.0);
    painter.text(
        origin,
        egui::Align2::LEFT_TOP,
        name,
        egui::FontId::proportional(19.0),
        ui.visuals().text_color(),
    );
    painter.text(
        origin + egui::vec2(0.0, 27.0),
        egui::Align2::LEFT_TOP,
        path.display().to_string(),
        egui::FontId::proportional(13.0),
        ui.visuals().weak_text_color(),
    );
    response.on_hover_text(path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use nozhik_config::{Config, ConfigManager};
    use nozhik_event_bus::EventBus;
    use std::time::{Duration, Instant};

    #[test]
    fn launcher_selects_recent_project_and_opens_only_on_success()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let project = root.path().join("My project");
        std::fs::create_dir(&project)?;
        let bus = EventBus::new(32)?;
        let settings = ConfigManager::from_path(root.path().join("settings.toml"), bus.clone())?;
        let mut shell = NozhikUi::new(bus, settings, Config::default());
        shell
            .recent_projects
            .entries
            .push(crate::recent_projects::RecentProject {
                path: project.clone(),
                last_opened: 1,
            });
        let ctx = egui::Context::default();
        let frame = |shell: &mut NozhikUi, events| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1000.0, 680.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| shell.welcome_screen(ctx),
            )
        };
        let _ = frame(&mut shell, vec![]);
        assert!(shell.project.is_none());
        assert!(!shell.welcome.transitioned);
        assert!(shell.terminal.is_none());
        shell.open_project(root.path().join("missing-project"));
        let failed_deadline = Instant::now() + Duration::from_secs(5);
        while shell.status.starts_with("Opening") {
            shell.receive_files();
            if Instant::now() > failed_deadline {
                return Err("Missing-project error timed out".into());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(shell.project.is_none());
        assert!(!shell.welcome.transitioned);
        shell.welcome.filter = "does not exist".into();
        let _ = frame(&mut shell, vec![]);
        assert_eq!(shell.welcome.rows, [] as [egui::Rect; 0]);
        shell.welcome.filter.clear();
        let _ = frame(&mut shell, vec![]);
        let position = shell.welcome.rows[0].center();
        for pressed in [true, false] {
            let _ = frame(
                &mut shell,
                vec![
                    egui::Event::PointerMoved(position),
                    egui::Event::PointerButton {
                        pos: position,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while shell.project.is_none() {
            shell.receive_files();
            if Instant::now() > deadline {
                return Err(shell.status.clone().into());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            shell.project.as_ref().map(|p| &p.root_path),
            Some(&project.canonicalize()?)
        );
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            shell.welcome_transition(ctx);
        });
        assert!(shell.welcome.transitioned);
        assert!(output.viewport_output.values().any(|v| {
            v.commands
                .iter()
                .any(|c| matches!(c, egui::ViewportCommand::InnerSize(_)))
        }));
        shell.recent_projects.finish();
        Ok(())
    }
}
