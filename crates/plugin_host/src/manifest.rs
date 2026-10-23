//! Strict, versioned declarative extension contract.
use crate::{Error, Result, registry::When};
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashSet},
    path::{Component, Path},
};

/// Plugin metadata; unknown fields at every level are rejected.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginManifest {
    /// Stable reverse-domain identity.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Plugin release.
    pub version: Version,
    /// Inclusive lower bound on the host version.
    pub min_ide_version: Version,
    /// Purpose of the extension.
    pub description: String,
    /// Publisher attribution.
    pub author: String,
    /// `onLanguage:id`, `onFileExtension:.ext`, `onCommand:id` or `*`.
    pub activation_events: Vec<String>,
    /// Declarative contributions registered before activation.
    #[serde(default)]
    pub contributes: Contributions,
    /// Required plugin IDs and semver requirements.
    #[serde(default)]
    pub dependencies: BTreeMap<String, VersionReq>,
    /// Optional executable, relative to the plugin directory.
    pub executable: Option<String>,
    /// Arguments passed directly, without a shell.
    #[serde(default)]
    pub args: Vec<String>,
}

/// Extension points available in this host version.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Contributions {
    /// File-language associations.
    pub languages: Vec<Language>,
    /// Search Everywhere actions.
    pub commands: Vec<Command>,
    /// Typed defaults exposed in the host snapshot.
    pub configuration: BTreeMap<String, Configuration>,
    /// Declarative tool tabs; generic panels show their description.
    pub tabs: Vec<Tab>,
}

/// A contributed language.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Language {
    /// Stable language ID.
    pub id: String,
    /// Dot-prefixed suffixes, matched case-insensitively.
    pub extensions: Vec<String>,
    /// Display aliases.
    #[serde(default)]
    pub aliases: Vec<String>,
}

/// A contributed command.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Command {
    /// Globally unique action ID.
    pub id: String,
    /// Label in the action palette.
    pub title: String,
    /// Supported conjunction of context keys, optionally negated.
    #[serde(default)]
    pub when: When,
}

/// Declarative tool tab metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tab {
    /// Globally unique tab ID.
    pub id: String,
    /// Tab label.
    pub title: String,
    /// Placeholder content for this contract version.
    pub description: String,
}

/// Type-checked configuration default.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Configuration {
    /// JSON type: string, boolean, integer, number, array or object.
    #[serde(rename = "type")]
    pub kind: String,
    /// Initial value, validated against `type`.
    pub default: Value,
    /// Help text.
    pub description: String,
}

impl PluginManifest {
    /// Parses and semantically validates the complete manifest.
    /// # Errors
    /// Returns invalid JSON, unknown fields or invalid contract values.
    pub fn parse(source: &str) -> Result<Self> {
        let manifest: Self = serde_json::from_str(source)?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Validates values, duplicate contributions, events and executable paths.
    /// # Errors
    /// Returns an invalid-contract error.
    pub fn validate(&self) -> Result<()> {
        let invalid = |message: &str| Error::Invalid(message.into());
        if !identifier(&self.id)
            || !self.id.contains('.')
            || self.name.trim().is_empty()
            || self.author.trim().is_empty()
        {
            return Err(invalid("plugin identity, name or author is invalid"));
        }
        if let Some(executable) = &self.executable {
            if executable.is_empty()
                || !Path::new(executable)
                    .components()
                    .all(|c| matches!(c, Component::Normal(_)))
                || executable.contains('\\')
                || executable.contains(':')
            {
                return Err(invalid(
                    "executable must be a relative path inside the plugin",
                ));
            }
        } else if !self.args.is_empty() {
            return Err(invalid("args require an executable"));
        }
        let mut commands = HashSet::new();
        for command in &self.contributes.commands {
            if !identifier(&command.id)
                || command.id.starts_with("ui.")
                || command.title.trim().is_empty()
                || !commands.insert(&command.id)
            {
                return Err(invalid("invalid or duplicate command"));
            }
            command.when.validate()?;
        }
        let mut languages = HashSet::new();
        let mut extensions = HashSet::new();
        for language in &self.contributes.languages {
            if !identifier(&language.id)
                || !languages.insert(&language.id)
                || language.extensions.is_empty()
            {
                return Err(invalid("invalid or duplicate language"));
            }
            for extension in &language.extensions {
                if !file_extension(extension) || !extensions.insert(extension.to_lowercase()) {
                    return Err(invalid("invalid or duplicate file extension"));
                }
            }
        }
        let mut tabs = HashSet::new();
        for tab in &self.contributes.tabs {
            if !identifier(&tab.id) || tab.title.trim().is_empty() || !tabs.insert(&tab.id) {
                return Err(invalid("invalid or duplicate tab"));
            }
        }
        for (key, config) in &self.contributes.configuration {
            let valid = match config.kind.as_str() {
                "string" => config.default.is_string(),
                "boolean" => config.default.is_boolean(),
                "integer" => config.default.is_i64() || config.default.is_u64(),
                "number" => config.default.is_number(),
                "array" => config.default.is_array(),
                "object" => config.default.is_object(),
                _ => false,
            };
            if !identifier(key) || !valid {
                return Err(invalid("invalid configuration type or default"));
            }
        }
        for event in &self.activation_events {
            if event == "*" {
                continue;
            }
            let valid = event.strip_prefix("onLanguage:").is_some_and(identifier)
                || event
                    .strip_prefix("onFileExtension:")
                    .is_some_and(file_extension)
                || event.strip_prefix("onCommand:").is_some_and(identifier);
            if !valid {
                return Err(invalid("unsupported activation event"));
            }
        }
        if self
            .dependencies
            .keys()
            .any(|id| !identifier(id) || id == &self.id)
        {
            return Err(invalid("invalid or self dependency"));
        }
        Ok(())
    }

    /// Checks the inclusive minimum IDE version, including prerelease ordering.
    #[must_use]
    pub fn compatible_with(&self, ide: &Version) -> bool {
        ide >= &self.min_ide_version
    }
}

pub(crate) fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.split('.').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        })
}

fn file_extension(value: &str) -> bool {
    value.strip_prefix('.').is_some_and(identifier)
}
