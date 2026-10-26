//! Dedicated host worker owns children and lifecycle state.
use crate::{
    Error, Request, Result,
    keymap::{KeymapConfig, KeymapRegistry},
    manifest::{Configuration, Language, PluginManifest, Tab},
    process::Session,
    registry::{Action, ActionRegistry},
};
use nozhik_event_bus::{Event, EventBus};
use semver::Version;
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::sync::{broadcast, mpsc, watch};

/// Observable lifecycle state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginStatus {
    /// Contributions are not registered.
    Disabled,
    /// Enabled, waiting for activation.
    Enabled,
    /// Successfully initialized.
    Active,
    /// Validation, dependency, process or RPC failure; contributions removed.
    Failed(String),
}
/// Installed extension metadata exposed to the UI.
#[derive(Debug, Clone)]
pub struct PluginInfo {
    /// Parsed manifest.
    pub manifest: PluginManifest,
    /// Installation directory.
    pub directory: PathBuf,
    /// Current lifecycle state.
    pub status: PluginStatus,
}
impl PluginInfo {
    /// Whether commands and associations are currently registered.
    #[must_use]
    pub const fn enabled(&self) -> bool {
        matches!(self.status, PluginStatus::Enabled | PluginStatus::Active)
    }
}
/// Authoritative contribution snapshot; UI can recover after missed notifications.
#[derive(Debug, Clone, Default)]
pub struct HostSnapshot {
    /// Installed plugins in deterministic ID order.
    pub plugins: Vec<PluginInfo>,
    /// Enabled language descriptors keyed by ID.
    pub languages: BTreeMap<String, Language>,
    /// Enabled extension-to-language associations.
    pub extensions: BTreeMap<String, String>,
    /// Enabled generic tool tabs keyed by ID.
    pub tabs: BTreeMap<String, Tab>,
    /// Enabled typed configuration defaults.
    pub configuration: BTreeMap<String, Configuration>,
    /// Discovery, queue recovery and keymap diagnostics.
    pub diagnostics: Vec<String>,
}

/// Nonblocking handle. Dropping it signals worker shutdown and kills direct children.
pub struct PluginManager {
    requests: mpsc::Sender<Request>,
    snapshot: watch::Receiver<HostSnapshot>,
    shutdown: watch::Sender<bool>,
    actions: ActionRegistry,
    keymap: KeymapRegistry,
}
impl PluginManager {
    /// Starts a dedicated runtime and scans default plugin locations in the background.
    /// # Errors
    /// Returns thread/runtime construction failures.
    pub fn start(bus: EventBus) -> Result<Self> {
        let home = directories::BaseDirs::new().map(|dirs| dirs.home_dir().join(".nozhik"));
        Self::with_paths(
            bus,
            home.as_ref()
                .map(|p| p.join("plugins"))
                .into_iter()
                .collect(),
            home.map(|p| p.join("keymap.json")),
        )
    }

    /// Starts a host using explicit global roots and keymap path (useful for embedders/tests).
    /// Project roots are supplied later with [`Self::scan`].
    /// # Errors
    /// Returns thread/runtime construction failures.
    pub fn with_paths(
        bus: EventBus,
        roots: Vec<PathBuf>,
        keymap_path: Option<PathBuf>,
    ) -> Result<Self> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let (requests, receiver) = mpsc::channel(256);
        let (updates, snapshot) = watch::channel(HostSnapshot::default());
        let (shutdown, stopping) = watch::channel(false);
        let events = bus.subscribe();
        let actions = ActionRegistry::new(bus, requests.clone());
        let keymap = KeymapRegistry::default();
        keymap.replace(KeymapConfig::default())?;
        let mut worker = Worker {
            roots,
            keymap_path,
            keymap_source: None,
            plugins: BTreeMap::new(),
            sessions: BTreeMap::new(),
            seen_files: BTreeSet::new(),
            actions: actions.clone(),
            keymap: keymap.clone(),
            updates,
            snapshot: HostSnapshot::default(),
            ide: Version::parse(env!("CARGO_PKG_VERSION"))
                .map_err(|e| Error::Invalid(e.to_string()))?,
        };
        std::thread::Builder::new()
            .name("nozhik-plugins".into())
            .spawn(move || {
                let mut stopping = stopping;
                runtime.block_on(async {
                    tokio::select! {
                        () = worker.run(receiver, events) => {},
                        _ = stopping.changed() => {},
                    }
                    for (_, session) in std::mem::take(&mut worker.sessions) {
                        session.stop().await;
                    }
                });
            })?;
        Ok(Self {
            requests,
            snapshot,
            shutdown,
            actions,
            keymap,
        })
    }

    /// Subscribe to lifecycle and keymap changes for event-driven UI wakeup.
    #[must_use]
    pub fn subscribe_updates(&self) -> watch::Receiver<HostSnapshot> {
        self.snapshot.clone()
    }

    /// Shared action registry for built-ins and extensions.
    #[must_use]
    pub fn actions(&self) -> ActionRegistry {
        self.actions.clone()
    }
    /// Shared portable keymap registry.
    #[must_use]
    pub fn keymap(&self) -> KeymapRegistry {
        self.keymap.clone()
    }
    /// Latest consistent lifecycle/contribution snapshot, without I/O.
    #[must_use]
    pub fn snapshot(&self) -> HostSnapshot {
        self.snapshot.borrow().clone()
    }
    /// Rescans global roots plus `<project>/.nozhik/plugins`, replacing old project plugins.
    /// # Errors
    /// Returns queue full/stopped. Scan failures are reported in the snapshot.
    pub fn scan(&self, project: Option<PathBuf>) -> Result<()> {
        self.requests
            .try_send(Request::Scan(project))
            .map_err(|_| Error::Queue)
    }
    /// Enables dependencies first, or disables the plugin and all its dependents.
    /// # Errors
    /// Returns queue full/stopped. Lifecycle errors appear in the snapshot.
    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<()> {
        self.requests
            .try_send(Request::Toggle {
                id: id.into(),
                enabled,
            })
            .map_err(|_| Error::Queue)
    }
}
impl Drop for PluginManager {
    fn drop(&mut self) {
        self.shutdown.send_replace(true);
    }
}

#[derive(Clone, PartialEq, Eq)]
enum KeymapSource {
    Missing,
    Present(Vec<u8>),
}

struct Worker {
    roots: Vec<PathBuf>,
    keymap_path: Option<PathBuf>,
    keymap_source: Option<KeymapSource>,
    plugins: BTreeMap<String, PluginInfo>,
    sessions: BTreeMap<String, Session>,
    seen_files: BTreeSet<PathBuf>,
    actions: ActionRegistry,
    keymap: KeymapRegistry,
    updates: watch::Sender<HostSnapshot>,
    snapshot: HostSnapshot,
    ide: Version,
}
impl Worker {
    async fn run(
        &mut self,
        mut requests: mpsc::Receiver<Request>,
        mut events: broadcast::Receiver<std::sync::Arc<Event>>,
    ) {
        self.scan(None).await;
        let mut timer = tokio::time::interval(Duration::from_secs(1));
        loop {
            tokio::select! {
                biased;
                request = requests.recv() => {
                    let Some(request) = request else { break; };
                    match request {
                        Request::Scan(project) => self.scan(project.as_deref()).await,
                        Request::Toggle { id, enabled } => {
                            if enabled {
                                if let Err(error) = self.enable(&id) { self.fail(&id, error.to_string()).await; }
                                self.activate_matching().await;
                            } else { self.disable(&id).await; }
                        }
                        Request::Execute { owner, command } => {
                            if self.plugins.get(&owner).is_some_and(|p| p.enabled() && p.manifest.contributes.commands.iter().any(|c| c.id == command)) {
                                let result = self.activate(&owner).await;
                                if let Err(error) = result { self.fail(&owner, error.to_string()).await; }
                                else if let Some(session) = self.sessions.get_mut(&owner) {
                                    if let Err(error) = session.call("executeCommand", json!({"command":command})).await {
                                        self.fail(&owner, error.to_string()).await;
                                    }
                                } else {
                                    self.snapshot.diagnostics.push(format!("{owner}: declarative command {command} has no executable handler"));
                                }
                            }
                        }
                    }
                    self.publish();
                }
                event = events.recv() => match event {
                    Ok(event) => if let Event::FileOpened(path) = event.as_ref() {
                        self.seen_files.insert(path.clone());
                        self.activate_matching().await;
                        self.publish();
                    },
                    Err(broadcast::error::RecvError::Lagged(count)) => {
                        self.snapshot.diagnostics.push(format!("plugin activation missed {count} events; reopening a file retries activation"));
                        self.publish();
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                },
                _ = timer.tick() => {
                    let old_source = self.keymap_source.clone();
                    let old_diagnostics = self.snapshot.diagnostics.len();
                    self.reload_keymap();
                    let failed: Vec<_> = self.sessions.iter_mut().filter_map(|(id, session)| match session.exited() {
                        Ok(false) => None,
                        Ok(true) => Some((id.clone(), "plugin process exited".into())),
                        Err(error) => Some((id.clone(), error.to_string())),
                    }).collect();
                    let changed = !failed.is_empty() || old_source != self.keymap_source || old_diagnostics != self.snapshot.diagnostics.len();
                    for (id, error) in failed { self.fail(&id, error).await; }
                    if changed { self.publish(); }
                }
            }
        }
    }

    async fn scan(&mut self, project: Option<&Path>) {
        let ids: Vec<_> = self.plugins.keys().cloned().collect();
        for id in ids {
            self.disable(&id).await;
        }
        self.plugins.clear();
        self.seen_files.clear();
        self.snapshot.diagnostics.clear();
        let mut roots = self.roots.clone();
        if let Some(project) = project {
            roots.push(project.join(".nozhik/plugins"));
        }
        for root in roots {
            let entries = match std::fs::read_dir(&root) {
                Ok(entries) => entries,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    self.snapshot
                        .diagnostics
                        .push(format!("{}: {error}", root.display()));
                    continue;
                }
            };
            let mut paths = Vec::new();
            for entry in entries {
                match entry {
                    Ok(entry) => paths.push(entry.path()),
                    Err(error) => self.snapshot.diagnostics.push(error.to_string()),
                }
            }
            paths.sort();
            for directory in paths {
                if !directory.is_dir() {
                    continue;
                }
                let result = std::fs::read_to_string(directory.join("plugin.json"))
                    .map_err(Error::from)
                    .and_then(|source| PluginManifest::parse(&source));
                match result {
                    Ok(manifest) => {
                        if self.plugins.contains_key(&manifest.id) {
                            self.snapshot.diagnostics.push(format!(
                                "duplicate plugin {} in {} (first installation wins)",
                                manifest.id,
                                directory.display()
                            ));
                        } else {
                            self.plugins.insert(
                                manifest.id.clone(),
                                PluginInfo {
                                    manifest,
                                    directory,
                                    status: PluginStatus::Disabled,
                                },
                            );
                        }
                    }
                    Err(error) => self
                        .snapshot
                        .diagnostics
                        .push(format!("{}: {error}", directory.display())),
                }
            }
        }
        let ids: Vec<_> = self.plugins.keys().cloned().collect();
        for id in ids {
            if let Err(error) = self.enable(&id) {
                self.fail(&id, error.to_string()).await;
            }
        }
        self.activate_matching().await;
        self.publish();
    }

    fn plan(
        &self,
        id: &str,
        visiting: &mut BTreeSet<String>,
        ordered: &mut Vec<String>,
    ) -> Result<()> {
        if ordered.iter().any(|item| item == id) {
            return Ok(());
        }
        if !visiting.insert(id.into()) {
            return Err(Error::Invalid(format!("cyclic dependency: {id}")));
        }
        let plugin = self
            .plugins
            .get(id)
            .ok_or_else(|| Error::Invalid(format!("missing dependency: {id}")))?;
        if !plugin.manifest.compatible_with(&self.ide) {
            return Err(Error::Invalid(format!(
                "{id} requires IDE {}",
                plugin.manifest.min_ide_version
            )));
        }
        for (dependency, requirement) in &plugin.manifest.dependencies {
            let target = self
                .plugins
                .get(dependency)
                .ok_or_else(|| Error::Invalid(format!("missing dependency: {dependency}")))?;
            if !requirement.matches(&target.manifest.version) {
                return Err(Error::Invalid(format!(
                    "{dependency} does not satisfy {requirement}"
                )));
            }
            self.plan(dependency, visiting, ordered)?;
        }
        visiting.remove(id);
        ordered.push(id.into());
        Ok(())
    }

    fn enable(&mut self, id: &str) -> Result<()> {
        let mut order = Vec::new();
        self.plan(id, &mut BTreeSet::new(), &mut order)?;
        for id in order {
            let Some(plugin) = self.plugins.get(&id) else {
                continue;
            };
            if plugin.enabled() {
                continue;
            }
            let manifest = &plugin.manifest;
            // Reject collisions before registering any contribution from this plugin.
            for other in self.plugins.values().filter(|p| p.enabled()) {
                let other = &other.manifest.contributes;
                if manifest.contributes.languages.iter().any(|l| {
                    other.languages.iter().any(|o| {
                        o.id == l.id
                            || o.extensions
                                .iter()
                                .any(|e| l.extensions.iter().any(|v| v.eq_ignore_ascii_case(e)))
                    })
                }) || manifest
                    .contributes
                    .tabs
                    .iter()
                    .any(|t| other.tabs.iter().any(|o| o.id == t.id))
                    || manifest
                        .contributes
                        .configuration
                        .keys()
                        .any(|k| other.configuration.contains_key(k))
                {
                    return Err(Error::Invalid(format!("contribution collision: {id}")));
                }
            }
            let actions = manifest
                .contributes
                .commands
                .iter()
                .map(|c| Action {
                    id: c.id.clone(),
                    title: c.title.clone(),
                    when: c.when.clone(),
                    owner: Some(id.clone()),
                })
                .collect();
            self.actions.register_batch(actions)?;
            if let Some(plugin) = self.plugins.get_mut(&id) {
                plugin.status = PluginStatus::Enabled;
            }
        }
        Ok(())
    }

    async fn disable(&mut self, id: &str) {
        let mut disabled = BTreeSet::from([id.to_owned()]);
        loop {
            let before = disabled.len();
            for (id, plugin) in &self.plugins {
                if plugin.enabled()
                    && plugin
                        .manifest
                        .dependencies
                        .keys()
                        .any(|dep| disabled.contains(dep))
                {
                    disabled.insert(id.clone());
                }
            }
            if disabled.len() == before {
                break;
            }
        }
        for id in disabled {
            if let Some(session) = self.sessions.remove(&id) {
                session.stop().await;
            }
            if let Err(error) = self.actions.unregister_owner(&id) {
                self.snapshot.diagnostics.push(error.to_string());
            }
            if let Some(plugin) = self.plugins.get_mut(&id) {
                plugin.status = PluginStatus::Disabled;
            }
        }
    }

    async fn fail(&mut self, id: &str, error: String) {
        self.disable(id).await;
        if let Some(plugin) = self.plugins.get_mut(id) {
            plugin.status = PluginStatus::Failed(error);
        } else {
            self.snapshot.diagnostics.push(error);
        }
    }

    async fn activate(&mut self, id: &str) -> Result<()> {
        let mut order = Vec::new();
        self.plan(id, &mut BTreeSet::new(), &mut order)?;
        for id in order {
            let plugin = self
                .plugins
                .get(&id)
                .ok_or_else(|| Error::Invalid("plugin removed".into()))?;
            if !plugin.enabled() {
                return Err(Error::Invalid(format!("plugin disabled: {id}")));
            }
            if plugin.status == PluginStatus::Active {
                continue;
            }
            if plugin.manifest.executable.is_some() {
                let session = Session::start(&plugin.directory, &plugin.manifest).await?;
                self.sessions.insert(id.clone(), session);
            }
            if let Some(plugin) = self.plugins.get_mut(&id) {
                plugin.status = PluginStatus::Active;
            }
        }
        Ok(())
    }

    async fn activate_matching(&mut self) {
        let mut events = BTreeSet::from(["*".to_owned()]);
        for path in &self.seen_files {
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_lowercase();
            for plugin in self.plugins.values().filter(|p| p.enabled()) {
                for language in &plugin.manifest.contributes.languages {
                    for extension in &language.extensions {
                        if name.ends_with(&extension.to_lowercase()) {
                            events.insert(format!("onLanguage:{}", language.id));
                        }
                    }
                }
            }
            for plugin in self.plugins.values() {
                for event in &plugin.manifest.activation_events {
                    if event
                        .strip_prefix("onFileExtension:")
                        .is_some_and(|ext| name.ends_with(&ext.to_lowercase()))
                    {
                        events.insert(event.clone());
                    }
                }
            }
        }
        let ids: Vec<_> = self
            .plugins
            .iter()
            .filter(|(_, p)| {
                p.enabled()
                    && p.status != PluginStatus::Active
                    && p.manifest
                        .activation_events
                        .iter()
                        .any(|e| events.contains(e))
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            if let Err(error) = self.activate(&id).await {
                self.fail(&id, error.to_string()).await;
            }
        }
    }

    fn reload_keymap(&mut self) {
        let Some(path) = &self.keymap_path else {
            return;
        };
        let source = match std::fs::read(path) {
            Ok(source) => KeymapSource::Present(source),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => KeymapSource::Missing,
            Err(error) => {
                let message = format!("keymap: {error}");
                if !self.snapshot.diagnostics.contains(&message) {
                    self.snapshot.diagnostics.push(message);
                }
                return;
            }
        };
        if self.keymap_source.as_ref() == Some(&source) {
            return;
        }
        let config = match &source {
            KeymapSource::Missing => Ok(KeymapConfig::default()),
            KeymapSource::Present(source) => serde_json::from_slice(source).map_err(Error::from),
        };
        match config.and_then(|config| self.keymap.replace(config)) {
            Ok(conflicts) => {
                for chord in conflicts {
                    self.snapshot
                        .diagnostics
                        .push(format!("keymap override: {chord}"));
                }
            }
            Err(error) => self.snapshot.diagnostics.push(format!(
                "keymap reload rejected (previous bindings retained): {error}"
            )),
        }
        self.keymap_source = Some(source);
    }

    fn publish(&mut self) {
        self.snapshot.plugins = self.plugins.values().cloned().collect();
        self.snapshot.languages.clear();
        self.snapshot.extensions.clear();
        self.snapshot.tabs.clear();
        self.snapshot.configuration.clear();
        for plugin in self.plugins.values().filter(|p| p.enabled()) {
            for language in &plugin.manifest.contributes.languages {
                self.snapshot
                    .languages
                    .insert(language.id.clone(), language.clone());
                for extension in &language.extensions {
                    self.snapshot
                        .extensions
                        .insert(extension.to_lowercase(), language.id.clone());
                }
            }
            for tab in &plugin.manifest.contributes.tabs {
                self.snapshot.tabs.insert(tab.id.clone(), tab.clone());
            }
            self.snapshot
                .configuration
                .extend(plugin.manifest.contributes.configuration.clone());
        }
        // Diagnostics remain bounded even if a plugin repeatedly fails.
        if self.snapshot.diagnostics.len() > 100 {
            self.snapshot
                .diagnostics
                .drain(..self.snapshot.diagnostics.len() - 100);
        }
        self.updates.send_replace(self.snapshot.clone());
    }
}
