//! Versioned offline PHP catalog. Disk loading belongs to a background worker.
use lsp_types::{CompletionItem, CompletionItemKind, CompletionItemLabelDetails, Documentation};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, path::Path};

/// Language levels accepted by settings and catalog paths.
pub const VERSIONS: &[&str] = &[
    "7.0", "7.1", "7.2", "7.3", "7.4", "8.0", "8.1", "8.2", "8.3", "8.4", "8.5",
];
/// The generated PHP 8.5 standard CLI runtime inventory.
pub const BUILTIN: &str = include_str!("../data/php_8.5.json");

/// Category used by both the JSON catalog and completion icons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PhpSymbolKind {
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

/// One callable parameter; default is PHP source text, not a JSON value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhpParameter {
    /// Parameter name without `$`.
    pub name: String,
    /// PHP type expression, or `mixed` when unavailable.
    pub r#type: String,
    /// Whether the argument may be omitted.
    #[serde(default)]
    pub optional: bool,
    /// Whether this is a variadic argument.
    #[serde(default)]
    pub variadic: bool,
    /// Whether this argument is passed by reference.
    #[serde(default)]
    pub by_reference: bool,
    /// PHP representation of the default value.
    #[serde(default)]
    pub default: Option<String>,
}

/// Uniform catalog entity. Classes/interfaces own their methods and constants.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhpSymbol {
    /// Entity category.
    pub kind: PhpSymbolKind,
    /// Fully qualified name without a leading namespace separator.
    pub name: String,
    /// Short documentation, when available.
    #[serde(default)]
    pub documentation: String,
    /// Extension or package name.
    #[serde(default)]
    pub package: String,
    /// Callable parameters in declaration order.
    #[serde(default)]
    pub parameters: Vec<PhpParameter>,
    /// Callable result type (including unions/intersections).
    #[serde(default)]
    pub return_type: String,
    /// Constant type.
    #[serde(default)]
    pub r#type: String,
    /// Constant value as PHP source text; platform-dependent values may be symbolic.
    #[serde(default)]
    pub value: Option<String>,
    /// Plain insertion text; `${0}` marks the final caret position.
    #[serde(default)]
    pub snippet: String,
    /// Public methods, including inherited declarations.
    #[serde(default)]
    pub methods: Vec<Self>,
    /// Public constants, including inherited declarations.
    #[serde(default)]
    pub constants: Vec<Self>,
    /// Whether a method is static.
    #[serde(default)]
    pub is_static: bool,
}

/// Portable JSON document for one PHP major.minor level.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhpVersionStubs {
    /// Schema revision; currently 1.
    pub schema_version: u32,
    /// PHP major.minor version.
    pub version: String,
    /// Loaded extension inventory in the generating runtime.
    #[serde(default)]
    pub extensions: Vec<String>,
    /// Top-level functions, types, constants and keywords.
    pub entities: Vec<PhpSymbol>,
}

/// Invalid, unreadable or mismatched catalogs never overwrite a user file.
#[derive(Debug, thiserror::Error)]
pub enum StubsError {
    /// Filesystem failure other than a missing version.
    #[error("PHP catalog I/O: {0}")]
    Io(#[from] std::io::Error),
    /// Malformed schema.
    #[error("PHP catalog JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// Invalid catalog metadata or entities.
    #[error("PHP catalog: {0}")]
    Invalid(&'static str),
}
impl PhpVersionStubs {
    /// Parses and validates a catalog.
    /// # Errors
    /// Rejects unsupported schema/version, empty names, duplicates or misplaced members.
    pub fn from_json(source: &str) -> Result<Self, StubsError> {
        let catalog: Self = serde_json::from_str(source)?;
        if catalog.schema_version != 1 || !VERSIONS.contains(&catalog.version.as_str()) {
            return Err(StubsError::Invalid("unsupported schema or language level"));
        }
        validate_symbols(&catalog.entities, None)?;
        Ok(catalog)
    }
    /// Parses the bundled PHP 8.5 runtime catalog.
    /// # Errors
    /// Returns an error if the shipped preset violates the schema.
    pub fn builtin() -> Result<Self, StubsError> {
        Self::from_json(BUILTIN)
    }
    /// Parses the bundled runtime catalog for a supported major.minor level.
    /// # Errors
    /// Rejects unsupported versions or invalid bundled JSON.
    pub fn bundled(version: &str) -> Result<Self, StubsError> {
        let source = match version {
            "7.0" => include_str!("../data/php_7.0.json"),
            "7.1" => include_str!("../data/php_7.1.json"),
            "7.2" => include_str!("../data/php_7.2.json"),
            "7.3" => include_str!("../data/php_7.3.json"),
            "7.4" => include_str!("../data/php_7.4.json"),
            "8.0" => include_str!("../data/php_8.0.json"),
            "8.1" => include_str!("../data/php_8.1.json"),
            "8.2" => include_str!("../data/php_8.2.json"),
            "8.3" => include_str!("../data/php_8.3.json"),
            "8.4" => include_str!("../data/php_8.4.json"),
            "8.5" => BUILTIN,
            _ => return Err(StubsError::Invalid("unsupported language level")),
        };
        Self::from_json(source)
    }
    /// Loads `php_<version>.json` from the supplied versions directory.
    /// A missing file uses the bundled catalog of the same level. Call on a worker.
    /// # Errors
    /// Returns read/parse/validation errors, including mismatched file metadata.
    pub fn load(root: &Path, version: &str) -> Result<Self, StubsError> {
        if !VERSIONS.contains(&version) {
            return Err(StubsError::Invalid("unsupported language level"));
        }
        match std::fs::read_to_string(root.join(format!("php_{version}.json"))) {
            Ok(source) => {
                let catalog = Self::from_json(&source)?;
                if catalog.version != version {
                    return Err(StubsError::Invalid("version does not match filename"));
                }
                Ok(catalog)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::bundled(version),
            Err(e) => Err(e.into()),
        }
    }
    /// Searches global names or explicit `Class::member` prefixes without an LSP.
    #[must_use]
    pub fn search(&self, prefix: &str) -> Vec<CompletionItem> {
        let prefix = prefix.trim_start_matches('\\');
        if prefix.starts_with('$') {
            return Vec::new();
        }
        let lowercase = prefix.to_lowercase();
        if let Some((class, member)) = prefix.rsplit_once("::") {
            self.entities
                .iter()
                .find(|s| s.name.eq_ignore_ascii_case(class))
                .map_or_else(Vec::new, |s| {
                    crate::completion::filter(
                        s.methods
                            .iter()
                            .chain(&s.constants)
                            .map(|symbol| symbol.matching_completion(member))
                            .collect(),
                        member,
                    )
                })
        } else {
            crate::completion::filter(
                self.entities
                    .iter()
                    .filter(|symbol| symbol.name.to_lowercase().starts_with(&lowercase))
                    .map(|symbol| symbol.matching_completion(prefix))
                    .collect(),
                prefix,
            )
        }
    }
}
fn validate_symbols(
    symbols: &[PhpSymbol],
    member: Option<PhpSymbolKind>,
) -> Result<(), StubsError> {
    let mut names = HashSet::new();
    for symbol in symbols {
        let key = if symbol.kind == PhpSymbolKind::Constant {
            symbol.name.clone()
        } else {
            symbol.name.to_lowercase()
        };
        if symbol.name.is_empty() || !names.insert((symbol.kind as u8, key)) {
            return Err(StubsError::Invalid("empty or duplicate symbol name"));
        }
        if member.is_some_and(|kind| kind != symbol.kind)
            || (member.is_none() && symbol.kind == PhpSymbolKind::Method)
            || (!matches!(symbol.kind, PhpSymbolKind::Class | PhpSymbolKind::Interface)
                && (!symbol.methods.is_empty() || !symbol.constants.is_empty()))
        {
            return Err(StubsError::Invalid("invalid member category"));
        }
        if symbol.snippet.matches("${0}").count() > 1
            || symbol
                .parameters
                .iter()
                .any(|p| p.name.is_empty() || p.r#type.is_empty())
        {
            return Err(StubsError::Invalid("invalid snippet or parameter"));
        }
        validate_symbols(&symbol.methods, Some(PhpSymbolKind::Method))?;
        validate_symbols(&symbol.constants, Some(PhpSymbolKind::Constant))?;
    }
    Ok(())
}
impl PhpSymbol {
    fn matching_completion(&self, prefix: &str) -> CompletionItem {
        let mut item = self.completion();
        // Preserve useful lowercase function matches before the many uppercase
        // STREAM_* constants when a short prefix hits the 100-result limit.
        item.sort_text = Some(format!(
            "{}{}",
            u8::from(!self.name.starts_with(prefix)),
            self.name.to_lowercase()
        ));
        item
    }
    fn completion(&self) -> CompletionItem {
        let callable = matches!(self.kind, PhpSymbolKind::Function | PhpSymbolKind::Method);
        let detail = if callable {
            let parameters: Vec<_> = self
                .parameters
                .iter()
                .map(|p| {
                    let value = format!(
                        "{}{}{}: {}{}",
                        if p.variadic { "..." } else { "" },
                        if p.by_reference { "&" } else { "" },
                        p.name,
                        p.r#type,
                        p.default
                            .as_ref()
                            .map_or_else(String::new, |v| format!(" = {v}"))
                    );
                    if p.optional {
                        format!("[{value}]")
                    } else {
                        value
                    }
                })
                .collect();
            format!("({})", parameters.join(", "))
        } else if self.kind == PhpSymbolKind::Constant {
            self.value
                .as_ref()
                .map_or_else(|| self.r#type.clone(), |v| format!(" = {v}"))
        } else if self.package.is_empty() {
            String::new()
        } else {
            format!(" [{}]", self.package)
        };
        let snippet = if self.snippet.is_empty() {
            if callable {
                format!("{}(${{0}})", self.name)
            } else {
                self.name.clone()
            }
        } else {
            self.snippet.clone()
        };
        let caret = snippet.find("${0}");
        CompletionItem {
            label: self.name.clone(),
            kind: Some(match self.kind {
                PhpSymbolKind::Function => CompletionItemKind::FUNCTION,
                PhpSymbolKind::Method => CompletionItemKind::METHOD,
                PhpSymbolKind::Class => CompletionItemKind::CLASS,
                PhpSymbolKind::Interface => CompletionItemKind::INTERFACE,
                PhpSymbolKind::Constant => CompletionItemKind::CONSTANT,
                PhpSymbolKind::Keyword => CompletionItemKind::KEYWORD,
            }),
            label_details: Some(CompletionItemLabelDetails {
                detail: Some(detail),
                description: Some(self.return_type.clone()),
            }),
            documentation: Some(Documentation::String(self.documentation.clone())),
            insert_text: Some(snippet.replace("${0}", "")),
            data: Some(serde_json::json!({"nozhik_php": true, "caret": caret})),
            ..CompletionItem::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preset_roundtrip_and_prefix_search() -> Result<(), Box<dyn std::error::Error>> {
        let catalog = PhpVersionStubs::builtin()?;
        assert!(catalog.entities.len() > 2500);
        assert_eq!(
            catalog,
            PhpVersionStubs::from_json(&serde_json::to_string(&catalog)?)?
        );
        let names: Vec<_> = catalog.search("str").into_iter().map(|s| s.label).collect();
        assert!(names.contains(&"strlen".into()) && names.contains(&"str_contains".into()));
        let names: Vec<_> = catalog
            .search("PHP_")
            .into_iter()
            .map(|s| s.label)
            .collect();
        assert!(names.contains(&"PHP_VERSION".into()) && names.contains(&"PHP_EOL".into()));
        assert_eq!(catalog.search("$str"), []);
        assert!(
            catalog
                .search("DateTime::fo")
                .iter()
                .any(|s| s.label == "format")
        );
        Ok(())
    }
    #[test]
    fn every_bundled_level_matches_its_runtime() -> Result<(), Box<dyn std::error::Error>> {
        for version in VERSIONS {
            let catalog = PhpVersionStubs::bundled(version)?;
            assert_eq!(&catalog.version, version);
            assert!(catalog.entities.len() > 2500);
            assert!(
                catalog
                    .entities
                    .iter()
                    .any(|s| s.kind == PhpSymbolKind::Function && s.name == "strlen")
            );
            let level: Vec<_> = version
                .split('.')
                .map(str::parse::<u8>)
                .collect::<Result<_, _>>()?;
            for (name, since) in [
                ("str_contains", [8, 0]),
                ("array_is_list", [8, 1]),
                ("json_validate", [8, 3]),
                ("array_find", [8, 4]),
                ("array_first", [8, 5]),
            ] {
                assert_eq!(
                    catalog
                        .entities
                        .iter()
                        .any(|s| s.kind == PhpSymbolKind::Function && s.name == name),
                    level.as_slice() >= since.as_slice(),
                    "{name} in {version}"
                );
            }
            for (name, since) in [("fn", [7, 4]), ("match", [8, 0]), ("readonly", [8, 1])] {
                assert_eq!(
                    catalog
                        .entities
                        .iter()
                        .any(|s| s.kind == PhpSymbolKind::Keyword && s.name == name),
                    level.as_slice() >= since.as_slice(),
                    "{name} in {version}"
                );
            }
            assert_eq!(
                catalog.entities.iter().any(|s| s.name == "UnitEnum"),
                level.as_slice() >= [8, 1].as_slice()
            );
            let exception = catalog
                .entities
                .iter()
                .find(|s| s.name == "Exception")
                .ok_or("Exception")?;
            assert!(exception.methods.iter().any(|m| m.name == "getMessage"));
            let constants = catalog.search("PHP_");
            assert!(constants.iter().any(|s| s.label == "PHP_VERSION"));
        }
        Ok(())
    }

    #[test]
    fn disk_versions_and_errors() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        assert_eq!(PhpVersionStubs::load(root.path(), "7.0")?.version, "7.0");
        let source = BUILTIN.replace("\"8.5\"", "\"8.4\"");
        std::fs::write(root.path().join("php_8.4.json"), source)?;
        assert_eq!(PhpVersionStubs::load(root.path(), "8.4")?.version, "8.4");
        assert_eq!(
            PhpVersionStubs::load(root.path(), "8.4")?.entities,
            PhpVersionStubs::builtin()?.entities,
            "User catalogs take priority over the bundled version"
        );
        std::fs::write(root.path().join("php_8.4.json"), "{}")?;
        assert!(PhpVersionStubs::load(root.path(), "8.4").is_err());
        assert!(PhpVersionStubs::load(root.path(), "../../bad").is_err());
        std::fs::write(root.path().join("php_8.4.json"), BUILTIN)?;
        assert!(PhpVersionStubs::load(root.path(), "8.4").is_err());
        Ok(())
    }
}
