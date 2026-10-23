//! Cross-platform notifications, filtered and debounced off the UI thread.
use crate::tree::service_path;
use notify::{RecursiveMode, Watcher};
use nozhik_event_bus::{Event, EventBus, FileEvent};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

/// Owns the OS watcher and stops its debounce worker on drop without blocking UI.
pub struct ProjectWatcher {
    _watcher: notify::RecommendedWatcher,
    stop: Arc<AtomicBool>,
}
impl ProjectWatcher {
    /// Starts recursive observation; publishes changed paths after 150 ms quiet time.
    /// # Errors
    /// Returns OS watcher setup failures.
    pub fn start(root: PathBuf, bus: EventBus) -> notify::Result<Self> {
        Self::start_with_wakeup(root, bus, Arc::new(crate::Wakeup::default()))
    }
    pub(crate) fn start_with_wakeup(
        root: PathBuf,
        bus: EventBus,
        wakeup: Arc<crate::Wakeup>,
    ) -> notify::Result<Self> {
        let (tx, rx) = mpsc::channel();
        let mut watcher = notify::recommended_watcher(tx)?;
        watcher.watch(&root, RecursiveMode::Recursive)?;
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        thread::spawn(move || {
            let mut pending = HashMap::new();
            while !stopping.load(Ordering::Relaxed) {
                match rx.recv_timeout(Duration::from_millis(25)) {
                    Ok(Ok(event)) if !matches!(event.kind, notify::EventKind::Access(_)) => {
                        for path in event.paths {
                            if visible(&root, &path) {
                                pending.insert(path, Instant::now());
                            }
                        }
                    }
                    Ok(Err(error)) => {
                        bus.publish(Event::File(FileEvent::Error(error.to_string())));
                        wakeup.wake();
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    _ => {}
                }
                pending.retain(|path, last| {
                    if last.elapsed() < Duration::from_millis(150) {
                        return true;
                    }
                    bus.publish(Event::File(FileEvent::Changed(path.clone())));
                    wakeup.wake();
                    false
                });
            }
        });
        Ok(Self {
            _watcher: watcher,
            stop,
        })
    }
}
impl Drop for ProjectWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
fn visible(root: &Path, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(root) else {
        return false;
    };
    if service_path(relative) {
        return false;
    }
    if matches!(
        path.file_name().and_then(|s| s.to_str()),
        Some(".gitignore" | ".ignore")
    ) {
        return true;
    }
    // Rebuild to observe edits to ignore files, including nested and parent rules.
    crate::tree::walker(root, None)
        .build_matchers()
        .into_iter()
        .next()
        .is_none_or(|mut matcher| !matcher.matched(relative, path.is_dir()).is_ignore())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn actual_watcher_publishes_debounced_changes() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let root = dir.path().canonicalize()?;
        let path = root.join("observed.rs");
        std::fs::write(&path, "before")?;
        let bus = EventBus::new(64)?;
        let mut receiver = bus.subscribe();
        let _watcher = ProjectWatcher::start(root, bus)?;
        std::fs::write(&path, "one")?;
        std::fs::write(&path, "two")?;
        let runtime = tokio_for_test()?;
        runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    let event = receiver.recv().await?;
                    if matches!(event.as_ref(), Event::File(FileEvent::Changed(changed)) if changed == &path) {
                        return Ok::<_, tokio::sync::broadcast::error::RecvError>(());
                    }
                }
            }).await
        })??;
        Ok(())
    }
    fn tokio_for_test() -> std::io::Result<tokio::runtime::Runtime> {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
    }
    #[test]
    fn filters_deleted_paths_and_service_dirs() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join(".gitignore"), "*.log\n")?;
        assert!(!visible(dir.path(), &dir.path().join("deleted.log")));
        assert!(!visible(dir.path(), &dir.path().join("target/a.rs")));
        assert!(visible(dir.path(), &dir.path().join("src/main.rs")));
        Ok(())
    }
}
