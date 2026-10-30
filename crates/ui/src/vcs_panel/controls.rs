//! Git chrome, branch chooser and concrete branch/merge/file actions.
use super::{ChangeStatus, Color32, Context, DiffTarget, FileChange, Task, VcsPanel, theme};
use egui::{Align2, FontId, Sense, Stroke, pos2, vec2};
pub(super) const TEXT: Color32 = Color32::from_rgb(225, 230, 235);
pub(super) const BLUE: Color32 = Color32::from_rgb(0, 99, 216);
pub(super) const SURFACE: Color32 = Color32::from_rgb(35, 42, 48);
pub(super) const HEADER: Color32 = Color32::from_rgb(29, 34, 38);
const SELECTED: Color32 = Color32::from_rgb(66, 78, 87);

#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub(super) enum FilterMode {
    #[default]
    All,
    Staged,
    Unstaged,
    Conflicted,
}
impl FilterMode {
    pub(super) fn matches(self, change: &FileChange) -> bool {
        match self {
            Self::All => true,
            Self::Staged => change.is_staged,
            Self::Unstaged => change.has_unstaged,
            Self::Conflicted => change.status == ChangeStatus::Conflicted,
        }
    }
}
#[derive(Default, Clone, PartialEq, Eq)]
pub(super) enum Dialog {
    #[default]
    None,
    NewBranch,
    Switch(String),
    Merge,
    Conflicts,
    Abort,
}
fn chrome_button(
    ui: &mut egui::Ui,
    width: f32,
    title: &str,
    subtitle: &str,
    arrow: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(width, 50.0), Sense::click());
    ui.painter()
        .rect_filled(rect, 0, if response.hovered() { SURFACE } else { HEADER });
    ui.painter().line_segment(
        [rect.right_top(), rect.right_bottom()],
        Stroke::new(1.0_f32, Color32::from_gray(16)),
    );
    let painter = ui
        .painter()
        .with_clip_rect(rect.shrink(2.0).intersect(ui.clip_rect()));
    crate::tool_icons::paint(
        &painter,
        rect.left_center() + vec2(17.0, 0.0),
        match title {
            "Current Branch" => crate::tool_icons::Icon::Git,
            _ => crate::tool_icons::Icon::Remote,
        },
        TEXT,
    );
    let current = title.starts_with("Current ");
    painter.text(
        rect.left_top() + vec2(36.0, 8.0),
        Align2::LEFT_TOP,
        title,
        FontId::proportional(if current { 12.0 } else { 14.0 }),
        if current { theme::MUTED } else { TEXT },
    );
    painter
        .with_clip_rect(
            egui::Rect::from_min_max(rect.left_top(), rect.right_bottom() - vec2(25.0, 0.0))
                .intersect(ui.clip_rect()),
        )
        .text(
            rect.left_top() + vec2(36.0, 25.0),
            Align2::LEFT_TOP,
            subtitle,
            FontId::proportional(if current { 14.0 } else { 12.0 }),
            if current { TEXT } else { theme::MUTED },
        );
    if arrow {
        chevron(ui.painter(), rect.right_center() - vec2(15.0, 0.0));
    }
    response
}
fn chevron(painter: &egui::Painter, center: egui::Pos2) {
    painter.add(egui::Shape::convex_polygon(
        vec![
            center + vec2(-4.0, -2.0),
            center + vec2(4.0, -2.0),
            center + vec2(0.0, 3.0),
        ],
        TEXT,
        Stroke::NONE,
    ));
}
impl VcsPanel {
    pub(crate) fn toolbar(&mut self, ui: &mut egui::Ui) {
        Self::git_style(ui);
        // Own the row layout so callers cannot stack fixed-height controls vertically.
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            self.toolbar_controls(ui);
        });
    }
    fn toolbar_controls(&mut self, ui: &mut egui::Ui) {
        let width = (ui.available_width() * 0.28).clamp(120.0, 230.0);
        let branch = self.branch().to_owned();
        let response = chrome_button(ui, width, "Current Branch", &branch, true);
        if response.clicked() {
            self.open_branches = !self.open_branches;
        }
        response.context_menu(|ui| {
            if ui.button("Choose Branch…").clicked() {
                self.open_branches = true;
                ui.close();
            }
            if ui.button("New Branch…").clicked() {
                self.dialog = Dialog::NewBranch;
                ui.close();
            }
            if ui.button(format!("Merge into {branch}…")).clicked() {
                self.open_merge();
                ui.close();
            }
        });
        if self.open_branches {
            self.branch_popup(ui.ctx(), response.rect);
        }
        self.remote_controls(ui, width);
    }
    pub(super) fn file_filter(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let menu = ui.menu_button("      ", |ui| {
                for (mode, label) in [
                    (FilterMode::All, "All files"),
                    (FilterMode::Staged, "Included files"),
                    (FilterMode::Unstaged, "Excluded / unstaged files"),
                    (FilterMode::Conflicted, "Conflicted files"),
                ] {
                    if ui
                        .selectable_value(&mut self.filter_mode, mode, label)
                        .clicked()
                    {
                        ui.close();
                    }
                }
            });
            let center = menu.response.rect.center() - vec2(4.0, 0.0);
            for (y, half) in [(-4.0, 6.0), (0.0, 4.0), (4.0, 2.0)] {
                ui.painter().line_segment(
                    [center + vec2(-half, y), center + vec2(half, y)],
                    Stroke::new(1.3_f32, TEXT),
                );
            }
            chevron(
                ui.painter(),
                menu.response.rect.right_center() - vec2(5.0, 0.0),
            );
            ui.add(
                egui::TextEdit::singleline(&mut self.filter)
                    .hint_text("Filter")
                    .desired_width(ui.available_width()),
            );
        });
    }
    fn remote_controls(&mut self, ui: &mut egui::Ui, width: f32) {
        let network = self.mutations.is_empty()
            && self
                .repo
                .as_ref()
                .is_some_and(|r| r.has_origin && r.operation.is_none());
        let (title, task) = self
            .repo
            .as_ref()
            .map_or(("Fetch origin", Task::Fetch), |r| {
                if !r.has_origin {
                    ("Fetch origin", Task::Fetch)
                } else if !r.has_upstream && !r.detached && r.head_sha.is_some() {
                    ("Publish branch", Task::Push)
                } else if r.ahead > 0 && r.behind > 0 {
                    ("Pull & Push origin", Task::PullPush)
                } else if r.ahead > 0 {
                    ("Push origin", Task::Push)
                } else if r.behind > 0 {
                    ("Pull origin", Task::Pull)
                } else {
                    ("Fetch origin", Task::Fetch)
                }
            });
        if title != "Fetch origin" {
            self.fetch_button(ui, network);
        }
        let age = if self.pending.contains(&self.fetch_request) {
            "Working…".into()
        } else {
            self.last_fetch.map_or_else(
                || "Fetch remote changes".into(),
                |t| {
                    let minutes = t.elapsed().as_secs() / 60;
                    if minutes == 0 {
                        "Last fetched just now".into()
                    } else {
                        format!("Last fetched {minutes} minutes ago")
                    }
                },
            )
        };
        let remote = ui
            .add_enabled_ui(network, |ui| {
                chrome_button(
                    ui,
                    width.min(230.0),
                    title,
                    if self
                        .repo
                        .as_ref()
                        .is_some_and(|r| r.ahead > 0 || r.behind > 0)
                    {
                        ""
                    } else {
                        &age
                    },
                    false,
                )
            })
            .inner;
        if let Some(repo) = &self.repo {
            let counts = [(true, repo.behind), (false, repo.ahead)]
                .into_iter()
                .filter(|(_, n)| *n > 0);
            let mut left = remote.rect.left() + 36.0;
            for (down, count) in counts {
                let text = count.to_string();
                let font = FontId::proportional(12.0);
                let galley = ui.painter().layout_no_wrap(text, font, TEXT);
                let size = galley.size() + vec2(24.0, 4.0);
                let rect = egui::Rect::from_min_size(
                    egui::pos2(left, remote.rect.bottom() - size.y - 3.0),
                    size,
                );
                ui.painter().rect_filled(rect, 8, BLUE);
                paint_vertical_arrow(
                    ui.painter(),
                    rect.left_center() + vec2(8.0, 0.0),
                    down,
                    TEXT,
                );
                ui.painter()
                    .galley(rect.min + vec2(19.0, 2.0), galley, TEXT);
                left = rect.right() + 5.0;
            }
        }
        if remote.clicked() {
            self.submit(task);
        }
        let remote = remote.on_hover_text(age);
        remote.context_menu(|ui| self.network_menu(ui, network));
        let menu = ui.menu_button("    ", |ui| {
            self.network_menu(ui, network);
        });
        chevron(ui.painter(), menu.response.rect.center());
    }
    fn fetch_button(&mut self, ui: &mut egui::Ui, network: bool) {
        let fetch = ui
            .add_enabled_ui(network, |ui| {
                chrome_button(ui, 150.0, "Fetch origin", "Fetch remote changes", false)
            })
            .inner;
        if fetch.clicked() {
            self.submit(Task::Fetch);
        }
    }
    fn network_menu(&mut self, ui: &mut egui::Ui, network: bool) {
        for (name, task) in [
            ("Fetch origin", Task::Fetch),
            ("Pull origin (fast-forward)", Task::Pull),
            ("Push origin", Task::Push),
            ("Pull & Push origin", Task::PullPush),
        ] {
            if ui.add_enabled(network, egui::Button::new(name)).clicked() {
                self.submit(task);
                ui.close();
            }
        }
        ui.separator();
        if let Some(repo) = &self.repo {
            ui.weak(format!("{} ahead · {} behind", repo.ahead, repo.behind));
        }
        if ui.button("Refresh Git status").clicked() {
            self.submit(Task::Refresh);
            ui.close();
        }
    }
    pub(super) fn git_style(ui: &mut egui::Ui) {
        let visuals = ui.visuals_mut();
        visuals.selection.bg_fill = BLUE;
        visuals.widgets.inactive.bg_fill = HEADER;
        visuals.widgets.hovered.bg_fill = SELECTED;
        visuals.widgets.active.bg_fill = BLUE;
        visuals.extreme_bg_color = HEADER;
        visuals.override_text_color = Some(Color32::from_rgb(225, 230, 235));
        visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(100, 114, 124));
        visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(145, 158, 168));
        visuals.widgets.active.bg_stroke = Stroke::new(1.0_f32, BLUE);
        visuals.widgets.inactive.corner_radius = egui::CornerRadius::same(4);
        visuals.widgets.hovered.corner_radius = egui::CornerRadius::same(4);
        visuals.widgets.active.corner_radius = egui::CornerRadius::same(4);
        ui.spacing_mut().button_padding = vec2(10.0, 6.0);
    }
    fn branch_popup(&mut self, ctx: &Context, anchor: egui::Rect) {
        let screen = ctx.screen_rect();
        let width = 370.0_f32.min(screen.width() - 20.0);
        let height = (screen.bottom() - anchor.bottom() - 12.0).max(100.0);
        let area = egui::Area::new(egui::Id::new("git_branch_popup"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos2(
                anchor.left().min(screen.right() - width - 10.0),
                anchor.bottom(),
            ))
            .show(ctx, |ui| {
                Self::git_style(ui);
                ui.set_width(width);
                egui::Frame::popup(ui.style())
                    .fill(SURFACE)
                    .inner_margin(10)
                    .show(ui, |ui| {
                        ui.set_width(width - 20.0);
                        ui.horizontal(|ui| {
                            ui.strong("Branches");
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.button("New Branch").clicked() {
                                        self.dialog = Dialog::NewBranch;
                                        self.open_branches = false;
                                    }
                                },
                            );
                        });
                        ui.add(
                            egui::TextEdit::singleline(&mut self.branch_filter)
                                .hint_text("Filter")
                                .desired_width(f32::INFINITY),
                        );
                        ui.separator();
                        if let Some(name) = self.branch_list(ui, height - 145.0, false) {
                            self.open_branches = false;
                            if self.repo.as_ref().is_some_and(|r| !r.changes.is_empty()) {
                                self.dialog = Dialog::Switch(name);
                            } else {
                                self.submit(Task::SwitchBranch {
                                    name,
                                    leave_changes: false,
                                });
                            }
                        }
                        ui.separator();
                        if ui
                            .add_sized(
                                [ui.available_width(), 30.0],
                                egui::Button::new(format!(
                                    "Choose a branch to merge into {}",
                                    self.branch()
                                )),
                            )
                            .clicked()
                        {
                            self.open_merge();
                            self.open_branches = false;
                        }
                    });
            });
        if ctx.input(|i| {
            i.key_pressed(egui::Key::Escape)
                || (i.pointer.any_pressed()
                    && i.pointer
                        .interact_pos()
                        .is_some_and(|p| !area.response.rect.contains(p) && !anchor.contains(p)))
        }) {
            self.open_branches = false;
        }
    }
    fn branch_list(&self, ui: &mut egui::Ui, height: f32, merging: bool) -> Option<String> {
        let repo = self.repo.as_ref()?;
        let filter = self.branch_filter.to_lowercase();
        let mut chosen = None;
        egui::ScrollArea::vertical()
            .id_salt(if merging {
                "merge_branches"
            } else {
                "switch_branches"
            })
            .max_height(height.max(50.0))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (heading, group) in [
                    ("Default Branch", 0),
                    ("Recent Branches", 1),
                    ("Other Branches", 2),
                ] {
                    let branches: Vec<_> = repo
                        .branch_info
                        .iter()
                        .filter(|b| {
                            (if b.is_default {
                                0
                            } else if b.is_remote {
                                2
                            } else {
                                1
                            }) == group
                                && b.name.to_lowercase().contains(&filter)
                        })
                        .collect();
                    if branches.is_empty() {
                        continue;
                    }
                    ui.add_space(8.0);
                    ui.strong(heading);
                    ui.add_space(6.0);
                    for branch in branches {
                        let current = branch.name == repo.branch;
                        let selected = if merging {
                            self.merge_branch.as_ref() == Some(&branch.name)
                        } else {
                            current
                        };
                        let (rect, response) = ui
                            .allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
                        if selected || response.hovered() {
                            ui.painter().rect_filled(
                                rect,
                                0,
                                if selected && merging { BLUE } else { SELECTED },
                            );
                        }
                        let age = &branch.relative_time;
                        let age_width = ui
                            .painter()
                            .layout_no_wrap(age.clone(), FontId::proportional(12.0), theme::MUTED)
                            .size()
                            .x;
                        paint_branch_icon(ui, rect, current);
                        let text_rect = egui::Rect::from_min_max(
                            rect.left_top() + vec2(28.0, 0.0),
                            rect.right_bottom() - vec2(age_width + 16.0, 0.0),
                        );
                        ui.painter()
                            .with_clip_rect(text_rect.intersect(ui.clip_rect()))
                            .text(
                                text_rect.left_center(),
                                Align2::LEFT_CENTER,
                                &branch.name,
                                FontId::proportional(13.0),
                                TEXT,
                            );
                        ui.painter().text(
                            rect.right_center() - vec2(6.0, 0.0),
                            Align2::RIGHT_CENTER,
                            age,
                            FontId::proportional(12.0),
                            theme::MUTED,
                        );
                        if response.on_hover_text(&branch.name).clicked()
                            && !current
                            && self.mutations.is_empty()
                            && repo.operation.is_none()
                        {
                            chosen = Some(branch.name.clone());
                        }
                    }
                }
            });
        chosen
    }
    fn open_merge(&mut self) {
        self.dialog = Dialog::Merge;
        self.branch_filter.clear();
        self.merge_branch = None;
        self.merge_preview = None;
        self.preview_request = 0;
    }
    pub(super) fn dialogs(&mut self, ctx: &Context) {
        let dialog = self.dialog.clone();
        if dialog == Dialog::None {
            return;
        }
        let modal = egui::Modal::new(egui::Id::new("git_operation_dialog"))
            .frame(egui::Frame::popup(&ctx.style()).fill(SURFACE).inner_margin(24).corner_radius(6))
            .show(ctx, |ui| {
            Self::git_style(ui);
            ui.set_width(500.0_f32.min(ctx.screen_rect().width()-70.0).max(150.0));
            let title = match &dialog { Dialog::NewBranch => "Create a Branch".into(), Dialog::Switch(_) => "Switch Branch".into(), Dialog::Merge => format!("Merge into {}", self.branch()), Dialog::Conflicts => "Resolve conflicts before Merge".into(), Dialog::Abort => "Abort Merge?".into(), Dialog::None => String::new() };
            ui.horizontal(|ui| { ui.heading(title); ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| { if ui.button("×").clicked() { self.dialog = Dialog::None; } }); });
            ui.add_space(12.0); ui.separator(); ui.add_space(12.0);
            match dialog {
                Dialog::NewBranch => self.create_branch_dialog(ui),
                Dialog::Switch(ref branch) => self.switch_dialog(ui, branch),
                Dialog::Merge => self.merge_dialog(ui),
                Dialog::Conflicts => self.conflicts_dialog(ui),
                Dialog::Abort => {
                    ui.label("Discard changes made during this merge and restore the current branch to HEAD? Files added outside the merge are retained.");
                    ui.separator();
                    ui.horizontal(|ui| { if ui.button("Cancel").clicked() {self.dialog=Dialog::Conflicts;}
 if ui.add_enabled(self.mutations.is_empty(),egui::Button::new("Abort Merge")).clicked() {self.submit(Task::AbortMerge); self.dialog=Dialog::None;} });
                }
                Dialog::None => {}
            }
            if let Some(error)=&self.error {ui.colored_label(theme::ERROR,error);}
        });
        if modal.should_close() {
            self.dialog = Dialog::None;
        }
    }
    fn create_branch_dialog(&mut self, ui: &mut egui::Ui) {
        ui.label("Name");
        ui.add(egui::TextEdit::singleline(&mut self.new_branch).desired_width(f32::INFINITY));
        ui.add_space(12.0);
        ui.label(format!(
            "Your new branch will be based on your currently checked out branch ({}).",
            self.branch()
        ));
        ui.add_space(20.0);
        ui.separator();
        ui.horizontal(|ui| {
            ui.add_space((ui.available_width() - 292.0).max(0.0));
            if ui
                .add_sized([140.0, 32.0], egui::Button::new("Cancel"))
                .clicked()
            {
                self.dialog = Dialog::None;
            }
            if ui
                .add_enabled(
                    self.mutations.is_empty()
                        && !self.new_branch.trim().is_empty()
                        && self.repo.as_ref().is_some_and(|r| r.operation.is_none()),
                    egui::Button::new((egui::Atom::grow(), "Create Branch", egui::Atom::grow()))
                        .fill(BLUE)
                        .min_size(vec2(140.0, 32.0)),
                )
                .clicked()
            {
                self.submit(Task::NewBranch(self.new_branch.trim().to_owned()));
                self.new_branch.clear();
                self.dialog = Dialog::None;
            }
        });
    }
    fn switch_dialog(&mut self, ui: &mut egui::Ui, branch: &str) {
        ui.label("You have changes on this branch. What would you like to do with them?");
        // Persist the radio choice in egui memory without altering the repository.
        let id = egui::Id::new("git_leave_changes");
        let mut leave = ui
            .ctx()
            .data_mut(|d| d.get_temp::<bool>(id))
            .unwrap_or(true);
        ui.add_space(12.0);
        for (value, title, description) in [
            (
                true,
                format!("Leave my changes on {}", self.branch()),
                "Your in-progress work will be stashed on this branch for you to return to later.",
            ),
            (
                false,
                format!("Bring my changes to {branch}"),
                "Your in-progress work will follow you to the new branch.",
            ),
        ] {
            let card = egui::Frame::new()
                .fill(if leave == value { SELECTED } else { SURFACE })
                .stroke(Stroke::new(1.0_f32, HEADER))
                .corner_radius(4)
                .inner_margin(12)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut leave, value, "");
                        ui.add(egui::Label::new(egui::RichText::new(title).strong()).truncate());
                    });
                    ui.indent(value, |ui| {
                        ui.label(description);
                    });
                })
                .response
                .interact(Sense::click());
            if card.clicked() {
                leave = value;
            }
        }
        ui.ctx().data_mut(|d| d.insert_temp(id, leave));
        ui.add_space(20.0);
        ui.separator();
        ui.horizontal(|ui| {
            ui.add_space((ui.available_width() - 292.0).max(0.0));
            if ui
                .add_sized([140.0, 32.0], egui::Button::new("Cancel"))
                .clicked()
            {
                self.dialog = Dialog::None;
            }
            if ui
                .add_enabled(
                    self.mutations.is_empty(),
                    egui::Button::new((egui::Atom::grow(), "Switch Branch", egui::Atom::grow()))
                        .fill(BLUE)
                        .min_size(vec2(140.0, 32.0)),
                )
                .clicked()
            {
                self.submit(Task::SwitchBranch {
                    name: branch.into(),
                    leave_changes: leave,
                });
                self.dialog = Dialog::None;
            }
        });
    }
    fn merge_dialog(&mut self, ui: &mut egui::Ui) {
        ui.add(
            egui::TextEdit::singleline(&mut self.branch_filter)
                .hint_text("Filter")
                .desired_width(f32::INFINITY),
        );
        if let Some(branch) = self.branch_list(
            ui,
            (ui.ctx().screen_rect().height() * 0.42).min(340.0),
            true,
        ) {
            self.merge_branch = Some(branch.clone());
            self.merge_preview = None;
            self.preview_request = self.submit(Task::PreviewMerge(branch));
        }
        ui.separator();
        let clean = self
            .repo
            .as_ref()
            .is_some_and(|r| r.operation.is_none() && r.changes.is_empty());
        if !clean {
            ui.colored_label(
                theme::WARNING,
                "Commit or stash your changes before merging.",
            );
        } else if let Some(count) = self.merge_preview {
            if count > 0 {
                ui.colored_label(
                    theme::WARNING,
                    format!(
                        "There will be {count} conflicted files when merging {} into {}",
                        self.merge_branch.as_deref().unwrap_or(""),
                        self.branch()
                    ),
                );
            } else {
                ui.weak("This branch can be merged without conflicts.");
            }
        } else {
            ui.weak(if self.merge_branch.is_some() {
                "Checking merge…"
            } else {
                "Choose a branch to merge"
            });
        }
        if ui
            .add_enabled(
                clean && self.mutations.is_empty() && self.merge_preview.is_some(),
                egui::Button::new((
                    egui::Atom::grow(),
                    "Create a merge commit",
                    egui::Atom::grow(),
                ))
                .fill(BLUE)
                .min_size(vec2(ui.available_width(), 34.0)),
            )
            .clicked()
            && let Some(branch) = self.merge_branch.clone()
        {
            self.submit(Task::Merge(branch));
            self.dialog = Dialog::None;
        }
    }
    fn conflicts_dialog(&mut self, ui: &mut egui::Ui) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let unresolved = repo.conflicts.iter().filter(|c| c.remaining > 0).count();
        ui.strong(format!("{unresolved} conflicted files"));
        ui.add_space(12.0);
        egui::ScrollArea::vertical()
            .id_salt("merge_conflicts")
            .max_height(ui.ctx().screen_rect().height() * 0.4)
            .show(ui, |ui| {
                for conflict in &repo.conflicts {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(conflict.path.to_string_lossy());
                            ui.colored_label(
                                if conflict.remaining == 0 {
                                    theme::SUCCESS
                                } else {
                                    theme::WARNING
                                },
                                if conflict.remaining == 0 {
                                    "Resolved".into()
                                } else {
                                    format!("{} conflicts", conflict.remaining)
                                },
                            );
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Open in Nozhik").clicked() {
                                self.open_paths.push(repo.root.join(&conflict.path));
                                self.dialog = Dialog::None;
                            }
                        });
                    });
                    ui.separator();
                }
            });
        ui.label("Edit and save each file to resolve its conflicts. Status updates automatically.");
        ui.add_space(16.0);
        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("Abort Merge").clicked() {
                self.dialog = Dialog::Abort;
            }
            if ui
                .add_enabled(
                    repo.operation.as_deref() == Some("Merge")
                        && unresolved == 0
                        && self.mutations.is_empty(),
                    egui::Button::new((egui::Atom::grow(), "Continue Merge", egui::Atom::grow()))
                        .fill(BLUE),
                )
                .clicked()
            {
                self.submit(Task::ContinueMerge);
                self.dialog = Dialog::None;
            }
        });
    }
    pub(crate) fn merge_banner(&mut self, ui: &mut egui::Ui) {
        if self
            .repo
            .as_ref()
            .is_some_and(|r| r.operation.as_deref() == Some("Merge"))
        {
            ui.horizontal(|ui| {
                ui.colored_label(
                    theme::WARNING,
                    format!(
                        "Resolve conflicts to continue merge into {}.",
                        self.branch()
                    ),
                );
                if ui.link("View conflicts").clicked() {
                    self.dialog = Dialog::Conflicts;
                }
            });
        }
    }
    pub(super) fn change_row(&mut self, ui: &mut egui::Ui, change: &FileChange, busy: bool) {
        let selected = self.selected_paths.contains(&change.path);
        let (rect, row) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click());
        ui.painter().rect_filled(
            rect,
            0,
            if selected {
                BLUE
            } else if row.hovered() {
                HEADER
            } else {
                SURFACE
            },
        );
        let check = egui::Rect::from_min_size(rect.left_top() + vec2(4.0, 3.0), vec2(22.0, 22.0));
        let mut staged = change.is_staged;
        let mut check_ui = ui.new_child(egui::UiBuilder::new().max_rect(check));
        if busy {
            check_ui.disable();
        }
        check_ui.visuals_mut().widgets.inactive.corner_radius = egui::CornerRadius::same(2);
        check_ui.visuals_mut().widgets.hovered.corner_radius = egui::CornerRadius::same(2);
        check_ui.visuals_mut().widgets.active.corner_radius = egui::CornerRadius::same(2);
        let check_response = check_ui.add(
            egui::Checkbox::new(&mut staged, "")
                .indeterminate(change.is_staged && change.has_unstaged),
        );
        if check_response.changed() && !busy {
            self.submit(Task::Stage {
                paths: vec![change.path.clone()],
                staged,
            });
        }
        let text_rect = egui::Rect::from_min_max(
            rect.left_top() + vec2(28.0, 0.0),
            rect.right_bottom() - vec2(28.0, 0.0),
        );
        ui.painter()
            .with_clip_rect(text_rect.intersect(ui.clip_rect()))
            .text(
                text_rect.left_center(),
                Align2::LEFT_CENTER,
                change.path.to_string_lossy(),
                FontId::proportional(13.0),
                TEXT,
            );
        paint_file_status(ui, change, rect);
        if row.clicked() {
            let modifiers = ui.input(|i| i.modifiers);
            if modifiers.shift {
                if let Some(repo) = &self.repo {
                    let a = repo
                        .changes
                        .iter()
                        .position(|c| Some(&c.path) == self.selected.as_ref());
                    let b = repo.changes.iter().position(|c| c.path == change.path);
                    if let (Some(a), Some(b)) = (a, b) {
                        self.selected_paths = repo.changes[a.min(b)..=a.max(b)]
                            .iter()
                            .map(|c| c.path.clone())
                            .collect();
                    }
                }
            } else if modifiers.command {
                if selected {
                    self.selected_paths.retain(|p| p != &change.path);
                } else {
                    self.selected_paths.push(change.path.clone());
                }
            } else {
                self.selected_paths = vec![change.path.clone()];
            }
            self.selected = Some(change.path.clone());
            if self.selected_paths.len() == 1 {
                self.request_diff(DiffTarget::Working {
                    path: change.path.clone(),
                    staged: self.staged_diff,
                });
            }
        }
        if row.secondary_clicked() && !selected {
            self.selected_paths = vec![change.path.clone()];
            self.selected = Some(change.path.clone());
        }
        row.context_menu(|ui| self.file_menu(ui, change, busy));
    }
    #[allow(
        clippy::too_many_lines,
        reason = "File context actions share the same immutable selected-path snapshot"
    )]
    fn file_menu(&mut self, ui: &mut egui::Ui, change: &FileChange, busy: bool) {
        let paths = if self.selected_paths.contains(&change.path) {
            self.selected_paths.clone()
        } else {
            vec![change.path.clone()]
        };
        let count = paths.len();
        if ui
            .add_enabled(
                !busy,
                egui::Button::new(format!("Discard {count} Selected Changes…")),
            )
            .clicked()
        {
            self.discard = Some(paths.clone());
            ui.close();
        }
        ui.separator();
        if ui
            .add_enabled(
                !busy,
                egui::Button::new(format!("Ignore {count} Selected Files (Add to .gitignore)")),
            )
            .clicked()
        {
            for path in &paths {
                self.submit(Task::Ignore(path.clone()));
            }
            ui.close();
        }
        let mut extensions: Vec<_> = paths
            .iter()
            .filter_map(|p| p.extension().and_then(|e| e.to_str()).map(str::to_owned))
            .collect();
        extensions.sort();
        extensions.dedup();
        for extension in extensions {
            if ui
                .add_enabled(
                    !busy,
                    egui::Button::new(format!("Ignore All .{extension} Files (Add to .gitignore)")),
                )
                .clicked()
            {
                self.submit(Task::IgnoreExtension(extension));
                ui.close();
            }
        }
        ui.separator();
        for (title, staged) in [
            ("Include Selected Files", true),
            ("Exclude Selected Files", false),
        ] {
            if ui.add_enabled(!busy, egui::Button::new(title)).clicked() {
                self.submit(Task::Stage {
                    paths: paths.clone(),
                    staged,
                });
                ui.close();
            }
        }
        ui.separator();
        if ui.button("Copy Paths").clicked() {
            if let Some(repo) = &self.repo {
                ui.ctx().copy_text(
                    paths
                        .iter()
                        .map(|p| repo.root.join(p).display().to_string())
                        .collect::<Vec<_>>()
                        .join("\n"),
                );
            }
            ui.close();
        }
        if ui.button("Copy Relative Paths").clicked() {
            ui.ctx().copy_text(
                paths
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
            ui.close();
        }
        ui.separator();
        if ui.button("Reveal in File Manager").clicked() {
            self.submit(Task::OpenExternal {
                paths: paths.clone(),
                reveal: true,
            });
            ui.close();
        }
        if ui.button("Open in Nozhik").clicked() {
            if let Some(repo) = &self.repo {
                self.open_paths
                    .extend(paths.iter().map(|p| repo.root.join(p)));
            }
            ui.close();
        }
        if ui.button("Open with Default Program").clicked() {
            self.submit(Task::OpenExternal {
                paths,
                reveal: false,
            });
            ui.close();
        }
    }
}

pub(super) fn paint_file_status(ui: &egui::Ui, change: &FileChange, rect: egui::Rect) {
    let color = match change.status {
        ChangeStatus::New => theme::SUCCESS,
        ChangeStatus::Deleted => theme::ERROR,
        ChangeStatus::Renamed => theme::INFO,
        ChangeStatus::Conflicted | ChangeStatus::Modified => theme::WARNING,
    };
    let center = rect.right_center() - vec2(12.0, 0.0);
    let painter = ui.painter();
    let stroke = Stroke::new(1.4_f32, color);
    if change.status == ChangeStatus::Conflicted {
        painter.add(egui::Shape::closed_line(
            vec![
                center + vec2(0.0, -7.0),
                center + vec2(7.0, 6.0),
                center + vec2(-7.0, 6.0),
            ],
            stroke,
        ));
        painter.line_segment([center - vec2(0.0, 3.0), center + vec2(0.0, 1.0)], stroke);
        painter.circle_filled(center + vec2(0.0, 3.5), 0.8, color);
    } else {
        painter.rect_stroke(
            egui::Rect::from_center_size(center, vec2(12.0, 12.0)),
            1,
            stroke,
            egui::StrokeKind::Inside,
        );
        if change.status == ChangeStatus::New || change.status == ChangeStatus::Deleted {
            painter.line_segment([center - vec2(3.0, 0.0), center + vec2(3.0, 0.0)], stroke);
            if change.status == ChangeStatus::New {
                painter.line_segment([center - vec2(0.0, 3.0), center + vec2(0.0, 3.0)], stroke);
            }
        } else {
            painter.circle_filled(center, 1.5, color);
        }
    }
}

fn paint_branch_icon(ui: &egui::Ui, rect: egui::Rect, current: bool) {
    if current {
        ui.painter().line_segment(
            [
                rect.left_center() + vec2(6.0, 0.0),
                rect.left_center() + vec2(10.0, 4.0),
            ],
            Stroke::new(1.5_f32, TEXT),
        );
        ui.painter().line_segment(
            [
                rect.left_center() + vec2(10.0, 4.0),
                rect.left_center() + vec2(18.0, -5.0),
            ],
            Stroke::new(1.5_f32, TEXT),
        );
    } else {
        crate::tool_icons::paint(
            ui.painter(),
            rect.left_center() + vec2(13.0, 0.0),
            crate::tool_icons::Icon::Git,
            TEXT,
        );
    }
}

/// Draw directions without depending on the bundled font's Unicode coverage.
pub(super) fn paint_vertical_arrow(
    painter: &egui::Painter,
    center: egui::Pos2,
    down: bool,
    color: Color32,
) {
    let direction = if down { 1.0 } else { -1.0 };
    let tip = center + vec2(0.0, direction * 4.0);
    let stroke = Stroke::new(1.3_f32, color);
    painter.line_segment([center - vec2(0.0, direction * 4.0), tip], stroke);
    painter.line_segment([tip + vec2(-3.0, -direction * 3.0), tip], stroke);
    painter.line_segment([tip + vec2(3.0, -direction * 3.0), tip], stroke);
}
