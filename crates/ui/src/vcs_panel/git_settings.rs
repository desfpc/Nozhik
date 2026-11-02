//! Project Git settings use the same worker and authentication form as cloning.
use super::{Task, VcsPanel};
use nozhik_vcs::connection::GitSettings;

#[derive(Default)]
pub(super) struct SettingsState {
    pub snapshot: Option<GitSettings>,
    draft: GitSettings,
    auth: crate::git_auth::AuthFields,
    request: Option<u64>,
    testing: Option<u64>,
    saving: bool,
    error: Option<String>,
    notice: String,
}
impl VcsPanel {
    pub(crate) fn git_settings_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Git");
        let Some(repo) = &self.repo else {
            ui.weak("Open a project containing a Git repository to configure Git.");
            return;
        };
        ui.small(repo.root.display().to_string());
        if self.git_settings.snapshot.is_none()
            && self.git_settings.request.is_none()
            && self.git_settings.error.is_none()
        {
            self.git_settings.request = Some(self.submit(Task::GitSettings));
        }
        let busy = self.git_settings.request.is_some()
            || self.git_settings.testing.is_some()
            || !self.mutations.is_empty();
        if self.git_settings.snapshot.is_some() {
            ui.add_enabled_ui(!busy, |ui| {
                ui.label("Origin fetch URL");
                ui.add(egui::TextEdit::singleline(&mut self.git_settings.draft.remote_url).desired_width(f32::INFINITY));
                ui.label("Origin push URL (optional)");
                ui.add(egui::TextEdit::singleline(&mut self.git_settings.draft.push_url).hint_text("Use fetch URL").desired_width(f32::INFINITY));
                ui.separator();
                ui.label("Commit author name");
                ui.add(egui::TextEdit::singleline(&mut self.git_settings.draft.author_name).desired_width(f32::INFINITY));
                ui.label("Commit author email");
                ui.add(egui::TextEdit::singleline(&mut self.git_settings.draft.author_email).desired_width(f32::INFINITY));
                ui.small("Author identity is used for local commits. Blank values use system Git settings.");
                ui.separator();
                if let Err(error) = self.git_settings.auth.show(ui) { self.git_settings.error = Some(error); }
                ui.label("Credential helper (optional)");
                ui.add(egui::TextEdit::singleline(&mut self.git_settings.draft.credential_helper).hint_text("Use system Git helper").desired_width(f32::INFINITY));
            });
        }
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(!busy, egui::Button::new("Reload Git Settings"))
                .clicked()
            {
                self.git_settings.error = None;
                self.git_settings.notice.clear();
                self.git_settings.request = Some(self.submit(Task::GitSettings));
            }
            if ui
                .add_enabled(
                    !busy && self.git_settings.snapshot.is_some(),
                    egui::Button::new("Save Git Settings"),
                )
                .clicked()
            {
                self.save_git_settings(false);
            }
            if ui
                .add_enabled(
                    !busy && self.git_settings.snapshot.is_some(),
                    egui::Button::new("Test Fetch Connection"),
                )
                .clicked()
            {
                self.save_git_settings(true);
            }
        });
        if busy {
            ui.label("Working…");
        }
        if let Some(error) = &self.git_settings.error {
            ui.colored_label(crate::theme::ERROR, error);
        }
        if !self.git_settings.notice.is_empty() {
            ui.label(&self.git_settings.notice);
        }
    }
    fn save_git_settings(&mut self, test: bool) {
        let authentication = self.git_settings.auth.authentication();
        let mut settings = self.git_settings.draft.clone();
        settings.auth = authentication.profile();
        self.git_settings.error = None;
        self.git_settings.notice.clear();
        if test {
            self.git_settings.testing = Some(self.submit(Task::TestGitConnection {
                settings,
                authentication,
            }));
        } else if let Some(expected) = self.git_settings.snapshot.clone() {
            self.git_settings.saving = true;
            self.git_settings.request = Some(self.submit(Task::SaveGitSettings {
                expected,
                settings,
                authentication,
            }));
        }
    }
    pub(super) fn accept_git_settings(&mut self, id: u64, settings: GitSettings) {
        if self.git_settings.request != Some(id) {
            return;
        }
        self.git_settings.request = None;
        self.git_settings.auth = crate::git_auth::AuthFields::from_profile(&settings.auth);
        self.git_settings.draft = settings.clone();
        self.git_settings.snapshot = Some(settings);
        if self.git_settings.saving {
            self.error = None;
            self.git_settings.saving = false;
            self.git_settings.notice =
                "Git settings saved. Session credentials are available until this project closes."
                    .into();
            self.submit(Task::Refresh);
        }
    }
    pub(super) fn git_settings_result(&mut self, id: u64, error: Option<String>) {
        if self.git_settings.request == Some(id) || self.git_settings.testing == Some(id) {
            self.git_settings.request = None;
            self.git_settings.testing = None;
            self.git_settings.saving = false;
            self.git_settings.error = error;
            if self.git_settings.error.is_none() {
                self.git_settings.notice =
                    "Fetch connection successful (push permissions are checked when pushing)."
                        .into();
            }
        }
    }
    pub(crate) fn clear_git_secrets(&mut self) {
        self.git_settings.auth.clear_secrets();
    }
}
