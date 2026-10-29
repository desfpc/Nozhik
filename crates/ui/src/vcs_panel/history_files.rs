//! Read-only file actions for commit history; opening uses the current working copy.
use super::{Task, VcsPanel};
use nozhik_vcs::model::HistoryFile;
impl VcsPanel {
    pub(super) fn history_file_menu(&mut self, ui: &mut egui::Ui, file: &HistoryFile) {
        let path = &file.change.path;
        let root = self.repo.as_ref().map(|repo| repo.root.clone());
        let local = root.is_some() && file.worktree_exists;
        if ui
            .add_enabled(local, egui::Button::new("Reveal in File Manager"))
            .clicked()
        {
            self.submit(Task::OpenExternal {
                paths: vec![path.clone()],
                reveal: true,
            });
            ui.close();
        }
        if ui
            .add_enabled(local, egui::Button::new("Open in Nozhik"))
            .clicked()
        {
            if let Some(root) = &root {
                self.open_paths.push(root.join(path));
            }
            ui.close();
        }
        if ui
            .add_enabled(local, egui::Button::new("Open with Default Program"))
            .clicked()
        {
            self.submit(Task::OpenExternal {
                paths: vec![path.clone()],
                reveal: false,
            });
            ui.close();
        }
        ui.separator();
        if ui
            .add_enabled(root.is_some(), egui::Button::new("Copy File Path"))
            .clicked()
        {
            if let Some(root) = &root {
                ui.ctx().copy_text(root.join(path).display().to_string());
            }
            ui.close();
        }
        if ui.button("Copy Relative File Path").clicked() {
            ui.ctx().copy_text(path.display().to_string());
            ui.close();
        }
        ui.separator();
        if ui
            .add_enabled(
                file.github_url.is_some(),
                egui::Button::new("View on GitHub"),
            )
            .clicked()
        {
            if let Some(url) = &file.github_url {
                ui.ctx().open_url(egui::OpenUrl::new_tab(url));
            }
            ui.close();
        }
    }
}
