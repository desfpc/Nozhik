//! Language-neutral documentation and future local-definition navigation contracts.
use crate::php_stubs::{PhpSymbol, PhpSymbolKind, PhpVersionStubs};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};

/// Language-neutral entity category used by catalogs, documentation and views.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    /// Global callable.
    Function,
    /// Class method.
    Method,
    /// Instantiable or abstract class.
    Class,
    /// Interface contract.
    Interface,
    /// Global or class constant.
    Constant,
    /// PHP syntax keyword.
    Keyword,
}

/// A navigation destination. Local positions are zero-based UTF-8 byte coordinates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DefinitionTarget {
    /// A project definition, supplied by a future project index.
    LocalFile {
        /// Native path to the source file.
        path: PathBuf,
        /// Zero-based line.
        line: u32,
        /// UTF-8 byte column.
        column: u32,
    },
    /// Standard-library documentation in the system browser.
    ExternalUrl(String),
}

/// Owned information for a resolved symbol; no UI or catalog lifetime dependency.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolDocInfo {
    /// Qualified canonical name.
    pub name: String,
    /// Entity category.
    pub kind: EntityKind,
    /// Typed callable signature or type/constant declaration.
    pub signature: String,
    /// Catalog documentation; may be empty when Reflection has no prose.
    pub description: String,
    /// Navigation target, when rules provide one.
    pub target: Option<DefinitionTarget>,
}

/// Shared documentation lookup contract. Implementations must not block on I/O.
pub trait LanguageDocProvider {
    /// Resolves an exact symbol in the active language catalog.
    fn resolve_symbol_info(&self, language_id: &str, symbol: &str) -> Option<SymbolDocInfo>;
    /// Resolves a URL only for a known entity of the requested category.
    fn resolve_doc_url(
        &self,
        language_id: &str,
        symbol: &str,
        entity_type: EntityKind,
    ) -> Option<String>;
}

/// A URL template with optional name transformations, independent of the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UrlRule {
    /// HTTPS template containing `{name}`.
    pub template: String,
    /// Lowercase canonical names before interpolation.
    #[serde(default)]
    pub lowercase: bool,
    /// Replace underscores with hyphens (e.g. PHP manual function paths).
    #[serde(default)]
    pub underscores_to_hyphens: bool,
}

/// One language's declarative documentation settings.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocRules {
    /// Language identifier, such as `php`.
    pub language_id: String,
    /// Human-readable destination used by the click hint.
    pub label: String,
    /// Templates by serialized entity category.
    pub rules: BTreeMap<String, UrlRule>,
    /// Universal alias used when a category has no dedicated template.
    pub fallback: Option<UrlRule>,
}
impl DocRules {
    /// Parses and validates templates. Loading belongs to the catalog worker.
    /// # Errors
    /// Rejects invalid JSON, missing placeholders or non-HTTPS templates.
    pub fn from_json(source: &str) -> Result<Self, crate::php_stubs::StubsError> {
        let rules: Self = serde_json::from_str(source)?;
        if rules.language_id.is_empty()
            || rules.label.is_empty()
            || rules.rules.values().chain(rules.fallback.iter()).any(|r| {
                !r.template.contains("{name}")
                    || crate::lsp_types::Url::parse(&r.template.replace("{name}", "symbol"))
                        .map_or(true, |u| u.scheme() != "https" || u.host_str().is_none())
            })
        {
            return Err(crate::php_stubs::StubsError::Invalid(
                "invalid documentation rules",
            ));
        }
        Ok(rules)
    }
    #[allow(
        clippy::literal_string_with_formatting_args,
        reason = "Declarative URL placeholders are not Rust formatting"
    )]
    fn url(&self, name: &str, kind: EntityKind) -> Option<String> {
        let key = match kind {
            EntityKind::Function => "function",
            EntityKind::Method => "method",
            EntityKind::Class => "class",
            EntityKind::Interface => "interface",
            EntityKind::Constant => "constant",
            EntityKind::Keyword => "keyword",
        };
        let rule = self.rules.get(key).or(self.fallback.as_ref())?;
        let mut name = if rule.lowercase {
            name.to_lowercase()
        } else {
            name.into()
        };
        if rule.underscores_to_hyphens {
            name = name.replace('_', "-");
        }
        // Encode path punctuation, namespace separators and non-ASCII bytes, keeping :: for PHP aliases.
        let encoded: String = name
            .bytes()
            .map(|b| {
                if b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b':') {
                    char::from(b).to_string()
                } else {
                    format!("%{b:02X}")
                }
            })
            .collect();
        Some(rule.template.replace("{name}", &encoded))
    }
}
impl PhpVersionStubs {
    fn doc_symbol(&self, name: &str) -> Option<(&PhpSymbol, String)> {
        let name = name.trim_start_matches('\\');
        let matches = |s: &&PhpSymbol, name: &str| {
            if s.kind == PhpSymbolKind::Constant {
                s.name == name
            } else {
                s.name.eq_ignore_ascii_case(name)
            }
        };
        if let Some((class, member)) = name.rsplit_once("::") {
            let class = self.entities.iter().find(|s| {
                matches!(s.kind, EntityKind::Class | EntityKind::Interface)
                    && s.name.eq_ignore_ascii_case(class)
            })?;
            let symbol = class
                .methods
                .iter()
                .chain(&class.constants)
                .find(|s| matches(s, member))?;
            Some((symbol, format!("{}::{}", class.name, symbol.name)))
        } else {
            let symbol = self.entities.iter().find(|s| matches(s, name))?;
            Some((symbol, symbol.name.clone()))
        }
    }
}
impl LanguageDocProvider for PhpVersionStubs {
    fn resolve_symbol_info(&self, language_id: &str, symbol: &str) -> Option<SymbolDocInfo> {
        if language_id != self.doc_rules.language_id {
            return None;
        }
        let (entity, name) = self.doc_symbol(symbol)?;
        if entity.kind == EntityKind::Keyword {
            return None;
        }
        let signature = if matches!(entity.kind, EntityKind::Function | EntityKind::Method) {
            let params: Vec<_> = entity
                .parameters
                .iter()
                .map(|p| {
                    format!(
                        "{} {}{}${}{}",
                        p.r#type,
                        if p.by_reference { "&" } else { "" },
                        if p.variadic { "..." } else { "" },
                        p.name,
                        p.default.as_ref().map_or_else(
                            || if p.optional {
                                " (optional)".into()
                            } else {
                                String::new()
                            },
                            |d| format!(" = {d}")
                        )
                    )
                })
                .collect();
            format!(
                "{name}({}): {}",
                params.join(", "),
                if entity.return_type.is_empty() {
                    "mixed"
                } else {
                    &entity.return_type
                }
            )
        } else if entity.kind == EntityKind::Constant {
            format!(
                "{} {name}{}",
                entity.r#type,
                entity
                    .value
                    .as_ref()
                    .map_or_else(String::new, |v| format!(" = {v}"))
            )
        } else {
            format!("{:?} {name}", entity.kind)
        };
        let target = self
            .doc_rules
            .url(&name, entity.kind)
            .map(DefinitionTarget::ExternalUrl);
        Some(SymbolDocInfo {
            name,
            kind: entity.kind,
            signature,
            description: entity.documentation.clone(),
            target,
        })
    }
    fn resolve_doc_url(
        &self,
        language_id: &str,
        symbol: &str,
        entity_type: EntityKind,
    ) -> Option<String> {
        let info = self.resolve_symbol_info(language_id, symbol)?;
        if info.kind != entity_type {
            return None;
        }
        match info.target? {
            DefinitionTarget::ExternalUrl(url) => Some(url),
            DefinitionTarget::LocalFile { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[allow(
        clippy::literal_string_with_formatting_args,
        reason = "Fixtures exercise URL template placeholders"
    )]
    fn exact_active_catalog_and_declarative_urls() -> Result<(), Box<dyn std::error::Error>> {
        let mut catalog = PhpVersionStubs::builtin()?;
        for (name, kind) in [
            ("var_dump", EntityKind::Function),
            ("DateTime", EntityKind::Class),
            ("PHP_VERSION", EntityKind::Constant),
            ("DateTime::format", EntityKind::Method),
        ] {
            assert_eq!(
                catalog.resolve_doc_url("php", name, kind),
                Some(format!("https://www.php.net/{name}"))
            );
        }
        assert!(catalog.resolve_symbol_info("rust", "var_dump").is_none());
        assert!(
            catalog
                .resolve_symbol_info("php", "not_a_builtin")
                .is_none()
        );
        assert!(catalog.resolve_symbol_info("php", "php_version").is_none());
        assert!(
            catalog
                .resolve_symbol_info("php", "\\DATETIME::FORMAT")
                .is_some()
        );
        assert!(
            catalog
                .resolve_symbol_info("php", "DateTime::not_a_method")
                .is_none()
        );
        assert!(
            catalog
                .resolve_doc_url("php", "var_dump", EntityKind::Class)
                .is_none()
        );
        catalog.doc_rules = DocRules::from_json(
            r#"{"language_id":"php","label":"php.net","rules":{"function":{"template":"https://www.php.net/manual/ru/function.{name}.php","lowercase":true,"underscores_to_hyphens":true}},"fallback":{"template":"https://www.php.net/{name}"}}"#,
        )?;
        assert_eq!(
            catalog
                .resolve_doc_url("php", "VAR_DUMP", EntityKind::Function)
                .as_deref(),
            Some("https://www.php.net/manual/ru/function.var-dump.php")
        );
        assert!(
            PhpVersionStubs::bundled("7.4")?
                .resolve_symbol_info("php", "str_contains")
                .is_none()
        );
        assert!(DocRules::from_json(r#"{"language_id":"php","label":"bad","rules":{},"fallback":{"template":"javascript:{name}"}}"#).is_err());
        Ok(())
    }
    #[test]
    #[allow(
        clippy::literal_string_with_formatting_args,
        reason = "The fixture supplies a declarative template"
    )]
    fn worker_rules_override_and_invalid_file_preservation()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("versions");
        std::fs::create_dir(&root)?;
        let path = temp.path().join("doc_rules.json");
        let mut rules = PhpVersionStubs::builtin()?.doc_rules;
        rules.fallback.as_mut().ok_or("fallback")?.template =
            "https://example.org/docs/{name}".into();
        std::fs::write(&path, serde_json::to_vec(&rules)?)?;
        let catalog = PhpVersionStubs::load(&root, "8.5")?;
        assert_eq!(
            catalog
                .resolve_doc_url("php", "DateTime::ATOM", EntityKind::Constant)
                .as_deref(),
            Some("https://example.org/docs/DateTime::ATOM")
        );
        assert_eq!(
            catalog
                .resolve_doc_url("php", "Countable", EntityKind::Interface)
                .as_deref(),
            Some("https://example.org/docs/Countable")
        );
        std::fs::write(&path, "{broken")?;
        assert!(PhpVersionStubs::load(&root, "8.5").is_err());
        assert_eq!(std::fs::read_to_string(&path)?, "{broken");
        assert!(catalog.resolve_symbol_info("php", "var_dump").is_some());
        Ok(())
    }
}
