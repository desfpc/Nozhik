//! Shared language availability, extension recognition and user opt-in policy.
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, path::Path};

/// Stable language identity shared by settings and symbol indexes.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LanguageId {
    /// PHP, including PHP templates.
    #[serde(alias = "Php")]
    Php,
    /// JavaScript and TypeScript (reserved until their language service is available).
    #[serde(rename = "javascript", alias = "JavaScript", alias = "java_script")]
    JavaScript,
    /// HTML (reserved).
    Html,
    /// YAML (reserved).
    Yaml,
    /// Go (reserved).
    #[serde(alias = "Go")]
    Go,
}
/// One compiled language registration. Extensions have no leading dot.
pub struct LanguageSpec {
    /// Stable identity for persistence/cache partitions.
    pub id: LanguageId,
    /// User-visible name.
    pub label: &'static str,
    /// Recognized suffixes, compared without ASCII case.
    pub extensions: &'static [&'static str],
    /// Whether a language service/index extractor is implemented in this build.
    pub available: bool,
}
/// Shared PHP suffixes for registration and LSP contribution metadata.
pub const PHP_EXTENSIONS: &[&str] = &["php", "phtml"];
/// Enable a registration only when its parser/service is wired into the application.
pub const LANGUAGES: &[LanguageSpec] = &[
    LanguageSpec {
        id: LanguageId::Php,
        label: "PHP",
        extensions: PHP_EXTENSIONS,
        available: true,
    },
    LanguageSpec {
        id: LanguageId::JavaScript,
        label: "JavaScript / TypeScript",
        extensions: &["js", "jsx", "mjs", "cjs", "ts", "tsx", "mts", "cts"],
        available: false,
    },
    LanguageSpec {
        id: LanguageId::Html,
        label: "HTML",
        extensions: &["html", "htm"],
        available: false,
    },
    LanguageSpec {
        id: LanguageId::Yaml,
        label: "YAML",
        extensions: &["yaml", "yml"],
        available: false,
    },
    LanguageSpec {
        id: LanguageId::Go,
        label: "Go",
        extensions: &["go"],
        available: false,
    },
];
/// Defaults include all languages implemented in this build, preserving old settings.
#[must_use]
pub fn default_enabled() -> BTreeSet<LanguageId> {
    LANGUAGES
        .iter()
        .filter(|s| s.available)
        .map(|s| s.id)
        .collect()
}
/// Recognizes a registered extension without reading the filesystem.
#[must_use]
pub fn language_for_path(path: &Path) -> Option<LanguageId> {
    let extension = path.extension()?.to_str()?;
    LANGUAGES
        .iter()
        .find(|s| {
            s.extensions
                .iter()
                .any(|suffix| suffix.eq_ignore_ascii_case(extension))
        })
        .map(|s| s.id)
}
/// Unavailable registrations never start a worker even if listed in a settings file.
#[must_use]
pub fn enabled(enabled: &BTreeSet<LanguageId>, id: LanguageId) -> bool {
    enabled.contains(&id) && LANGUAGES.iter().any(|s| s.id == id && s.available)
}
/// Selects only implemented, user-enabled source types without file I/O.
#[must_use]
pub fn enabled_for_path(enabled_languages: &BTreeSet<LanguageId>, path: &Path) -> bool {
    language_for_path(path).is_some_and(|id| enabled(enabled_languages, id))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn availability_extensions_and_empty_selection() -> Result<(), Box<dyn std::error::Error>> {
        let defaults = default_enabled();
        for path in ["a.php", "a.PHP", "template.phtml", "template.PHTML"] {
            assert!(enabled_for_path(&defaults, Path::new(path)));
        }
        for path in [
            "a.html",
            "a.yaml",
            "a.js",
            "a.go",
            "a.png",
            "a.php.bak",
            "no_extension",
        ] {
            assert!(!enabled_for_path(&defaults, Path::new(path)));
        }
        assert!(!enabled_for_path(&BTreeSet::new(), Path::new("a.php")));
        assert!(
            !enabled_for_path(&BTreeSet::from([LanguageId::Go]), Path::new("a.go")),
            "reserved languages cannot be falsely activated"
        );
        assert_eq!(
            serde_json::from_str::<LanguageId>("\"Php\"")?,
            LanguageId::Php,
            "existing index caches remain readable"
        );
        Ok(())
    }
}
