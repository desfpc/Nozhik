//! Project entry mutations and OS reveal, executed exclusively by the filesystem worker.
use crate::{Command, FsError, Reply, Worker};
use std::path::Path;

impl Worker {
    pub(super) fn create_entry(
        &mut self,
        parent: &Path,
        name: &str,
        directory: bool,
    ) -> Result<Vec<Reply>, FsError> {
        crate::validate_project_name(name).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Enter a single name without separators or reserved characters",
            )
        })?;
        let parent = parent.canonicalize()?;
        let tree = self.tree.as_ref().ok_or(FsError::OutsideProject)?;
        if !parent.starts_with(&tree.root_path) {
            return Err(FsError::OutsideProject);
        }
        let path = parent.join(name);
        if directory {
            std::fs::create_dir(&path)?;
        } else {
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)?;
        }
        // Report success before refreshing: a refresh failure must never invite a duplicate create.
        let mut replies = vec![Reply::EntryCreated { path, directory }];
        match self.execute(Command::LoadDir(parent)) {
            Ok(loaded) => replies.extend(loaded),
            Err(error) => replies.push(Reply::Error(error.to_string())),
        }
        Ok(replies)
    }
}

pub fn show_in_file_manager(path: &Path) -> Result<(), FsError> {
    // Keep the entry identity for selection, and verify that it still exists.
    std::fs::symlink_metadata(path)?;
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = std::process::Command::new("open");
        command.arg("-R").arg("--").arg(path);
        command
    };
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = std::process::Command::new("explorer.exe");
        command.arg(format!("/select,{}", path.display()));
        command
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut command = {
        let mut command = std::process::Command::new("xdg-open");
        command.arg(path.parent().unwrap_or(path));
        command
    };
    if !command.status()?.success() {
        return Err(std::io::Error::other("Could not open the system file manager").into());
    }
    Ok(())
}
