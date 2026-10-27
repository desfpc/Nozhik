//! Single-owner background project indexing, disk cache and buffer overlays.
use crate::symbol_index::{FileSymbols, LanguageId, SymbolIndex};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
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
            match worker.load() {
                Ok(()) => notify(worker.snapshot()),
                Err(e) => notify(IndexEvent::Error(e.to_string())),
            }
            if let Err(e) = worker.reconcile(&stopping, &notify) {
                notify(IndexEvent::Error(e.to_string()));
            }
            notify(worker.snapshot());
            while !stopping.load(Ordering::Relaxed) {
                let Ok(command) = commands.recv() else {
                    break;
                };
                let result = worker.command(command, &stopping, &notify);
                if let Err(e) = result {
                    notify(IndexEvent::Error(e.to_string()));
                }
                notify(worker.snapshot());
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
struct Worker {
    root: PathBuf,
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
            disk: SymbolIndex::new(LanguageId::Php),
            live: SymbolIndex::new(LanguageId::Php),
            stamps: HashMap::new(),
            overlays: HashMap::new(),
            parsed: 0,
        }
    }
    fn cache_dir(&self) -> PathBuf {
        self.root.join(".nozhik/cache")
    }
    fn load(&mut self) -> std::io::Result<()> {
        let file = match std::fs::File::open(self.cache_dir().join("index_php.json")) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };
        let cache: Cache = serde_json::from_reader(std::io::BufReader::new(file))?;
        if cache.version != 2 {
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
            &Cache {
                version: 2,
                root: self.root.clone(),
                stamps: self.stamps.clone(),
                index: self.disk.clone(),
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
    fn update(&mut self, path: &Path) -> std::io::Result<()> {
        if !path.is_file() {
            self.disk.remove(path);
            self.stamps.remove(path);
            if !self.overlays.contains_key(path) {
                self.live.remove(path);
            }
            return Ok(());
        }
        if !crate::lsp::manager::is_php(path) {
            return Ok(());
        }
        let before = stamp(path)?;
        if self.stamps.get(path) == Some(&before) {
            return Ok(());
        }
        let source = std::fs::read_to_string(path)?;
        let facts = crate::php_symbols::parse(path, &source).map_err(std::io::Error::other)?;
        // A concurrent write must be reconciled again rather than stamped as parsed.
        if stamp(path)? != before {
            self.stamps.remove(path);
            return self.update(path);
        }
        self.parsed += 1;
        self.disk.replace(path.into(), facts.clone());
        if !self.overlays.contains_key(path) {
            self.live.replace(path.into(), facts);
        }
        self.stamps.insert(path.into(), before);
        Ok(())
    }
    fn paths(root: &Path) -> Vec<PathBuf> {
        ignore::WalkBuilder::new(root)
            .hidden(false)
            .filter_entry(|entry| {
                !matches!(
                    entry.file_name().to_str(),
                    Some(".git" | ".nozhik" | "target" | "vendor" | "node_modules")
                )
            })
            .build()
            .filter_map(Result::ok)
            .filter(|e| {
                e.file_type().is_some_and(|t| t.is_file()) && crate::lsp::manager::is_php(e.path())
            })
            .map(ignore::DirEntry::into_path)
            .collect()
    }
    fn reconcile(
        &mut self,
        stop: &AtomicBool,
        notify: &impl Fn(IndexEvent),
    ) -> std::io::Result<()> {
        notify(IndexEvent::Progress(0, 0));
        let paths = Self::paths(&self.root);
        let known: HashSet<_> = paths.iter().cloned().collect();
        let removed: Vec<_> = self
            .stamps
            .keys()
            .filter(|p| !known.contains(*p))
            .cloned()
            .collect();
        for path in removed {
            self.update(&path)?;
        }
        for (i, path) in paths.iter().enumerate() {
            if stop.load(Ordering::Relaxed) {
                return Ok(());
            }
            if let Err(e) = self.update(path) {
                notify(IndexEvent::Error(format!("{}: {e}", path.display())));
            }
            if i % 20 == 0 {
                notify(IndexEvent::Progress(i, paths.len()));
            }
        }
        self.persist()?;
        notify(IndexEvent::Progress(paths.len(), paths.len()));
        notify(IndexEvent::Finished);
        Ok(())
    }
    fn command(
        &mut self,
        command: IndexCommand,
        stop: &AtomicBool,
        notify: &impl Fn(IndexEvent),
    ) -> std::io::Result<()> {
        match command {
            IndexCommand::Overlay {
                path,
                revision,
                text,
            } => {
                if !path.starts_with(&self.root) {
                    return Ok(());
                }
                if self.overlays.get(&path).is_none_or(|(r, _)| *r != revision) {
                    let facts = crate::php_symbols::parse(&path, &text.to_string())
                        .map_err(std::io::Error::other)?;
                    self.parsed += 1;
                    self.live.replace(path.clone(), facts.clone());
                    self.overlays.insert(path, (revision, facts));
                }
            }
            IndexCommand::Close(path) => {
                self.overlays.remove(&path);
                self.update(&path)?;
                self.live.replace(
                    path.clone(),
                    self.disk.file_symbols(&path).cloned().unwrap_or_default(),
                );
            }
            IndexCommand::Changed(path) => {
                if !path.starts_with(&self.root)
                    || path.strip_prefix(&self.root).is_ok_and(|p| {
                        p.components().any(|c| {
                            matches!(
                                c.as_os_str().to_str(),
                                Some(".nozhik" | ".git" | "target" | "vendor" | "node_modules")
                            )
                        })
                    })
                {
                    return Ok(());
                }
                if path.is_dir() {
                    for file in Self::paths(&path) {
                        if stop.load(Ordering::Relaxed) {
                            return Ok(());
                        }
                        self.update(&file)?;
                    }
                } else {
                    let removed: Vec<_> = self
                        .stamps
                        .keys()
                        .filter(|p| p.starts_with(&path))
                        .cloned()
                        .collect();
                    for file in removed {
                        self.update(&file)?;
                    }
                    self.update(&path)?;
                }
                self.persist()?;
            }
            IndexCommand::Reconcile => self.reconcile(stop, notify)?,
            IndexCommand::Invalidate => {
                match std::fs::remove_dir_all(self.cache_dir()) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e),
                }
                self.disk = SymbolIndex::new(LanguageId::Php);
                self.live = self.disk.clone();
                self.stamps.clear();
                notify(self.snapshot());
                self.reconcile(stop, notify)?;
                for (path, (_, facts)) in &self.overlays {
                    self.live.replace(path.clone(), facts.clone());
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        cache["version"] = serde_json::json!(1);
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
