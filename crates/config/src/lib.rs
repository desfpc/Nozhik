//! Validated TOML/JSON settings with explicit persistence and change notifications.

pub mod code_style;
pub mod languages;

use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

use directories::ProjectDirs;
use nozhik_core::ConfigKey;
use nozhik_event_bus::{Event, EventBus};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;
use thiserror::Error;

/// Application settings. Missing fields use defaults; unknown fields are errors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Number of columns per indentation step, between 1 and 16.
    pub tab_width: u8,
    /// Font size in logical pixels, between 6 and 96.
    pub font_size: u16,
    /// Nonempty theme identifier; theme resolution belongs to the future UI.
    pub theme: String,
    /// Editor font family: monospace or proportional.
    pub editor_font: String,
    /// Global PHP major.minor language level for offline completion.
    pub php_language_level: String,
    /// Enabled language services/indexers. Empty means plain-text editing only.
    pub enabled_languages: std::collections::BTreeSet<languages::LanguageId>,
    /// Enable local Ollama ghost completions.
    pub ai_enabled: bool,
    /// Ollama model name; downloads are always explicit.
    pub ai_model: String,
    /// Input quiet period in milliseconds (50–5000).
    pub ai_debounce_ms: u16,
    /// Per-extension profile and syntax palette selections.
    pub file_types: std::collections::BTreeMap<String, code_style::FileTypeSettings>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            tab_width: 4,
            font_size: 14,
            theme: "dark".into(),
            editor_font: "monospace".into(),
            php_language_level: "8.5".into(),
            enabled_languages: languages::default_enabled(),
            ai_enabled: true,
            ai_model: "qwen2.5-coder:1.5b-base".into(),
            ai_debounce_ms: 1000,
            file_types: std::collections::BTreeMap::new(),
        }
    }
}

impl Config {
    /// Checks semantic constraints after deserialization or before replacement.
    ///
    /// # Errors
    /// Returns [`ConfigError::Invalid`] for unsupported values.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if !(1..=16).contains(&self.tab_width) {
            return Err(ConfigError::Invalid("tab_width must be between 1 and 16"));
        }
        if !(6..=96).contains(&self.font_size) {
            return Err(ConfigError::Invalid("font_size must be between 6 and 96"));
        }
        if !matches!(self.editor_font.as_str(), "monospace" | "proportional") {
            return Err(ConfigError::Invalid("unsupported editor font"));
        }
        if self
            .file_types
            .values()
            .any(|s| !matches!(s.syntax_theme.as_str(), "default" | "muted" | "plain"))
        {
            return Err(ConfigError::Invalid("unsupported syntax theme"));
        }
        if !matches!(
            self.php_language_level.as_str(),
            "7.0" | "7.1" | "7.2" | "7.3" | "7.4" | "8.0" | "8.1" | "8.2" | "8.3" | "8.4" | "8.5"
        ) {
            return Err(ConfigError::Invalid("unsupported PHP language level"));
        }
        if self.ai_model.trim().is_empty() || !(50..=5000).contains(&self.ai_debounce_ms) {
            return Err(ConfigError::Invalid(
                "AI model must be nonempty and debounce must be 50–5000 ms",
            ));
        }
        if self.theme.trim().is_empty() {
            return Err(ConfigError::Invalid("theme must not be empty"));
        }
        Ok(())
    }
}

/// Shared access contract. Prefer manager methods to preserve validation/events.
pub type SharedConfig = Arc<RwLock<Config>>;

/// Settings loading, serialization, synchronization, and persistence failures.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// No platform-specific user configuration directory could be determined.
    #[error("user configuration directory is unavailable")]
    NoConfigDirectory,
    /// The path must end in `.toml` or `.json`.
    #[error("unsupported settings path: {0}")]
    UnsupportedFormat(PathBuf),
    /// A filesystem operation failed.
    #[error("settings I/O failed: {0}")]
    Io(#[from] io::Error),
    /// TOML parsing failed.
    #[error("invalid TOML: {0}")]
    TomlRead(#[from] toml::de::Error),
    /// TOML serialization failed.
    #[error("cannot serialize TOML: {0}")]
    TomlWrite(#[from] toml::ser::Error),
    /// JSON parsing or serialization failed.
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// A settings value is outside its supported domain.
    #[error("invalid settings: {0}")]
    Invalid(&'static str),
    /// Another thread panicked while holding the settings write lock.
    #[error("settings lock is poisoned")]
    Poisoned,
}

#[derive(Clone, Copy)]
enum Format {
    Toml,
    Json,
}

impl Format {
    fn from_path(path: &Path) -> Result<Self, ConfigError> {
        match path.extension().and_then(std::ffi::OsStr::to_str) {
            Some("toml") => Ok(Self::Toml),
            Some("json") => Ok(Self::Json),
            _ => Err(ConfigError::UnsupportedFormat(path.to_path_buf())),
        }
    }
}

/// Manager for one settings file. Disk I/O is synchronous; call it on workers.
#[derive(Debug)]
pub struct ConfigManager {
    path: PathBuf,
    config: SharedConfig,
    bus: EventBus,
}

impl ConfigManager {
    /// Loads `settings.toml` from the OS user configuration directory.
    ///
    /// # Errors
    /// Returns directory resolution, I/O, parsing, or validation failures.
    pub fn load(bus: EventBus) -> Result<Self, ConfigError> {
        let dirs =
            ProjectDirs::from("dev", "Nozhik", "Nozhik").ok_or(ConfigError::NoConfigDirectory)?;
        Self::from_path(dirs.config_dir().join("settings.toml"), bus)
    }

    /// Loads a specific TOML/JSON file. Only a missing file uses full defaults.
    ///
    /// # Errors
    /// Returns unsupported extension, I/O, parsing, or validation failures.
    pub fn from_path(path: PathBuf, bus: EventBus) -> Result<Self, ConfigError> {
        let config = Self::read(&path)?;
        Ok(Self {
            path,
            config: Arc::new(RwLock::new(config)),
            bus,
        })
    }

    fn read(path: &Path) -> Result<Config, ConfigError> {
        let format = Format::from_path(path)?;
        let config = match fs::read_to_string(path) {
            Ok(source) => match format {
                Format::Toml => toml::from_str(&source)?,
                Format::Json => serde_json::from_str(&source)?,
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => Config::default(),
            Err(error) => return Err(error.into()),
        };
        config.validate()?;
        Ok(config)
    }

    /// Returns the actual settings file path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Obtains the shared settings handle. Never hold its guard across `.await`.
    #[must_use]
    pub fn shared(&self) -> SharedConfig {
        Arc::clone(&self.config)
    }

    /// Returns an owned, consistent snapshot.
    ///
    /// # Errors
    /// Returns [`ConfigError::Poisoned`] if the lock is poisoned.
    pub fn snapshot(&self) -> Result<Config, ConfigError> {
        Ok(self
            .config
            .read()
            .map_err(|_| ConfigError::Poisoned)?
            .clone())
    }

    /// Validates and replaces in-memory settings, then broadcasts invalidation.
    /// Unchanged values emit no event. Persistence requires an explicit `save`.
    ///
    /// # Errors
    /// Returns validation or poisoned-lock errors without replacing the settings.
    pub fn replace(&self, config: Config) -> Result<(), ConfigError> {
        config.validate()?;
        {
            let mut current = self.config.write().map_err(|_| ConfigError::Poisoned)?;
            if *current == config {
                return Ok(());
            }
            *current = config;
        }
        self.bus.publish(Event::ConfigChanged(ConfigKey::All));
        Ok(())
    }

    /// Reloads the file and broadcasts invalidation if the settings changed.
    ///
    /// # Errors
    /// Returns read, validation, or poisoned-lock errors; keeps old values on failure.
    pub fn reload(&self) -> Result<(), ConfigError> {
        self.replace(Self::read(&self.path)?)
    }

    /// Writes settings via a temporary file in the destination directory.
    /// Holds a read lock through replacement to serialize against in-memory edits.
    ///
    /// # Errors
    /// Returns lock, validation, serialization, or filesystem errors.
    pub fn save(&self) -> Result<(), ConfigError> {
        let config = self.config.read().map_err(|_| ConfigError::Poisoned)?;
        config.validate()?;
        let source = match Format::from_path(&self.path)? {
            Format::Toml => toml::to_string_pretty(&*config)?,
            Format::Json => serde_json::to_string_pretty(&*config)?,
        };
        let parent = self
            .path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;
        let mut temporary = NamedTempFile::new_in(parent)?;
        temporary.write_all(source.as_bytes())?;
        temporary.as_file().sync_all()?;
        temporary.persist(&self.path).map_err(|error| error.error)?;
        drop(config);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Config, ConfigManager, languages};
    use nozhik_core::ConfigKey;
    use nozhik_event_bus::{Event, EventBus};
    use std::{error::Error, fs, path::Path};

    #[test]
    fn defaults_and_round_trips() -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        for extension in ["toml", "json"] {
            let path = dir.path().join(format!("nested/settings.{extension}"));
            let manager = ConfigManager::from_path(path.clone(), EventBus::new(8)?)?;
            assert_eq!(manager.snapshot()?, Config::default());
            assert!(!path.exists());
            manager.save()?;
            let changed = Config {
                tab_width: 2,
                enabled_languages: std::collections::BTreeSet::new(),
                php_language_level: "8.2".into(),
                ai_enabled: false,
                ai_model: "custom:7b".into(),
                ai_debounce_ms: 700,
                ..Config::default()
            };
            manager.replace(changed.clone())?;
            manager.save()?; // Also verifies replacement of an existing file.
            let loaded = ConfigManager::from_path(path, EventBus::new(8)?)?;
            assert_eq!(loaded.snapshot()?, changed);
        }
        let partial: Config = toml::from_str("tab_width = 2")?;
        assert_eq!(partial.font_size, Config::default().font_size);
        assert_eq!(partial.php_language_level, "8.5");
        assert_eq!(partial.enabled_languages, languages::default_enabled());
        let disabled: Config = toml::from_str("enabled_languages = []")?;
        assert!(disabled.enabled_languages.is_empty());
        assert!(
            serde_json::from_str::<Config>(r#"{"enabled_languages":["unknown_language"]}"#)
                .is_err()
        );
        let go_only: Config = toml::from_str("enabled_languages = ['go']")?;
        assert!(!languages::enabled_for_path(
            &go_only.enabled_languages,
            Path::new("test.php")
        ));
        let partial_json: Config = serde_json::from_str(r#"{"tab_width":2}"#)?;
        assert_eq!(partial_json, partial);
        Ok(())
    }

    #[test]
    fn validates_and_publishes_after_commit() -> Result<(), Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("settings.toml");
        let bus = EventBus::new(8)?;
        let mut events = bus.subscribe();
        let manager = ConfigManager::from_path(path.clone(), bus)?;
        let changed = Config {
            theme: "light".into(),
            ..Config::default()
        };
        manager.replace(changed.clone())?;
        assert_eq!(*events.try_recv()?, Event::ConfigChanged(ConfigKey::All));
        assert_eq!(manager.snapshot()?, changed);
        manager.replace(changed.clone())?;
        assert!(events.try_recv().is_err());
        let invalid = Config {
            tab_width: 0,
            ..changed.clone()
        };
        assert!(manager.replace(invalid).is_err());
        for source in [
            "[broken",
            "tab_width = 0",
            "typo = true",
            "ai_model = ' '",
            "ai_debounce_ms = 0",
            "ai_debounce_ms = 5001",
            "php_language_level = '9.0'",
            "php_language_level = '../../bad'",
        ] {
            fs::write(&path, source)?;
            assert!(manager.reload().is_err());
            assert_eq!(manager.snapshot()?, changed);
        }
        fs::write(&path, "tab_width = 8")?;
        manager.reload()?;
        assert_eq!(manager.snapshot()?.tab_width, 8);
        assert_eq!(*events.try_recv()?, Event::ConfigChanged(ConfigKey::All));
        Ok(())
    }
}
