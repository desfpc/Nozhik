//! Filename discovery and syntax-based editor run markers, without a semantic index.
use std::path::{Path, PathBuf};
use tree_sitter::Node;

/// A run action on a zero-based editor line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunMarker {
    /// Source line.
    pub line: usize,
    /// None starts all services; Some starts the named service.
    pub service: Option<String>,
}
/// Recognizes canonical Compose names and dotted variants, case-insensitively.
#[must_use]
pub fn is_compose_file(path: &Path) -> bool {
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    let Some(stem) = name
        .strip_suffix(".yaml")
        .or_else(|| name.strip_suffix(".yml"))
    else {
        return false;
    };
    ["compose", "docker-compose"].iter().any(|prefix| {
        stem == *prefix
            || stem
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.starts_with('.') && rest.len() > 1)
    })
}
pub(crate) fn discover(root: &Path) -> crate::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in ignore::WalkBuilder::new(root)
        .hidden(false)
        .ignore(false)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .follow_links(false)
        .filter_entry(|entry| {
            !entry.file_type().is_some_and(|t| t.is_dir())
                || !matches!(
                    entry.file_name().to_str(),
                    Some(".git" | ".nozhik" | "target" | "node_modules" | "vendor")
                )
        })
        .build()
    {
        let entry = entry.map_err(|e| crate::Error::Message(e.to_string()))?;
        if entry.file_type().is_some_and(|t| t.is_file()) && is_compose_file(entry.path()) {
            files.push(entry.into_path());
        }
    }
    files.sort();
    Ok(files)
}
/// Selects the base plus conventional override, or base plus explicitly selected variant.
/// Paths come from the discovered project file inventory.
#[must_use]
pub fn configuration(selected: &Path, files: &[PathBuf]) -> Vec<PathBuf> {
    let directory = selected.parent().unwrap_or_else(|| Path::new("."));
    let find = |names: &[&str]| {
        names.iter().find_map(|name| {
            files
                .iter()
                .find(|p| {
                    p.parent() == Some(directory)
                        && p.file_name()
                            .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(name))
                })
                .cloned()
        })
    };
    let base = find(&[
        "compose.yaml",
        "compose.yml",
        "docker-compose.yaml",
        "docker-compose.yml",
    ]);
    let Some(base) = base else {
        return vec![selected.into()];
    };
    if base == selected {
        let modern = base.file_name().is_some_and(|n| {
            n.to_string_lossy()
                .to_ascii_lowercase()
                .starts_with("compose.")
        });
        let override_file = if modern {
            find(&["compose.override.yaml", "compose.override.yml"])
        } else {
            find(&[
                "docker-compose.override.yaml",
                "docker-compose.override.yml",
            ])
        };
        let mut result = vec![base];
        result.extend(override_file);
        result
    } else {
        vec![base, selected.into()]
    }
}
fn mapping(node: Node<'_>) -> Option<Node<'_>> {
    if node.kind() == "block_mapping" {
        return Some(node);
    }
    if matches!(
        node.kind(),
        "flow_node" | "block_node" | "document" | "stream"
    ) {
        let mut cursor = node.walk();
        return node.named_children(&mut cursor).find_map(mapping);
    }
    None
}
fn key_text(node: Node<'_>, source: &str) -> String {
    let raw = source[node.byte_range()].trim();
    if raw.starts_with('"') {
        serde_json::from_str(raw).unwrap_or_else(|_| raw.into())
    } else {
        raw.strip_prefix('\'')
            .and_then(|s| s.strip_suffix('\''))
            .map_or_else(|| raw.into(), |s| s.replace("''", "'"))
    }
}
/// Extracts services and their source lines using YAML syntax, never text matching.
/// Invalid documents produce no actionable markers.
/// # Errors
/// Returns grammar initialization or parse cancellation errors.
pub fn run_markers(source: &str) -> crate::Result<Vec<RunMarker>> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_yaml::LANGUAGE.into())
        .map_err(|e| crate::Error::Message(e.to_string()))?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| crate::Error::Message("YAML parse cancelled".into()))?;
    if tree.root_node().has_error() {
        return Ok(Vec::new());
    }
    let Some(root) = mapping(tree.root_node()) else {
        return Ok(Vec::new());
    };
    let mut result = Vec::new();
    let mut cursor = root.walk();
    for pair in root.named_children(&mut cursor) {
        if pair.kind() != "block_mapping_pair" {
            continue;
        }
        let Some(key) = pair.child_by_field_name("key") else {
            continue;
        };
        if key_text(key, source) != "services" {
            continue;
        }
        result.push(RunMarker {
            line: key.start_position().row,
            service: None,
        });
        if let Some(services) = pair.child_by_field_name("value").and_then(mapping) {
            let mut cursor = services.walk();
            for service in services.named_children(&mut cursor) {
                if service.kind() != "block_mapping_pair" {
                    continue;
                }
                if let Some(key) = service.child_by_field_name("key") {
                    let name = key_text(key, source);
                    if valid_service(&name) {
                        result.push(RunMarker {
                            line: key.start_position().row,
                            service: Some(name),
                        });
                    }
                }
            }
        }
    }
    Ok(result)
}
pub(crate) fn valid_service(name: &str) -> bool {
    name.bytes()
        .next()
        .is_some_and(|b| b.is_ascii_alphanumeric())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}
