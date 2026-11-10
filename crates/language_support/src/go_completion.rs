//! Go completion. Receiver types come from the current line and the symbol index.
//! The UI thread never parses a tree.

use crate::{
    completion, go_stdlib,
    lsp_types::{CompletionItem, CompletionItemKind},
    symbol_index::{PhpTypeSource, SymbolDeclaration, SymbolIndex, SymbolKind},
    triggers::LanguageTriggerConfig,
};
use std::{collections::HashSet, ops::Range, path::Path};

enum Receiver {
    /// An imported or standard package.
    Package(String),
    /// A value or type whose method set can be listed.
    Value(String),
}

/// Suggestions for one caret. `source` may be a single line; `absolute_byte` is the file offset.
#[must_use]
pub fn complete(
    index: Option<&SymbolIndex>,
    path: &Path,
    source: &str,
    byte: usize,
    absolute_byte: usize,
) -> Vec<CompletionItem> {
    let Some(before) = source.get(..byte) else {
        return Vec::new();
    };
    if numeric_member(before) {
        return Vec::new();
    }
    let has_call = source
        .get(byte..)
        .is_some_and(|tail| tail.trim_start().starts_with('('));
    let triggers = LanguageTriggerConfig::go();
    let mut items = if let Some((object, _, _)) = triggers.access(before) {
        member_items(
            index,
            path,
            expression_tail(object),
            absolute_byte,
            has_call,
        )
    } else {
        global_items(index, path, absolute_byte, has_call)
    };
    let prefix = completion::query(source, byte);
    items = completion::filter(items, prefix);
    items
}

/// Hover for `receiver.member` when the caret is on the member name.
#[must_use]
pub fn member_hover(
    index: Option<&SymbolIndex>,
    path: &Path,
    line: &str,
    local_byte: usize,
    absolute_byte: usize,
) -> Option<crate::lsp_types::Hover> {
    let (start, end) = word_bounds(line, local_byte)?;
    let name = line.get(start..end)?;
    let head = line.get(..start)?.trim_end();
    let object = head.strip_suffix('.')?.trim_end();
    if object.is_empty() {
        return None;
    }
    match resolve_expression(index, path, object, absolute_byte)? {
        Receiver::Package(package) => package_hover(index, &package, name),
        Receiver::Value(type_name) => type_hover(index, &type_name, name),
    }
}

fn member_items(
    index: Option<&SymbolIndex>,
    path: &Path,
    expression: &str,
    byte: usize,
    has_call: bool,
) -> Vec<CompletionItem> {
    match resolve_expression(index, path, expression, byte) {
        Some(Receiver::Package(package)) => package_items(index, path, &package, has_call),
        Some(Receiver::Value(type_name)) => type_items(index, path, &type_name, has_call),
        None => Vec::new(),
    }
}

fn global_items(
    index: Option<&SymbolIndex>,
    path: &Path,
    byte: usize,
    has_call: bool,
) -> Vec<CompletionItem> {
    let mut items = Vec::new();
    let mut seen = HashSet::new();
    let prefix = index.and_then(|index| file_prefix(index, path));
    if let Some(index) = index {
        push_bindings(&mut items, &mut seen, index, path, byte, has_call);
        if let Some(prefix) = &prefix {
            for declaration in index.declarations() {
                if is_top_level(declaration, prefix) {
                    push_decl(&mut items, &mut seen, declaration, has_call, 1);
                }
            }
        }
        push_dot_imports(&mut items, &mut seen, index, path, has_call);
    }
    push_language_items(&mut items, &mut seen, has_call);
    items
}

fn push_bindings(
    items: &mut Vec<CompletionItem>,
    seen: &mut HashSet<String>,
    index: &SymbolIndex,
    path: &Path,
    byte: usize,
    has_call: bool,
) {
    for declaration in bindings_at(index, path, byte) {
        let imported = declaration.signature.starts_with("import ");
        push_name(
            items,
            seen,
            Suggestion {
                label: &declaration.name,
                kind: if imported {
                    CompletionItemKind::MODULE
                } else {
                    kind_of(declaration.kind)
                },
                detail: &declaration.signature,
                documentation: &declaration.documentation,
                callable: false,
                has_call,
                group: if imported { 2 } else { 0 },
            },
        );
    }
}

fn push_dot_imports(
    items: &mut Vec<CompletionItem>,
    seen: &mut HashSet<String>,
    index: &SymbolIndex,
    path: &Path,
    has_call: bool,
) {
    for declaration in index.declarations() {
        if declaration.path == path
            && declaration.name == "."
            && let Some(imported) = package_of(declaration)
        {
            for item in package_symbols(index, path, imported, true) {
                push_decl(items, seen, item, has_call, 1);
            }
            if let Some(package) = go_stdlib::package(imported) {
                push_catalog_package(items, seen, package, has_call, 1);
            }
        }
    }
}

fn push_language_items(
    items: &mut Vec<CompletionItem>,
    seen: &mut HashSet<String>,
    has_call: bool,
) {
    for item in go_stdlib::BUILTINS {
        push_name(
            items,
            seen,
            Suggestion {
                label: item.name,
                kind: CompletionItemKind::FUNCTION,
                detail: item.signature,
                documentation: item.documentation,
                callable: true,
                has_call,
                group: 3,
            },
        );
    }
    for item in go_stdlib::CONSTANTS {
        push_name(
            items,
            seen,
            Suggestion {
                label: item.name,
                kind: CompletionItemKind::CONSTANT,
                detail: item.signature,
                documentation: item.documentation,
                callable: false,
                has_call,
                group: 3,
            },
        );
    }
    for name in go_stdlib::PREDECLARED {
        push_name(
            items,
            seen,
            Suggestion {
                label: name,
                kind: CompletionItemKind::CLASS,
                detail: &format!("type {name}"),
                documentation: "",
                callable: false,
                has_call,
                group: 3,
            },
        );
    }
    for name in go_stdlib::KEYWORDS {
        push_name(
            items,
            seen,
            Suggestion {
                label: name,
                kind: CompletionItemKind::KEYWORD,
                detail: name,
                documentation: "",
                callable: false,
                has_call,
                group: 4,
            },
        );
    }
    for package in go_stdlib::PACKAGES {
        push_name(
            items,
            seen,
            Suggestion {
                label: package.alias,
                kind: CompletionItemKind::MODULE,
                detail: &format!("import \"{}\"", package.path),
                documentation: package.documentation,
                callable: false,
                has_call,
                group: 5,
            },
        );
    }
}

fn package_items(
    index: Option<&SymbolIndex>,
    path: &Path,
    package: &str,
    has_call: bool,
) -> Vec<CompletionItem> {
    let mut items = Vec::new();
    let mut seen = HashSet::new();
    let same = index
        .and_then(|index| file_prefix(index, path))
        .is_some_and(|prefix| prefix == package);
    if let Some(index) = index {
        for declaration in package_symbols(index, path, package, !same) {
            push_decl(&mut items, &mut seen, declaration, has_call, 1);
        }
    }
    if let Some(catalog) = go_stdlib::package(package) {
        push_catalog_package(&mut items, &mut seen, catalog, has_call, 1);
    }
    items
}

fn type_items(
    index: Option<&SymbolIndex>,
    path: &Path,
    type_name: &str,
    has_call: bool,
) -> Vec<CompletionItem> {
    let mut items = Vec::new();
    let mut seen = HashSet::new();
    let same = index
        .and_then(|index| file_prefix(index, path))
        .is_some_and(|prefix| type_name == prefix || type_name.starts_with(&format!("{prefix}::")));
    if let Some(index) = index {
        for owner in index.class_lineage(type_name) {
            for declaration in index.declarations() {
                if declaration.owner.as_deref() != Some(owner.as_str()) {
                    continue;
                }
                if !same && !exported(&declaration.name) {
                    continue;
                }
                push_decl(&mut items, &mut seen, declaration, has_call, 1);
            }
        }
    }
    for item in catalog_members(type_name) {
        push_name(
            &mut items,
            &mut seen,
            Suggestion {
                label: item.name,
                kind: if item.field {
                    CompletionItemKind::FIELD
                } else {
                    CompletionItemKind::METHOD
                },
                detail: item.signature,
                documentation: item.documentation,
                callable: !item.field,
                has_call,
                group: 1,
            },
        );
    }
    items
}

fn package_symbols<'a>(
    index: &'a SymbolIndex,
    _path: &Path,
    package: &str,
    exported_only: bool,
) -> Vec<&'a SymbolDeclaration> {
    index
        .declarations()
        .filter(|declaration| is_top_level(declaration, package))
        .filter(|declaration| !exported_only || exported(&declaration.name))
        .collect()
}

fn push_catalog_package(
    items: &mut Vec<CompletionItem>,
    seen: &mut HashSet<String>,
    package: &go_stdlib::Package,
    has_call: bool,
    group: u8,
) {
    for item in package.items {
        if !exported(item.name) {
            continue;
        }
        push_name(
            items,
            seen,
            Suggestion {
                label: item.name,
                kind: if item.field {
                    CompletionItemKind::FIELD
                } else if item.signature.starts_with("const ") {
                    CompletionItemKind::CONSTANT
                } else {
                    CompletionItemKind::FUNCTION
                },
                detail: item.signature,
                documentation: item.documentation,
                callable: !item.field && !item.signature.starts_with("const "),
                has_call,
                group,
            },
        );
    }
    for kind in package.types {
        if !exported(kind.name) {
            continue;
        }
        push_name(
            items,
            seen,
            Suggestion {
                label: kind.name,
                kind: CompletionItemKind::CLASS,
                detail: &format!("type {}", kind.name),
                documentation: kind.documentation,
                callable: false,
                has_call,
                group,
            },
        );
    }
}

fn push_decl(
    items: &mut Vec<CompletionItem>,
    seen: &mut HashSet<String>,
    declaration: &SymbolDeclaration,
    has_call: bool,
    group: u8,
) {
    let callable = matches!(declaration.kind, SymbolKind::Function | SymbolKind::Method);
    push_name(
        items,
        seen,
        Suggestion {
            label: &declaration.name,
            kind: kind_of(declaration.kind),
            detail: &declaration.signature,
            documentation: &declaration.documentation,
            callable,
            has_call,
            group,
        },
    );
}

#[derive(Clone, Copy)]
struct Suggestion<'a> {
    label: &'a str,
    kind: CompletionItemKind,
    detail: &'a str,
    documentation: &'a str,
    callable: bool,
    has_call: bool,
    group: u8,
}

fn push_name(
    items: &mut Vec<CompletionItem>,
    seen: &mut HashSet<String>,
    suggestion: Suggestion<'_>,
) {
    let Suggestion {
        label,
        kind,
        detail,
        documentation,
        callable,
        has_call,
        group,
    } = suggestion;
    if label.is_empty() || label == "_" || label == "." || !seen.insert(label.to_string()) {
        return;
    }
    let insert_call = callable && !has_call;
    let insert = if insert_call {
        format!("{label}()")
    } else {
        label.to_string()
    };
    let caret = insert_call.then_some(label.len() + 1);
    items.push(CompletionItem {
        label: label.to_string(),
        kind: Some(kind),
        detail: Some(detail.to_string()),
        documentation: (!documentation.is_empty())
            .then(|| crate::lsp_types::Documentation::String(documentation.to_string())),
        insert_text: Some(insert),
        filter_text: Some(label.to_string()),
        sort_text: Some(format!("{group:02}_{label}")),
        data: caret.map(|caret| serde_json::json!({"nozhik_go": true, "caret": caret})),
        ..CompletionItem::default()
    });
}

fn package_hover(
    index: Option<&SymbolIndex>,
    package: &str,
    name: &str,
) -> Option<crate::lsp_types::Hover> {
    if let Some(declaration) = index.and_then(|index| index.resolve(&format!("{package}::{name}")))
    {
        return Some(go_stdlib::hover_markup(
            &declaration.signature,
            &declaration.documentation,
        ));
    }
    let package = go_stdlib::package(package)?;
    if let Some(item) = package.items.iter().find(|item| item.name == name) {
        return Some(go_stdlib::hover_markup(item.signature, item.documentation));
    }
    package
        .types
        .iter()
        .find(|kind| kind.name == name)
        .map(|kind| go_stdlib::hover_markup(&format!("type {}", kind.name), kind.documentation))
}

fn type_hover(
    index: Option<&SymbolIndex>,
    type_name: &str,
    name: &str,
) -> Option<crate::lsp_types::Hover> {
    if let Some(declaration) =
        index.and_then(|index| index.resolve(&format!("{type_name}::{name}")))
    {
        return Some(go_stdlib::hover_markup(
            &declaration.signature,
            &declaration.documentation,
        ));
    }
    catalog_members(type_name)
        .iter()
        .find(|item| item.name == name)
        .map(|item| go_stdlib::hover_markup(item.signature, item.documentation))
}

fn resolve_expression(
    index: Option<&SymbolIndex>,
    path: &Path,
    expression: &str,
    byte: usize,
) -> Option<Receiver> {
    let expression = strip_unary(expression);
    if expression.is_empty() {
        return None;
    }
    if expression.ends_with(')') || expression.ends_with(']') {
        let close = expression.len().checked_sub(1)?;
        let open = matching_open(expression.as_bytes(), close)?;
        if expression.ends_with(')') {
            return resolve_call(index, path, expression.get(..open)?.trim_end(), byte);
        }
        return resolve_expression(index, path, expression.get(..open)?.trim_end(), byte);
    }
    if let Some((receiver, field)) = split_selector(expression) {
        return resolve_selector(index, path, receiver, field, byte);
    }
    resolve_ident(index, path, expression, byte)
}

fn resolve_selector(
    index: Option<&SymbolIndex>,
    path: &Path,
    receiver: &str,
    field: &str,
    byte: usize,
) -> Option<Receiver> {
    match resolve_expression(index, path, receiver, byte)? {
        Receiver::Package(package) => {
            if let Some(type_name) = go_stdlib::func_return(&package, field) {
                return Some(Receiver::Value(type_name.to_string()));
            }
            if go_stdlib::catalog_type(&format!("{package}::{field}")).is_some() {
                return Some(Receiver::Value(format!("{package}::{field}")));
            }
            let declaration = index?.resolve(&format!("{package}::{field}"))?;
            if declaration.kind == SymbolKind::Class {
                return Some(Receiver::Value(declaration.fqn.clone()));
            }
            declaration.type_name.clone().map(Receiver::Value)
        }
        Receiver::Value(type_name) => member_type(index, &type_name, field),
    }
}

fn resolve_call(
    index: Option<&SymbolIndex>,
    path: &Path,
    callee: &str,
    byte: usize,
) -> Option<Receiver> {
    if let Some((receiver, field)) = split_selector(callee) {
        return match resolve_expression(index, path, receiver, byte)? {
            Receiver::Package(package) => go_stdlib::func_return(&package, field)
                .map(|type_name| Receiver::Value(type_name.to_string()))
                .or_else(|| {
                    index
                        .and_then(|index| index.resolve(&format!("{package}::{field}")))
                        .and_then(|declaration| declaration.type_name.clone())
                        .map(Receiver::Value)
                }),
            Receiver::Value(type_name) => method_return(index, &type_name, field),
        };
    }
    if let Some(type_name) = go_stdlib::builtin_return(callee) {
        return Some(Receiver::Value(type_name.to_string()));
    }
    let index = index?;
    let prefix = file_prefix(index, path)?;
    index
        .declaration(&format!("{prefix}::{callee}"))
        .and_then(|declaration| declaration.type_name.clone())
        .map(Receiver::Value)
}

fn resolve_ident(
    index: Option<&SymbolIndex>,
    path: &Path,
    name: &str,
    byte: usize,
) -> Option<Receiver> {
    if let Some(index) = index {
        if let Some(binding) = bindings_at(index, path, byte)
            .into_iter()
            .find(|declaration| declaration.name == name)
        {
            if let Some(package) = package_of(binding) {
                return Some(Receiver::Package(package.to_string()));
            }
            if let Some(type_name) = &binding.type_name {
                return Some(Receiver::Value(type_name.clone()));
            }
            if let Some(source) = &binding.type_source
                && let Some(type_name) = index.php_type(source)
            {
                return Some(Receiver::Value(type_name));
            }
            return None;
        }
        if let Some(prefix) = file_prefix(index, path)
            && let Some(declaration) = index.declaration(&format!("{prefix}::{name}"))
            && !declaration.signature.starts_with("package ")
        {
            if declaration.kind == SymbolKind::Class {
                return Some(Receiver::Value(declaration.fqn.clone()));
            }
            if let Some(type_name) = &declaration.type_name {
                return Some(Receiver::Value(type_name.clone()));
            }
            return None;
        }
    }
    go_stdlib::package(name).map(|package| Receiver::Package(package.path.to_string()))
}

fn member_type(index: Option<&SymbolIndex>, type_name: &str, field: &str) -> Option<Receiver> {
    if let Some(type_name) = index
        .and_then(|index| index.resolve(&format!("{type_name}::{field}")))
        .and_then(|declaration| declaration.type_name.clone())
    {
        return Some(Receiver::Value(type_name));
    }
    catalog_members(type_name)
        .iter()
        .find(|item| item.name == field && !item.returns.is_empty())
        .map(|item| Receiver::Value(item.returns.to_string()))
}

fn method_return(index: Option<&SymbolIndex>, type_name: &str, field: &str) -> Option<Receiver> {
    if let Some(type_name) = index
        .and_then(|index| index.resolve(&format!("{type_name}::{field}")))
        .and_then(|declaration| declaration.type_name.clone())
    {
        return Some(Receiver::Value(type_name));
    }
    catalog_members(type_name)
        .iter()
        .find(|item| item.name == field && !item.field && !item.returns.is_empty())
        .map(|item| Receiver::Value(item.returns.to_string()))
}

fn catalog_members(type_name: &str) -> &'static [go_stdlib::Item] {
    if let Some(name) = type_name.strip_prefix("builtin::") {
        return go_stdlib::builtin_type(name).unwrap_or(&[]);
    }
    go_stdlib::catalog_type(type_name).map_or(&[], |(_, kind)| kind.members)
}

fn file_prefix(index: &SymbolIndex, path: &Path) -> Option<String> {
    index.declarations().find_map(|declaration| {
        if declaration.path != path || !declaration.signature.starts_with("package ") {
            return None;
        }
        declaration
            .fqn
            .strip_prefix("package::")
            .map(ToString::to_string)
    })
}

fn bindings_at<'a>(index: &'a SymbolIndex, path: &Path, byte: usize) -> Vec<&'a SymbolDeclaration> {
    let mut best = Vec::new();
    for declaration in index.declarations() {
        if declaration.path != path
            || declaration.kind != SymbolKind::Variable
            || declaration.name == "."
            || declaration.signature.starts_with("package ")
            || !in_scope(&declaration.scope, byte)
        {
            continue;
        }
        if let Some(position) = best
            .iter()
            .position(|found: &&SymbolDeclaration| found.name == declaration.name)
        {
            let current = best[position].scope.end - best[position].scope.start;
            let next = declaration.scope.end - declaration.scope.start;
            if next < current {
                best[position] = declaration;
            }
        } else {
            best.push(declaration);
        }
    }
    best
}

const fn in_scope(scope: &Range<usize>, byte: usize) -> bool {
    byte >= scope.start && byte <= scope.end
}

fn is_top_level(declaration: &SymbolDeclaration, package: &str) -> bool {
    declaration.owner.is_none()
        && declaration.fqn.strip_prefix(package).is_some_and(|rest| {
            rest.strip_prefix("::")
                .is_some_and(|name| !name.is_empty() && !name.contains("::"))
        })
        && !declaration.signature.starts_with("package ")
        && !declaration.signature.starts_with("import ")
}

fn package_of(declaration: &SymbolDeclaration) -> Option<&str> {
    match &declaration.type_source {
        Some(PhpTypeSource::Class(name)) => name.strip_prefix("package:"),
        _ => None,
    }
}

fn exported(name: &str) -> bool {
    name.chars().next().is_some_and(char::is_uppercase)
}

const fn kind_of(kind: SymbolKind) -> CompletionItemKind {
    match kind {
        SymbolKind::Class => CompletionItemKind::CLASS,
        SymbolKind::Function => CompletionItemKind::FUNCTION,
        SymbolKind::Method => CompletionItemKind::METHOD,
        SymbolKind::Property => CompletionItemKind::FIELD,
        SymbolKind::Constant => CompletionItemKind::CONSTANT,
        SymbolKind::Variable => CompletionItemKind::VARIABLE,
    }
}

fn numeric_member(before: &str) -> bool {
    let Some(dot) = before.rfind('.') else {
        return false;
    };
    let after = before.get(dot + 1..).unwrap_or("");
    if !after.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    let left = before.get(..dot).unwrap_or("");
    let mut end = left.len();
    let bytes = left.as_bytes();
    while end > 0 && (bytes[end - 1].is_ascii_digit() || bytes[end - 1] == b'_') {
        end -= 1;
    }
    !left
        .get(..end)
        .unwrap_or("")
        .chars()
        .next_back()
        .is_some_and(|character| character.is_alphanumeric() || character == ')')
}

fn expression_tail(left: &str) -> &str {
    let end = left.trim_end().len();
    let Some(text) = left.get(..end) else {
        return "";
    };
    let start = operand_start(text, end);
    text.get(start..end).unwrap_or(text).trim()
}

fn operand_start(text: &str, mut end: usize) -> usize {
    for _ in 0..64 {
        end = trim_end_index(text, end);
        if end == 0 {
            return 0;
        }
        let Some((index, character)) = text[..end].char_indices().next_back() else {
            return 0;
        };
        if character == ')' || character == ']' {
            let Some(open) = matching_open(text.as_bytes(), index) else {
                return end;
            };
            end = open;
            continue;
        }
        if character == '.' {
            end = index;
            continue;
        }
        if is_name(character) {
            let name_start = consume_name_start(text, end);
            let before = trim_end_index(text, name_start);
            if before > 0 && text[..before].ends_with('.') {
                end = before - 1;
                continue;
            }
            return name_start;
        }
        return end;
    }
    0
}

fn strip_unary(expression: &str) -> &str {
    let mut text = expression.trim();
    for _ in 0..8 {
        if text.starts_with('(') && text.ends_with(')') && text.len() >= 2 {
            let inner = text.get(1..text.len() - 1).unwrap_or(text).trim();
            if matching_open(text.as_bytes(), text.len() - 1) == Some(0) {
                text = inner;
                continue;
            }
        }
        if let Some(rest) = text.strip_prefix('*').or_else(|| text.strip_prefix('&')) {
            text = rest.trim();
            continue;
        }
        break;
    }
    text
}

fn split_selector(expression: &str) -> Option<(&str, &str)> {
    let mut depth = 0i32;
    let mut index = expression.len();
    let bytes = expression.as_bytes();
    while index > 0 {
        index -= 1;
        match bytes.get(index).copied() {
            Some(b')' | b']') => depth += 1,
            Some(b'(' | b'[') => depth -= 1,
            Some(b'.') if depth == 0 => {
                let receiver = expression.get(..index)?.trim();
                let field = expression.get(index + 1..)?.trim();
                if receiver.is_empty() || field.is_empty() || !field.chars().all(is_name) {
                    return None;
                }
                return Some((receiver, field));
            }
            _ => {}
        }
    }
    None
}

fn matching_open(bytes: &[u8], close: usize) -> Option<usize> {
    let close_byte = *bytes.get(close)?;
    let open = match close_byte {
        b')' => b'(',
        b']' => b'[',
        _ => return None,
    };
    let mut depth = 0i32;
    let mut index = close;
    while index > 0 {
        index -= 1;
        match bytes.get(index).copied() {
            Some(byte) if byte == close_byte => depth += 1,
            Some(byte) if byte == open => {
                if depth == 0 {
                    return Some(index);
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    None
}

fn trim_end_index(text: &str, mut end: usize) -> usize {
    while end > 0 {
        let Some((index, character)) = text[..end].char_indices().next_back() else {
            break;
        };
        if !character.is_whitespace() {
            break;
        }
        end = index;
    }
    end
}

fn consume_name_start(text: &str, end: usize) -> usize {
    let mut start = end;
    while let Some((index, character)) = text[..start].char_indices().next_back() {
        if !is_name(character) {
            break;
        }
        start = index;
    }
    start
}

fn word_bounds(source: &str, byte: usize) -> Option<(usize, usize)> {
    if !source.is_char_boundary(byte.min(source.len())) {
        return None;
    }
    let mut index = byte.min(source.len());
    let on_name = source[index..].chars().next().is_some_and(is_name);
    if !on_name {
        let (previous, character) = source[..index].char_indices().next_back()?;
        if !is_name(character) {
            return None;
        }
        index = previous;
    }
    let start = consume_name_start(
        source,
        index + source[index..].chars().next().map_or(0, char::len_utf8),
    );
    let mut end = index;
    for (offset, character) in source[index..].char_indices() {
        if !is_name(character) {
            break;
        }
        end = index + offset + character.len_utf8();
    }
    (start < end).then_some((start, end))
}

fn is_name(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

#[cfg(test)]
mod tests {
    use super::complete;
    use crate::symbol_index::{SymbolIndex, SymbolKind};
    use std::path::Path;

    #[test]
    fn standard_packages_calls_and_numeric_literals() -> Result<(), Box<dyn std::error::Error>> {
        let source = "fmt.Pri";
        let items = complete(
            None,
            Path::new("demo.go"),
            source,
            source.len(),
            source.len(),
        );
        let printf = items
            .iter()
            .find(|item| item.label == "Printf")
            .ok_or("Printf")?;
        assert_eq!(printf.insert_text.as_deref(), Some("Printf()"));
        assert_eq!(
            printf
                .data
                .as_ref()
                .and_then(|data| data["nozhik_go"].as_bool()),
            Some(true)
        );
        assert_eq!(
            printf.data.as_ref().and_then(|data| data["caret"].as_u64()),
            Some(u64::try_from("Printf".len())? + 1)
        );
        let typed = "len";
        assert!(
            complete(None, Path::new("demo.go"), typed, typed.len(), typed.len())
                .iter()
                .any(|item| item.label == "len")
        );
        assert!(
            complete(None, Path::new("demo.go"), "f", 1, 1)
                .iter()
                .any(|item| item.label == "fmt" && item.detail.as_deref() == Some("import \"fmt\""))
        );
        let numeric = "value := 1.5";
        assert_eq!(
            complete(
                None,
                Path::new("demo.go"),
                numeric,
                numeric.len(),
                numeric.len()
            ),
            Vec::new()
        );
        Ok(())
    }

    #[test]
    fn project_fields_and_methods_follow_the_receiver() -> Result<(), Box<dyn std::error::Error>> {
        let source = "package demo\n\nfunc (s *Server) Listen(addr string) error {\n\treturn nil\n}\n\ntype Server struct {\n\tName string\n}\n";
        let path = Path::new("/work/demo.go");
        let facts = crate::go_symbols::parse(path, source)?;
        let mut index = SymbolIndex::new(crate::symbol_index::LanguageId::Go);
        index.replace(path.into(), facts);
        let byte = source.find("return nil").ok_or("return")?;
        let probe = "s.";
        let mut edited = source.to_string();
        edited.insert_str(byte, probe);
        let caret = byte + probe.len();
        let items = complete(Some(&index), path, &edited, caret, caret);
        assert!(
            items
                .iter()
                .any(|item| item.label == "Listen"
                    && item.insert_text.as_deref() == Some("Listen()"))
        );
        assert!(items.iter().any(|item| item.label == "Name"
            && item.kind == Some(crate::lsp_types::CompletionItemKind::FIELD)));
        assert!(index.declarations().any(|declaration| {
            declaration.name == "Server" && declaration.kind == SymbolKind::Class
        }));
        Ok(())
    }
}
