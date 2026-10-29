//! Single-owner background project indexing, disk cache and buffer overlays.
use crate::symbol_index::{FileSymbols, LanguageId, SymbolIndex};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};

/// Commands never parse or perform I/O on the caller thread.
pub enum IndexCommand {
    /// Reconcile a changed path (including directories and deleted paths).
    Changed(PathBuf),
    /// Index the latest immutable buffer snapshot, without persisting unsaved text.
    Overlay {
        /// Source path.
        path: PathBuf,
        /// Buffer revision.
        revision: u64,
        /// Immutable source.
        text: ropey::Rope,
    },
    /// Restore disk facts when a document closes.
    Close(PathBuf),
    /// Delete the cache directory and rebuild from disk in the worker.
    Invalidate,
    /// Reconcile metadata after dropped watcher events; unchanged files are never parsed.
    Reconcile,
}
/// Worker notifications, scoped to this indexer's project lifetime.
pub enum IndexEvent {
    /// Shared immutable live index plus overlay revision gates.
    Snapshot(Arc<SymbolIndex>, HashMap<PathBuf, u64>),
    /// Completed and total files; zero total denotes discovery.
    Progress(usize, usize),
    /// Traversal and cache persistence completed, including projects with no PHP files.
    Finished,
    /// Actionable I/O/parser/cache error.
    Error(String),
}
/// Nonblocking worker handle; dropping cancels traversal and pending work.
pub struct ProjectIndexer {
    sender: mpsc::Sender<IndexCommand>,
    events: mpsc::Receiver<IndexEvent>,
    stop: Arc<AtomicBool>,
}
impl ProjectIndexer {
    /// Loads the cache and reconciles file metadata on a dedicated thread.
    #[must_use]
    pub fn start(root: PathBuf, wake: impl Fn() + Send + 'static) -> Self {
        Self::with_languages(root, nozhik_config::languages::default_enabled(), wake)
    }
    /// Starts only the implemented language partitions selected in settings.
    #[must_use]
    pub fn with_languages(
        root: PathBuf,
        enabled_languages: BTreeSet<LanguageId>,
        wake: impl Fn() + Send + 'static,
    ) -> Self {
        let (sender, commands) = mpsc::channel();
        let (output, events) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        std::thread::spawn(move || {
            let notify = |event| {
                let _sent = output.send(event);
                wake();
            };
            let mut worker = Worker::new(root);
            worker.enabled_languages = enabled_languages;
            if !worker.php_enabled() {
                notify(worker.snapshot());
                notify(IndexEvent::Finished);
                return;
            }
            match worker.load() {
                Ok(()) => notify(worker.snapshot()),
                Err(e) => notify(IndexEvent::Error(e.to_string())),
            }
            if let Err(e) = worker.reconcile(&stopping, &notify) {
                notify(IndexEvent::Error(e.to_string()));
            }
            if stopping.load(Ordering::Relaxed) {
                return;
            }
            notify(worker.snapshot());
            while !stopping.load(Ordering::Relaxed) {
                let Ok(command) = commands.recv() else {
                    break;
                };
                match worker.command(command, &stopping, &notify) {
                    Ok(true) => notify(worker.snapshot()),
                    Ok(false) => {}
                    Err(e) => notify(IndexEvent::Error(e.to_string())),
                }
            }
        });
        Self {
            sender,
            events,
            stop,
        }
    }
    /// Reliable queue while the worker is alive; callers retain/retry on false.
    #[must_use]
    pub fn send(&self, command: IndexCommand) -> bool {
        self.sender.send(command).is_ok()
    }
    /// Bounded, nonblocking notification drain.
    pub fn drain(&self) -> impl Iterator<Item = IndexEvent> + '_ {
        self.events.try_iter().take(128)
    }
}
impl Drop for ProjectIndexer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct Stamp {
    seconds: u64,
    nanos: u32,
    length: u64,
}
fn stamp(path: &Path) -> std::io::Result<Stamp> {
    let meta = path.metadata()?;
    let modified = meta
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(std::io::Error::other)?;
    Ok(Stamp {
        seconds: modified.as_secs(),
        nanos: modified.subsec_nanos(),
        length: meta.len(),
    })
}
#[derive(Serialize, Deserialize)]
struct Cache {
    version: u32,
    root: PathBuf,
    stamps: HashMap<PathBuf, Stamp>,
    index: SymbolIndex,
}
#[derive(Serialize)]
struct CacheRef<'a> {
    version: u32,
    root: &'a Path,
    stamps: &'a HashMap<PathBuf, Stamp>,
    index: &'a SymbolIndex,
}
struct Worker {
    root: PathBuf,
    enabled_languages: BTreeSet<LanguageId>,
    disk: SymbolIndex,
    live: SymbolIndex,
    stamps: HashMap<PathBuf, Stamp>,
    overlays: HashMap<PathBuf, (u64, FileSymbols)>,
    parsed: usize,
}
impl Worker {
    fn new(root: PathBuf) -> Self {
        Self {
            root,
            enabled_languages: nozhik_config::languages::default_enabled(),
            disk: SymbolIndex::new(LanguageId::Php),
            live: SymbolIndex::new(LanguageId::Php),
            stamps: HashMap::new(),
            overlays: HashMap::new(),
            parsed: 0,
        }
    }
    fn php_enabled(&self) -> bool {
        nozhik_config::languages::enabled(&self.enabled_languages, LanguageId::Php)
    }
    fn supports_path(&self, path: &Path) -> bool {
        self.php_enabled()
            && nozhik_config::languages::language_for_path(path) == Some(LanguageId::Php)
    }
    fn eligible(&self, path: &Path) -> bool {
        path.strip_prefix(&self.root)
            .is_ok_and(|relative| !relative.components().any(|c| excluded(c.as_os_str())))
    }
    fn cache_dir(&self) -> PathBuf {
        self.root.join(".nozhik/cache")
    }
    fn load(&mut self) -> std::io::Result<()> {
        if !self.php_enabled() {
            return Ok(());
        }
        let file = match std::fs::File::open(self.cache_dir().join("index_php.json")) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };
        let cache: Cache = serde_json::from_reader(std::io::BufReader::new(file))?;
        if cache.version != 3 {
            return Ok(());
        }
        if cache.root != self.root || cache.index.language != LanguageId::Php {
            return Err(std::io::Error::other(
                "Incompatible project index cache; use Invalidate Caches",
            ));
        }
        self.disk = cache.index;
        self.live = self.disk.clone();
        self.stamps = cache.stamps;
        Ok(())
    }
    fn persist(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(self.cache_dir())?;
        let mut file = tempfile::NamedTempFile::new_in(self.cache_dir())?;
        serde_json::to_writer(
            std::io::BufWriter::new(file.as_file_mut()),
            &CacheRef {
                version: 3,
                root: &self.root,
                stamps: &self.stamps,
                index: &self.disk,
            },
        )?;
        file.persist(self.cache_dir().join("index_php.json"))
            .map_err(|e| e.error)?;
        Ok(())
    }
    fn snapshot(&self) -> IndexEvent {
        IndexEvent::Snapshot(
            Arc::new(self.live.clone()),
            self.overlays
                .iter()
                .map(|(p, (r, _))| (p.clone(), *r))
                .collect(),
        )
    }
    fn remove(&mut self, path: &Path) -> bool {
        if self.stamps.remove(path).is_none() {
            return false;
        }
        self.disk.remove(path);
        if !self.overlays.contains_key(path) {
            self.live.remove(path);
        }
        true
    }
    fn read_update(&mut self, path: &Path) -> std::io::Result<Option<(FileSymbols, Stamp)>> {
        if !self.supports_path(path) {
            return Ok(None);
        }
        let before = stamp(path)?;
        if self.stamps.get(path) == Some(&before) {
            return Ok(None);
        }
        let source = std::fs::read_to_string(path)?;
        let facts = crate::php_symbols::parse(path, &source).map_err(std::io::Error::other)?;
        self.parsed += 1;
        if stamp(path)? != before {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "File changed while indexing; retry on the next watcher update",
            ));
        }
        Ok(Some((facts, before)))
    }
    fn apply_updates(&mut self, updates: Vec<(PathBuf, FileSymbols, Stamp)>) -> bool {
        if updates.is_empty() {
            return false;
        }
        let live = updates
            .iter()
            .filter(|(p, _, _)| !self.overlays.contains_key(p))
            .map(|(p, facts, _)| (p.clone(), facts.clone()))
            .collect();
        let disk = updates
            .into_iter()
            .map(|(path, facts, stamp)| {
                self.stamps.insert(path.clone(), stamp);
                (path, facts)
            })
            .collect();
        self.disk.replace_files(disk);
        self.live.replace_files(live);
        true
    }
    fn update(&mut self, path: &Path) -> std::io::Result<bool> {
        if !self.supports_path(path) {
            return Ok(false);
        }
        if !path.is_file() {
            return Ok(self.remove(path));
        }
        let Some((facts, stamp)) = self.read_update(path)? else {
            return Ok(false);
        };
        Ok(self.apply_updates(vec![(path.into(), facts, stamp)]))
    }
    fn paths(&self, root: &Path, stop: &AtomicBool) -> std::io::Result<Vec<PathBuf>> {
        let mut paths = Vec::new();
        // Git membership is a display concern. Every supported source participates,
        // including ignored dependencies; only application/Git internals are skipped.
        let mut walk = ignore::WalkBuilder::new(root);
        walk.hidden(false)
            .follow_links(false)
            .parents(false)
            .ignore(false)
            .git_ignore(false)
            .git_global(false)
            .git_exclude(false)
            .filter_entry(|entry| !excluded(entry.file_name()) || entry.depth() == 0);
        for entry in walk.build().take_while(|_| !stop.load(Ordering::Relaxed)) {
            let entry = entry.map_err(std::io::Error::other)?;
            if entry.file_type().is_some_and(|t| t.is_file()) && self.supports_path(entry.path()) {
                paths.push(entry.into_path());
            }
        }
        paths.sort();
        Ok(paths)
    }
    fn reconcile(
        &mut self,
        stop: &AtomicBool,
        notify: &impl Fn(IndexEvent),
    ) -> std::io::Result<bool> {
        if !self.php_enabled() {
            notify(IndexEvent::Finished);
            return Ok(false);
        }
        notify(IndexEvent::Progress(0, 0));
        let paths = self.paths(&self.root, stop)?;
        if stop.load(Ordering::Relaxed) {
            return Ok(false);
        }
        let known: HashSet<_> = paths.iter().cloned().collect();
        let removed: Vec<_> = self
            .stamps
            .keys()
            .filter(|p| !known.contains(*p))
            .cloned()
            .collect();
        let mut changed = false;
        for path in removed {
            changed |= self.remove(&path);
        }
        let mut updates = Vec::new();
        for (i, path) in paths.iter().enumerate() {
            if stop.load(Ordering::Relaxed) {
                return Ok(false);
            }
            match self.read_update(path) {
                Ok(Some((facts, stamp))) => updates.push((path.clone(), facts, stamp)),
                Ok(None) => {}
                Err(e) => notify(IndexEvent::Error(format!("{}: {e}", path.display()))),
            }
            if i % 20 == 0 {
                notify(IndexEvent::Progress(i, paths.len()));
            }
        }
        if stop.load(Ordering::Relaxed) {
            return Ok(false);
        }
        changed |= self.apply_updates(updates);
        if stop.load(Ordering::Relaxed) {
            return Ok(false);
        }
        if changed {
            self.persist()?;
        }
        notify(IndexEvent::Progress(paths.len(), paths.len()));
        notify(IndexEvent::Finished);
        Ok(changed)
    }
    fn invalidate(
        &mut self,
        stop: &AtomicBool,
        notify: &impl Fn(IndexEvent),
    ) -> std::io::Result<bool> {
        match std::fs::remove_dir_all(self.cache_dir()) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        self.disk = SymbolIndex::new(LanguageId::Php);
        self.live = self.disk.clone();
        self.stamps.clear();
        // Revision gates must only advertise facts present in this snapshot.
        self.live.replace_files(
            self.overlays
                .iter()
                .map(|(path, (_, facts))| (path.clone(), facts.clone()))
                .collect(),
        );
        notify(self.snapshot());
        self.reconcile(stop, notify)?;
        Ok(true)
    }
    fn command(
        &mut self,
        command: IndexCommand,
        stop: &AtomicBool,
        notify: &impl Fn(IndexEvent),
    ) -> std::io::Result<bool> {
        if !self.php_enabled() {
            return Ok(false);
        }
        match command {
            IndexCommand::Overlay {
                path,
                revision,
                text,
            } => {
                if !self.eligible(&path)
                    || !self.supports_path(&path)
                    || self
                        .overlays
                        .get(&path)
                        .is_some_and(|(r, _)| *r == revision)
                {
                    return Ok(false);
                }
                let facts = crate::php_symbols::parse(&path, &text.to_string())
                    .map_err(std::io::Error::other)?;
                self.parsed += 1;
                self.live.replace(path.clone(), facts.clone());
                self.overlays.insert(path, (revision, facts));
                Ok(true)
            }
            IndexCommand::Close(path) => {
                if self.overlays.remove(&path).is_none() {
                    return Ok(false);
                }
                self.update(&path)?;
                if let Some(facts) = self.disk.file_symbols(&path).cloned() {
                    self.live.replace(path, facts);
                } else {
                    self.live.remove(&path);
                }
                Ok(true)
            }
            IndexCommand::Changed(path) => {
                if !self.eligible(&path) {
                    return Ok(false);
                }
                let mut changed = false;
                if path.is_dir() {
                    let mut updates = Vec::new();
                    for file in self.paths(&path, stop)? {
                        if stop.load(Ordering::Relaxed) {
                            return Ok(false);
                        }
                        if let Some((facts, stamp)) = self.read_update(&file)? {
                            updates.push((file, facts, stamp));
                        }
                    }
                    changed |= self.apply_updates(updates);
                } else {
                    if !self.supports_path(&path) && path.is_file() {
                        return Ok(false);
                    }
                    // Only a removed directory can invalidate descendants. Ordinary file
                    // saves, including unsupported types, must not scan every indexed path.
                    let removed: Vec<_> = if path.exists() {
                        Vec::new()
                    } else {
                        self.stamps
                            .keys()
                            .filter(|p| *p != &path && p.starts_with(&path))
                            .cloned()
                            .collect()
                    };
                    for file in removed {
                        changed |= self.remove(&file);
                    }
                    changed |= self.update(&path)?;
                }
                if changed {
                    self.persist()?;
                }
                Ok(changed)
            }
            IndexCommand::Reconcile => self.reconcile(stop, notify),
            IndexCommand::Invalidate => self.invalidate(stop, notify),
        }
    }
}
fn excluded(name: &std::ffi::OsStr) -> bool {
    matches!(name.to_str(), Some(".git" | ".nozhik"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "Exercises cold/warm cache, invalidation, overlays and incremental cross-file type relinking together"
    )]
    fn ignored_vendor_promotion_typed_chains_and_invalidation()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let root = dir.path();
        std::fs::create_dir_all(root.join("vendor/local/framework"))?;
        std::fs::create_dir_all(root.join("vendor/unused"))?;
        std::fs::write(root.join(".gitignore"), "vendor/\n")?;
        std::fs::write(
            root.join("composer.json"),
            r#"{"autoload":{"psr-4":{"Framework\\":"vendor/local/framework/"}}}"#,
        )?;
        let base = root.join("vendor/local/framework/Base.php");
        std::fs::write(
            &base,
            "<?php namespace Framework; class Settings { public function read() {} } class Base { protected Settings $settings; public function inherited() {} public function factory(): Settings {} }",
        )?;
        std::fs::write(root.join("vendor/unused/Skip.php"), "<?php class Skip {}")?;
        std::fs::create_dir_all(root.join(".nozhik"))?;
        std::fs::write(
            root.join(".nozhik/internal.php"),
            "<?php class InternalCache {}",
        )?;
        std::fs::write(root.join("vendor/unused/data.json"), [0xff, 0xfe])?;
        let outside = tempfile::tempdir()?;
        std::fs::write(outside.path().join("escape.php"), "<?php class Outside {}")?;
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path(), root.join("linked-outside"))?;

        let path = root.join("Service.php");
        let source = "<?php use Framework\\Base; use Framework\\Settings; class Service extends Base { public function __construct(private readonly Settings $config) { $config->read(); $this->config->read(); } public function work() { $this->inherited(); $this->settings->read(); $value = $this->factory(); $value->read(); (new Settings())->read(); } }";
        std::fs::write(&path, source)?;
        let stop = AtomicBool::new(false);
        let mut worker = Worker::new(root.into());
        worker.reconcile(&stop, &|_| {})?;
        assert_eq!(worker.parsed, 3);
        assert!(worker.live.declaration("\\skip").is_some());
        assert!(worker.live.declaration("\\internalcache").is_none());
        assert!(worker.live.declaration("\\outside").is_none());
        assert_eq!(
            worker
                .live
                .declaration("\\framework\\settings::read")
                .map(|d| d.usages_count),
            Some(5)
        );
        assert_eq!(
            worker
                .live
                .declaration("\\framework\\base::inherited")
                .map(|d| d.usages_count),
            Some(1)
        );
        assert_eq!(
            worker
                .live
                .declaration("\\service::config")
                .map(|d| d.usages_count),
            Some(1)
        );
        for (offset, _) in source.match_indices("read()") {
            assert_eq!(
                worker.live.at(&path, offset).map(|d| d.fqn.as_str()),
                Some("\\framework\\settings::read")
            );
        }
        worker.command(
            IndexCommand::Overlay {
                path: path.clone(),
                revision: 7,
                text: ropey::Rope::from_str(source),
            },
            &stop,
            &|_| {},
        )?;
        worker.command(IndexCommand::Invalidate, &stop, &|event| {
            if let IndexEvent::Snapshot(index, revisions) = event {
                assert_eq!(revisions.get(&path), Some(&7));
                assert!(
                    index.declaration("\\service::work").is_some(),
                    "revision gate never points at missing overlay facts"
                );
            }
        })?;
        assert_eq!(
            worker
                .live
                .declaration("\\framework\\settings::read")
                .map(|d| d.usages_count),
            Some(5)
        );
        let mut warm = Worker::new(root.into());
        warm.load()?;
        warm.reconcile(&stop, &|_| {})?;
        assert_eq!(warm.parsed, 0);
        assert_eq!(
            warm.live
                .at(&path, source.rfind("read()").ok_or("call")?)
                .map(|d| d.fqn.as_str()),
            Some("\\framework\\settings::read")
        );
        // The caller remains parsed once when a dependency's declared return type changes.
        std::fs::write(
            &base,
            "<?php namespace Framework; class Other { public function read() {} } class Settings { public function read() {} } class Base { protected Other $settings; public function inherited() {} public function factory(): Other {} }",
        )?;
        let parsed = warm.parsed;
        warm.command(IndexCommand::Changed(base), &stop, &|_| {})?;
        assert_eq!(warm.parsed, parsed + 1);
        assert_eq!(
            warm.live
                .declaration("\\framework\\other::read")
                .map(|d| d.usages_count),
            Some(2)
        );
        assert_eq!(
            warm.live
                .declaration("\\framework\\settings::read")
                .map(|d| d.usages_count),
            Some(3)
        );
        // No Composer configuration is required: even ignored dependencies remain indexed.
        std::fs::write(root.join("composer.json"), "{broken")?;
        assert!(!warm.command(
            IndexCommand::Changed(root.join("composer.json")),
            &stop,
            &|_| {}
        )?);
        assert!(warm.live.declaration("\\framework\\base").is_some());
        Ok(())
    }
    #[test]
    fn mixed_files_disabled_languages_and_noop_cache_updates()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        for name in ["a.php", "b.PHTML", "c.PHP"] {
            std::fs::write(
                dir.path().join(name),
                format!("<?php class {} {{}}", name.replace('.', "_")),
            )?;
        }
        for name in [
            "a.html",
            "a.js",
            "a.yaml",
            "a.go",
            "a.png",
            "a.txt",
            "a.php.bak",
        ] {
            std::fs::write(dir.path().join(name), [0xff, 0xfe, 0])?;
        }
        std::fs::create_dir(dir.path().join("excluded"))?;
        std::fs::write(
            dir.path().join("excluded/ignored.php"),
            "<?php class Ignored {}",
        )?;
        std::fs::write(dir.path().join(".gitignore"), "excluded/\n")?;
        let mut worker = Worker::new(dir.path().into());
        let stop = AtomicBool::new(false);
        let totals = std::cell::RefCell::new(Vec::new());
        worker.reconcile(&stop, &|e| {
            if let IndexEvent::Progress(_, total) = e {
                totals.borrow_mut().push(total);
            }
        })?;
        assert_eq!(
            worker.parsed, 4,
            "only implemented PHP extensions are opened/parsed"
        );
        assert_eq!(worker.stamps.len(), 4);
        assert!(
            totals
                .borrow()
                .iter()
                .all(|total| *total == 0 || *total == 4)
        );
        let cache_path = worker.cache_dir().join("index_php.json");
        let contents = std::fs::read(&cache_path)?;
        let modified = cache_path.metadata()?.modified()?;
        for path in ["a.html", "a.php", ".nozhik/cache/index_php.json"] {
            assert!(!worker.command(
                IndexCommand::Changed(dir.path().join(path)),
                &stop,
                &|_| {}
            )?);
        }
        assert!(!worker.command(
            IndexCommand::Overlay {
                path: dir.path().join("a.html"),
                revision: 1,
                text: ropey::Rope::from_str("<?php class Wrong {}")
            },
            &stop,
            &|_| {}
        )?);
        assert!(!worker.reconcile(&stop, &|_| {})?);
        assert_eq!(
            cache_path.metadata()?.modified()?,
            modified,
            "no-op events/startup must not rewrite the cache"
        );
        assert_eq!(worker.parsed, 4);
        assert_eq!(std::fs::read(&cache_path)?, contents);
        let mut restored = Worker::new(dir.path().into());
        restored.load()?;
        assert!(!restored.reconcile(&stop, &|_| {})?);
        assert_eq!(restored.parsed, 0);
        assert!(
            restored.live.declaration("\\a_php").is_some(),
            "derived lookup restored without parsing"
        );
        Ok(())
    }
    #[test]
    fn disabled_languages_never_read_sources_or_cache() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let mut worker = Worker::new(dir.path().into());
        worker.enabled_languages.clear();
        std::fs::create_dir_all(worker.cache_dir())?;
        let cache_path = worker.cache_dir().join("index_php.json");
        std::fs::write(&cache_path, "invalid cache that must not be read")?;
        std::fs::write(dir.path().join("bad.php"), [0xff, 0xfe])?;
        worker.load()?;
        let stop = AtomicBool::new(false);
        assert!(!worker.reconcile(&stop, &|_| {})?);
        assert!(!worker.command(
            IndexCommand::Changed(dir.path().join("bad.php")),
            &stop,
            &|_| {}
        )?);
        assert!(!worker.command(
            IndexCommand::Overlay {
                path: dir.path().join("bad.php"),
                revision: 1,
                text: ropey::Rope::from_str("<?php class Wrong {}")
            },
            &stop,
            &|_| {}
        )?);
        worker.enabled_languages.insert(LanguageId::Go);
        assert!(
            !worker.reconcile(&stop, &|_| {})?,
            "unimplemented Go cannot activate the PHP worker"
        );
        assert_eq!(worker.parsed, 0);
        assert_eq!(worker.disk.declarations_by_file.len(), 0);
        assert_eq!(
            std::fs::read_to_string(cache_path)?,
            "invalid cache that must not be read"
        );
        Ok(())
    }
    #[test]
    fn initial_batch_links_large_mixed_project_and_keeps_single_file_edits_local()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        for i in 0..500 {
            std::fs::write(
                dir.path().join(format!("class{i}.php")),
                format!(
                    "<?php class C{i} extends Base {{ public function test() {{ $this->run(); }} }}"
                ),
            )?;
            std::fs::write(
                dir.path().join(format!("data{i}.json")),
                "{ malformed unsupported data }",
            )?;
        }
        std::fs::write(
            dir.path().join("base.php"),
            "<?php class Base { public function run() {} }",
        )?;
        let mut worker = Worker::new(dir.path().into());
        let stop = AtomicBool::new(false);
        let started = std::time::Instant::now();
        worker.reconcile(&stop, &|_| {})?;
        eprintln!(
            "501 PHP + 500 JSON entries indexed in {:?}",
            started.elapsed()
        );
        assert_eq!(worker.parsed, 501);
        assert_eq!(
            worker
                .live
                .declaration("\\base::run")
                .map(|d| d.usages_count),
            Some(500)
        );
        assert_eq!(
            worker.live.declaration("\\base").map(|d| d.usages_count),
            Some(500)
        );
        let path = dir.path().join("class1.php");
        std::fs::write(
            &path,
            "<?php class C1 extends Base { public function test() {} }",
        )?;
        worker.command(IndexCommand::Changed(path), &stop, &|_| {})?;
        assert_eq!(worker.parsed, 502);
        assert_eq!(
            worker
                .live
                .declaration("\\base::run")
                .map(|d| d.usages_count),
            Some(499)
        );
        let roundtrip: SymbolIndex = serde_json::from_str(&serde_json::to_string(&worker.live)?)?;
        assert_eq!(
            roundtrip.declaration("\\base::run").map(|d| d.usages_count),
            Some(499)
        );
        Ok(())
    }
    #[test]
    fn fifty_files_differential_cache_and_overlay() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        for i in 0..50 {
            std::fs::write(
                dir.path().join(format!("class{i}.php")),
                format!(
                    "<?php class Car{i} {{ public function run() {{}} private function unused() {{}} }}"
                ),
            )?;
        }
        let root = dir.path().to_path_buf();
        let stop = AtomicBool::new(false);
        let mut worker = Worker::new(root.clone());
        worker.reconcile(&stop, &|_| {})?;
        assert_eq!(worker.parsed, 50);
        let path = root.join("class0.php");
        std::fs::write(&path, "<?php class Car0 { public function drive() {} }")?;
        worker.command(IndexCommand::Changed(path.clone()), &stop, &|_| {})?;
        assert_eq!(worker.parsed, 51, "exactly one of fifty sources parsed");
        assert!(worker.live.declaration("\\car0::run").is_none());
        assert!(worker.live.declaration("\\car0::drive").is_some());
        worker.command(
            IndexCommand::Overlay {
                path: path.clone(),
                revision: 3,
                text: ropey::Rope::from_str("<?php class Unsaved {}"),
            },
            &stop,
            &|_| {},
        )?;
        assert!(worker.live.declaration("\\unsaved").is_some());
        worker.persist()?;
        let mut restored = Worker::new(root);
        restored.load()?;
        restored.reconcile(&stop, &|_| {})?;
        assert_eq!(restored.parsed, 0, "restart reuses unchanged cache");
        assert!(restored.live.declaration("\\unsaved").is_none());
        assert!(restored.live.declaration("\\car0::drive").is_some());
        let sentinel = restored.cache_dir().join("obsolete.bin");
        std::fs::write(&sentinel, "obsolete")?;
        restored.command(IndexCommand::Invalidate, &stop, &|_| {})?;
        assert!(!sentinel.exists());
        assert_eq!(restored.parsed, 50);
        assert_eq!(restored.live.declarations_by_file.len(), 50);
        std::fs::remove_file(&path)?;
        restored.command(IndexCommand::Changed(path), &stop, &|_| {})?;
        assert!(restored.live.declaration("\\car0").is_none());
        Ok(())
    }
    #[test]
    fn php_references_completion_scopes_and_unused() -> Result<(), Box<dyn std::error::Error>> {
        use crate::{php_symbols::parse, type_inference::complete};
        let a = PathBuf::from("car.php");
        let b = PathBuf::from("caller.php");
        let declaration = "<?php namespace App; class MyCar { public function run() {} public static function create() {} public $color; private function unusedHelper() {} private const SECRET = 1; }";
        let source = "<?php namespace App; function work(MyCar $param) { $car = new MyCar(); $car->run(); $car->run(); } function other() { $foreign = 1; }";
        let mut index = SymbolIndex::new(LanguageId::Php);
        // Caller first exercises unresolved dependencies without reparsing it when the class arrives.
        index.replace(b.clone(), parse(&b, source)?);
        index.replace(a.clone(), parse(&a, declaration)?);
        let method = index
            .declaration("\\app\\mycar::run")
            .ok_or("missing run")?;
        assert_eq!(method.usages_count, 2);
        assert!(!method.is_unused());
        assert!(
            index
                .declaration("\\app\\mycar::unusedhelper")
                .is_some_and(crate::symbol_index::SymbolDeclaration::is_unused)
        );
        let caret = source.find("$car->run").ok_or("missing call")? + 6;
        let items = complete(&index, &b, source, caret);
        assert!(items.iter().any(|i| i.label == "run"));
        assert!(
            !items
                .iter()
                .any(|i| i.label == "create" || i.label == "unusedHelper")
        );
        let locals = complete(&index, &b, source, caret - 6);
        assert!(locals.iter().any(|i| i.label == "$param"));
        assert!(!locals.iter().any(|i| i.label == "$foreign"));
        let mut static_source = source[..caret - 6].to_string();
        static_source.push_str("MyCar::");
        let items = complete(&index, &b, &static_source, static_source.len());
        assert!(items.iter().any(|i| i.label == "create"));
        assert!(
            !items
                .iter()
                .any(|i| i.label == "run" || i.label == "SECRET")
        );
        let click = source.find("run()").ok_or("missing method")?;
        assert_eq!(index.at(&b, click).map(|d| d.path.clone()), Some(a));
        index.replace(
            b.clone(),
            parse(&b, "<?php // $car->run();\n $s = 'run()';")?,
        );
        assert_eq!(
            index
                .declaration("\\app\\mycar::run")
                .map(|d| d.usages_count),
            Some(0)
        );
        assert!(!index.references_by_symbol.contains_key("\\app\\mycar::run"));
        let config = crate::triggers::LanguageTriggerConfig::php();
        assert!(config.activates("$car->"));
        assert!(config.access("$car->").is_some());
        assert!(config.access("MyCar::").is_some());
        assert!(!config.activates("$car."));
        Ok(())
    }
    const FILES: &[(&str, &str)] = &[
        (
            "Contracts/Vehicle.php",
            include_str!("../../../examples/php-symbol-navigation/Contracts/Vehicle.php"),
        ),
        (
            "Model/Vehicle.php",
            include_str!("../../../examples/php-symbol-navigation/Model/Vehicle.php"),
        ),
        (
            "Model/Car.php",
            include_str!("../../../examples/php-symbol-navigation/Model/Car.php"),
        ),
        (
            "Model/Truck.php",
            include_str!("../../../examples/php-symbol-navigation/Model/Truck.php"),
        ),
        (
            "Model/ElectricCar.php",
            include_str!("../../../examples/php-symbol-navigation/Model/ElectricCar.php"),
        ),
        (
            "Fleet.php",
            include_str!("../../../examples/php-symbol-navigation/Fleet.php"),
        ),
        (
            "main.php",
            include_str!("../../../examples/php-symbol-navigation/main.php"),
        ),
    ];
    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "Asserts exact multi-file counts, reverse edges, parse-order independence and differential inheritance relinking"
    )]
    fn hierarchy_class_references_and_bidirectional_targets()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        for (name, source) in FILES {
            let path = dir.path().join(name);
            std::fs::create_dir_all(path.parent().ok_or("parent missing")?)?;
            std::fs::write(path, source)?;
        }
        let mut worker = Worker::new(dir.path().into());
        let stop = AtomicBool::new(false);
        worker.reconcile(&stop, &|_| {})?;
        let index = &worker.live;
        let expected = [
            ("\\demo\\contracts\\vehicle", 4),
            ("\\demo\\model\\vehicle", 9),
            ("\\demo\\model\\car", 6),
            ("\\demo\\model\\truck", 2),
            ("\\demo\\model\\electriccar", 2),
            ("\\demo\\app\\fleet", 1),
            ("\\demo\\model\\vehicle::run", 5),
            ("\\demo\\model\\truck::run", 1),
            ("\\demo\\model\\vehicle::name", 5),
            ("\\demo\\model\\vehicle::instances", 2),
        ];
        for (name, count) in expected {
            assert_eq!(
                index.declaration(name).map(|d| d.usages_count),
                Some(count),
                "precise count for {name}"
            );
            assert_eq!(
                index.references_by_symbol.get(name).map(HashSet::len),
                Some(count),
                "counter and rows agree for {name}"
            );
        }
        let mut reversed = SymbolIndex::new(LanguageId::Php);
        for (name, source) in FILES.iter().rev() {
            let path = dir.path().join(name);
            reversed.replace(path.clone(), crate::php_symbols::parse(&path, source)?);
        }
        for (name, count) in expected {
            assert_eq!(
                reversed.declaration(name).map(|d| d.usages_count),
                Some(count),
                "parse order must not change {name}"
            );
        }
        let fleet = dir.path().join("Fleet.php");
        let source = std::fs::read_to_string(&fleet)?;
        for (call, target) in [
            ("$car->run", "\\demo\\model\\vehicle::run"),
            ("$truck->run", "\\demo\\model\\truck::run"),
            ("$electric->run", "\\demo\\model\\vehicle::run"),
            ("$electric->charge", "\\demo\\model\\electriccar::charge"),
            ("$car->name", "\\demo\\model\\vehicle::name"),
            ("$this->vehicle->run", "\\demo\\model\\vehicle::run"),
        ] {
            let byte = source.find(call).ok_or("call missing")?
                + call.rfind("->").ok_or("operator missing")?
                + 2;
            let symbol = index
                .at(&fleet, byte)
                .ok_or_else(|| format!("unresolved {call}"))?;
            assert_eq!(symbol.fqn, target, "{call} resolves actual declaring class");
            assert!(
                index
                    .references_by_symbol
                    .get(target)
                    .is_some_and(|refs| refs.iter().any(|r| r.path == fleet && r.byte == byte)),
                "definition has a reverse edge to the same call"
            );
        }
        for class in [
            "\\demo\\contracts\\vehicle",
            "\\demo\\model\\vehicle",
            "\\demo\\model\\car",
            "\\demo\\model\\truck",
            "\\demo\\model\\electriccar",
            "\\demo\\app\\fleet",
        ] {
            assert!(
                index.declaration(class).is_some_and(|d| d.usages_count > 0),
                "class {class} must have actual type references"
            );
        }
        let caret = source.find("$car->run").ok_or("completion receiver")? + "$car->".len();
        let completions = crate::type_inference::complete(index, &fleet, &source, caret);
        for name in ["run", "park", "name"] {
            assert!(
                completions.iter().any(|c| c.label == name),
                "inherited/own completion {name}"
            );
        }
        assert!(
            !completions
                .iter()
                .any(|c| c.label == "describe" || c.label == "unusedHelper")
        );
        // Removing an override relinks callers to the base without parsing their files.
        let truck = dir.path().join("Model/Truck.php");
        let parses = worker.parsed;
        let base_count = worker
            .live
            .declaration("\\demo\\model\\vehicle::run")
            .ok_or("base run")?
            .usages_count;
        std::fs::write(
            &truck,
            "<?php namespace Demo\\Model; class Truck extends Vehicle {}",
        )?;
        worker.command(IndexCommand::Changed(truck), &stop, &|_| {})?;
        assert_eq!(worker.parsed, parses + 1);
        let byte = source.find("$truck->run").ok_or("truck call")? + "$truck->".len();
        assert_eq!(
            worker.live.at(&fleet, byte).map(|d| d.fqn.as_str()),
            Some("\\demo\\model\\vehicle::run")
        );
        // One parent::run use disappeared and the external Truck call moved to the base.
        assert_eq!(
            worker
                .live
                .declaration("\\demo\\model\\vehicle::run")
                .map(|d| d.usages_count),
            Some(base_count)
        );
        worker.persist()?;
        let mut restored = Worker::new(dir.path().into());
        restored.load()?;
        restored.reconcile(&stop, &|_| {})?;
        assert_eq!(restored.parsed, 0);
        assert_eq!(
            restored.live.at(&fleet, byte).map(|d| d.fqn.as_str()),
            Some("\\demo\\model\\vehicle::run")
        );
        let truck_source = FILES
            .iter()
            .find(|(name, _)| *name == "Model/Truck.php")
            .ok_or("truck fixture")?
            .1;
        std::fs::write(dir.path().join("Model/Truck.php"), truck_source)?;
        restored.command(
            IndexCommand::Changed(dir.path().join("Model/Truck.php")),
            &stop,
            &|_| {},
        )?;
        let car_path = dir.path().join("Model/Car.php");
        let car_source = std::fs::read_to_string(&car_path)?;
        std::fs::write(
            &car_path,
            car_source.replace("extends Vehicle", "extends Truck"),
        )?;
        restored.command(IndexCommand::Changed(car_path), &stop, &|_| {})?;
        assert_eq!(
            restored.parsed, 2,
            "only the two edited sources were parsed"
        );
        for call in ["$car->run", "$electric->run"] {
            let byte = source.find(call).ok_or("inherited call")?
                + call.rfind("->").ok_or("member operator")?
                + 2;
            assert_eq!(
                restored.live.at(&fleet, byte).map(|d| d.fqn.as_str()),
                Some("\\demo\\model\\truck::run")
            );
        }
        assert_eq!(
            restored
                .live
                .declaration("\\demo\\model\\vehicle::run")
                .map(|d| d.usages_count),
            Some(2)
        );
        assert_eq!(
            restored
                .live
                .declaration("\\demo\\model\\truck::run")
                .map(|d| d.usages_count),
            Some(4)
        );
        restored.persist()?;
        let cache_path = restored.cache_dir().join("index_php.json");
        let mut cache: serde_json::Value =
            serde_json::from_reader(std::fs::File::open(&cache_path)?)?;
        cache["version"] = serde_json::json!(2);
        serde_json::to_writer(std::fs::File::create(cache_path)?, &cache)?;
        let mut upgraded = Worker::new(dir.path().into());
        upgraded.load()?;
        upgraded.reconcile(&stop, &|_| {})?;
        assert_eq!(
            upgraded.parsed,
            FILES.len(),
            "old extraction schema is rebuilt once despite unchanged mtimes"
        );
        assert_eq!(
            upgraded
                .live
                .declaration("\\demo\\model\\truck::run")
                .map(|d| d.usages_count),
            Some(4)
        );
        Ok(())
    }
    #[test]
    fn php_type_positions_count_without_comment_or_string_matches()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut index = SymbolIndex::new(LanguageId::Php);
        let types = PathBuf::from("types.php");
        index.replace(types.clone(), crate::php_symbols::parse(&types, "<?php namespace Model; class Thing { public static function go() {} public static $state; }")?);
        let path = PathBuf::from("uses.php");
        let source = "<?php namespace App; use Model\\Thing as Alias; class Child extends Alias {} Alias::go(); Alias::$state; Alias::class; $x = new Alias(); $x instanceof Alias; #[Alias] function f(Alias $p): ?Alias {} try {} catch (Alias $e) {} // Alias::go();\n$s = 'new Alias(); Alias::go();';";
        index.replace(path.clone(), crate::php_symbols::parse(&path, source)?);
        assert_eq!(
            index.declaration("\\model\\thing").map(|d| d.usages_count),
            Some(11)
        );
        assert_eq!(
            index
                .declaration("\\model\\thing::go")
                .map(|d| d.usages_count),
            Some(1)
        );
        index.remove(&path);
        assert_eq!(
            index.declaration("\\model\\thing").map(|d| d.usages_count),
            Some(0)
        );
        assert!(
            !index
                .references_by_symbol
                .contains_key("\\model\\thing::go")
        );
        // Malformed cyclic inheritance must not hang resolution.
        let path = PathBuf::from("cycle.php");
        index.replace(
            path.clone(),
            crate::php_symbols::parse(
                &path,
                "<?php class A extends B {} class B extends A {} A::missing();",
            )?,
        );
        assert_eq!(index.class_lineage("\\a").len(), 2);
        assert!(index.resolve("\\a::missing").is_none());
        Ok(())
    }
}
