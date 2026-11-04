//! Declarative formatting profiles. Directory scanning belongs on a worker.
use crate::ConfigError;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, path::Path};

/// Profile indentation mode.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndentStyle {
    /// Spaces.
    Spaces,
    /// Tabs.
    Tabs,
}
/// Opening brace placement.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BracePosition {
    /// After the declaration.
    NextLine,
    /// Beside the declaration.
    SameLine,
}
/// Indentation measured in logical columns.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Indent {
    /// Indent characters.
    pub style: IndentStyle,
    /// Columns per level (1..=16).
    pub size: u8,
}
/// Brace policy by AST declaration kind.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Braces {
    /// Classes, interfaces and traits.
    pub classes: BracePosition,
    /// Functions and methods.
    pub functions: BracePosition,
    /// If/for/while and other control blocks.
    pub control_structures: BracePosition,
}
/// Token spacing policy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spacing {
    /// Binary operators.
    pub around_operators: bool,
    /// Comma separators.
    pub after_comma: bool,
    /// Nonempty parentheses.
    pub inside_parentheses: bool,
}
/// Blank lines between declarations.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmptyLines {
    /// After a namespace statement.
    pub after_namespace: u8,
    /// Between methods.
    pub between_methods: u8,
}
/// A complete, validated JSON formatting standard.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodeStyle {
    /// LSP language identifier.
    pub language: String,
    /// Display name.
    pub name: String,
    /// Indentation.
    pub indent: Indent,
    /// Braces.
    pub braces: Braces,
    /// Spacing.
    pub spacing: Spacing,
    /// Blank lines.
    pub empty_lines: EmptyLines,
}
impl CodeStyle {
    /// Parses and validates a complete profile without modifying its source.
    /// # Errors
    /// Returns JSON or semantic validation errors.
    pub fn parse(source: &str) -> Result<Self, ConfigError> {
        let style: Self = serde_json::from_str(source)?;
        if style.language.is_empty()
            || style.name.trim().is_empty()
            || !(1..=16).contains(&style.indent.size)
            || style.empty_lines.after_namespace > 8
            || style.empty_lines.between_methods > 8
        {
            return Err(ConfigError::Invalid("invalid code style"));
        }
        Ok(style)
    }
    /// One indentation level, as configured by the profile.
    #[must_use]
    pub fn unit(&self) -> String {
        match self.indent.style {
            IndentStyle::Spaces => " ".repeat(usize::from(self.indent.size)),
            IndentStyle::Tabs => "\t".into(),
        }
    }
}
/// Per-extension selections; `None` explicitly disables formatting.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FileTypeSettings {
    /// Stable profile filename, without a directory.
    pub formatting: Option<String>,
    /// Syntax palette identifier.
    pub syntax_theme: String,
}
impl Default for FileTypeSettings {
    fn default() -> Self {
        Self {
            formatting: None,
            syntax_theme: "default".into(),
        }
    }
}
/// Profiles keyed by extension and stable filename. No implicit fallback on missing files.
#[derive(Clone, Debug, Default)]
pub struct CodeStyles(pub BTreeMap<String, BTreeMap<String, CodeStyle>>);
impl CodeStyles {
    /// Includes the shipped PHP, CSS and YAML profiles, without touching the user filesystem.
    /// # Errors
    /// Returns a malformed embedded profile error.
    pub fn builtin() -> Result<Self, ConfigError> {
        let mut catalog = Self::default();
        let php = CodeStyle::parse(include_str!("../code_styles/php/php_psr12.json"))?;
        for extension in ["php", "phtml"] {
            catalog
                .0
                .entry(extension.into())
                .or_default()
                .insert("php_psr12.json".into(), php.clone());
        }
        catalog.0.entry("css".into()).or_default().insert(
            "css_default.json".into(),
            CodeStyle::parse(include_str!("../code_styles/css/css_default.json"))?,
        );
        catalog.0.entry("json".into()).or_default().insert(
            "json_default.json".into(),
            CodeStyle::parse(include_str!("../code_styles/json/json_default.json"))?,
        );
        let yaml = CodeStyle::parse(include_str!("../code_styles/yaml/yaml_default.json"))?;
        for extension in ["yaml", "yml"] {
            catalog
                .0
                .entry(extension.into())
                .or_default()
                .insert("yaml_default.json".into(), yaml.clone());
        }
        Ok(catalog)
    }
    /// Loads `<root>/<extension>/*.json`; later roots override matching filenames.
    /// Missing directories are allowed. Malformed profiles are reported, never overwritten.
    /// # Errors
    /// Returns I/O, JSON or validation errors.
    pub fn scan(&mut self, root: &Path) -> Result<(), ConfigError> {
        let entries = match fs::read_dir(root) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.into()),
        };
        for entry in entries {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let extension = entry.file_name().to_string_lossy().to_lowercase();
            for file in fs::read_dir(entry.path())? {
                let file = file?;
                if file.path().extension().is_some_and(|e| e == "json") {
                    let style = CodeStyle::parse(&fs::read_to_string(file.path())?)?;
                    self.0
                        .entry(extension.clone())
                        .or_default()
                        .insert(file.file_name().to_string_lossy().into_owned(), style);
                }
            }
        }
        Ok(())
    }
    /// Resolves an explicit selection, or the shipped language default for an unset extension.
    #[must_use]
    pub fn selected(
        &self,
        extension: &str,
        settings: &BTreeMap<String, FileTypeSettings>,
    ) -> Option<&CodeStyle> {
        let extension = extension.to_lowercase();
        let name = match settings.get(&extension) {
            Some(s) => s.formatting.as_deref()?,
            None => default_profile(&extension)?,
        };
        self.0.get(&extension)?.get(name)
    }
}

/// Built-in formatting selection for a newly supported file type.
#[must_use]
pub fn default_profile(extension: &str) -> Option<&'static str> {
    match extension {
        "php" | "phtml" => Some("php_psr12.json"),
        "json" => Some("json_default.json"),
        "css" => Some("css_default.json"),
        "yaml" | "yml" => Some("yaml_default.json"),
        _ => None,
    }
}

/// Configuration search locations, ordered from user home to platform config.
#[must_use]
pub fn search_roots(settings_path: &Path) -> Vec<std::path::PathBuf> {
    let mut roots = Vec::new();
    if let Some(base) = directories::BaseDirs::new() {
        roots.push(base.home_dir().join(".nozhik/code_styles"));
    }
    if let Some(parent) = settings_path.parent() {
        roots.push(parent.join("code_styles"));
    }
    roots
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn disk_profiles_and_none_selection_persist() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        fs::create_dir(dir.path().join("php"))?;
        fs::write(
            dir.path().join("php/custom.json"),
            include_str!("../code_styles/php/php_psr12.json").replace("PSR-12", "Team"),
        )?;
        let mut styles = CodeStyles::builtin()?;
        styles.scan(dir.path())?;
        let mut config = crate::Config::default();
        config.file_types.insert(
            "php".into(),
            FileTypeSettings {
                formatting: Some("custom.json".into()),
                syntax_theme: "muted".into(),
            },
        );
        assert_eq!(
            styles
                .selected("php", &config.file_types)
                .map(|s| s.name.as_str()),
            Some("Team")
        );
        let path = dir.path().join("settings.toml");
        let manager =
            crate::ConfigManager::from_path(path.clone(), nozhik_event_bus::EventBus::new(8)?)?;
        manager.replace(config.clone())?;
        manager.save()?;
        let loaded = crate::ConfigManager::from_path(path, nozhik_event_bus::EventBus::new(8)?)?;
        assert_eq!(loaded.snapshot()?, config);
        config
            .file_types
            .insert("php".into(), FileTypeSettings::default());
        let encoded = toml::to_string(&config)?;
        let decoded: crate::Config = toml::from_str(&encoded)?;
        assert!(styles.selected("php", &decoded.file_types).is_none());
        fs::remove_file(dir.path().join("php/custom.json"))?;
        let mut rescanned = CodeStyles::builtin()?;
        rescanned.scan(dir.path())?;
        assert!(
            rescanned
                .selected("php", &loaded.snapshot()?.file_types)
                .is_none()
        );
        Ok(())
    }

    #[test]
    fn profiles_and_plain_text() -> Result<(), Box<dyn std::error::Error>> {
        let mut styles = CodeStyles::builtin()?;
        let mut settings = BTreeMap::new();
        assert!(styles.selected("txt", &settings).is_none());
        assert_eq!(
            styles.selected("php", &settings).map(|s| s.indent.size),
            Some(4)
        );
        settings.insert("php".into(), FileTypeSettings::default());
        assert!(styles.selected("php", &settings).is_none());
        let dir = tempfile::tempdir()?;
        fs::create_dir(dir.path().join("php"))?;
        fs::write(dir.path().join("php/broken.json"), "{")?;
        assert!(styles.scan(dir.path()).is_err());
        assert_eq!(fs::read_to_string(dir.path().join("php/broken.json"))?, "{");
        assert!(
            CodeStyle::parse(&include_str!("../code_styles/php/php_psr12.json").replace('4', "0"))
                .is_err()
        );
        Ok(())
    }
}
