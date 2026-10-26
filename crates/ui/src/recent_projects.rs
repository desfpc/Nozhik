//! Persistent project MRU; one ordered worker owns all disk operations.
use serde::{Deserialize, Serialize};
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock, mpsc},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecentProject {
    pub path: PathBuf,
    /// Unix milliseconds; entries are sorted newest first.
    pub last_opened: u64,
}
fn load(path: &Path) -> Result<Vec<RecentProject>, String> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.to_string()),
    };
    let mut entries: Vec<RecentProject> =
        serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if entries.iter().any(|entry| !entry.path.is_absolute()) {
        return Err("Recent project paths must be absolute".into());
    }
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.last_opened));
    let mut seen = std::collections::HashSet::new();
    entries.retain(|entry| seen.insert(entry.path.clone()));
    Ok(entries)
}
fn record(entries: &mut Vec<RecentProject>, path: PathBuf, now: u64) {
    let last_opened = entries
        .first()
        .map_or(now, |entry| now.max(entry.last_opened.saturating_add(1)));
    entries.retain(|entry| entry.path != path);
    entries.insert(0, RecentProject { path, last_opened });
}
fn save(path: &Path, entries: &[RecentProject]) -> Result<(), String> {
    // Revalidate before replacement; never overwrite a malformed user file.
    load(path)?;
    let parent = path.parent().ok_or("History path has no parent")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    temp.write_all(&serde_json::to_vec_pretty(entries).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    temp.as_file().sync_all().map_err(|e| e.to_string())?;
    temp.persist(path).map_err(|e| e.to_string())?;
    Ok(())
}
enum Task {
    Opened(PathBuf),
    Finish(mpsc::Sender<()>),
}
type Update = (Vec<RecentProject>, Option<String>);
pub struct RecentProjects {
    pub entries: Vec<RecentProject>,
    pub error: Option<String>,
    commands: mpsc::Sender<Task>,
    updates: mpsc::Receiver<Update>,
    wake: Arc<OnceLock<egui::Context>>,
}
impl RecentProjects {
    pub fn start(path: PathBuf) -> Self {
        let (commands, tasks) = mpsc::channel();
        let (output, updates) = mpsc::channel();
        let wake = Arc::new(OnceLock::<egui::Context>::new());
        let notification = wake.clone();
        std::thread::spawn(move || {
            let loaded = load(&path);
            let mut error = loaded.as_ref().err().cloned();
            let mut entries = loaded.unwrap_or_default();
            let send = |entries: &Vec<RecentProject>, error: &Option<String>| {
                let _sent = output.send((entries.clone(), error.clone()));
                if let Some(ctx) = notification.get() {
                    ctx.request_repaint();
                }
            };
            send(&entries, &error);
            while let Ok(task) = tasks.recv() {
                match task {
                    Task::Opened(project) => {
                        let now = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis();
                        record(
                            &mut entries,
                            project,
                            u64::try_from(now).unwrap_or(u64::MAX),
                        );
                        if error.is_none() {
                            error = save(&path, &entries).err();
                        }
                        send(&entries, &error);
                    }
                    Task::Finish(done) => {
                        let _sent = done.send(());
                        break;
                    }
                }
            }
        });
        Self {
            entries: Vec::new(),
            error: None,
            commands,
            updates,
            wake,
        }
    }
    pub fn note(&self, path: PathBuf) {
        let _sent = self.commands.send(Task::Opened(path));
    }
    pub fn tick(&mut self, ctx: &egui::Context) {
        self.wake.get_or_init(|| ctx.clone());
        for (entries, error) in self.updates.try_iter() {
            self.entries = entries;
            self.error = error;
        }
    }
    pub fn list(&self, ui: &mut egui::Ui) -> Option<PathBuf> {
        let mut selected = None;
        if let Some(error) = &self.error {
            ui.colored_label(crate::theme::ERROR, format!("Project history: {error}"));
        }
        if self.entries.is_empty() {
            ui.weak("No recent projects");
        }
        for entry in &self.entries {
            let name = entry
                .path
                .file_name()
                .unwrap_or(entry.path.as_os_str())
                .to_string_lossy();
            if ui
                .button(name)
                .on_hover_text(entry.path.display().to_string())
                .clicked()
            {
                selected = Some(entry.path.clone());
            }
            ui.small(entry.path.display().to_string());
        }
        selected
    }
    pub fn finish(&self) {
        let (done, wait) = mpsc::channel();
        if self.commands.send(Task::Finish(done)).is_ok() {
            let _finished = wait.recv_timeout(Duration::from_secs(1));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn history_roundtrip_reorder_deduplicate_and_preserve_invalid()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("recent_projects.json");
        let a = dir.path().join("Первый проект");
        let b = dir.path().join("second");
        let mut entries = load(&path)?;
        record(&mut entries, a.clone(), 100);
        record(&mut entries, b.clone(), 200);
        record(&mut entries, a.clone(), 150); // Clock went backwards; opening order still wins.
        save(&path, &entries)?;
        let loaded = load(&path)?;
        assert_eq!(loaded, entries);
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].path, a);
        assert_eq!(loaded[1].path, b);
        assert!(loaded[0].last_opened > loaded[1].last_opened);
        std::fs::write(&path, "broken")?;
        assert!(save(&path, &entries).is_err());
        assert_eq!(std::fs::read_to_string(&path)?, "broken");
        Ok(())
    }
}
