//! Window coordinator. Each project owns its workers, settings and editor state.
use crate::{NozhikUi, integration::CloseTarget, theme};
use egui::{Color32, Context};
use nozhik_config::{Config, ConfigManager};
use nozhik_event_bus::EventBus;
use std::{
    path::{Path, PathBuf},
    sync::mpsc,
};

#[derive(Default)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "Independent window ownership, render phase and queued close intents"
)]
pub struct SessionLinks {
    pub managed: bool,
    pub open: Option<PathBuf>,
    pub entries: Vec<PathBuf>,
    pub switch: Option<usize>,
    pub close: Option<usize>,
    pub reorder: Option<(usize, usize)>,
    pub closed: bool,
    pub exit: bool,
    pub failed: Option<String>,
    pub background: bool,
    dragged: Option<ProjectDrag>,
    tab_offset: f32,
    #[cfg(test)]
    tab_viewport: Option<egui::Rect>,
    #[cfg(test)]
    tab_scroll_id: Option<egui::Id>,
    #[cfg(test)]
    tab_buttons: Vec<egui::Rect>,
    #[cfg(test)]
    tab_rects: Vec<egui::Rect>,
    #[cfg(test)]
    switcher: Option<egui::Rect>,
}

struct ProjectDrag {
    path: PathBuf,
    origin: egui::Pos2,
    moving: bool,
}

pub fn project_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}

type OpenResult = Result<(PathBuf, Option<(EventBus, ConfigManager, Config)>), String>;
struct Prepared {
    origin: Option<PathBuf>,
    result: OpenResult,
}

/// One native window containing independently owned project sessions.
pub struct Workspace {
    launcher: NozhikUi,
    projects: Vec<NozhikUi>,
    active: Option<usize>,
    runtime: tokio::runtime::Handle,
    prepared: mpsc::Receiver<Prepared>,
    output: mpsc::Sender<Prepared>,
    opening: usize,
    next_session: u64,
    closing_window: bool,
    closing_project: bool,
    ai: tokio::sync::watch::Sender<Option<String>>,
    last_title: String,
}

impl Workspace {
    /// Wraps the launcher; project settings and canonical paths load on a worker.
    #[must_use]
    pub fn new(
        mut launcher: NozhikUi,
        runtime: tokio::runtime::Handle,
        ai: tokio::sync::watch::Sender<Option<String>>,
    ) -> Self {
        launcher.workspace.managed = true;
        let (output, prepared) = mpsc::channel();
        Self {
            launcher,
            projects: Vec::new(),
            active: None,
            runtime,
            prepared,
            output,
            opening: 0,
            next_session: 1,
            closing_window: false,
            closing_project: false,
            ai,
            last_title: String::new(),
        }
    }

    /// Requests opening a folder in a new tab, or a file in the current project.
    pub fn open_path(&mut self, path: PathBuf, ctx: &Context) {
        self.current().status = format!("Opening {}…", path.display());
        self.opening += 1;
        let origin = self
            .active
            .and_then(|i| self.projects.get(i))
            .map(|project| project.settings.path().to_path_buf());
        let output = self.output.clone();
        let wake = ctx.clone();
        std::thread::spawn(move || {
            let result = (|| {
                let path = std::fs::canonicalize(path).map_err(|e| e.to_string())?;
                if !path.is_dir() {
                    return Ok((path, None));
                }
                let bus = EventBus::new(1024).map_err(|e| e.to_string())?;
                let settings =
                    ConfigManager::from_path(path.join(".nozhik/settings.toml"), bus.clone())
                        .map_err(|e| e.to_string())?;
                let config = settings.snapshot().map_err(|e| e.to_string())?;
                Ok((path, Some((bus, settings, config))))
            })();
            let _sent = output.send(Prepared { origin, result });
            wake.request_repaint();
        });
    }

    fn current(&mut self) -> &mut NozhikUi {
        self.active
            .and_then(|i| self.projects.get_mut(i))
            .unwrap_or(&mut self.launcher)
    }

    fn activate(&mut self, index: usize) {
        if index >= self.projects.len() || self.active == Some(index) {
            return;
        }
        self.current().editor.ai.cancel();
        #[cfg(target_os = "macos")]
        let native = self.current().native_menu.take();
        self.active = Some(index);
        #[cfg(target_os = "macos")]
        {
            self.current().native_menu = native;
        }
        self.current().editor_focus = None;
    }

    fn accept_opened(&mut self, ctx: &Context) {
        if self.closing_window || self.closing_project || self.current().global_modal() {
            return;
        }
        let replies: Vec<_> = self.prepared.try_iter().collect();
        for reply in replies {
            self.opening = self.opening.saturating_sub(1);
            match reply.result {
                Err(error) => self.current().status = error,
                Ok((path, None)) => {
                    if let Some(origin) = reply.origin {
                        if let Some(project) = self
                            .projects
                            .iter_mut()
                            .find(|p| p.settings.path() == origin)
                        {
                            project.open_file(path);
                        }
                    } else {
                        self.launcher.open_file(path);
                    }
                }
                Ok((path, Some((bus, settings, config)))) => {
                    if let Some(index) = self
                        .projects
                        .iter()
                        .position(|p| p.settings.path() == settings.path())
                    {
                        self.activate(index);
                        continue;
                    }
                    let mut project = NozhikUi::new(bus, settings, config);
                    project.next_buffer_id = self.next_session.saturating_mul(1_000_000_000);
                    self.next_session += 1;
                    // Queue the initial root before enabling routing to this coordinator.
                    project.open_project(path);
                    project.workspace.managed = true;
                    project.attach_runtime(ctx, &self.runtime);
                    self.launcher.recent_projects.note(
                        project
                            .settings
                            .path()
                            .parent()
                            .and_then(Path::parent)
                            .unwrap_or_else(|| Path::new("/"))
                            .to_path_buf(),
                    );
                    self.projects.push(project);
                    self.activate(self.projects.len() - 1);
                }
            }
        }
    }

    fn close_active(&mut self, ctx: &Context) {
        #[cfg(target_os = "macos")]
        let native = self.current().native_menu.take();
        if let Some(index) = self.active.take() {
            self.projects.remove(index);
            self.active = (!self.projects.is_empty()).then(|| index.min(self.projects.len() - 1));
        }
        #[cfg(target_os = "macos")]
        {
            self.current().native_menu = native;
        }
        self.closing_project = false;
        if self.projects.is_empty() && self.closing_window {
            self.launcher.integration.confirmed = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn tick_background(&mut self, ctx: &Context) {
        for project in &mut self.projects {
            project
                .recent_projects
                .entries
                .clone_from(&self.launcher.recent_projects.entries);
        }
        // Drain inactive worker queues as well, preserving their independent background state.
        for (index, project) in self.projects.iter_mut().enumerate() {
            if self.active == Some(index) {
                continue;
            }
            project.workspace.background = true;
            project.receive_files();
            project.receive_events();
            project.receive_remote(ctx);
            project.database_tick(ctx);
            project.language_tick(ctx);
            project.settings_tick(ctx);
            project.vcs.tick_background(ctx);
            if let Some(terminal) = &mut project.terminal {
                terminal.tick();
            }
            project.workspace.background = false;
        }
    }

    fn update(&mut self, ctx: &Context) {
        self.launcher.recent_projects.tick(ctx);
        self.accept_opened(ctx);
        self.tick_background(ctx);
        if ctx.input(|i| i.viewport().close_requested()) && !self.launcher.integration.confirmed {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.closing_window = true;
        }
        if self.closing_window && !self.closing_project {
            if self.active.is_some() {
                self.current().request_close(CloseTarget::Project, ctx);
                self.closing_project = true;
            } else {
                self.launcher.integration.confirmed = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
        let entries = self
            .projects
            .iter()
            .map(|p| {
                p.settings
                    .path()
                    .parent()
                    .and_then(Path::parent)
                    .unwrap_or_else(|| Path::new(""))
                    .to_path_buf()
            })
            .collect();
        self.current().workspace.entries = entries;
        self.current().update_frame(ctx);
        let desired = self
            .active
            .and_then(|i| self.projects.get(i))
            .and_then(|project| {
                project
                    .config
                    .ai_enabled
                    .then(|| project.config.ai_model.clone())
            });
        self.ai.send_if_modified(|old| {
            if *old == desired {
                false
            } else {
                *old = desired;
                true
            }
        });
        if let Some(error) = self.current().workspace.failed.take() {
            self.close_active(ctx);
            self.current().status = error;
            ctx.request_repaint();
        }
        if let Some(path) = self.current().workspace.open.take() {
            self.open_path(path, ctx);
        }
        if self.current().workspace.exit {
            self.current().workspace.exit = false;
            self.closing_window = true;
            ctx.request_repaint();
        }
        if self.current().workspace.closed {
            self.close_active(ctx);
            ctx.request_repaint();
        } else if self.closing_project && self.current().integration.close.is_none() {
            // Cancel aborts a window-wide close sequence, including inactive projects.
            self.closing_project = false;
            self.closing_window = false;
        }
        if !self.current().global_modal() && !self.closing_project {
            let switch = self.current().workspace.switch.take();
            let close = self.current().workspace.close.take();
            let reorder = self.current().workspace.reorder.take();
            if let Some(index) = close {
                self.activate(index);
                self.current().request_close(CloseTarget::Project, ctx);
                self.closing_project = true;
                ctx.request_repaint();
            } else if let Some(index) = switch {
                self.activate(index);
                ctx.request_repaint();
            }
            if let Some((from, to)) = reorder {
                self.reorder(from, to);
                ctx.request_repaint();
            }
        }
        let title = self.current().project.as_ref().map_or_else(
            || "Nozhik".into(),
            |project| format!("{} — Nozhik", project_name(&project.root_path)),
        );
        if title != self.last_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.last_title = title;
        }
    }

    fn reorder(&mut self, from: usize, to: usize) {
        if from >= self.projects.len() || to >= self.projects.len() {
            return;
        }
        let active_path = self
            .active
            .and_then(|i| self.projects.get(i))
            .map(|p| p.settings.path().to_path_buf());
        let project = self.projects.remove(from);
        self.projects.insert(to, project);
        self.active = self
            .projects
            .iter()
            .position(|p| Some(p.settings.path()) == active_path.as_deref());
    }
}

impl eframe::App for Workspace {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        theme::CHROME.to_normalized_gamma_f32()
    }
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        self.update(ctx);
    }
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        for project in &mut self.projects {
            project.shutdown_systems();
        }
        self.launcher.shutdown_systems();
    }
}

impl NozhikUi {
    #[allow(
        clippy::too_many_lines,
        reason = "Project switcher shares its popup and anchor geometry"
    )]
    pub(crate) fn project_switcher(&mut self, ui: &mut egui::Ui) {
        let name = self
            .project
            .as_ref()
            .map_or_else(|| "Nozhik".into(), |p| project_name(&p.root_path));
        let width = ui.fonts(|f| {
            f.layout_no_wrap(
                name.clone(),
                egui::FontId::proportional(14.0),
                Color32::WHITE,
            )
            .size()
            .x
        }) + 61.0;
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(width, 30.0), egui::Sense::click());
        #[cfg(test)]
        {
            self.workspace.switcher = Some(rect);
        }
        let painter = ui.painter();
        if response.hovered()
            || egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&response))
        {
            painter.rect_filled(rect, 7.0, Color32::from_rgb(57, 59, 62));
        }
        crate::project_badge::paint(
            painter,
            egui::Rect::from_min_size(rect.min + egui::vec2(8.0, 5.0), egui::vec2(20.0, 20.0)),
            &name,
        );
        painter.text(
            rect.left_center() + egui::vec2(35.0, 0.0),
            egui::Align2::LEFT_CENTER,
            name,
            egui::FontId::proportional(14.0),
            ui.visuals().text_color(),
        );
        let center = rect.right_center() - egui::vec2(12.0, 0.0);
        painter.add(egui::Shape::line(
            vec![
                center + egui::vec2(-3.0, -1.5),
                center + egui::vec2(0.0, 1.5),
                center + egui::vec2(3.0, -1.5),
            ],
            egui::Stroke::new(1.2_f32, Color32::GRAY),
        ));
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Open Projects")
        });
        let entries = self.workspace.entries.clone();
        let recent = self.recent_projects.entries.clone();
        egui::Popup::menu(&response)
            .frame(
                egui::Frame::popup(ui.style())
                    .fill(Color32::from_rgb(37, 39, 41))
                    .corner_radius(9)
                    .inner_margin(7)
                    .stroke(egui::Stroke::new(1.0_f32, Color32::from_rgb(88, 91, 94))),
            )
            .show(|ui| {
                ui.set_width(350.0_f32.min(ui.ctx().screen_rect().width() - 40.0));
                ui.spacing_mut().item_spacing.y = 3.0;
                if project_menu_action(ui, "Open…", crate::tool_icons::Icon::Project).clicked() {
                    ui.close();
                    self.choose_project_folder(ui.ctx());
                }
                if project_menu_action(ui, "Clone Repository…", crate::tool_icons::Icon::Git)
                    .clicked()
                {
                    self.welcome.clone.open = true;
                    ui.close();
                }
                ui.separator();
                egui::ScrollArea::vertical()
                    .max_height((ui.ctx().screen_rect().height() - 150.0).max(100.0))
                    .show(ui, |ui| {
                        ui.weak("Open Projects");
                        for (index, path) in entries.iter().enumerate() {
                            if project_menu_row(ui, path).clicked() {
                                self.workspace.switch = Some(index);
                                ui.close();
                            }
                        }
                        ui.separator();
                        ui.weak("Recent Projects");
                        for entry in recent.iter().filter(|p| !entries.contains(&p.path)) {
                            if project_menu_row(ui, &entry.path).clicked() {
                                self.open_project(entry.path.clone());
                                ui.close();
                            }
                        }
                    });
            });
    }

    #[allow(
        clippy::too_many_lines,
        reason = "One project row shares scroll, close, click and drag geometry"
    )]
    pub(crate) fn project_tabs(&mut self, ctx: &Context) {
        if self.workspace.entries.is_empty() {
            return;
        }
        #[cfg(test)]
        {
            self.workspace.tab_rects.clear();
            self.workspace.tab_buttons.clear();
        }
        let entries = self.workspace.entries.clone();
        let active = self.project.as_ref().map(|p| &p.root_path);
        let blocked = self.global_modal();
        let press_origin = ctx.input(|i| {
            i.events.iter().find_map(|event| match event {
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    ..
                } => Some(*pos),
                _ => None,
            })
        });
        let mut clicked = None;
        let mut rects = Vec::new();
        let mut viewport = egui::Rect::NOTHING;
        egui::TopBottomPanel::top("project_tabs")
            .exact_height(36.0)
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(37, 39, 41))
                    .inner_margin(egui::Margin {
                        left: 6,
                        right: 6,
                        top: 4,
                        bottom: 4,
                    }),
            )
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(2.0, 0.0);
                let count = f32::from(u16::try_from(entries.len()).unwrap_or(u16::MAX));
                let overflow = count.mul_add(141.0, -1.0) > ui.available_width() + 0.5;
                let view_width =
                    (ui.available_width() - if overflow { 52.0 } else { 0.0 }).max(1.0);
                let width = ((view_width - (count - 1.0)) / count).max(140.0);
                ui.horizontal(|ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(view_width, 28.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.set_max_width(view_width);
                            ui.style_mut().always_scroll_the_only_direction = true;
                            let scroll = egui::ScrollArea::horizontal()
                                .id_salt("project_tab_scroll")
                                .scroll_bar_visibility(
                                    egui::scroll_area::ScrollBarVisibility::AlwaysHidden,
                                )
                                .scroll_source(egui::scroll_area::ScrollSource {
                                    scroll_bar: false,
                                    drag: false,
                                    mouse_wheel: true,
                                })
                                .horizontal_scroll_offset(self.workspace.tab_offset)
                                .auto_shrink([false, false])
                                .max_width(view_width)
                                .max_height(28.0)
                                .animated(false)
                                .show(ui, |ui| {
                                    ui.spacing_mut().item_spacing.x = 1.0;
                                    ui.horizontal_top(|ui| {
                                        for (index, path) in entries.iter().enumerate() {
                                            let (rect, response) = ui.allocate_exact_size(
                                                egui::vec2(width, 27.0),
                                                egui::Sense::click_and_drag(),
                                            );
                                            rects.push(rect);
                                            let hovered = ui.rect_contains_pointer(rect);
                                            paint_project_tab(
                                                ui,
                                                rect,
                                                &project_name(path),
                                                active == Some(path),
                                                hovered,
                                            );
                                            let close_rect = egui::Rect::from_center_size(
                                                rect.left_center() + egui::vec2(14.0, 0.0),
                                                egui::vec2(20.0, 22.0),
                                            );
                                            if hovered {
                                                let close = ui
                                                    .interact(
                                                        close_rect,
                                                        response.id.with("close_project"),
                                                        egui::Sense::click(),
                                                    )
                                                    .on_hover_text("Close Project");
                                                paint_project_close(
                                                    ui,
                                                    close_rect,
                                                    close.hovered(),
                                                );
                                                if close.clicked() && !blocked {
                                                    self.workspace.close = Some(index);
                                                }
                                            }
                                            if !blocked
                                                && let Some(origin) = press_origin
                                                && rect.contains(origin)
                                                && ui.clip_rect().contains(origin)
                                                && !close_rect.contains(origin)
                                            {
                                                self.workspace.dragged = Some(ProjectDrag {
                                                    path: path.clone(),
                                                    origin,
                                                    moving: false,
                                                });
                                            }
                                            if response.clicked() && !blocked {
                                                clicked = Some(index);
                                            }
                                            response.on_hover_text(path.display().to_string());
                                        }
                                    });
                                });
                            viewport = scroll.inner_rect;
                            self.workspace.tab_offset = scroll.state.offset.x;
                            #[cfg(test)]
                            {
                                self.workspace.tab_rects.clone_from(&rects);
                                self.workspace.tab_viewport = Some(scroll.inner_rect);
                                self.workspace.tab_scroll_id = Some(scroll.id);
                            }
                        },
                    );
                    if overflow {
                        let maximum = count.mul_add(width + 1.0, -1.0) - view_width;
                        for (direction, enabled) in [
                            (-1.0, self.workspace.tab_offset > 0.5),
                            (1.0, self.workspace.tab_offset < maximum - 0.5),
                        ] {
                            let button = project_scroll_button(ui, direction, enabled && !blocked);
                            #[cfg(test)]
                            self.workspace.tab_buttons.push(button.rect);
                            if button.clicked() {
                                self.workspace.tab_offset = (direction
                                    .mul_add(view_width * 0.75, self.workspace.tab_offset))
                                .clamp(0.0, maximum.max(0.0));
                                ctx.request_repaint();
                            }
                        }
                    }
                });
            });
        self.project_tab_drag(ctx, &entries, &rects, viewport, blocked);
        let moving = self
            .workspace
            .dragged
            .as_ref()
            .is_some_and(|drag| drag.moving);
        if !moving && let Some(index) = clicked {
            self.workspace.switch = Some(index);
        }
        if ctx.input(|i| i.pointer.primary_released() || !i.pointer.primary_down()) {
            self.workspace.dragged = None;
        }
    }

    fn project_tab_drag(
        &mut self,
        ctx: &Context,
        entries: &[PathBuf],
        rects: &[egui::Rect],
        viewport: egui::Rect,
        blocked: bool,
    ) {
        let Some(drag) = &mut self.workspace.dragged else {
            return;
        };
        let Some(pointer) = ctx.pointer_interact_pos() else {
            return;
        };
        if blocked {
            return;
        }
        drag.moving |= (pointer.x - drag.origin.x).abs() > 4.0;
        if !drag.moving {
            return;
        }
        ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        let Some(from) = entries.iter().position(|path| *path == drag.path) else {
            return;
        };
        // Swap as the held pointer crosses a neighbor's midpoint. A stable path keeps
        // ownership across frames after the indices change, for active and inactive tabs.
        if !viewport.expand2(egui::vec2(0.0, 12.0)).contains(pointer) {
            return;
        }
        for (to, rect) in rects.iter().enumerate() {
            if ((to > from && pointer.x >= rect.center().x)
                || (to < from && pointer.x <= rect.center().x))
                && rect.x_range().contains(pointer.x)
            {
                self.workspace.reorder = Some((from, to));
                break;
            }
        }
    }
}

fn paint_project_tab(ui: &egui::Ui, rect: egui::Rect, name: &str, selected: bool, hovered: bool) {
    let fill = if selected {
        Color32::from_rgb(23, 25, 26)
    } else if hovered {
        Color32::from_rgb(52, 54, 57)
    } else {
        Color32::from_rgb(37, 39, 41)
    };
    ui.painter().rect_filled(rect, 8.0, fill);
    let text_rect = rect.shrink2(egui::vec2(28.0, 0.0));
    let text = egui::WidgetText::from(name).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        text_rect.width(),
        egui::FontId::proportional(12.0),
    );
    ui.painter().galley(
        text_rect.center() - text.size() * 0.5,
        text,
        if selected {
            Color32::from_rgb(208, 210, 215)
        } else {
            Color32::from_rgb(132, 136, 141)
        },
    );
}

fn paint_project_close(ui: &egui::Ui, rect: egui::Rect, hovered: bool) {
    let color = if hovered {
        Color32::WHITE
    } else {
        Color32::from_rgb(135, 138, 143)
    };
    for sign in [-1.0, 1.0] {
        ui.painter().line_segment(
            [
                rect.center() + egui::vec2(-2.5, sign * -2.5),
                rect.center() + egui::vec2(2.5, sign * 2.5),
            ],
            egui::Stroke::new(1.0_f32, color),
        );
    }
}

fn project_scroll_button(ui: &mut egui::Ui, direction: f32, enabled: bool) -> egui::Response {
    let response = ui
        .add_enabled(
            enabled,
            egui::Button::new("")
                .frame(false)
                .min_size(egui::vec2(24.0, 27.0)),
        )
        .on_hover_text(if direction < 0.0 {
            "Scroll projects left"
        } else {
            "Scroll projects right"
        });
    let center = response.rect.center();
    let color = if enabled {
        Color32::from_rgb(185, 188, 194)
    } else {
        Color32::from_rgb(75, 78, 83)
    };
    ui.painter().add(egui::Shape::line(
        vec![
            center + egui::vec2(-direction * 2.5, -4.0),
            center + egui::vec2(direction * 2.5, 0.0),
            center + egui::vec2(-direction * 2.5, 4.0),
        ],
        egui::Stroke::new(1.3_f32, color),
    ));
    response
}

fn project_menu_action(
    ui: &mut egui::Ui,
    label: &str,
    icon: crate::tool_icons::Icon,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 28.0), egui::Sense::click());
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, 5.0, Color32::from_rgb(48, 70, 113));
    }
    crate::tool_icons::paint(
        ui.painter(),
        rect.left_center() + egui::vec2(15.0, 0.0),
        icon,
        Color32::from_rgb(210, 212, 217),
    );
    ui.painter().text(
        rect.left_center() + egui::vec2(29.0, 0.0),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(14.0),
        Color32::from_rgb(210, 212, 217),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    response
}

fn project_menu_row(ui: &mut egui::Ui, path: &Path) -> egui::Response {
    let name = project_name(path);
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 48.0), egui::Sense::click());
    let painter = ui.painter().with_clip_rect(rect);
    if response.hovered() {
        painter.rect_filled(rect, 5.0, Color32::from_rgb(48, 70, 113));
    }
    crate::project_badge::paint(
        &painter,
        egui::Rect::from_min_size(rect.min + egui::vec2(8.0, 8.0), egui::vec2(23.0, 23.0)),
        &name,
    );
    painter.text(
        rect.min + egui::vec2(40.0, 7.0),
        egui::Align2::LEFT_TOP,
        &name,
        egui::FontId::proportional(15.0),
        Color32::from_rgb(210, 212, 217),
    );
    let display = std::env::var_os("HOME")
        .map(PathBuf::from)
        .and_then(|home| {
            path.strip_prefix(home)
                .ok()
                .map(|relative| format!("~/{}", relative.display()))
        })
        .unwrap_or_else(|| path.display().to_string());
    painter.text(
        rect.min + egui::vec2(40.0, 28.0),
        egui::Align2::LEFT_TOP,
        display,
        egui::FontId::proportional(12.0),
        Color32::from_rgb(124, 128, 134),
    );
    response.on_hover_text(path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn workspace(
        root: &Path,
        runtime: &tokio::runtime::Runtime,
    ) -> Result<Workspace, Box<dyn std::error::Error>> {
        let bus = EventBus::new(128)?;
        let settings = ConfigManager::from_path(root.join("settings.toml"), bus.clone())?;
        let launcher = NozhikUi::new(bus, settings, Config::default());
        let (ai, _) = tokio::sync::watch::channel(None);
        Ok(Workspace::new(launcher, runtime.handle().clone(), ai))
    }

    fn frame(
        workspace: &mut Workspace,
        ctx: &Context,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1200.0, 800.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| workspace.update(ctx),
        )
    }

    fn wait(workspace: &mut Workspace, ctx: &Context, ready: impl Fn(&Workspace) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !ready(workspace) && Instant::now() < deadline {
            frame(workspace, ctx, Vec::new());
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(ready(workspace), "workspace did not settle");
        frame(workspace, ctx, Vec::new());
    }

    fn pointer(
        workspace: &mut Workspace,
        ctx: &Context,
        pos: egui::Pos2,
        pressed: bool,
    ) -> egui::FullOutput {
        frame(
            workspace,
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
        )
    }

    fn click(workspace: &mut Workspace, ctx: &Context, pos: egui::Pos2) {
        pointer(workspace, ctx, pos, true);
        pointer(workspace, ctx, pos, false);
        frame(workspace, ctx, Vec::new());
    }

    fn text_position(output: &egui::FullOutput, label: &str) -> Option<egui::Pos2> {
        output.shapes.iter().find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.job.text == label => {
                Some(text.pos + text.galley.size() * 0.5)
            }
            _ => None,
        })
    }

    fn open(workspace: &mut Workspace, ctx: &Context, path: &Path, count: usize) {
        workspace.open_path(path.to_path_buf(), ctx);
        wait(workspace, ctx, |w| {
            w.projects.len() == count && w.projects.iter().all(|p| p.project.is_some())
        });
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "End-to-end session retention, settings UI and reorder regression"
    )]
    fn projects_preserve_editors_consoles_tree_and_settings() -> TestResult {
        let temp = tempfile::tempdir()?;
        let a = temp.path().join("Alpha");
        let b = temp.path().join("Beta");
        std::fs::create_dir_all(a.join(".nozhik"))?;
        std::fs::create_dir(&b)?;
        std::fs::write(
            a.join(".nozhik/settings.toml"),
            "font_size = 20\nai_enabled = false\nenabled_languages = []",
        )?;
        let runtime = tokio::runtime::Runtime::new()?;
        let ctx = Context::default();
        theme::apply(&ctx);
        let mut workspace = workspace(temp.path(), &runtime)?;
        open(&mut workspace, &ctx, &a, 1);
        assert_eq!(workspace.current().config.font_size, 20);
        workspace
            .current()
            .loaded_file(a.join("draft.txt"), "unsaved", false);
        workspace.current().documents[0].buffer.insert(7, "!")?;
        workspace.current().expanded.insert(a.join("retained"));
        workspace
            .current()
            .database
            .tabs
            .push(crate::sql_console::SqlConsole::new(
                42,
                "alpha".into(),
                String::new(),
                "select 1",
                "Alpha console".into(),
            ));
        workspace.current().terminal = Some(crate::terminal_panel::TerminalPanel::new(&ctx));
        let buffer_id = workspace.current().documents[0].buffer.snapshot().buffer_id;
        open(&mut workspace, &ctx, &b, 2);
        assert_eq!(workspace.current().config, Config::default());
        assert_eq!(workspace.current().config.ai_debounce_ms, 1000);
        assert!(workspace.current().documents.is_empty());
        assert!(workspace.current().database.tabs.is_empty());
        assert!(workspace.current().terminal.is_none());
        assert!(!workspace.current().expanded.contains(&a.join("retained")));
        std::fs::write(a.join("late.txt"), "background file")?;
        workspace.activate(0);
        workspace.open_path(a.join("late.txt"), &ctx);
        workspace.activate(1);
        wait(&mut workspace, &ctx, |w| w.projects[0].documents.len() == 2);
        assert!(workspace.current().documents.is_empty());

        // Edit the actual font slider, exercising project-scoped background persistence.
        workspace.current().settings_open = true;
        let mut output = frame(&mut workspace, &ctx, Vec::new());
        for _ in 0..3 {
            output = frame(&mut workspace, &ctx, Vec::new());
        }
        assert!(text_position(&output, "Project: Beta").is_some());
        let label = text_position(&output, "Editor font size").ok_or("font slider missing")?;
        click(&mut workspace, &ctx, label - egui::vec2(125.0, 0.0));
        let changed_font = workspace.current().config.font_size;
        assert_ne!(changed_font, Config::default().font_size);
        workspace.current().settings_open = false;
        workspace.current().settings_dialog.finish();
        let saved = ConfigManager::from_path(b.join(".nozhik/settings.toml"), EventBus::new(16)?)?;
        assert_eq!(saved.snapshot()?.font_size, changed_font);
        assert!(!temp.path().join("settings.toml").exists());
        // Pointer selection restores the original session without recreating its buffers.
        frame(&mut workspace, &ctx, Vec::new());
        let first = workspace.current().workspace.tab_rects[0].center();
        click(&mut workspace, &ctx, first);
        assert_eq!(workspace.active, Some(0));
        assert_eq!(
            workspace.current().documents[0].buffer.snapshot().buffer_id,
            buffer_id
        );
        assert!(workspace.current().documents[0].buffer.is_dirty());
        assert_eq!(
            workspace.current().documents[0].buffer.rope().to_string(),
            "unsaved!"
        );
        assert_eq!(workspace.current().database.tabs[0].title, "Alpha console");
        assert!(workspace.current().terminal.is_some());
        assert_eq!(workspace.current().config.font_size, 20);
        assert!(workspace.current().expanded.contains(&a.join("retained")));
        workspace.open_path(a.join("."), &ctx);
        wait(&mut workspace, &ctx, |w| w.opening == 0);
        assert_eq!(workspace.projects.len(), 2);
        // Actual horizontal drag reorders the tabs and retains active project identity.
        let from = workspace.current().workspace.tab_rects[0].center();
        let to = workspace.current().workspace.tab_rects[1].center();
        pointer(&mut workspace, &ctx, from, true);
        frame(&mut workspace, &ctx, vec![egui::Event::PointerMoved(to)]);
        pointer(&mut workspace, &ctx, to, false);
        frame(&mut workspace, &ctx, Vec::new());
        assert_eq!(workspace.active, Some(1));
        assert_eq!(
            project_name(
                &workspace
                    .current()
                    .project
                    .as_ref()
                    .ok_or("no root")?
                    .root_path
            ),
            "Alpha"
        );

        // Fast native gestures can arrive in one frame, including a leftward drop.
        let from = workspace.current().workspace.tab_rects[1].center();
        let to = workspace.current().workspace.tab_rects[0].center();
        let output = frame(
            &mut workspace,
            &ctx,
            vec![
                egui::Event::PointerMoved(from),
                egui::Event::PointerButton {
                    pos: from,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerMoved(to),
                egui::Event::PointerButton {
                    pos: to,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        assert_eq!(workspace.active, Some(0));
        assert_eq!(
            output
                .viewport_output
                .get(&egui::ViewportId::ROOT)
                .map(|viewport| viewport.repaint_delay),
            Some(Duration::ZERO)
        );
        // Reopen a closed project and reload its saved settings.
        workspace.activate(1);
        workspace
            .current()
            .request_close(CloseTarget::Project, &ctx);
        frame(&mut workspace, &ctx, Vec::new());
        assert_eq!(workspace.projects.len(), 1);
        open(&mut workspace, &ctx, &b, 2);
        assert_eq!(workspace.current().config.font_size, changed_font);
        Ok(())
    }

    #[test]
    fn inactive_project_close_prompts_cancel_save_and_errors_preserve_sessions() -> TestResult {
        let temp = tempfile::tempdir()?;
        let a = temp.path().join("A");
        let b = temp.path().join("B");
        std::fs::create_dir(&a)?;
        std::fs::create_dir(&b)?;
        let runtime = tokio::runtime::Runtime::new()?;
        let ctx = Context::default();
        let mut workspace = workspace(temp.path(), &runtime)?;
        open(&mut workspace, &ctx, &a, 1);
        std::fs::write(a.join("draft.txt"), "draft")?;
        workspace
            .current()
            .loaded_file(a.join("draft.txt"), "draft", false);
        workspace.current().documents[0].buffer.insert(5, "!")?;
        open(&mut workspace, &ctx, &b, 2);
        let first = workspace.current().workspace.tab_rects[0];
        frame(
            &mut workspace,
            &ctx,
            vec![egui::Event::PointerMoved(first.center())],
        );
        click(
            &mut workspace,
            &ctx,
            first.left_center() + egui::vec2(14.0, 0.0),
        );
        assert_eq!(workspace.active, Some(0));
        assert!(workspace.current().integration.close.is_some());
        let output = frame(&mut workspace, &ctx, Vec::new());
        click(
            &mut workspace,
            &ctx,
            text_position(&output, "Cancel").ok_or("cancel missing")?,
        );
        assert_eq!(workspace.projects.len(), 2);
        assert!(workspace.current().documents[0].buffer.is_dirty());
        workspace
            .current()
            .request_close(CloseTarget::Project, &ctx);
        workspace.closing_project = true;
        let output = frame(&mut workspace, &ctx, Vec::new());
        click(
            &mut workspace,
            &ctx,
            text_position(&output, "Save All").ok_or("save missing")?,
        );
        wait(&mut workspace, &ctx, |w| w.projects.len() == 1);
        assert_eq!(std::fs::read_to_string(a.join("draft.txt"))?, "draft!");
        workspace.open_path(temp.path().join("missing"), &ctx);
        wait(&mut workspace, &ctx, |w| w.opening == 0);
        assert_eq!(workspace.projects.len(), 1);
        std::fs::create_dir_all(a.join(".nozhik"))?;
        std::fs::write(a.join(".nozhik/settings.toml"), "broken")?;
        workspace.open_path(a.clone(), &ctx);
        wait(&mut workspace, &ctx, |w| w.opening == 0);
        assert_eq!(workspace.projects.len(), 1);
        assert_eq!(
            std::fs::read_to_string(a.join(".nozhik/settings.toml"))?,
            "broken"
        );
        Ok(())
    }
    #[test]
    fn project_menu_switches_and_window_exit_checks_inactive_dirty_buffers() -> TestResult {
        let temp = tempfile::tempdir()?;
        let a = temp.path().join("First");
        let b = temp.path().join("Second");
        std::fs::create_dir(&a)?;
        std::fs::create_dir(&b)?;
        let runtime = tokio::runtime::Runtime::new()?;
        let ctx = Context::default();
        let mut workspace = workspace(temp.path(), &runtime)?;
        open(&mut workspace, &ctx, &a, 1);
        workspace
            .current()
            .loaded_file(a.join("draft.txt"), "draft", false);
        workspace.current().documents[0].buffer.insert(5, "!")?;
        open(&mut workspace, &ctx, &b, 2);
        workspace.projects[0].database.new_connection();
        let output = frame(&mut workspace, &ctx, Vec::new());
        assert!(text_position(&output, "New Database Connection").is_none());
        workspace.activate(0);
        let mut output = frame(&mut workspace, &ctx, Vec::new());
        for _ in 0..2 {
            output = frame(&mut workspace, &ctx, Vec::new());
        }
        assert!(text_position(&output, "New Database Connection").is_some());
        click(
            &mut workspace,
            &ctx,
            text_position(&output, "Cancel").ok_or("database cancel missing")?,
        );
        workspace.activate(1);
        frame(&mut workspace, &ctx, Vec::new());
        let anchor = workspace
            .current()
            .workspace
            .switcher
            .ok_or("switcher missing")?
            .center();
        click(&mut workspace, &ctx, anchor);
        let output = frame(&mut workspace, &ctx, Vec::new());
        assert!(text_position(&output, "Open…").is_some());
        assert!(text_position(&output, "Clone Repository…").is_some());
        assert!(text_position(&output, "Open Projects").is_some());
        assert!(text_position(&output, "Recent Projects").is_some());
        let row = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text)
                    if text.galley.job.text == "First" && text.pos.y > 110.0 =>
                {
                    Some(text.pos + text.galley.size() * 0.5)
                }
                _ => None,
            })
            .ok_or("first project menu row missing")?;
        click(&mut workspace, &ctx, row);
        assert_eq!(workspace.active, Some(0));
        workspace.activate(1);
        workspace.current().action("ui.exit");
        frame(&mut workspace, &ctx, Vec::new());
        wait(&mut workspace, &ctx, |w| {
            w.projects.len() == 1 && w.projects[0].integration.close.is_some()
        });
        assert!(workspace.current().documents[0].buffer.is_dirty());
        assert!(!workspace.launcher.integration.confirmed);
        let output = frame(&mut workspace, &ctx, Vec::new());
        click(
            &mut workspace,
            &ctx,
            text_position(&output, "Cancel").ok_or("cancel missing")?,
        );
        assert!(!workspace.closing_window);
        assert_eq!(workspace.projects.len(), 1);
        workspace
            .current()
            .request_close(CloseTarget::Project, &ctx);
        workspace.closing_project = true;
        let output = frame(&mut workspace, &ctx, Vec::new());
        click(
            &mut workspace,
            &ctx,
            text_position(&output, "Discard").ok_or("discard missing")?,
        );
        assert!(workspace.projects.is_empty());
        assert!(workspace.active.is_none());
        assert!(!workspace.launcher.integration.confirmed);
        Ok(())
    }
    #[test]
    fn held_drag_reorders_across_neighbors_without_changing_active_project() -> TestResult {
        let temp = tempfile::tempdir()?;
        let runtime = tokio::runtime::Runtime::new()?;
        let ctx = Context::default();
        let mut workspace = workspace(temp.path(), &runtime)?;
        for (index, name) in ["Alpha", "Beta", "Gamma"].iter().enumerate() {
            let root = temp.path().join(name);
            std::fs::create_dir_all(root.join(".nozhik"))?;
            std::fs::write(
                root.join(".nozhik/settings.toml"),
                "ai_enabled = false\nenabled_languages = []",
            )?;
            open(&mut workspace, &ctx, &root, index + 1);
        }
        let active = workspace.current().settings.path().to_path_buf();
        let rects = workspace.current().workspace.tab_rects.clone();
        pointer(&mut workspace, &ctx, rects[0].center(), true);
        frame(
            &mut workspace,
            &ctx,
            vec![egui::Event::PointerMoved(rects[1].center())],
        );
        assert!(ctx.input(|i| i.pointer.primary_down()));
        assert_eq!(
            workspace.projects[1]
                .project
                .as_ref()
                .map(|p| project_name(&p.root_path))
                .as_deref(),
            Some("Alpha")
        );
        assert_eq!(workspace.current().settings.path(), active);
        frame(
            &mut workspace,
            &ctx,
            vec![egui::Event::PointerMoved(rects[2].center())],
        );
        assert_eq!(
            workspace.projects[2]
                .project
                .as_ref()
                .map(|p| project_name(&p.root_path))
                .as_deref(),
            Some("Alpha")
        );
        assert_eq!(workspace.current().settings.path(), active);
        pointer(&mut workspace, &ctx, rects[2].center(), false);
        frame(&mut workspace, &ctx, Vec::new());
        pointer(&mut workspace, &ctx, rects[2].center(), true);
        frame(
            &mut workspace,
            &ctx,
            vec![egui::Event::PointerMoved(rects[0].center())],
        );
        assert!(ctx.input(|i| i.pointer.primary_down()));
        assert_eq!(
            workspace.projects[0]
                .project
                .as_ref()
                .map(|p| project_name(&p.root_path))
                .as_deref(),
            Some("Alpha")
        );
        assert_eq!(workspace.current().settings.path(), active);
        pointer(&mut workspace, &ctx, rects[0].center(), false);
        Ok(())
    }

    fn strip_frame(shell: &mut NozhikUi, ctx: &Context, events: Vec<egui::Event>) {
        let _output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(440.0, 200.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| shell.project_tabs(ctx),
        );
    }

    fn strip_click(shell: &mut NozhikUi, ctx: &Context, pos: egui::Pos2) {
        for pressed in [true, false] {
            strip_frame(
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
        strip_frame(shell, ctx, Vec::new());
    }

    #[test]
    fn overflowing_projects_have_no_scrollbar_and_scroll_by_buttons_and_both_wheel_axes()
    -> TestResult {
        let temp = tempfile::tempdir()?;
        let bus = EventBus::new(16)?;
        let settings = ConfigManager::from_path(temp.path().join("settings.toml"), bus.clone())?;
        let mut shell = NozhikUi::new(bus, settings, Config::default());
        shell.workspace.entries = (0..20)
            .map(|i| temp.path().join(format!("Project {i}")))
            .collect();
        let ctx = Context::default();
        theme::apply(&ctx);
        for _ in 0..4 {
            strip_frame(&mut shell, &ctx, Vec::new());
        }
        let viewport = shell.workspace.tab_viewport.ok_or("missing viewport")?;

        for rect in &shell.workspace.tab_rects {
            assert!(rect.top() >= viewport.top());
            assert!(rect.bottom() <= viewport.bottom());
        }
        assert_eq!(shell.workspace.tab_buttons.len(), 2);
        assert!((viewport.height() - 28.0).abs() < 0.5);
        assert!(
            shell
                .workspace
                .tab_rects
                .last()
                .is_some_and(|rect| rect.right() > viewport.right())
        );
        for pos in [
            viewport.center_bottom() - egui::vec2(0.0, 1.0),
            viewport.center(),
        ] {
            strip_frame(&mut shell, &ctx, vec![egui::Event::PointerMoved(pos)]);
            assert_eq!(shell.workspace.tab_viewport, Some(viewport));
        }
        let right = shell.workspace.tab_buttons[1].center();
        strip_click(&mut shell, &ctx, right);
        let after_button = shell.workspace.tab_offset;
        assert!(after_button > 100.0);
        for delta in [egui::vec2(-120.0, 0.0), egui::vec2(0.0, -120.0)] {
            let before = shell.workspace.tab_offset;
            strip_frame(
                &mut shell,
                &ctx,
                vec![
                    egui::Event::PointerMoved(viewport.center()),
                    egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
            for _ in 0..8 {
                strip_frame(&mut shell, &ctx, Vec::new());
            }
            assert!(shell.workspace.tab_offset > before + 20.0);
        }
        let before_left = shell.workspace.tab_offset;
        let left = shell.workspace.tab_buttons[0].center();
        strip_click(&mut shell, &ctx, left);
        assert!(shell.workspace.tab_offset < before_left);
        let scroll_id = shell.workspace.tab_scroll_id.ok_or("missing scroll ID")?;
        assert!(
            egui::scroll_area::State::load(&ctx, scroll_id)
                .is_some_and(|state| state.offset.y == 0.0)
        );
        shell.workspace.entries.truncate(2);
        strip_frame(&mut shell, &ctx, Vec::new());
        strip_frame(&mut shell, &ctx, Vec::new());
        assert_eq!(shell.workspace.tab_buttons, [] as [egui::Rect; 0]);
        assert_eq!(shell.workspace.tab_offset, 0.0);
        Ok(())
    }
}
