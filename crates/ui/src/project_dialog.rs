//! Native pickers. `AppKit` supports selecting either a file or directory in one panel.
use std::path::{Path, PathBuf};

#[derive(Clone, Copy)]
pub enum Picker {
    Open,
    Folder,
    File,
}

#[cfg(target_os = "macos")]
pub fn pick(kind: Picker, directory: Option<&Path>) -> Result<Option<PathBuf>, String> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSModalResponseCancel, NSModalResponseOK, NSOpenPanel};
    use objc2_foundation::{NSString, NSURL};
    let mtm = MainThreadMarker::new().ok_or("Native picker requires the main thread")?;
    let panel = NSOpenPanel::openPanel(mtm);
    panel.setCanChooseDirectories(!matches!(kind, Picker::File));
    panel.setCanChooseFiles(!matches!(kind, Picker::Folder));
    panel.setAllowsMultipleSelection(false);
    panel.setTitle(Some(&NSString::from_str(match kind {
        Picker::Open => "Open File or Folder",
        Picker::Folder => "Choose Project Directory",
        Picker::File => "Open File",
    })));
    if let Some(url) = directory.and_then(NSURL::from_file_path) {
        panel.setDirectoryURL(Some(&url));
    }
    match panel.runModal() {
        response if response == NSModalResponseOK => panel
            .URL()
            .and_then(|url| url.to_file_path())
            .map(Some)
            .ok_or_else(|| "Native picker returned no local path".into()),
        response if response == NSModalResponseCancel => Ok(None),
        response => Err(format!("Native picker failed ({response})")),
    }
}

#[cfg(not(target_os = "macos"))]
#[allow(
    clippy::unnecessary_wraps,
    reason = "Shared picker API: AppKit reports main-thread and panel errors"
)]
pub fn pick(kind: Picker, directory: Option<&Path>) -> Result<Option<PathBuf>, String> {
    let mut dialog = rfd::FileDialog::new();
    if let Some(directory) = directory {
        dialog = dialog.set_directory(directory);
    }
    // Windows and XDG portals expose separate file/folder modes. File has its own menu entry.
    Ok(if matches!(kind, Picker::File) {
        dialog.pick_file()
    } else {
        dialog.pick_folder()
    })
}

impl crate::NozhikUi {
    pub(crate) fn choose_project_folder(&mut self, ctx: &egui::Context) {
        self.pick_path(Picker::Folder, ctx);
    }
    pub(crate) fn pick_path(&mut self, kind: Picker, ctx: &egui::Context) {
        match pick(kind, self.project.as_ref().map(|p| p.root_path.as_path())) {
            Ok(Some(path)) => self.queue(nozhik_fs_model::Command::OpenPath(path)),
            Ok(None) => {}
            Err(e) => self.status = e,
        }
        ctx.request_repaint();
    }
}
