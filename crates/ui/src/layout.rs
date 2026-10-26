//! Persistent dock proportions; disk operations run on one ordered worker.
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

/// Logical proportions of the application viewport, clamped for usable splitters.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Layout {
    /// Left dock width / viewport width.
    pub left: f32,
    /// Right dock width / viewport width.
    pub right: f32,
    /// Bottom dock height / viewport height.
    pub bottom: f32,
    /// SQL editor share of console height.
    pub sql: f32,
}
impl Default for Layout {
    fn default() -> Self {
        Self {
            left: 0.21,
            right: 0.22,
            bottom: 0.26,
            sql: 0.45,
        }
    }
}
impl Layout {
    fn valid(self) -> bool {
        [self.left, self.right, self.bottom, self.sql]
            .into_iter()
            .all(|v| v.is_finite() && (0.1..=0.8).contains(&v))
            && self.left + self.right <= 0.8
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
fn save(path: &Path, layout: Layout) -> Result<(), String> {
    use std::io::Write;
    load(path)?;
    if !layout.valid() {
        return Err("Invalid layout proportions".into());
    }
    let parent = path.parent().ok_or("Layout path has no parent")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    temp.write_all(&serde_json::to_vec_pretty(&layout).map_err(|e| e.to_string())?)
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
                            save(&path, layout)
                        } else {
                            Err("Malformed layout.json preserved".into())
                        };
                        let _sent = output.send(result.map(|()| None));
                        wake();
                    }
                    Task::Finish(layout, done) => {
                        if writable && let Err(e) = save(&path, layout) {
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
        if let Some(output) = &self.output {
            while let Ok(reply) = output.try_recv() {
                match reply {
                    Ok(Some(value)) if !self.touched => self.value = value,
                    Ok(_) => {}
                    Err(e) => self.error = Some(e),
                }
            }
        }
        if let Some(changed) = self.changed {
            let remaining = Duration::from_millis(250).saturating_sub(changed.elapsed());
            if remaining.is_zero() {
                self.changed = None;
                if let Some(worker) = &self.worker {
                    let _sent = worker.send(Task::Save(self.value));
                }
            } else {
                ctx.request_repaint_after(remaining);
            }
        }
    }
    pub fn set(&mut self, value: Layout) {
        if value.valid() && value != self.value {
            self.value = value;
            self.touched = true;
            self.changed = Some(Instant::now());
        }
    }
    pub fn finish(&mut self) {
        if let Some(worker) = self.worker.take() {
            let (done, wait) = mpsc::channel();
            if worker.send(Task::Finish(self.value, done)).is_ok() {
                let _finished = wait.recv_timeout(Duration::from_secs(1));
            }
        }
    }
}

pub fn seed(ctx: &egui::Context, id: &str, width: f32, height: f32) {
    if !ctx.input(|i| i.pointer.any_down()) {
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
        save(&path, value)?;
        assert_eq!(load(&path)?, value);
        std::fs::write(&path, "bad")?;
        assert!(save(&path, value).is_err());
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
}
