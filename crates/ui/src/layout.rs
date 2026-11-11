//! Persistent dock proportions; disk operations run on one ordered worker.
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

/// Logical proportions of the application viewport, clamped for usable splitters.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Layout {
    /// Left dock width / viewport width.
    pub left: f32,
    /// Git sidebar width / viewport width, independent of the project dock.
    pub git: f32,
    /// Legacy right dock proportion retained for existing layout.json compatibility.
    pub right: f32,
    /// Bottom dock height / viewport height.
    pub bottom: f32,
    /// SQL editor share of console height.
    pub sql: f32,
    /// Snippets tag sidebar width / viewport width, independent of the project dock.
    pub snippets: f32,
    /// HTTP Requests tag sidebar width / viewport width, independent of the project dock.
    #[serde(default = "default_http_requests")]
    pub http_requests: f32,
    /// Preferred modal content sizes in logical pixels; scoped to this project.
    pub dialogs: crate::dialog::Sizes,
}

const fn default_http_requests() -> f32 {
    0.18
}
impl Default for Layout {
    fn default() -> Self {
        Self {
            left: 0.21,
            git: 0.27,
            right: 0.22,
            bottom: 0.40,
            sql: 0.45,
            snippets: 0.18,
            http_requests: default_http_requests(),
            dialogs: crate::dialog::Sizes::default(),
        }
    }
}
impl Layout {
    fn valid(&self) -> bool {
        [
            self.left,
            self.git,
            self.right,
            self.sql,
            self.snippets,
            self.http_requests,
        ]
        .into_iter()
        .all(|v| v.is_finite() && (0.1..=0.8).contains(&v))
            && self.bottom.is_finite()
            && (0.05..=1.0).contains(&self.bottom)
            && self.left + self.right <= 0.8
            && self.dialogs.iter().all(|(key, size)| {
                !key.is_empty() && size.iter().all(|v| v.is_finite() && *v > 0.0)
            })
    }
}
fn load(path: &Path) -> Result<Layout, String> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Layout::default()),
        Err(e) => return Err(e.to_string()),
    };
    let layout: Layout = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if layout.valid() {
        Ok(layout)
    } else {
        Err("Invalid layout proportions".into())
    }
}
fn save(path: &Path, layout: &Layout) -> Result<(), String> {
    use std::io::Write;
    load(path)?;
    if !layout.valid() {
        return Err("Invalid layout proportions".into());
    }
    let parent = path.parent().ok_or("Layout path has no parent")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    temp.write_all(&serde_json::to_vec_pretty(layout).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    temp.as_file().sync_all().map_err(|e| e.to_string())?;
    temp.persist(path).map_err(|e| e.to_string())?;
    Ok(())
}
enum Task {
    Save(Layout),
    Finish(Layout, mpsc::Sender<()>),
}
#[derive(Default)]
pub struct LayoutManager {
    pub value: Layout,
    dialogs: crate::dialog::SharedSizes,
    worker: Option<mpsc::Sender<Task>>,
    output: Option<mpsc::Receiver<Result<Option<Layout>, String>>>,
    changed: Option<Instant>,
    touched: bool,
    pub error: Option<String>,
}
impl LayoutManager {
    pub fn start(&mut self, path: PathBuf, wake: Arc<dyn Fn() + Send + Sync>) {
        let (sender, input) = mpsc::channel();
        let (output, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let loaded = load(&path);
            let writable = loaded.is_ok();
            let _sent = output.send(loaded.map(Some));
            wake();
            while let Ok(task) = input.recv() {
                match task {
                    Task::Save(layout) => {
                        let result = if writable {
                            save(&path, &layout)
                        } else {
                            Err("Malformed layout.json preserved".into())
                        };
                        let _sent = output.send(result.map(|()| None));
                        wake();
                    }
                    Task::Finish(layout, done) => {
                        if writable && let Err(e) = save(&path, &layout) {
                            eprintln!("Layout save: {e}");
                        }
                        let _sent = done.send(());
                        break;
                    }
                }
            }
        });
        self.worker = Some(sender);
        self.output = Some(receiver);
    }
    pub fn tick(&mut self, ctx: &egui::Context) {
        self.collect_dialogs();
        if let Some(output) = &self.output {
            while let Ok(reply) = output.try_recv() {
                match reply {
                    Ok(Some(value)) if !self.touched => {
                        crate::dialog::replace(&self.dialogs, value.dialogs.clone());
                        self.value = value;
                    }
                    Ok(_) => {}
                    Err(e) => self.error = Some(e),
                }
            }
        }
        crate::dialog::bind(ctx, self.dialogs.clone());
        if let Some(changed) = self.changed {
            let remaining = Duration::from_millis(250).saturating_sub(changed.elapsed());
            if remaining.is_zero() {
                self.changed = None;
                if let Some(worker) = &self.worker {
                    let _sent = worker.send(Task::Save(self.value.clone()));
                }
            } else {
                ctx.request_repaint_after(remaining);
            }
        }
    }
    pub fn set(&mut self, mut value: Layout) {
        if value.dialogs == self.value.dialogs {
            value.dialogs = crate::dialog::snapshot(&self.dialogs);
        }
        if value.valid() && value != self.value {
            crate::dialog::replace(&self.dialogs, value.dialogs.clone());
            self.value = value;
            self.touched = true;
            self.changed = Some(Instant::now());
        }
    }
    fn collect_dialogs(&mut self) {
        let dialogs = crate::dialog::snapshot(&self.dialogs);
        if dialogs != self.value.dialogs {
            self.value.dialogs = dialogs;
            self.touched = true;
            self.changed = Some(Instant::now());
        }
    }
    pub fn finish(&mut self) {
        self.collect_dialogs();
        if let Some(worker) = self.worker.take() {
            let (done, wait) = mpsc::channel();
            if worker.send(Task::Finish(self.value.clone(), done)).is_ok() {
                let _finished = wait.recv_timeout(Duration::from_secs(1));
            }
        }
    }
}

pub fn seed(ctx: &egui::Context, id: &str, width: f32, height: f32) {
    if !ctx.input(|i| i.pointer.any_down() || i.pointer.any_released()) {
        ctx.data_mut(|d| {
            d.insert_persisted(
                egui::Id::new(id),
                egui::containers::panel::PanelState {
                    rect: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, height)),
                },
            );
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn layout_roundtrip_and_invalid_file_preservation() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("layout.json");
        let value = Layout {
            left: 0.25,
            ..Layout::default()
        };
        save(&path, &value)?;
        assert_eq!(load(&path)?, value);
        std::fs::write(
            &path,
            r#"{"left":0.21,"git":0.27,"right":0.22,"bottom":0.4,"sql":0.45,"snippets":0.18}"#,
        )?;
        let older = load(&path)?;
        assert!((older.http_requests - 0.18).abs() < f32::EPSILON);
        std::fs::write(&path, "bad")?;
        assert!(save(&path, &value).is_err());
        assert_eq!(std::fs::read_to_string(&path)?, "bad");
        assert!(
            !Layout {
                left: f32::NAN,
                ..value
            }
            .valid()
        );
        Ok(())
    }
    #[test]
    fn dialog_sizes_survive_restart_and_project_switch_and_preserve_invalid_files()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("layout.json");
        let ctx = egui::Context::default();
        let mut manager = LayoutManager::default();
        manager.start(path.clone(), Arc::new(|| {}));
        manager.tick(&ctx);
        let sizes = crate::dialog::Sizes::from([
            ("new_database".into(), [820.0, 720.0]),
            ("settings".into(), [1200.0, 800.0]),
        ]);
        crate::dialog::replace(&manager.dialogs, sizes.clone());
        let mut split = manager.value.clone();
        split.sql = 0.6;
        manager.set(split);
        assert_eq!(crate::dialog::snapshot(&manager.dialogs), sizes);
        // Finish collects the last drag even when no further UI frame ran.
        manager.finish();
        assert_eq!(load(&path)?.dialogs, sizes);
        let mut restored = LayoutManager::default();
        restored.start(path.clone(), Arc::new(|| {}));
        let deadline = Instant::now() + Duration::from_secs(2);
        while restored.value.dialogs.is_empty() && Instant::now() < deadline {
            restored.tick(&ctx);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(restored.value.dialogs, sizes);
        let mut other = LayoutManager::default();
        other.tick(&ctx);
        crate::dialog::replace(
            &other.dialogs,
            crate::dialog::Sizes::from([("new_database".into(), [450.0, 400.0])]),
        );
        other.tick(&ctx);
        restored.tick(&ctx);
        assert_eq!(restored.value.dialogs, sizes);
        assert_ne!(other.value.dialogs, sizes);
        restored.finish();
        std::fs::write(&path, r#"{"dialogs":{"new_database":[-1,300]}}"#)?;
        assert!(save(&path, &Layout::default()).is_err());
        assert_eq!(
            std::fs::read_to_string(&path)?,
            r#"{"dialogs":{"new_database":[-1,300]}}"#
        );
        Ok(())
    }
}
