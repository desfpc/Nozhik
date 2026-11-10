//! JavaScript completion. Receiver types come from the current line and the symbol index.
//! The UI thread never parses a tree.

use crate::{
    completion, js_stdlib,
    lsp_types::{CompletionItem, CompletionItemKind},
    symbol_index::{PhpTypeSource, SymbolDeclaration, SymbolIndex, SymbolKind},
    triggers::LanguageTriggerConfig,
};
use std::collections::HashSet;
use std::path::Path;

#[derive(Clone)]
enum Receiver {
    /// The binding is a constructor or namespace.
    Static(String),
    /// The binding is an instance of a built-in or project type.
    Instance(String),
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
    let triggers = LanguageTriggerConfig::javascript();
    let has_call = source
        .get(byte..)
        .is_some_and(|tail| tail.trim_start().starts_with('('));
    let after_new = keyword_tail(before_prefix(source, byte), "new");
    let mut items = if let Some((object, _prefix, _)) = triggers.access(before) {
        // The next editor frame resolves the whole buffer. Keep only the
        // expression that the dot applies to, not the statements before it.
        member_items(
            index,
            path,
            expression_receiver(object),
            absolute_byte,
            has_call,
        )
    } else {
        global_items(index, path, absolute_byte, has_call, after_new)
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
    let object = expression_receiver(
        head.strip_suffix("?.")
            .or_else(|| head.strip_suffix('.'))?
            .trim_end(),
    );
    if object.is_empty() {
        return None;
    }
    let receiver = resolve_expression(index, path, object, absolute_byte, 0)?;
    let (type_name, static_access) = match &receiver {
        Receiver::Static(type_name) => (type_name.as_str(), true),
        Receiver::Instance(type_name) => (type_name.as_str(), false),
    };
    if let Some(index) = index
        && type_name.contains("::")
        && let Some(declaration) = index.resolve(&format!("{type_name}::{name}"))
    {
        return Some(js_stdlib::hover_markup(
            &declaration.signature,
            &declaration.documentation,
        ));
    }
    let (signature, url) = js_stdlib::member_info(type_name, name, static_access)?;
    Some(js_stdlib::hover_markup(signature, &url))
}

fn before_prefix(source: &str, byte: usize) -> &str {
    let start = completion::prefix(source, byte).start;
    source.get(..start).unwrap_or("")
}

fn keyword_tail(text: &str, word: &str) -> bool {
    let head = text.trim_end();
    let Some(rest) = head.strip_suffix(word) else {
        return false;
    };
    rest.chars()
        .next_back()
        .is_none_or(|c| !(c.is_alphanumeric() || matches!(c, '_' | '$' | '#')))
}

fn numeric_member(before: &str) -> bool {
    let Some(dot) = before.rfind('.') else {
        return false;
    };
    if before.get(..dot).is_some_and(|left| left.ends_with('?')) {
        return false;
    }
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
    let remainder = left.get(..end).unwrap_or("");
    !remainder
        .chars()
        .next_back()
        .is_some_and(|c| c.is_alphanumeric() || c == '$' || c == ')')
}

fn word_bounds(source: &str, byte: usize) -> Option<(usize, usize)> {
    if !source.is_char_boundary(byte.min(source.len())) {
        return None;
    }
    let mut index = byte.min(source.len());
    let on_name = source[index..].chars().next().is_some_and(is_name_char);
    if !on_name {
        let (previous, character) = source[..index].char_indices().next_back()?;
        if !is_name_char(character) {
            return None;
        }
        index = previous;
    }
    let mut start = index;
    while let Some((previous, character)) = source[..start].char_indices().next_back() {
        if !is_name_char(character) {
            break;
        }
        start = previous;
    }
    let mut end = index;
    for (offset, character) in source[index..].char_indices() {
        if !is_name_char(character) {
            break;
        }
        end = index + offset + character.len_utf8();
    }
    (start < end).then_some((start, end))
}

fn is_name_char(character: char) -> bool {
    character.is_alphanumeric() || matches!(character, '_' | '$' | '#')
}

/// Text of the primary expression that ends at `left`.
///
/// `left` is everything before the completion dot. A later editor frame passes
/// the whole buffer, so `function init() { document` must resolve as `document`.
fn expression_receiver(left: &str) -> &str {
    let end = left.trim_end().len();
    let text = left.get(..end).unwrap_or("");
    let start = expression_start(text);
    text.get(start..).unwrap_or(text).trim()
}

fn expression_start(text: &str) -> usize {
    let bytes = text.as_bytes();
    let mut cursor = text.len();
    let mut guard = text.len().saturating_add(1);
    while guard > 0 {
        guard -= 1;
        let trimmed = text.get(..cursor).unwrap_or("").trim_end().len();
        if trimmed == 0 {
            return 0;
        }
        // Whitespace before `new` or a finished word is a token boundary.
        // Whitespace before `.` / `?.` still belongs to that member chain.
        if trimmed != cursor && !on_member_operator(text, cursor) {
            return cursor;
        }
        let Some((previous, character)) = text[..trimmed].char_indices().next_back() else {
            return 0;
        };
        if character == ')' || character == ']' {
            let Some(open) = matching_open(bytes, previous) else {
                return cursor;
            };
            if open >= cursor {
                return cursor;
            }
            cursor = open;
            continue;
        }
        if matches!(character, '"' | '\'' | '`') {
            let Some(open) = opening_quote(bytes, previous) else {
                return cursor;
            };
            if let Some(operator) = member_operator_at(text, open)
                && operator < cursor
            {
                cursor = operator;
                continue;
            }
            return open;
        }
        if is_name_char(character) {
            let name_start = identifier_start(text, trimmed);
            if let Some(operator) = member_operator_at(text, name_start)
                && operator < cursor
            {
                cursor = operator;
                continue;
            }
            if let Some(keyword) = keyword_ends(text, name_start, "new")
                && keyword < cursor
            {
                cursor = keyword;
                continue;
            }
            return name_start;
        }
        return cursor;
    }
    0
}

fn on_member_operator(text: &str, cursor: usize) -> bool {
    let bytes = text.as_bytes();
    bytes.get(cursor) == Some(&b'.')
        || (bytes.get(cursor) == Some(&b'?') && bytes.get(cursor + 1) == Some(&b'.'))
}

fn identifier_start(text: &str, end: usize) -> usize {
    let mut start = end;
    while let Some((previous, character)) = text[..start].char_indices().next_back() {
        if !is_name_char(character) {
            break;
        }
        start = previous;
    }
    start
}

fn member_operator_at(text: &str, before: usize) -> Option<usize> {
    let head = text.get(..before)?.trim_end();
    let bytes = head.as_bytes();
    if bytes.last() != Some(&b'.') {
        return None;
    }
    let mut start = head.len() - 1;
    if start > 0 && bytes[start - 1] == b'?' {
        start -= 1;
    }
    Some(start)
}

fn keyword_ends(text: &str, before: usize, keyword: &str) -> Option<usize> {
    let head = text.get(..before)?.trim_end();
    let rest = head.strip_suffix(keyword)?;
    rest.chars()
        .next_back()
        .is_none_or(|character| !is_name_char(character))
        .then_some(head.len() - keyword.len())
}

fn matching_open(bytes: &[u8], close: usize) -> Option<usize> {
    let want = match bytes.get(close)? {
        b')' => b'(',
        b']' => b'[',
        _ => return None,
    };
    let mut depth = 0_i32;
    let mut index = close;
    loop {
        if bytes[index] & 0xc0 != 0x80 {
            if matches!(bytes[index], b'"' | b'\'' | b'`')
                && let Some(open) = opening_quote(bytes, index)
            {
                if open == 0 || open >= index {
                    return None;
                }
                index = open - 1;
                continue;
            }
            match bytes[index] {
                b')' | b']' | b'}' => depth += 1,
                b'(' | b'[' | b'{' => {
                    depth -= 1;
                    if depth == 0 {
                        return (bytes[index] == want).then_some(index);
                    }
                }
                _ => {}
            }
        }
        if index == 0 {
            return None;
        }
        index -= 1;
    }
}

fn opening_quote(bytes: &[u8], close: usize) -> Option<usize> {
    let quote = *bytes.get(close)?;
    if !matches!(quote, b'"' | b'\'' | b'`') {
        return None;
    }
    let mut index = close;
    while index > 0 {
        index -= 1;
        if bytes[index] != quote {
            continue;
        }
        let mut slashes = 0_usize;
        let mut look = index;
        while look > 0 && bytes[look - 1] == b'\\' {
            slashes += 1;
            look -= 1;
        }
        if slashes.is_multiple_of(2) {
            return Some(index);
        }
    }
    None
}

/// Fields a page script uses on almost every element. The completion list keeps
/// 100 rows, so these stay with the methods and ahead of the longer form,
/// media and layout names.
fn core_property(name: &str) -> bool {
    matches!(
        name,
        "id" | "className"
            | "classList"
            | "style"
            | "dataset"
            | "innerHTML"
            | "outerHTML"
            | "innerText"
            | "textContent"
            | "value"
            | "href"
            | "src"
            | "checked"
            | "disabled"
            | "name"
            | "type"
            | "children"
            | "parentElement"
            | "parentNode"
            | "nodeType"
            | "hidden"
            | "title"
            | "placeholder"
            | "files"
            | "tabIndex"
            | "tagName"
            | "length"
            | "size"
    )
}

fn member_items(
    index: Option<&SymbolIndex>,
    path: &Path,
    object: &str,
    absolute_byte: usize,
    has_call: bool,
) -> Vec<CompletionItem> {
    let Some(receiver) = resolve_expression(index, path, object.trim(), absolute_byte, 0) else {
        return Vec::new();
    };
    let mut items = Vec::new();
    let mut seen = HashSet::new();
    match &receiver {
        Receiver::Static(type_name) | Receiver::Instance(type_name)
            if type_name.contains("::") && index.is_some() =>
        {
            let object_keys = matches!(receiver, Receiver::Instance(_))
                && crate::js_symbols::is_object_shape(type_name);
            if let Some(index) = index {
                let static_access = matches!(receiver, Receiver::Static(_));
                for draft in project_members(index, path, type_name, static_access, absolute_byte) {
                    push_unique(&mut items, &mut seen, symbol_item(&draft, has_call, 0));
                }
            }
            if object_keys {
                for member in js_stdlib::members("Object", false) {
                    push_unique(&mut items, &mut seen, member_item(member, has_call, 1));
                }
            }
        }
        Receiver::Static(type_name) | Receiver::Instance(type_name) => {
            let static_access = matches!(receiver, Receiver::Static(_));
            for member in js_stdlib::members(type_name, static_access) {
                let extra = member.property && !core_property(member.name);
                push_unique(
                    &mut items,
                    &mut seen,
                    member_item(member, has_call, u8::from(extra)),
                );
            }
        }
    }
    items
}

struct OwnedMember {
    name: String,
    kind: SymbolKind,
    signature: String,
    documentation: String,
    fqn: String,
}

fn project_members(
    index: &SymbolIndex,
    path: &Path,
    type_name: &str,
    static_access: bool,
    absolute_byte: usize,
) -> Vec<OwnedMember> {
    let lineage = index.class_lineage(type_name);
    let current = current_class(index, path, absolute_byte);
    let mut copied = Vec::new();
    for declaration in index.declarations() {
        let Some(owner) = declaration.owner.as_deref() else {
            continue;
        };
        if !lineage.iter().any(|name| name == owner) {
            continue;
        }
        if declaration.name == "constructor" {
            continue;
        }
        let visible = if static_access {
            declaration.is_static
        } else {
            !declaration.is_static
                && matches!(declaration.kind, SymbolKind::Method | SymbolKind::Property)
        };
        if !visible {
            continue;
        }
        if declaration.visibility == crate::symbol_index::Visibility::Private
            && current.as_deref() != Some(owner)
        {
            continue;
        }
        copied.push(OwnedMember {
            name: declaration.name.clone(),
            kind: declaration.kind,
            signature: declaration.signature.clone(),
            documentation: declaration.documentation.clone(),
            fqn: declaration.fqn.clone(),
        });
    }
    let mut seen = HashSet::new();
    let mut ordered = Vec::new();
    for owner in &lineage {
        for member in &copied {
            if member
                .fqn
                .rsplit_once("::")
                .is_some_and(|(owner_name, _)| owner_name != owner)
            {
                continue;
            }
            if !seen.insert(member.name.clone()) {
                continue;
            }
            let resolved = index
                .resolve(&format!("{type_name}::{}", member.name))
                .map(|declaration| declaration.fqn.clone());
            if resolved.as_deref() == Some(member.fqn.as_str()) {
                ordered.push(OwnedMember {
                    name: member.name.clone(),
                    kind: member.kind,
                    signature: member.signature.clone(),
                    documentation: member.documentation.clone(),
                    fqn: member.fqn.clone(),
                });
            }
        }
    }
    ordered
}

fn current_class(index: &SymbolIndex, path: &Path, byte: usize) -> Option<String> {
    index
        .declarations()
        .filter(|declaration| {
            declaration.kind == SymbolKind::Class
                && declaration.path == path
                && declaration.scope.contains(&byte)
                && declaration.name != "default"
                && !declaration.name.starts_with("anon")
        })
        .max_by_key(|declaration| declaration.scope.start)
        .map(|declaration| declaration.fqn.clone())
}

#[allow(
    clippy::too_many_lines,
    reason = "Locals, project symbols, built-ins and keywords are one completion list"
)]
fn global_items(
    index: Option<&SymbolIndex>,
    path: &Path,
    absolute_byte: usize,
    has_call: bool,
    after_new: bool,
) -> Vec<CompletionItem> {
    let mut items = Vec::new();
    let mut seen = HashSet::new();
    if let Some(index) = index {
        let mut locals = Vec::new();
        let mut same = Vec::new();
        let mut other = Vec::new();
        for declaration in index.declarations() {
            match declaration.kind {
                SymbolKind::Variable
                    if declaration.path == path
                        && declaration.scope.contains(&absolute_byte)
                        && declaration.range.start < absolute_byte =>
                {
                    locals.push(declaration);
                }
                SymbolKind::Class | SymbolKind::Function if declaration.path == path => {
                    same.push(declaration);
                }
                SymbolKind::Class | SymbolKind::Function => other.push(declaration),
                SymbolKind::Variable
                | SymbolKind::Method
                | SymbolKind::Property
                | SymbolKind::Constant => {}
            }
        }
        locals.sort_by_key(|declaration| std::cmp::Reverse(declaration.scope.start));
        for declaration in locals {
            push_unique(
                &mut items,
                &mut seen,
                symbol_item(
                    &OwnedMember {
                        name: declaration.name.clone(),
                        kind: declaration.kind,
                        signature: declaration.signature.clone(),
                        documentation: declaration.documentation.clone(),
                        fqn: declaration.fqn.clone(),
                    },
                    false,
                    0,
                ),
            );
        }
        for (group, declarations) in [(1_u8, same), (2, other)] {
            for declaration in declarations {
                let callable = declaration.kind == SymbolKind::Function;
                let construct = after_new && declaration.kind == SymbolKind::Class;
                push_unique(
                    &mut items,
                    &mut seen,
                    named_item(
                        &declaration.name,
                        kind_of(declaration.kind),
                        &declaration.signature,
                        &declaration.documentation,
                        callable || construct,
                        has_call,
                        group,
                    ),
                );
            }
        }
    }
    for ty in js_stdlib::types() {
        let construct = after_new && ty.constructable;
        push_unique(
            &mut items,
            &mut seen,
            named_item(
                ty.name,
                CompletionItemKind::CLASS,
                ty.name,
                ty.documentation,
                construct,
                has_call,
                3,
            ),
        );
    }
    for fun in js_stdlib::global_functions() {
        push_unique(
            &mut items,
            &mut seen,
            named_item(
                fun.name,
                CompletionItemKind::FUNCTION,
                fun.signature,
                fun.documentation,
                true,
                has_call,
                3,
            ),
        );
    }
    for binding in js_stdlib::global_bindings() {
        push_unique(
            &mut items,
            &mut seen,
            named_item(
                binding.name,
                CompletionItemKind::VARIABLE,
                binding.type_name,
                binding.documentation,
                false,
                has_call,
                3,
            ),
        );
    }
    for (name, documentation, _) in js_stdlib::global_constants() {
        push_unique(
            &mut items,
            &mut seen,
            named_item(
                name,
                CompletionItemKind::CONSTANT,
                name,
                documentation,
                false,
                has_call,
                3,
            ),
        );
    }
    for construct in js_stdlib::constructs() {
        if seen.insert(construct.name.to_string()) {
            items.push(keyword_item(construct));
        }
    }
    items
}

fn symbol_item(member: &OwnedMember, has_call: bool, group: u8) -> CompletionItem {
    let callable = matches!(member.kind, SymbolKind::Method | SymbolKind::Function);
    named_item(
        &member.name,
        kind_of(member.kind),
        &member.signature,
        &member.documentation,
        callable,
        has_call,
        group,
    )
}

fn member_item(member: &js_stdlib::Member, has_call: bool, group: u8) -> CompletionItem {
    named_item(
        member.name,
        if member.property {
            CompletionItemKind::PROPERTY
        } else {
            CompletionItemKind::METHOD
        },
        member.signature,
        member.documentation,
        !member.property,
        has_call,
        group,
    )
}

fn named_item(
    label: &str,
    kind: CompletionItemKind,
    detail: &str,
    documentation: &str,
    callable: bool,
    has_call: bool,
    group: u8,
) -> CompletionItem {
    let insert_call = callable && !has_call;
    let insert = if insert_call {
        format!("{label}()")
    } else {
        label.to_string()
    };
    let caret = insert_call.then_some(label.len() + 1);
    item(label, kind, detail, documentation, insert, caret, group)
}

fn keyword_item(construct: &js_stdlib::Construct) -> CompletionItem {
    item(
        construct.name,
        CompletionItemKind::KEYWORD,
        construct.name,
        construct.documentation,
        construct.insert.to_string(),
        construct.caret,
        4,
    )
}

fn item(
    label: &str,
    kind: CompletionItemKind,
    detail: &str,
    documentation: &str,
    insert: String,
    caret: Option<usize>,
    group: u8,
) -> CompletionItem {
    CompletionItem {
        label: label.to_string(),
        kind: Some(kind),
        detail: Some(detail.to_string()),
        documentation: (!documentation.is_empty())
            .then(|| crate::lsp_types::Documentation::String(documentation.to_string())),
        insert_text: Some(insert),
        filter_text: Some(label.to_string()),
        sort_text: Some(format!("{group:02}_{label}")),
        data: caret.map(|caret| serde_json::json!({"nozhik_js": true, "caret": caret})),
        ..CompletionItem::default()
    }
}

fn push_unique(items: &mut Vec<CompletionItem>, seen: &mut HashSet<String>, item: CompletionItem) {
    if seen.insert(item.label.clone()) {
        items.push(item);
    }
}

const fn kind_of(kind: SymbolKind) -> CompletionItemKind {
    match kind {
        SymbolKind::Class => CompletionItemKind::CLASS,
        SymbolKind::Function => CompletionItemKind::FUNCTION,
        SymbolKind::Method => CompletionItemKind::METHOD,
        SymbolKind::Property => CompletionItemKind::PROPERTY,
        SymbolKind::Constant => CompletionItemKind::CONSTANT,
        SymbolKind::Variable => CompletionItemKind::VARIABLE,
    }
}

fn resolve_expression(
    index: Option<&SymbolIndex>,
    path: &Path,
    expression: &str,
    absolute_byte: usize,
    depth: u8,
) -> Option<Receiver> {
    if depth > 8 {
        return None;
    }
    let expression = trim_wrapping(expression.trim());
    if expression.is_empty() {
        return None;
    }
    if let Some(rest) = strip_new(expression) {
        let inner = trim_wrapping(rest.trim());
        let constructor = split_call(inner).unwrap_or(inner);
        return match resolve_expression(index, path, constructor, absolute_byte, depth + 1)? {
            Receiver::Static(type_name) | Receiver::Instance(type_name) => {
                Some(Receiver::Instance(type_name))
            }
        };
    }
    if let Some(callee) = split_call(expression) {
        return call_receiver(index, path, callee, absolute_byte, depth + 1);
    }
    if let Some((object, property)) = split_member(expression) {
        let receiver = resolve_expression(index, path, object, absolute_byte, depth + 1)?;
        return member_receiver(index, &receiver, property);
    }
    atom_receiver(index, path, expression, absolute_byte)
}

fn strip_new(expression: &str) -> Option<&str> {
    let rest = expression.strip_prefix("new")?;
    rest.starts_with(|c: char| c.is_whitespace())
        .then_some(rest.trim())
}

fn trim_wrapping(expression: &str) -> &str {
    let trimmed = expression.trim();
    if wrapped(trimmed) {
        trim_wrapping(trimmed.get(1..trimmed.len() - 1).unwrap_or(trimmed))
    } else {
        trimmed
    }
}

fn wrapped(expression: &str) -> bool {
    if !expression.starts_with('(') || !expression.ends_with(')') {
        return false;
    }
    let mut depth = 0_i32;
    for (index, character) in expression.char_indices() {
        match character {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return index + character.len_utf8() == expression.len();
                }
            }
            _ => {}
        }
    }
    false
}

fn split_call(expression: &str) -> Option<&str> {
    if !expression.ends_with(')') {
        return None;
    }
    let mut depth = 0_i32;
    for (index, character) in expression.char_indices().rev() {
        match character {
            ')' => depth += 1,
            '(' => {
                depth -= 1;
                if depth == 0 {
                    return expression.get(..index).map(str::trim_end);
                }
            }
            _ => {}
        }
    }
    None
}

fn split_member(expression: &str) -> Option<(&str, &str)> {
    let (start, end) = last_member(expression)?;
    let property = expression.get(end..)?.trim();
    if property.is_empty() || !property.chars().all(is_name_char) {
        return None;
    }
    Some((expression.get(..start)?.trim_end(), property))
}

fn last_member(expression: &str) -> Option<(usize, usize)> {
    let bytes = expression.as_bytes();
    let mut index = 0;
    let mut depth = 0_i32;
    let mut quote = None;
    let mut template = false;
    let mut template_expr = 0_i32;
    let mut found = None;
    while index < bytes.len() {
        let byte = bytes[index];
        if let Some(active) = quote {
            if byte == b'\\' {
                index += 2;
                continue;
            }
            if byte == active {
                quote = None;
            }
            index += 1;
            continue;
        }
        if template && template_expr == 0 {
            if byte == b'\\' {
                index += 2;
                continue;
            }
            if byte == b'`' {
                template = false;
            } else if byte == b'$' && bytes.get(index + 1) == Some(&b'{') {
                template_expr = 1;
                index += 2;
                continue;
            }
            index += 1;
            continue;
        }
        match byte {
            b'\'' | b'"' => quote = Some(byte),
            b'`' => template = true,
            b'(' | b'[' | b'{' => {
                if template_expr > 0 && byte == b'{' {
                    template_expr += 1;
                }
                depth += 1;
            }
            b')' | b']' | b'}' => {
                if template_expr > 0 && byte == b'}' {
                    template_expr -= 1;
                    if template_expr == 0 {
                        index += 1;
                        continue;
                    }
                }
                depth = depth.saturating_sub(1);
            }
            b'?' if depth == 0 && bytes.get(index + 1) == Some(&b'.') => {
                found = Some((index, index + 2));
                index += 2;
                continue;
            }
            b'.' if depth == 0 => found = Some((index, index + 1)),
            _ => {}
        }
        index += 1;
    }
    found
}

fn call_receiver(
    index: Option<&SymbolIndex>,
    path: &Path,
    callee: &str,
    absolute_byte: usize,
    depth: u8,
) -> Option<Receiver> {
    if let Some((object, property)) = split_member(callee) {
        let receiver = resolve_expression(index, path, object, absolute_byte, depth)?;
        return member_receiver(index, &receiver, property);
    }
    let name = trim_wrapping(callee);
    if let Some(fun) = js_stdlib::global_fn(name) {
        return (!fun.returns.is_empty()).then(|| Receiver::Instance(fun.returns.to_string()));
    }
    if let Some(index) = index
        && let Some(declaration) = lookup_name(index, path, name, absolute_byte)
        && declaration.kind == SymbolKind::Function
    {
        return receiver_from_declaration(index, declaration, false);
    }
    None
}

fn member_receiver(
    index: Option<&SymbolIndex>,
    receiver: &Receiver,
    property: &str,
) -> Option<Receiver> {
    let (type_name, static_access) = match receiver {
        Receiver::Static(type_name) => (type_name.as_str(), true),
        Receiver::Instance(type_name) => (type_name.as_str(), false),
    };
    if let Some(member) = js_stdlib::member(type_name, property, static_access) {
        return (!member.returns.is_empty())
            .then(|| Receiver::Instance(member.returns.to_string()));
    }
    let index = index?;
    if !type_name.contains("::") {
        return None;
    }
    let declaration = index.resolve(&format!("{type_name}::{property}"))?;
    receiver_from_declaration(index, declaration, false)
}

fn atom_receiver(
    index: Option<&SymbolIndex>,
    path: &Path,
    atom: &str,
    absolute_byte: usize,
) -> Option<Receiver> {
    if let Some(type_name) = literal_type(atom) {
        return Some(Receiver::Instance(type_name.to_string()));
    }
    if let Some(index) = index
        && let Some(declaration) = lookup_name(index, path, atom, absolute_byte)
    {
        return receiver_from_declaration(index, declaration, declaration.is_static);
    }
    if let Some(binding) = js_stdlib::global_binding(atom) {
        return Some(Receiver::Instance(binding.type_name.to_string()));
    }
    if js_stdlib::type_named(atom).is_some() {
        return Some(Receiver::Static(atom.to_string()));
    }
    if js_stdlib::global_fn(atom).is_some() {
        return Some(Receiver::Instance("Function".into()));
    }
    None
}

fn lookup_name<'a>(
    index: &'a SymbolIndex,
    path: &Path,
    name: &str,
    absolute_byte: usize,
) -> Option<&'a SymbolDeclaration> {
    if let Some(declaration) = index
        .declarations()
        .filter(|declaration| {
            declaration.kind == SymbolKind::Variable
                && declaration.name == name
                && declaration.path == path
                && declaration.scope.contains(&absolute_byte)
                && declaration.range.start < absolute_byte
        })
        .max_by_key(|declaration| (declaration.scope.start, declaration.range.start))
    {
        return Some(declaration);
    }
    index.declarations().find(|declaration| {
        declaration.path == path
            && declaration.name == name
            && matches!(declaration.kind, SymbolKind::Class | SymbolKind::Function)
    })
}

fn receiver_from_declaration(
    index: &SymbolIndex,
    declaration: &SymbolDeclaration,
    constructor: bool,
) -> Option<Receiver> {
    match declaration.kind {
        SymbolKind::Class => Some(if constructor || declaration.name != "default" {
            Receiver::Static(declaration.fqn.clone())
        } else {
            Receiver::Instance(declaration.fqn.clone())
        }),
        SymbolKind::Function => Some(Receiver::Instance("Function".into())),
        SymbolKind::Variable => typed_receiver(index, declaration, declaration.is_static),
        SymbolKind::Method | SymbolKind::Property | SymbolKind::Constant => {
            typed_receiver(index, declaration, false)
        }
    }
}

fn typed_receiver(
    index: &SymbolIndex,
    declaration: &SymbolDeclaration,
    constructor: bool,
) -> Option<Receiver> {
    declaration.type_source.as_ref().map_or_else(
        || {
            declaration
                .type_name
                .as_deref()
                .and_then(|name| named_type(index, name, constructor))
        },
        |source| resolve_type_source(index, source, constructor),
    )
}

fn resolve_type_source(
    index: &SymbolIndex,
    source: &PhpTypeSource,
    constructor: bool,
) -> Option<Receiver> {
    match source {
        PhpTypeSource::Class(name) => named_type(index, name, constructor),
        PhpTypeSource::Symbol(names) => {
            let declaration = names.iter().find_map(|name| index.resolve(name))?;
            match declaration.kind {
                SymbolKind::Class => Some(if constructor {
                    Receiver::Static(declaration_type(declaration))
                } else {
                    Receiver::Instance(declaration_type(declaration))
                }),
                SymbolKind::Function => Some(Receiver::Instance("Function".into())),
                SymbolKind::Variable | SymbolKind::Method | SymbolKind::Property => {
                    receiver_from_declaration(index, declaration, constructor)
                }
                SymbolKind::Constant => None,
            }
        }
        PhpTypeSource::Member(receiver, name) => {
            let owner = resolve_type_source(index, receiver, false)?;
            member_receiver(Some(index), &owner, name)
        }
    }
}

fn declaration_type(declaration: &SymbolDeclaration) -> String {
    declaration
        .type_name
        .clone()
        .filter(|name| name.contains("::") || js_stdlib::type_named(name).is_some())
        .unwrap_or_else(|| declaration.fqn.clone())
}

fn named_type(index: &SymbolIndex, name: &str, constructor: bool) -> Option<Receiver> {
    if let Some(declaration) = index.declaration(name) {
        return match declaration.kind {
            SymbolKind::Class => Some(if constructor {
                Receiver::Static(declaration.fqn.clone())
            } else {
                Receiver::Instance(declaration.fqn.clone())
            }),
            SymbolKind::Function => Some(Receiver::Instance("Function".into())),
            SymbolKind::Variable
            | SymbolKind::Method
            | SymbolKind::Property
            | SymbolKind::Constant => receiver_from_declaration(index, declaration, constructor),
        };
    }
    if js_stdlib::type_named(name).is_some() {
        return Some(if constructor {
            Receiver::Static(name.to_string())
        } else {
            Receiver::Instance(name.to_string())
        });
    }
    if crate::js_symbols::is_object_shape(name) {
        return Some(Receiver::Instance(name.to_string()));
    }
    None
}

fn literal_type(atom: &str) -> Option<&'static str> {
    if matches!(atom, "true" | "false") {
        return Some("Boolean");
    }
    if (atom.starts_with('"') && atom.ends_with('"') && atom.len() >= 2)
        || (atom.starts_with('\'') && atom.ends_with('\'') && atom.len() >= 2)
        || (atom.starts_with('`') && atom.ends_with('`') && atom.len() >= 2)
    {
        return Some("String");
    }
    if atom.starts_with('/') && atom.len() > 2 && atom.matches('/').count() >= 2 {
        return Some("RegExp");
    }
    if atom.starts_with('[') && atom.ends_with(']') {
        return Some("Array");
    }
    if atom.starts_with('{') && atom.ends_with('}') {
        return Some("Object");
    }
    let bytes = atom.as_bytes();
    let numeric = !bytes.is_empty()
        && bytes.iter().any(u8::is_ascii_digit)
        && bytes.iter().all(|byte| {
            byte.is_ascii_hexdigit()
                || matches!(
                    byte,
                    b'_' | b'.' | b'x' | b'X' | b'e' | b'E' | b'+' | b'-' | b'n'
                )
        });
    numeric.then_some("Number")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        js_symbols,
        symbol_index::{LanguageId, SymbolIndex},
    };
    use std::path::PathBuf;

    fn labels(source: &str, index: Option<&SymbolIndex>, absolute: usize) -> Vec<String> {
        complete(
            index,
            Path::new("sample.js"),
            source,
            source.len(),
            absolute,
        )
        .into_iter()
        .map(|item| item.label)
        .collect()
    }

    #[test]
    fn instance_members_hide_static_and_private() -> Result<(), Box<dyn std::error::Error>> {
        let source = "class Car { drive() {} static build() {} #hidden() {} }\nconst car = new Car();\ncar.\n";
        let path = PathBuf::from("sample.js");
        let mut index = SymbolIndex::new(LanguageId::JavaScript);
        index.replace(path.clone(), js_symbols::parse(&path, source)?);
        let dot = source.rfind('.').ok_or("dot")?;
        let line = "car.";
        let items = complete(Some(&index), &path, line, line.len(), dot + 1);
        let labels: Vec<_> = items.iter().map(|item| item.label.as_str()).collect();
        assert!(labels.contains(&"drive"), "{labels:?}");
        assert!(!labels.contains(&"build"), "{labels:?}");
        assert!(!labels.contains(&"#hidden"), "{labels:?}");
        Ok(())
    }

    #[test]
    fn stdlib_array_math_keyword_and_decimal() {
        let array: Vec<_> = labels("Arr", None, 3);
        assert!(array.iter().any(|label| label == "Array"));
        let math = labels("Math.", None, 5);
        assert!(math.iter().any(|label| label == "max"));
        assert!(!math.iter().any(|label| label == "push"));
        let keyword = labels("fun", None, 3);
        assert!(keyword.iter().any(|label| label == "function"));
        assert_eq!(labels("1.", None, 2), Vec::<String>::new());
        assert_eq!(labels("1.5", None, 3), Vec::<String>::new());
        let fixed = labels("(1).toF", None, 7);
        assert!(fixed.iter().any(|label| label == "toFixed"));
        let document = labels("document.", None, 9);
        assert!(document.iter().any(|label| label == "querySelectorAll"));
        assert!(document.iter().any(|label| label == "getElementById"));
        assert!(document.iter().any(|label| label == "addEventListener"));
        assert!(!document.iter().any(|label| label == "push"));
    }

    #[test]
    fn query_selector_all_types_the_collection_and_its_items()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = "const links = document.querySelectorAll('.js-contact-show');\nlinks.forEach(function (link) {\n    link.addEventListener('click', handleClick);\n});\n";
        let path = PathBuf::from("sample.js");
        let mut index = SymbolIndex::new(LanguageId::JavaScript);
        index.replace(path.clone(), js_symbols::parse(&path, source)?);
        let collection_dot = source.find("links.").ok_or("collection")?;
        let element_dot = source.find("link.").ok_or("element")?;
        let collection = complete(
            Some(&index),
            &path,
            "links.",
            "links.".len(),
            collection_dot + "links.".len(),
        );
        let element = complete(
            Some(&index),
            &path,
            "link.",
            "link.".len(),
            element_dot + "link.".len(),
        );
        let collection: Vec<_> = collection.iter().map(|item| item.label.as_str()).collect();
        let element: Vec<_> = element.iter().map(|item| item.label.as_str()).collect();
        assert!(collection.contains(&"forEach"), "{collection:?}");
        assert!(collection.contains(&"length"), "{collection:?}");
        assert!(collection.contains(&"item"), "{collection:?}");
        assert!(!collection.contains(&"push"), "{collection:?}");
        assert!(element.contains(&"addEventListener"), "{element:?}");
        assert!(element.contains(&"querySelector"), "{element:?}");
        assert!(element.contains(&"classList"), "{element:?}");
        Ok(())
    }

    #[test]
    fn host_catalog_offers_page_script_members() {
        let queried = labels("document.querySelector('a').", None, 28);
        for name in [
            "style",
            "innerText",
            "dataset",
            "value",
            "href",
            "addEventListener",
        ] {
            assert!(queried.iter().any(|label| label == name), "{queried:?}");
        }
        let style = labels("document.querySelector('a').style.", None, 34);
        assert!(style.iter().any(|label| label == "backgroundColor"));
        assert!(style.iter().any(|label| label == "setProperty"));
        let address = labels("new URL('https://example.com').", None, 30);
        assert!(address.iter().any(|label| label == "searchParams"));
        let form = labels("new FormData().", None, 14);
        assert!(form.iter().any(|label| label == "append"));
        let history = labels("history.", None, 8);
        assert!(history.iter().any(|label| label == "pushState"));
        let crypto = labels("crypto.", None, 7);
        assert!(crypto.iter().any(|label| label == "randomUUID"));
        let digest = labels("crypto.subtle.", None, 14);
        assert!(digest.iter().any(|label| label == "digest"));
        let format = labels("Intl.NumberFormat().", None, 20);
        assert!(format.iter().any(|label| label == "format"));
        let global_document = labels("globalThis.document.", None, 20);
        assert!(
            global_document
                .iter()
                .any(|label| label == "querySelectorAll")
        );
        let screen = labels("screen.", None, 7);
        assert!(screen.iter().any(|label| label == "width"));
        let node = labels("Node.", None, 5);
        assert!(node.iter().any(|label| label == "ELEMENT_NODE"));
        let fetched = labels("fetch(url).", None, 10);
        assert!(fetched.iter().any(|label| label == "then"));
        assert!(!fetched.iter().any(|label| label == "json"));
    }

    #[test]
    fn object_literal_offers_own_keys_after_dot() -> Result<(), Box<dyn std::error::Error>> {
        let source = "const payload = {\n    owner_id: parseInt(link.dataset.owner, 10),\n    contact_type: link.dataset.type,\n    source: link.dataset.source,\n    meta: { kind: \"page\" },\n};\nconst copy = payload;\npayload.\n";
        let path = PathBuf::from("sample.js");
        let mut index = SymbolIndex::new(LanguageId::JavaScript);
        index.replace(path.clone(), js_symbols::parse(&path, source)?);
        let dot = source.rfind('.').ok_or("dot")?;
        let names = |line: &str| {
            complete(Some(&index), &path, line, line.len(), dot + 1)
                .into_iter()
                .map(|item| item.label)
                .collect::<Vec<_>>()
        };
        let payload = names("payload.");
        for name in ["owner_id", "contact_type", "source", "meta", "toString"] {
            assert!(payload.iter().any(|label| label == name), "{payload:?}");
        }
        let keys = payload
            .iter()
            .position(|label| label == "owner_id")
            .ok_or("owner_id order")?;
        let prototype = payload
            .iter()
            .position(|label| label == "toString")
            .ok_or("toString order")?;
        assert!(keys < prototype, "{payload:?}");
        let copy = names("copy.");
        assert!(copy.iter().any(|label| label == "owner_id"), "{copy:?}");
        let nested = names("payload.meta.");
        assert!(nested.iter().any(|label| label == "kind"), "{nested:?}");
        let number = names("payload.owner_id.");
        assert!(number.iter().any(|label| label == "toFixed"), "{number:?}");
        let blank_source = "const blank = {};\nblank.\n";
        index.replace(path.clone(), js_symbols::parse(&path, blank_source)?);
        let blank_dot = blank_source.rfind('.').ok_or("blank")?;
        let blank = complete(Some(&index), &path, "blank.", "blank.".len(), blank_dot + 1);
        assert!(blank.iter().any(|item| item.label == "toString"));
        assert!(!blank.iter().any(|item| item.label == "owner_id"));
        Ok(())
    }

    #[test]
    fn member_completion_ignores_statements_before_the_dot()
    -> Result<(), Box<dyn std::error::Error>> {
        let path = PathBuf::from("sample.js");
        let labels_of = |source: &str| -> Result<Vec<String>, Box<dyn std::error::Error>> {
            let caret = source.rfind('.').ok_or("dot")? + 1;
            Ok(complete(None, &path, source, caret, caret)
                .into_iter()
                .map(|item| item.label)
                .collect())
        };
        let document = labels_of("function init() {\n    document.\n}\n")?;
        assert!(
            document.iter().any(|label| label == "querySelectorAll"),
            "{document:?}"
        );
        let console = labels_of("function init() {\n    console.\n}\n")?;
        assert!(console.iter().any(|label| label == "log"), "{console:?}");
        let math = labels_of("function init() {\n    Math.\n}\n")?;
        assert!(math.iter().any(|label| label == "max"), "{math:?}");
        let address =
            labels_of("function init() {\n    return new URL('https://example.com').\n}\n")?;
        assert!(
            address.iter().any(|label| label == "searchParams"),
            "{address:?}"
        );
        let field = labels_of("const row = {\n    source: document.\n};\n")?;
        assert!(
            field.iter().any(|label| label == "querySelectorAll"),
            "{field:?}"
        );
        let source = "function init() {\n    const payload = {\n        owner_id: parseInt(link.dataset.owner, 10),\n        contact_type: link.dataset.type,\n        source: link.dataset.source,\n    };\n    payload.\n}\n";
        let mut index = SymbolIndex::new(LanguageId::JavaScript);
        index.replace(path.clone(), js_symbols::parse(&path, source)?);
        let caret = source.rfind('.').ok_or("payload dot")? + 1;
        let payload: Vec<_> = complete(Some(&index), &path, source, caret, caret)
            .into_iter()
            .map(|item| item.label)
            .collect();
        for name in ["owner_id", "contact_type", "source", "toString"] {
            assert!(payload.iter().any(|label| label == name), "{payload:?}");
        }
        Ok(())
    }
}
