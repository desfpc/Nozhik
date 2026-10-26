//! GitHub Desktop style Changes/History; channel replies are authoritative.
use crate::{diff_view::DiffView, theme};
use egui::{Color32, Context};
use nozhik_vcs::{
    Outcome, Task, Vcs,
    model::{ChangeStatus, CommitInfo, DiffTarget, FileChange, GitRepoStatus},
};
use std::{
    collections::HashSet,
    path::PathBuf,
    time::{Duration, Instant},
};

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum Mode {
    #[default]
    Changes,
    History,
}
/// Persistent UI state. No repository handles or disk operations live here.
pub struct VcsPanel {
    pub open_branches: bool,
    worker: Vcs,
    project: Option<PathBuf>,
    generation: u64,
    sequence: u64,
    pending: HashSet<u64>,
    repo: Option<GitRepoStatus>,
    mode: Mode,
    filter: String,
    summary: String,
    body: String,
    history: Vec<CommitInfo>,
    history_limit: usize,
    selected: Option<PathBuf>,
    history_sha: Option<String>,
    details: Option<(CommitInfo, Vec<FileChange>)>,
    details_request: u64,
    diff_target: Option<DiffTarget>,
    diff_request: u64,
    diff: DiffView,
    staged_diff: bool,
    refresh_at: Option<Instant>,
    next_poll: Instant,
    branch_filter: String,
    new_branch: String,
    discard: Option<PathBuf>,
    error: Option<String>,
    notice: String,
}
impl Default for VcsPanel {
    fn default() -> Self {
        Self {
            open_branches: false,
            worker: Vcs::start(),
            project: None,
            generation: 0,
            sequence: 0,
            pending: HashSet::new(),
            repo: None,
            mode: Mode::Changes,
            filter: String::new(),
            summary: String::new(),
            body: String::new(),
            history: Vec::new(),
            history_limit: 100,
            selected: None,
            history_sha: None,
            details: None,
            details_request: 0,
            diff_target: None,
            diff_request: 0,
            diff: DiffView::default(),
            staged_diff: false,
            refresh_at: None,
            next_poll: Instant::now(),
            branch_filter: String::new(),
            new_branch: String::new(),
            discard: None,
            error: None,
            notice: String::new(),
        }
    }
}
impl VcsPanel {
    pub(crate) fn set_project(&mut self, root: &std::path::Path) {
        if self.project.as_deref() == Some(root) {
            return;
        }
        self.generation = self.generation.wrapping_add(1);
        self.project = Some(root.into());
        self.repo = None;
        self.pending.clear();
        self.history.clear();
        self.details = None;
        self.selected = None;
        self.history_sha = None;
        self.diff_target = None;
        self.diff.clear();
        self.summary.clear();
        self.body.clear();
        self.discard = None;
        self.error = None;
        self.notice.clear();
        self.refresh_at = None;
        self.submit(Task::Open(root.into()));
    }
    pub(crate) fn run_global(&mut self, task: Task) {
        self.submit(task);
    }
    fn submit(&mut self, task: Task) -> u64 {
        self.sequence = self.sequence.wrapping_add(1);
        let id = self.sequence;
        if self.worker.send(self.generation, id, task) {
            self.pending.insert(id);
        } else {
            self.error = Some("Git worker stopped".into());
        }
        id
    }
    pub(crate) fn invalidate(&mut self) {
        self.refresh_at = Some(Instant::now() + Duration::from_millis(200));
    }
    pub(crate) fn branch(&self) -> &str {
        self.repo
            .as_ref()
            .map_or("No repository", |r| r.branch.as_str())
    }
    pub(crate) fn tick(&mut self, ctx: &Context, visible: bool) {
        let wake = ctx.clone();
        self.worker.set_wakeup(move || wake.request_repaint());
        let replies: Vec<_> = self.worker.drain().collect();
        for reply in replies {
            if reply.generation != self.generation {
                continue;
            }
            self.pending.remove(&reply.id);
            match reply.result {
                Ok(outcome) => self.accept(reply.id, outcome, ctx),
                Err(error) => {
                    self.error = Some(error.to_string());
                    eprintln!("Git: {error}");
                }
            }
        }
        let now = Instant::now();
        if self.project.is_some()
            && self.pending.is_empty()
            && (self.refresh_at.is_some_and(|time| now >= time)
                || ((visible || self.repo.is_some()) && now >= self.next_poll))
        {
            self.refresh_at = None;
            self.next_poll = now + Duration::from_secs(3);
            if self.repo.is_some() {
                self.submit(Task::Refresh);
            } else if let Some(root) = &self.project {
                self.submit(Task::Open(root.clone()));
            }
        }
        if self.project.is_some() && self.pending.is_empty() {
            let due = self
                .refresh_at
                .into_iter()
                .chain((visible || self.repo.is_some()).then_some(self.next_poll))
                .min();
            if let Some(due) = due {
                ctx.request_repaint_after(due.saturating_duration_since(Instant::now()));
            }
        }
        self.confirm_discard(ctx);
    }
    fn accept(&mut self, id: u64, outcome: Outcome, ctx: &Context) {
        match outcome {
            Outcome::Status(repo) => {
                let changed_head = self.repo.as_ref().and_then(|r| r.head_sha.as_ref())
                    != repo.as_ref().and_then(|r| r.head_sha.as_ref());
                let first = self.repo.is_none();
                self.repo = repo;
                ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
                    "Nozhik — {}",
                    self.branch()
                )));
                if (first || changed_head) && self.repo.is_some() {
                    self.submit(Task::History(self.history_limit));
                }
                if self.mode == Mode::Changes {
                    self.refresh_selection();
                }
            }
            Outcome::Diff(diff) if id == self.diff_request => self.diff.set(diff),
            Outcome::History(history) => {
                self.history = history;
            }
            Outcome::Details { commit, files }
                if self.mode == Mode::History && id == self.details_request =>
            {
                let first = files.first().map(|f| f.path.clone());
                let sha = commit.sha.clone();
                self.details = Some((commit, files));
                if let Some(path) = first {
                    self.request_diff(DiffTarget::Commit { sha, path });
                } else {
                    self.diff.clear();
                }
            }
            Outcome::Committed(commit) => {
                self.summary.clear();
                self.body.clear();
                self.error = None;
                self.notice = format!("Committed {}", &commit.sha[..7]);
                self.submit(Task::Refresh);
            }
            Outcome::Done(message) => {
                self.error = None;
                self.notice = message;
                self.submit(Task::Refresh);
            }
            _ => {}
        }
    }
    fn refresh_selection(&mut self) {
        let Some(repo) = &self.repo else {
            self.diff.clear();
            return;
        };
        let selected = self
            .selected
            .as_ref()
            .filter(|p| repo.changes.iter().any(|c| &c.path == *p))
            .cloned()
            .or_else(|| repo.changes.first().map(|c| c.path.clone()));
        self.selected.clone_from(&selected);
        if let Some(path) = selected {
            self.request_diff(DiffTarget::Working {
                path,
                staged: self.staged_diff,
            });
        } else {
            self.diff.clear();
            self.diff_target = None;
        }
    }
    fn request_diff(&mut self, target: DiffTarget) {
        if self.diff_target.as_ref() != Some(&target) {
            self.diff.clear();
        }
        self.diff_target = Some(target.clone());
        self.diff_request = self.submit(Task::Diff(target));
    }
    pub(crate) fn toolbar(&mut self, ui: &mut egui::Ui) {
        if self.open_branches {
            let mut open = true;
            egui::Window::new("Branches")
                .open(&mut open)
                .show(ui.ctx(), |ui| {
                    self.branch_controls(ui);
                });
            self.open_branches = open;
        }
        self.branch_controls(ui);
    }
    fn branch_controls(&mut self, ui: &mut egui::Ui) {
        let label = format!("⑂ {} v", self.branch());
        let branches = self
            .repo
            .as_ref()
            .map(|r| r.branches.clone())
            .unwrap_or_default();
        ui.menu_button(label, |ui| {
            ui.add(egui::TextEdit::singleline(&mut self.branch_filter).hint_text("Find a branch"));
            let filter = self.branch_filter.to_lowercase();
            for branch in branches
                .iter()
                .filter(|b| b.to_lowercase().contains(&filter))
            {
                if ui
                    .add_enabled(self.pending.is_empty(), egui::Button::new(branch))
                    .clicked()
                {
                    self.submit(Task::Checkout(branch.clone()));
                    ui.close();
                }
            }
            ui.separator();
            ui.add(egui::TextEdit::singleline(&mut self.new_branch).hint_text("New branch name"));
            if ui
                .add_enabled(
                    self.repo.is_some()
                        && self.pending.is_empty()
                        && !self.new_branch.trim().is_empty(),
                    egui::Button::new("New Branch"),
                )
                .clicked()
            {
                self.submit(Task::NewBranch(self.new_branch.trim().to_owned()));
                ui.close();
            }
        });
        let can_network =
            self.pending.is_empty() && self.repo.as_ref().is_some_and(|r| r.has_origin);
        let push = self.repo.as_ref().is_some_and(|r| {
            r.ahead > 0 || (!r.has_upstream && !r.detached && r.head_sha.is_some())
        });
        if ui
            .add_enabled(
                can_network,
                egui::Button::new(if push {
                    "↑ Push origin"
                } else {
                    "↓ Fetch origin"
                }),
            )
            .clicked()
        {
            self.submit(if push { Task::Push } else { Task::Fetch });
        }
        ui.menu_button("…", |ui| {
            for (label, task) in [
                ("Fetch origin", Task::Fetch),
                ("Pull origin (fast-forward)", Task::Pull),
                ("Push origin", Task::Push),
            ] {
                if ui
                    .add_enabled(can_network, egui::Button::new(label))
                    .clicked()
                {
                    self.submit(task);
                    ui.close();
                }
            }
            if ui
                .add_enabled(
                    self.pending.is_empty() && self.repo.is_some(),
                    egui::Button::new("Refresh Git status"),
                )
                .clicked()
            {
                self.submit(Task::Refresh);
                ui.close();
            }
        });
        if let Some(repo) = &self.repo {
            ui.weak(format!("↑{} ↓{}", repo.ahead, repo.behind));
        }
        if !self.pending.is_empty() {
            ui.spinner();
        }
    }
    pub(crate) fn panel(&mut self, ui: &mut egui::Ui) {
        let count = self.repo.as_ref().map_or(0, |r| r.changes.len());
        ui.horizontal(|ui| {
            for (mode, label) in [
                (Mode::Changes, format!("Changes [{count}]")),
                (Mode::History, "History".into()),
            ] {
                if ui.selectable_label(self.mode == mode, label).clicked() && self.mode != mode {
                    self.mode = mode;
                    self.diff.clear();
                    self.diff_target = None;
                    // Invalidate an outstanding diff from the other mode.
                    self.diff_request = 0;
                    if mode == Mode::Changes {
                        self.refresh_selection();
                    } else if let Some(sha) = self
                        .history_sha
                        .clone()
                        .or_else(|| self.history.first().map(|c| c.sha.clone()))
                    {
                        self.select_commit(sha);
                    }
                }
            }
        });
        ui.separator();
        if let Some(error) = &self.error {
            ui.colored_label(crate::theme::ERROR, error);
        }
        if !self.notice.is_empty() {
            ui.weak(&self.notice);
        }
        let Some(repo) = &self.repo else {
            ui.weak("No Git repository found in this project");
            return;
        };
        ui.weak(repo.root.display().to_string());
        if let Some(operation) = &repo.operation {
            ui.colored_label(
                crate::theme::WARNING,
                format!("Repository operation: {operation}"),
            );
        }
        match self.mode {
            Mode::Changes => self.changes(ui),
            Mode::History => self.history_list(ui),
        }
    }
    pub(crate) fn commit_selected(&mut self) {
        if self.pending.is_empty()
            && self.discard.is_none()
            && !self.summary.trim().is_empty()
            && self.repo.as_ref().is_some_and(|r| {
                r.operation.is_none()
                    && r.changes.iter().any(|c| c.is_staged)
                    && !r
                        .changes
                        .iter()
                        .any(|c| c.status == ChangeStatus::Conflicted)
            })
        {
            self.submit(Task::Commit {
                summary: self.summary.clone(),
                body: self.body.clone(),
            });
        }
    }
    fn changes(&mut self, ui: &mut egui::Ui) {
        ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("Filter"));
        let Some(repo) = &self.repo else {
            return;
        };
        let changes = repo.changes.clone();
        let branch = repo.branch.clone();
        let busy = !self.pending.is_empty();
        let mut all = !changes.is_empty() && changes.iter().all(|c| c.is_staged && !c.has_unstaged);
        if ui
            .add_enabled(
                !busy && !changes.is_empty(),
                egui::Checkbox::new(&mut all, format!("{} changed files", changes.len())),
            )
            .changed()
        {
            self.submit(Task::Stage {
                paths: changes.iter().map(|c| c.path.clone()).collect(),
                staged: all,
            });
        }
        let filter = self.filter.to_lowercase();
        let list_height = (ui.available_height() - 220.0).max(60.0);
        egui::ScrollArea::vertical()
            .id_salt("git_changes")
            .max_height(list_height)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for change in changes
                    .iter()
                    .filter(|c| c.path.to_string_lossy().to_lowercase().contains(&filter))
                {
                    self.change_row(ui, change, busy);
                }
                if changes.is_empty() {
                    ui.weak("No local changes");
                }
            });
        ui.separator();
        ui.add(egui::TextEdit::singleline(&mut self.summary).hint_text("Summary (required)"));
        ui.add(
            egui::TextEdit::multiline(&mut self.body)
                .hint_text("Description")
                .desired_rows(4),
        );
        let staged = changes.iter().filter(|c| c.is_staged).count();
        ui.weak("Commit uses the staged version; save editor buffers first.");
        let allowed = !busy
            && staged > 0
            && !self.summary.trim().is_empty()
            && self.repo.as_ref().is_some_and(|r| {
                r.operation.is_none()
                    && !r
                        .changes
                        .iter()
                        .any(|c| c.status == ChangeStatus::Conflicted)
            });
        if ui
            .add_enabled(
                allowed,
                egui::Button::new(format!("Commit {staged} files to {branch}")).fill(theme::ACCENT),
            )
            .clicked()
        {
            self.submit(Task::Commit {
                summary: self.summary.clone(),
                body: self.body.clone(),
            });
        }
    }
    fn change_row(&mut self, ui: &mut egui::Ui, change: &FileChange, busy: bool) {
        ui.horizontal(|ui| {
            let mut staged = change.is_staged;
            if ui
                .add_enabled(
                    !busy,
                    egui::Checkbox::new(&mut staged, "")
                        .indeterminate(change.is_staged && change.has_unstaged),
                )
                .changed()
            {
                self.submit(Task::Stage {
                    paths: vec![change.path.clone()],
                    staged,
                });
            }
            let (marker, color) = match change.status {
                ChangeStatus::New => ("+", crate::theme::SUCCESS),
                ChangeStatus::Deleted => ("−", crate::theme::ERROR),
                ChangeStatus::Renamed => ("→", crate::theme::INFO),
                ChangeStatus::Conflicted => ("!", crate::theme::ERROR),
                ChangeStatus::Modified => ("●", crate::theme::WARNING),
            };
            ui.colored_label(color, marker);
            let row = ui.selectable_label(
                self.selected.as_ref() == Some(&change.path),
                change.path.to_string_lossy(),
            );
            if row.clicked() {
                self.selected = Some(change.path.clone());
                self.request_diff(DiffTarget::Working {
                    path: change.path.clone(),
                    staged: self.staged_diff,
                });
            }
            row.context_menu(|ui| {
                if ui
                    .add_enabled(!busy, egui::Button::new("Discard Changes…"))
                    .clicked()
                {
                    self.discard = Some(change.path.clone());
                    ui.close();
                }
                if ui
                    .add_enabled(!busy, egui::Button::new("Ignore File (Add to .gitignore)"))
                    .clicked()
                {
                    self.submit(Task::Ignore(change.path.clone()));
                    ui.close();
                }
            });
        });
    }
    fn select_commit(&mut self, sha: String) {
        self.history_sha = Some(sha.clone());
        self.details = None;
        self.diff.clear();
        self.diff_target = None;
        self.diff_request = 0;
        self.details_request = self.submit(Task::Details(sha));
    }
    fn history_list(&mut self, ui: &mut egui::Ui) {
        let mut select = None;
        if self.history.is_empty() {
            ui.weak("No commits yet");
        }
        let height = (ui.available_height() - 36.0).max(60.0);
        egui::ScrollArea::vertical()
            .id_salt("git_history")
            .max_height(height)
            .auto_shrink([false, false])
            .show_rows(ui, 48.0, self.history.len(), |ui, rows| {
                for index in rows {
                    let commit = &self.history[index];
                    let selected = self.history_sha.as_ref() == Some(&commit.sha);
                    egui::Frame::new()
                        .fill(if selected {
                            theme::ACCENT
                        } else {
                            Color32::TRANSPARENT
                        })
                        .inner_margin(6)
                        .show(ui, |ui| {
                            let response = ui.add(
                                egui::Label::new(egui::RichText::new(&commit.summary).strong())
                                    .truncate()
                                    .sense(egui::Sense::click()),
                            );
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(format!(
                                        "{} · {}",
                                        commit.author, commit.relative_time
                                    ))
                                    .small(),
                                )
                                .truncate(),
                            );
                            if response.clicked() {
                                select = Some(commit.sha.clone());
                            }
                        });
                }
            });
        if self.history.len() >= self.history_limit
            && self.history_limit < 10_000
            && ui
                .add_enabled(
                    self.pending.is_empty(),
                    egui::Button::new("Load more commits"),
                )
                .clicked()
        {
            self.history_limit = (self.history_limit + 100).min(10_000);
            self.submit(Task::History(self.history_limit));
        }
        if let Some(sha) = select {
            self.select_commit(sha);
        }
    }
    pub(crate) fn center(&mut self, ui: &mut egui::Ui, font_size: f32) {
        if self.mode == Mode::History {
            let mut target = None;
            if let Some((commit, files)) = &self.details {
                ui.heading(&commit.summary);
                ui.label(format!("{} · {}", commit.author, commit.relative_time));
                if !commit.body.is_empty() {
                    egui::ScrollArea::vertical()
                        .id_salt("commit_body")
                        .max_height(100.0)
                        .show(ui, |ui| {
                            ui.label(&commit.body);
                        });
                }
                ui.horizontal(|ui| {
                    ui.monospace(&commit.sha);
                    if ui.button("Copy SHA").clicked() {
                        ui.ctx().copy_text(commit.sha.clone());
                    }
                    ui.weak(format!("short: {}", &commit.sha[..7]));
                });
                egui::ScrollArea::vertical()
                    .id_salt("commit_files")
                    .max_height(130.0)
                    .show(ui, |ui| {
                        for file in files {
                            let diff_target = DiffTarget::Commit {
                                sha: commit.sha.clone(),
                                path: file.path.clone(),
                            };
                            if ui
                                .selectable_label(
                                    self.diff_target.as_ref() == Some(&diff_target),
                                    file.path.to_string_lossy(),
                                )
                                .clicked()
                            {
                                target = Some(diff_target);
                            }
                        }
                    });
                ui.separator();
            } else {
                ui.weak("Select a commit in History");
            }
            if let Some(target) = target {
                self.request_diff(target);
            }
        } else {
            ui.horizontal(|ui| {
                ui.strong("Diff");
                if ui.checkbox(&mut self.staged_diff, "Staged only").changed() {
                    self.refresh_selection();
                }
                ui.weak(if self.staged_diff {
                    "HEAD → index (commit contents)"
                } else {
                    "HEAD → working tree"
                });
            });
            ui.separator();
        }
        self.diff.show(ui, font_size);
    }
    fn confirm_discard(&mut self, ctx: &Context) {
        let Some(path) = self.discard.clone() else {
            return;
        };
        let modal = egui::Modal::new(egui::Id::new("discard_changes")).show(ctx, |ui| {
            ui.heading("Discard Changes?");
            ui.label(path.to_string_lossy());
            ui.label("This restores the file and its staging to HEAD. An untracked file is deleted. Editor buffers are retained.");
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() { self.discard = None; }
                if ui.add_enabled(self.pending.is_empty(), egui::Button::new("Discard Changes").fill(crate::theme::DESTRUCTIVE)).clicked() {
                    self.submit(Task::Discard(path)); self.discard = None;
                }
            });
        });
        if modal.should_close() {
            self.discard = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn until(
        panel: &mut VcsPanel,
        ctx: &Context,
        done: impl Fn(&VcsPanel) -> bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let end = Instant::now() + Duration::from_secs(5);
        while !done(panel) {
            panel.tick(ctx, false);
            if Instant::now() > end {
                return Err(format!("VCS timeout: {:?}", panel.error).into());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(())
    }
    #[test]
    fn changes_commit_history_and_diff_render_without_a_window()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let repo = git2::Repository::init(dir.path())?;
        let mut config = repo.config()?;
        config.set_str("user.name", "UI Test")?;
        config.set_str("user.email", "ui@example.invalid")?;
        drop(config);
        std::fs::write(dir.path().join("main.rs"), "fn main() {}\n")?;
        let mut panel = VcsPanel::default();
        let ctx = Context::default();
        theme::apply(&ctx);
        panel.set_project(dir.path());
        until(&mut panel, &ctx, |p| {
            p.repo.is_some() && p.pending.is_empty()
        })?;
        panel.summary = "Initial commit".into();
        panel.commit_selected();
        assert!(panel.pending.is_empty()); // Nothing staged.
        panel.submit(Task::Stage {
            paths: vec!["main.rs".into()],
            staged: true,
        });
        until(&mut panel, &ctx, |p| {
            p.pending.is_empty()
                && p.repo
                    .as_ref()
                    .is_some_and(|r| r.changes.iter().any(|c| c.is_staged))
        })?;
        panel.commit_selected();
        assert_eq!(panel.summary, "Initial commit"); // Clear only after success.
        until(&mut panel, &ctx, |p| {
            p.pending.is_empty() && p.summary.is_empty() && p.history.len() == 1
        })?;
        assert!(panel.repo.as_ref().is_some_and(|r| r.changes.is_empty()));
        panel.mode = Mode::History;
        let sha = panel.history[0].sha.clone();
        panel.select_commit(sha);
        until(&mut panel, &ctx, |p| {
            p.pending.is_empty() && p.details.is_some()
        })?;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::SidePanel::left("vcs_test").show(ctx, |ui| {
                panel.panel(ui);
            });
            egui::CentralPanel::default().show(ctx, |ui| {
                panel.center(ui, 14.0);
            });
        });
        // An old Details response must not switch a Changes view to a historical diff.
        let commit = panel.history[0].clone();
        panel.mode = Mode::Changes;
        panel.diff_request = 0;
        panel.accept(
            panel.details_request,
            Outcome::Details {
                commit,
                files: vec![FileChange {
                    path: "main.rs".into(),
                    old_path: None,
                    status: ChangeStatus::New,
                    is_staged: false,
                    has_unstaged: false,
                }],
            },
            &ctx,
        );
        assert_eq!(panel.diff_request, 0);
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                panel.panel(ui);
            });
        });
        Ok(())
    }
}

#[cfg(test)]
mod idle_tests {
    use super::*;
    #[test]
    fn hidden_non_repository_does_not_schedule_an_expired_poll()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let mut panel = VcsPanel::default();
        let ctx = egui::Context::default();
        panel.set_project(dir.path());
        let deadline = Instant::now() + Duration::from_secs(3);
        while !panel.pending.is_empty() && Instant::now() < deadline {
            let _frame = ctx.run(egui::RawInput::default(), |ctx| panel.tick(ctx, false));
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(panel.pending.is_empty());
        assert!(panel.repo.is_none());
        let mut delay = Duration::ZERO;
        for i in 0..10 {
            let frame = ctx.run(
                egui::RawInput {
                    time: Some(f64::from(i)),
                    ..egui::RawInput::default()
                },
                |ctx| panel.tick(ctx, false),
            );
            delay = frame
                .viewport_output
                .get(&egui::ViewportId::ROOT)
                .map_or(Duration::MAX, |v| v.repaint_delay);
        }
        assert!(
            delay > Duration::from_secs(1),
            "expired hidden poll caused {delay:?}"
        );
        Ok(())
    }
}
