//! PHP lexical scopes, class-name resolution and context-aware completion.
use crate::{
    symbol_index::{SymbolIndex, SymbolKind, Visibility},
    triggers::LanguageTriggerConfig,
};
use std::path::Path;

/// Qualifies a PHP name in the current namespace, preserving an absolute prefix.
#[must_use]
pub fn qualify(name: &str, namespace: &str) -> String {
    if name.starts_with('\\') {
        name.into()
    } else if namespace.is_empty() {
        format!("\\{name}")
    } else {
        format!("\\{namespace}\\{name}")
    }
}
/// Resolves a simple receiver from indexed variables, parameters or `$this`.
#[must_use]
pub fn receiver_type(
    index: &SymbolIndex,
    path: &Path,
    byte: usize,
    receiver: &str,
) -> Option<String> {
    let receiver = receiver
        .rsplit(|c: char| !(c.is_alphanumeric() || matches!(c, '_' | '$' | '\\')))
        .next()?;
    let declarations = index.declarations_by_file.get(path)?;
    if receiver == "parent" {
        return receiver_type(index, path, byte, "self")
            .and_then(|owner| index.declaration(&owner))
            .and_then(|d| d.base_types.first())
            .cloned();
    }
    if receiver == "$this" || matches!(receiver, "self" | "static") {
        return declarations
            .iter()
            .filter(|d| d.kind == SymbolKind::Class && d.scope.contains(&byte))
            .min_by_key(|d| d.scope.len())
            .map(|d| d.fqn.clone());
    }
    if receiver.starts_with('$') {
        return declarations
            .iter()
            .filter(|d| d.name == receiver && d.scope.contains(&byte) && d.range.start < byte)
            .max_by_key(|d| d.range.start)
            .and_then(|d| {
                d.type_name
                    .clone()
                    .or_else(|| index.php_type(d.type_source.as_ref()?))
            });
    }
    let context = index.file_symbols(path).and_then(|f| {
        f.name_scopes
            .iter()
            .filter(|s| s.range.contains(&byte))
            .max_by_key(|s| s.range.start)
    });
    let (first, rest) = receiver.split_once('\\').unwrap_or((receiver, ""));
    if let Some(alias) = context.and_then(|c| c.aliases.get(&first.to_lowercase())) {
        return Some(if rest.is_empty() {
            alias.clone()
        } else {
            format!("{alias}\\{rest}")
        });
    }
    Some(qualify(receiver, context.map_or("", |c| c.namespace.as_str())).to_lowercase())
}
/// Local/project results; callers append standard-library results after these.
#[must_use]
pub fn complete(
    index: &SymbolIndex,
    path: &Path,
    source: &str,
    byte: usize,
) -> Vec<crate::lsp_types::CompletionItem> {
    complete_fragment(index, path, source, byte, byte)
}
/// Completes from a single source line while retaining absolute lexical-scope coordinates.
#[must_use]
pub fn complete_fragment(
    index: &SymbolIndex,
    path: &Path,
    source: &str,
    byte: usize,
    absolute_byte: usize,
) -> Vec<crate::lsp_types::CompletionItem> {
    let Some(before) = source.get(..byte) else {
        return Vec::new();
    };
    let config = LanguageTriggerConfig::php();
    let access = config.access(before);
    let target = access.and_then(|(left, _, _)| receiver_type(index, path, absolute_byte, left));
    let current = receiver_type(index, path, absolute_byte, "$this");
    let target_lineage = target
        .as_ref()
        .map_or_else(Vec::new, |name| index.class_lineage(name));
    let current_lineage = current
        .as_ref()
        .map_or_else(Vec::new, |name| index.class_lineage(name));
    let prefix = access.map_or_else(
        || &source[crate::completion::prefix(source, byte)],
        |(_, prefix, _)| prefix,
    );
    let mut symbols: Vec<_> = index
        .declarations()
        .filter(|d| {
            if let Some((_, _, static_access)) = access {
                d.owner
                    .as_ref()
                    .is_some_and(|owner| target_lineage.contains(owner))
                    && (d.visibility == Visibility::Public
                        || d.owner == current
                        || (d.visibility == Visibility::Protected
                            && d.owner
                                .as_ref()
                                .is_some_and(|owner| current_lineage.contains(owner))))
                    && target
                        .as_ref()
                        .and_then(|owner| {
                            index.resolve(&format!(
                                "{owner}::{}",
                                d.fqn.rsplit_once("::").map_or("", |(_, member)| member)
                            ))
                        })
                        .is_some_and(|resolved| resolved.fqn == d.fqn)
                    && if static_access {
                        d.is_static || d.kind == SymbolKind::Constant
                    } else {
                        !d.is_static && matches!(d.kind, SymbolKind::Method | SymbolKind::Property)
                    }
            } else {
                matches!(d.kind, SymbolKind::Class | SymbolKind::Function)
                    || (d.kind == SymbolKind::Constant && d.owner.is_none())
                    || (prefix == "$"
                        && d.kind == SymbolKind::Property
                        && d.owner.is_some()
                        && d.owner == current
                        && !d.is_static)
                    || (d.kind == SymbolKind::Variable
                        && d.path == path
                        && d.scope.contains(&absolute_byte)
                        && d.range.start < absolute_byte)
            }
        })
        .filter(|d| {
            (if access.is_some_and(|(_, _, static_access)| !static_access) {
                d.name.trim_start_matches('$')
            } else {
                &d.name
            })
            .to_lowercase()
            .starts_with(&prefix.to_lowercase())
        })
        .collect();
    symbols.sort_by_key(|d| (d.kind != SymbolKind::Variable, d.name.clone()));
    symbols.dedup_by(|a, b| a.name == b.name);
    symbols
        .into_iter()
        .take(100)
        .map(|d| {
            completion_item(
                d,
                access.map(|(_, _, static_access)| static_access),
                source
                    .get(byte..)
                    .is_some_and(|tail| tail.trim_start().starts_with('(')),
            )
        })
        .collect()
}

fn completion_item(
    d: &crate::symbol_index::SymbolDeclaration,
    access: Option<bool>,
    has_parentheses: bool,
) -> crate::lsp_types::CompletionItem {
    use crate::lsp_types::{CompletionItem, CompletionItemKind as K};

    let label: String = if access.is_none() && d.kind == SymbolKind::Property {
        format!("$this->{}", d.name.trim_start_matches('$'))
    } else if access.is_some_and(|static_access| !static_access) {
        d.name.trim_start_matches('$').into()
    } else {
        d.name.clone()
    };
    let callable = matches!(d.kind, SymbolKind::Method | SymbolKind::Function) && !has_parentheses;
    CompletionItem {
        insert_text: callable.then(|| format!("{label}()")),
        data: callable.then(|| serde_json::json!({"nozhik_php": true, "caret": label.len() + 1})),
        label,
        detail: Some(d.signature.clone()),
        documentation: Some(crate::lsp_types::Documentation::String(
            d.documentation.clone(),
        )),
        kind: Some(match d.kind {
            SymbolKind::Class => K::CLASS,
            SymbolKind::Function => K::FUNCTION,
            SymbolKind::Method => K::METHOD,
            SymbolKind::Property => K::PROPERTY,
            SymbolKind::Constant => K::CONSTANT,
            SymbolKind::Variable => K::VARIABLE,
        }),
        ..CompletionItem::default()
    }
}
