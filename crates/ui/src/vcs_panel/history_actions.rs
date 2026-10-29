//! Commit selection, context menus and confirmed history actions.
use super::{
    Mode, Task, VcsPanel,
    controls::{BLUE, SURFACE},
    theme,
};
use nozhik_vcs::model::{HistoryAction, HistoryRequest, ResetMode};

#[derive(Clone)]
pub(super) struct ActionDialog {
    title: String,
    request: HistoryRequest,
    summary: String,
    body: String,
    branch: String,
    filter: String,
    create: bool,
    moved: Vec<String>,
    request_id: Option<u64>,
}
impl VcsPanel {
    pub(super) fn update_history(&mut self, history: Vec<super::CommitInfo>) {
        let previous_selection = self.history_selection.clone();
        self.history_selection
            .retain(|sha| history.iter().any(|c| &c.sha == sha));
        self.history = history;
        if self.mode == Mode::History
            && self
                .history_sha
                .as_ref()
                .is_none_or(|sha| !self.history.iter().any(|c| &c.sha == sha))
        {
            self.details = None;
            self.details_request = 0;
            self.diff_request = 0;
            self.diff_target = None;
            self.diff.clear();
            if let Some(sha) = self.history.first().map(|c| c.sha.clone()) {
                self.history_click(&sha, egui::Modifiers::NONE);
            }
        } else if self.mode == Mode::History && previous_selection != self.history_selection {
            if let Some(sha) = self.history_selection.first().cloned() {
                self.select_commit(sha);
            } else if let Some(sha) = self.history.first().map(|c| c.sha.clone()) {
                self.history_click(&sha, egui::Modifiers::NONE);
            }
        }
    }
    pub(super) fn history_click(&mut self, sha: &str, modifiers: egui::Modifiers) {
        if modifiers.shift {
            let anchor = self
                .history_anchor
                .as_ref()
                .and_then(|s| self.history.iter().position(|c| &c.sha == s));
            let end = self.history.iter().position(|c| c.sha == sha);
            if let (Some(a), Some(b)) = (anchor, end) {
                let range: Vec<_> = self.history[a.min(b)..=a.max(b)]
                    .iter()
                    .map(|c| c.sha.clone())
                    .collect();
                if !modifiers.command {
                    self.history_selection.clear();
                }
                for id in range {
                    if !self.history_selection.contains(&id) {
                        self.history_selection.push(id);
                    }
                }
            } else {
                self.history_selection = vec![sha.to_owned()];
                self.history_anchor = Some(sha.to_owned());
            }
        } else {
            if modifiers.command {
                if self.history_selection.iter().any(|s| s == sha) {
                    self.history_selection.retain(|s| s != sha);
                } else {
                    self.history_selection.push(sha.to_owned());
                }
            } else {
                self.history_selection = vec![sha.to_owned()];
            }
            self.history_anchor = Some(sha.to_owned());
        }
        if self.history_selection.is_empty() {
            self.history_sha = None;
            self.details = None;
            self.details_request = 0;
            self.diff_target = None;
            self.diff_request = 0;
            self.diff.clear();
        } else {
            self.select_commit(sha.to_owned());
        }
    }
    fn selected_commits(&self) -> Vec<String> {
        self.history
            .iter()
            .filter(|c| self.history_selection.contains(&c.sha))
            .map(|c| c.sha.clone())
            .collect()
    }
    #[allow(
        clippy::too_many_lines,
        reason = "Single and multiple selections share the same contextual action table"
    )]
    pub(super) fn history_menu(&mut self, ui: &mut egui::Ui) {
        let ids = self.selected_commits();
        if ids.is_empty() {
            return;
        }
        let count = ids.len();
        let busy = !self.mutations.is_empty();
        let head = self.repo.as_ref().and_then(|r| r.head_sha.as_ref());
        let single = count == 1;
        let contiguous = self
            .history
            .iter()
            .enumerate()
            .filter_map(|(i, c)| ids.contains(&c.sha).then_some(i))
            .collect::<Vec<_>>();
        let consecutive = contiguous
            .last()
            .zip(contiguous.first())
            .is_some_and(|(b, a)| b - a + 1 == count);
        let actions = if single {
            vec![
                (
                    "Amend Commit…".into(),
                    HistoryAction::Amend(String::new()),
                    head == ids.first(),
                ),
                (
                    "Reset to Commit…".into(),
                    HistoryAction::Reset(ResetMode::Mixed),
                    true,
                ),
                ("Checkout Commit…".into(), HistoryAction::Checkout, true),
                ("Reorder Commit…".into(), HistoryAction::Reorder, true),
                (
                    "Revert Changes in Commit…".into(),
                    HistoryAction::Revert,
                    true,
                ),
                (
                    "Create Branch from Commit…".into(),
                    HistoryAction::Branch(String::new()),
                    true,
                ),
                (
                    "Create Tag…".into(),
                    HistoryAction::Tag(String::new()),
                    true,
                ),
                (
                    "Cherry-pick Commit…".into(),
                    HistoryAction::CherryPick {
                        branch: String::new(),
                        create: false,
                    },
                    true,
                ),
            ]
        } else {
            vec![
                (
                    format!("Cherry-pick {count} Commits…"),
                    HistoryAction::CherryPick {
                        branch: String::new(),
                        create: false,
                    },
                    true,
                ),
                (
                    format!("Squash {count} Commits…"),
                    HistoryAction::Squash(String::new()),
                    consecutive,
                ),
                (
                    format!("Reorder {count} Commits…"),
                    HistoryAction::Reorder,
                    true,
                ),
            ]
        };
        for (title, action, enabled) in actions {
            if ui
                .add_enabled(!busy && enabled, egui::Button::new(&title))
                .clicked()
            {
                self.open_history_action(title, action, ids.clone());
                ui.close();
            }
        }
        ui.separator();
        if ui
            .button(if single { "Copy SHA" } else { "Copy SHAs" })
            .clicked()
        {
            ui.ctx().copy_text(ids.join("\n"));
            ui.close();
        }
        if single {
            for (title, action) in [
                ("Copy Tag", HistoryAction::CopyTags),
                ("View on GitHub", HistoryAction::View),
            ] {
                if ui
                    .add_enabled(
                        title != "Copy Tag"
                            || self
                                .history
                                .iter()
                                .any(|c| ids.contains(&c.sha) && !c.tags.is_empty()),
                        egui::Button::new(title),
                    )
                    .clicked()
                {
                    self.submit_history_metadata(ids.clone(), action);
                    ui.close();
                }
            }
        }
    }
    fn submit_history_metadata(&mut self, commits: Vec<String>, action: HistoryAction) {
        if let Some(expected_head) = self.repo.as_ref().and_then(|r| r.head_sha.clone()) {
            self.submit(Task::HistoryAction(HistoryRequest {
                expected_head,
                expected_branch: self.branch().to_owned(),
                commits,
                action,
            }));
        }
    }
    fn open_history_action(
        &mut self,
        title: String,
        action: HistoryAction,
        mut commits: Vec<String>,
    ) {
        let Some(expected_head) = self.repo.as_ref().and_then(|r| r.head_sha.clone()) else {
            return;
        };
        let selected = self
            .history
            .iter()
            .find(|c| commits.first() == Some(&c.sha));
        let summary = selected.map_or_else(String::new, |c| c.summary.clone());
        let body = if matches!(action, HistoryAction::Squash(_)) {
            self.history
                .iter()
                .filter(|c| commits.contains(&c.sha))
                .map(|c| format!("{}\n{}", c.summary, c.body))
                .collect::<Vec<_>>()
                .join("\n\n")
        } else {
            selected.map_or_else(String::new, |c| c.body.clone())
        };
        let moved = commits.clone();
        if matches!(action, HistoryAction::Reorder)
            && let Some(end) = self.history.iter().rposition(|c| commits.contains(&c.sha))
        {
            commits = self.history[..=end].iter().map(|c| c.sha.clone()).collect();
            // Include one older commit so even the HEAD row can move downward.
            if let Some(older) = self.history.get(end + 1).filter(|c| c.parent_count > 0) {
                commits.push(older.sha.clone());
            }
        }
        self.error = None;
        self.history_dialog = Some(ActionDialog {
            title,
            request: HistoryRequest {
                expected_head,
                expected_branch: self.branch().to_owned(),
                commits,
                action,
            },
            summary,
            body,
            branch: String::new(),
            filter: String::new(),
            create: false,
            moved,
            request_id: None,
        });
    }
    pub(super) fn history_action_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.history_dialog.take() else {
            return;
        };
        if dialog
            .request_id
            .is_some_and(|id| !self.pending.contains(&id))
            && self.error.is_none()
        {
            return;
        }
        let working = dialog
            .request_id
            .is_some_and(|id| self.pending.contains(&id));
        let mut close = false;
        let mut apply = false;
        let modal = egui::Modal::new(egui::Id::new("git_history_action"))
            .frame(egui::Frame::popup(&ctx.style()).fill(SURFACE).inner_margin(22))
            .show(ctx, |ui| {
                ui.set_width(540.0_f32.min((ctx.screen_rect().width()-64.0).max(180.0)));
                ui.horizontal(|ui| { ui.heading(dialog.title.trim_end_matches('…')); ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| { if ui.button("×").clicked() { close = true; } }); });
                ui.separator();
                ui.add_enabled_ui(!working, |ui| {
                    self.history_dialog_fields(ui, &mut dialog, &mut apply);
                    ui.weak("Merge commits use their first parent; replaying them flattens the merge. Git acts on saved files; existing editor buffers are retained.");
                    ui.separator();
                    if let Some(error) = &self.error { ui.colored_label(theme::ERROR, error); }
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() { close = true; }
                        if ui.add_enabled(self.mutations.is_empty() && dialog.valid(), egui::Button::new(dialog.confirm_label()).fill(BLUE).min_size(egui::vec2(ui.available_width(), 32.0))).clicked() { apply = true; }
                    });
                });
                if working { ui.spinner(); ui.label("Applying history action…"); }
            });
        if modal.should_close() && !working {
            close = true;
        }
        if apply && !working && self.mutations.is_empty() && dialog.valid() {
            let message = format!("{}\n\n{}", dialog.summary.trim(), dialog.body.trim());
            match &mut dialog.request.action {
                HistoryAction::Amend(text) | HistoryAction::Squash(text) => *text = message,
                HistoryAction::Branch(name) | HistoryAction::Tag(name) => {
                    dialog.branch.trim().clone_into(name);
                }
                HistoryAction::CherryPick { branch, create } => {
                    dialog.branch.trim().clone_into(branch);
                    *create = dialog.create;
                }
                _ => {}
            }
            self.error = None;
            dialog.request_id = Some(self.submit(Task::HistoryAction(dialog.request.clone())));
        }
        if !close || working {
            self.history_dialog = Some(dialog);
        }
    }
    #[allow(
        clippy::too_many_lines,
        reason = "Exhaustive dialog fields share one action snapshot and confirm gate"
    )]
    fn history_dialog_fields(
        &self,
        ui: &mut egui::Ui,
        dialog: &mut ActionDialog,
        apply: &mut bool,
    ) {
        match &mut dialog.request.action {
            HistoryAction::Amend(_) | HistoryAction::Squash(_) => {
                ui.label("Summary");
                ui.add(
                    egui::TextEdit::singleline(&mut dialog.summary).desired_width(f32::INFINITY),
                );
                ui.label("Description");
                ui.add(
                    egui::TextEdit::multiline(&mut dialog.body)
                        .desired_width(f32::INFINITY)
                        .desired_rows(7),
                );
                ui.colored_label(
                    theme::WARNING,
                    "This rewrites commit IDs. Published history may require coordination.",
                );
                if matches!(dialog.request.action, HistoryAction::Amend(_)) {
                    ui.label(
                        "Current staged changes will be included; unstaged changes are retained.",
                    );
                }
            }
            HistoryAction::Branch(_) | HistoryAction::Tag(_) => {
                ui.label("Name");
                ui.text_edit_singleline(&mut dialog.branch);
            }
            HistoryAction::CherryPick { .. } => self.cherry_pick_fields(ui, dialog),
            HistoryAction::Reset(mode) => {
                ui.radio_value(
                    mode,
                    ResetMode::Soft,
                    "Soft — keep staged and working changes",
                );
                ui.radio_value(
                    mode,
                    ResetMode::Mixed,
                    "Mixed — unstage changes, keep working files",
                );
                ui.radio_value(
                    mode,
                    ResetMode::Hard,
                    "Hard — discard tracked working and staged changes",
                );
                ui.colored_label(
                    theme::WARNING,
                    "Moves the branch to the selected commit. Later commits leave this branch.",
                );
            }
            HistoryAction::Reorder => {
                ui.label("Use the Up / Down keys to move selected commits. Enter confirms; Escape cancels.");
                let up = move_button(ui, false) || ui.input(|i| i.key_pressed(egui::Key::ArrowUp));
                let down =
                    move_button(ui, true) || ui.input(|i| i.key_pressed(egui::Key::ArrowDown));
                if up {
                    move_commits(&mut dialog.request.commits, &dialog.moved, true);
                }
                if down
                    && dialog
                        .request
                        .commits
                        .last()
                        .is_some_and(|id| dialog.moved.contains(id))
                    && let Some(next) = dialog
                        .request
                        .commits
                        .last()
                        .and_then(|id| self.history.iter().position(|c| &c.sha == id))
                        .and_then(|i| self.history.get(i + 1))
                        .filter(|c| c.parent_count > 0)
                {
                    dialog.request.commits.push(next.sha.clone());
                }
                if down {
                    move_commits(&mut dialog.request.commits, &dialog.moved, false);
                }
                egui::ScrollArea::vertical()
                    .max_height(260.0)
                    .show(ui, |ui| {
                        for sha in &dialog.request.commits {
                            if let Some(c) = self.history.iter().find(|c| &c.sha == sha) {
                                ui.selectable_label(dialog.moved.contains(sha), &c.summary)
                                    .on_hover_text(sha);
                            }
                        }
                    });
                ui.colored_label(
                    theme::WARNING,
                    "Rewrites this linear history through HEAD. Conflicting orders are refused.",
                );
                *apply |= ui.input(|i| i.key_pressed(egui::Key::Enter));
            }
            HistoryAction::Revert => {
                ui.label(
                    "Create a new commit that reverses the selected commit on the current branch.",
                );
            }
            HistoryAction::Checkout => {
                ui.label("Check out this commit in detached HEAD mode. Commit or stash local changes first.");
            }
            HistoryAction::CopyTags | HistoryAction::View => {}
        }
    }
    fn cherry_pick_fields(&self, ui: &mut egui::Ui, dialog: &mut ActionDialog) {
        ui.checkbox(&mut dialog.create, "New Branch (based on current HEAD)");
        if dialog.create {
            ui.text_edit_singleline(&mut dialog.branch);
        } else {
            ui.add(
                egui::TextEdit::singleline(&mut dialog.filter)
                    .hint_text("Filter branches")
                    .desired_width(f32::INFINITY),
            );
            egui::ScrollArea::vertical()
                .max_height(260.0)
                .show(ui, |ui| {
                    if let Some(repo) = &self.repo {
                        for branch in &repo.branch_info {
                            if !branch.is_remote
                                && branch
                                    .name
                                    .to_lowercase()
                                    .contains(&dialog.filter.to_lowercase())
                            {
                                ui.selectable_value(
                                    &mut dialog.branch,
                                    branch.name.clone(),
                                    format!("{}    {}", branch.name, branch.relative_time),
                                );
                            }
                        }
                    }
                });
        }
        ui.label(format!(
            "Replay {} commits onto {} and switch to it.",
            dialog.request.commits.len(),
            dialog.branch
        ));
        ui.weak("Conflicting patches are refused before changing files or branches.");
    }
}
impl ActionDialog {
    fn confirm_label(&self) -> String {
        let count = self.request.commits.len();
        match &self.request.action {
            HistoryAction::Amend(_) => "Amend Commit".into(),
            HistoryAction::Reset(_) => "Reset to Commit".into(),
            HistoryAction::Checkout => "Checkout Commit".into(),
            HistoryAction::Revert => "Revert Commit".into(),
            HistoryAction::Branch(_) => "Create Branch".into(),
            HistoryAction::Tag(_) => "Create Tag".into(),
            HistoryAction::CherryPick { .. } => {
                format!("Cherry-pick {count} Commits to {}", self.branch)
            }
            HistoryAction::Squash(_) => format!("Squash {count} Commits"),
            HistoryAction::Reorder => "Reorder Commits".into(),
            HistoryAction::CopyTags | HistoryAction::View => "Confirm".into(),
        }
    }
    fn valid(&self) -> bool {
        match self.request.action {
            HistoryAction::Amend(_) | HistoryAction::Squash(_) => !self.summary.trim().is_empty(),
            HistoryAction::Branch(_) | HistoryAction::Tag(_) | HistoryAction::CherryPick { .. } => {
                !self.branch.trim().is_empty()
            }
            _ => true,
        }
    }
}
fn move_commits(order: &mut [String], selected: &[String], up: bool) {
    if up {
        for i in 1..order.len() {
            if selected.contains(&order[i]) && !selected.contains(&order[i - 1]) {
                order.swap(i, i - 1);
            }
        }
    } else {
        for i in (0..order.len().saturating_sub(1)).rev() {
            if selected.contains(&order[i]) && !selected.contains(&order[i + 1]) {
                order.swap(i, i + 1);
            }
        }
    }
}

fn move_button(ui: &mut egui::Ui, down: bool) -> bool {
    let response = ui.add(
        egui::Button::new(if down { "Move down" } else { "Move up" })
            .min_size(egui::vec2(116.0, 28.0)),
    );
    super::controls::paint_vertical_arrow(
        ui.painter(),
        response.rect.left_center() + egui::vec2(10.0, 0.0),
        down,
        super::controls::TEXT,
    );
    response.clicked()
}
