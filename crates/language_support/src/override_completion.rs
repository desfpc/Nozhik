//! PHP override completion for method names, parameters and return types.
//!
//! A child class can complete `public` and `protected` methods of its parent
//! classes, interfaces and traits. `private` members stay on the declaring type.
//! The typed source may be a single line; brace depth uses the indexed class
//! prefix when the line no longer contains the class body.
use crate::{
    php_stubs::{PhpParameter, PhpSymbol, PhpSymbolKind, PhpVersionStubs},
    symbol_index::{SymbolDeclaration, SymbolIndex, SymbolKind, Visibility},
};
use lsp_types::{CompletionItem, CompletionItemKind, CompletionItemLabelDetails, Documentation};
use std::{collections::HashSet, path::Path};

/// Local completion plus whether unrelated global names should stay hidden.
pub struct CompletionBatch {
    /// Suggestions in display order.
    pub items: Vec<CompletionItem>,
    /// The caret is in a new or edited method name at class-body level.
    pub method_name: bool,
}

/// One indexed or catalog method the child is allowed to declare.
struct InheritedMethod {
    name: String,
    owner: String,
    visibility: Visibility,
    is_static: bool,
    parameters: Vec<String>,
    return_type: String,
    documentation: String,
}

/// What the caret is editing inside a method declaration.
enum Focus {
    Name { private: bool },
    Parameters { method: String, inside: String },
    Return { method: String },
}

/// Completions for one caret, including inherited PHP methods when declared.
#[allow(
    clippy::too_many_arguments,
    reason = "One completion pass needs the index, both caret coordinates, stubs, class text and ordinary items"
)]
#[must_use]
pub fn complete(
    index: &SymbolIndex,
    path: &Path,
    source: &str,
    byte: usize,
    absolute_byte: usize,
    stubs: Option<&PhpVersionStubs>,
    class_prefix: Option<&str>,
    ordinary: Vec<CompletionItem>,
) -> CompletionBatch {
    let Some(focus) = focus_at(source, byte) else {
        return CompletionBatch {
            items: ordinary,
            method_name: false,
        };
    };
    let Some(class) = enclosing_class(index, path, absolute_byte) else {
        return CompletionBatch {
            items: ordinary,
            method_name: false,
        };
    };
    if !at_class_body(source, byte, absolute_byte, class.scope.start, class_prefix) {
        return CompletionBatch {
            items: ordinary,
            method_name: false,
        };
    }
    let inherited = inherited_methods(index, &class.fqn, absolute_byte, stubs);
    match focus {
        Focus::Name { private: true } => CompletionBatch {
            items: ordinary,
            method_name: false,
        },
        Focus::Name { private: false } => {
            let prefix = ident_prefix(source, byte);
            CompletionBatch {
                items: method_items(&inherited, prefix),
                method_name: true,
            }
        }
        Focus::Parameters { method, inside } => {
            let chunk = last_argument(&inside).trim().to_string();
            CompletionBatch {
                items: merge(
                    parameter_items(&inherited, &method, &inside, &chunk),
                    ordinary,
                ),
                method_name: false,
            }
        }
        Focus::Return { method } => {
            let prefix = ident_prefix(source, byte);
            CompletionBatch {
                items: merge(return_items(&inherited, &method, prefix), ordinary),
                method_name: false,
            }
        }
    }
}

/// Class byte range containing `byte`, preferring the innermost type.
#[must_use]
pub fn class_scope_containing(
    index: &SymbolIndex,
    path: &Path,
    byte: usize,
) -> Option<std::ops::Range<usize>> {
    enclosing_class(index, path, byte).map(|class| class.scope.clone())
}

fn enclosing_class<'a>(
    index: &'a SymbolIndex,
    path: &Path,
    byte: usize,
) -> Option<&'a SymbolDeclaration> {
    index
        .declarations_by_file
        .get(path)?
        .iter()
        .filter(|declaration| {
            declaration.kind == SymbolKind::Class && declaration.scope.contains(&byte)
        })
        .min_by_key(|declaration| declaration.scope.len())
}

fn at_class_body(
    source: &str,
    byte: usize,
    absolute_byte: usize,
    class_start: usize,
    class_prefix: Option<&str>,
) -> bool {
    if let Some(prefix) = class_prefix {
        return brace_depth(prefix) == 1;
    }
    if absolute_byte == byte
        && let Some(slice) = source.get(class_start..byte)
    {
        return brace_depth(slice) == 1;
    }
    // A single edited line has no class brace. The indexed prefix is supplied
    // once the project snapshot exists; until then keep the declaration usable.
    true
}

fn brace_depth(text: &str) -> i32 {
    let mut depth = 0_i32;
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '\'' => skip_quoted(&mut chars, '\''),
            '"' => skip_quoted(&mut chars, '"'),
            '#' => skip_line(&mut chars),
            '/' if chars.peek() == Some(&'/') => {
                chars.next();
                skip_line(&mut chars);
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                skip_block(&mut chars);
            }
            '{' => depth += 1,
            '}' => depth -= 1,
            _ => {}
        }
    }
    depth
}

fn skip_quoted(chars: &mut std::iter::Peekable<std::str::Chars<'_>>, quote: char) {
    while let Some(character) = chars.next() {
        if character == '\\' {
            chars.next();
        } else if character == quote {
            break;
        }
    }
}

fn skip_line(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    for character in chars.by_ref() {
        if character == '\n' {
            break;
        }
    }
}

fn skip_block(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    while let Some(character) = chars.next() {
        if character == '*' && chars.peek() == Some(&'/') {
            chars.next();
            break;
        }
    }
}

fn focus_at(source: &str, byte: usize) -> Option<Focus> {
    let head = source.get(..byte)?;
    let statement = statement_text(head);
    let (private, after_modifiers) = modifiers(statement);
    let after_function = after_modifiers.strip_prefix("function")?;
    if after_function
        .chars()
        .next()
        .is_some_and(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        return None;
    }
    let after_function = after_function.trim_start();
    let (name, after_name) = leading_ident(after_function);
    let after_name = after_name.trim_start();
    if after_name.is_empty() {
        return Some(Focus::Name { private });
    }
    let inside = after_name.strip_prefix('(')?;
    if name.is_empty() {
        return None;
    }
    if let Some(after_params) = closed_params(inside) {
        let after_params = after_params.trim_start();
        if after_params.starts_with(':') && !after_params.starts_with("::") {
            return Some(Focus::Return {
                method: name.to_string(),
            });
        }
        return None;
    }
    Some(Focus::Parameters {
        method: name.to_string(),
        inside: inside.to_string(),
    })
}

fn statement_text(head: &str) -> &str {
    let start = head
        .char_indices()
        .rev()
        .find_map(|(index, character)| {
            (character == '{' || character == ';' || character == '}')
                .then_some(index + character.len_utf8())
        })
        .unwrap_or(0);
    head.get(start..).unwrap_or(head).trim_start()
}

fn modifiers(mut text: &str) -> (bool, &str) {
    let mut private = false;
    loop {
        let trimmed = text.trim_start();
        let Some((word, rest)) = next_word(trimmed) else {
            return (private, trimmed);
        };
        if !matches!(
            word,
            "public"
                | "protected"
                | "private"
                | "static"
                | "abstract"
                | "final"
                | "readonly"
                | "var"
        ) {
            return (private, trimmed);
        }
        private |= word == "private";
        text = rest;
    }
}

fn next_word(text: &str) -> Option<(&str, &str)> {
    let end = text
        .find(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
        .unwrap_or(text.len());
    (end > 0).then_some((&text[..end], &text[end..]))
}

fn leading_ident(text: &str) -> (&str, &str) {
    let end = text
        .find(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
        .unwrap_or(text.len());
    (&text[..end], &text[end..])
}

fn ident_prefix(source: &str, byte: usize) -> &str {
    let head = source.get(..byte).unwrap_or("");
    let start = head
        .char_indices()
        .rev()
        .find_map(|(index, character)| {
            (!(character.is_ascii_alphanumeric()
                || character == '_'
                || character == '\\'
                || character == '$'))
                .then_some(index + character.len_utf8())
        })
        .unwrap_or(0);
    head.get(start..).unwrap_or("")
}

fn closed_params(inside: &str) -> Option<&str> {
    let mut depth = 0_i32;
    for (index, character) in inside.char_indices() {
        match character {
            '(' => depth += 1,
            ')' if depth == 0 => return inside.get(index + 1..),
            ')' => depth -= 1,
            _ => {}
        }
    }
    None
}

fn last_argument(inside: &str) -> &str {
    let mut depth = 0_i32;
    let mut start = 0;
    for (index, character) in inside.char_indices() {
        match character {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            ',' if depth == 0 => start = index + 1,
            _ => {}
        }
    }
    inside.get(start..).unwrap_or(inside)
}

fn inherited_methods(
    index: &SymbolIndex,
    class_fqn: &str,
    caret: usize,
    stubs: Option<&PhpVersionStubs>,
) -> Vec<InheritedMethod> {
    let lineage = index.class_lineage(class_fqn);
    let mut rank = std::collections::HashMap::<String, usize>::new();
    for (position, name) in lineage.iter().enumerate().skip(1) {
        rank.entry(name.clone()).or_insert(position);
    }
    let own = own_method_names(index, class_fqn, caret);
    let mut best: Vec<InheritedMethod> = Vec::new();
    for declaration in index.declarations() {
        let Some(owner) = declaration.owner.as_deref() else {
            continue;
        };
        if declaration.kind != SymbolKind::Method
            || !rank.contains_key(owner)
            || !accessible(declaration.visibility)
            || is_final(&declaration.signature)
            || own.contains(&declaration.name.to_lowercase())
        {
            continue;
        }
        let Some(method) = from_declaration(index, declaration) else {
            continue;
        };
        remember(&mut best, &rank, owner, method);
    }
    if let Some(stubs) = stubs {
        for owner in lineage.iter().skip(1) {
            if index.declaration(owner).is_some() {
                continue;
            }
            let Some(entity) = global_stub(stubs, owner) else {
                continue;
            };
            for method in &entity.methods {
                if method.kind != PhpSymbolKind::Method || own.contains(&method.name.to_lowercase())
                {
                    continue;
                }
                let mut inherited = from_stub(entity, method);
                inherited.owner.clone_from(owner);
                remember(&mut best, &rank, owner, inherited);
            }
        }
    }
    best.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then(left.name.cmp(&right.name))
    });
    best
}

fn remember(
    best: &mut Vec<InheritedMethod>,
    rank: &std::collections::HashMap<String, usize>,
    owner: &str,
    method: InheritedMethod,
) {
    let owner_rank = rank.get(owner).copied().unwrap_or(usize::MAX);
    if let Some(existing) = best
        .iter_mut()
        .find(|item| item.name.eq_ignore_ascii_case(&method.name))
    {
        let existing_rank = rank.get(&existing.owner).copied().unwrap_or(usize::MAX);
        if owner_rank < existing_rank {
            *existing = method;
        }
        return;
    }
    best.push(method);
}

fn own_method_names(index: &SymbolIndex, class_fqn: &str, caret: usize) -> HashSet<String> {
    index
        .declarations()
        .filter(|declaration| {
            declaration.kind == SymbolKind::Method
                && declaration.owner.as_deref() == Some(class_fqn)
                && !declaration.range.contains(&caret)
                && caret != declaration.range.end
        })
        .map(|declaration| declaration.name.to_lowercase())
        .collect()
}

const fn accessible(visibility: Visibility) -> bool {
    matches!(visibility, Visibility::Public | Visibility::Protected)
}

fn is_final(signature: &str) -> bool {
    signature
        .split("function")
        .next()
        .unwrap_or("")
        .split_whitespace()
        .any(|word| word == "final")
}

/// Parameter list and return type from the text after the opening `(`.
fn parameter_and_return(inside: &str) -> (&str, &str) {
    let Some(after) = closed_params(inside) else {
        return (inside, "");
    };
    let close_at = inside.len().saturating_sub(after.len());
    let params = inside.get(..close_at.saturating_sub(1)).unwrap_or("");
    let return_type = after
        .trim_start()
        .strip_prefix(':')
        .map(str::trim)
        .filter(|text| !text.is_empty() && !text.starts_with(':'))
        .unwrap_or("");
    (params, return_type)
}

fn from_declaration(
    index: &SymbolIndex,
    declaration: &SymbolDeclaration,
) -> Option<InheritedMethod> {
    let tail = callable_tail(&declaration.signature)?
        .trim()
        .trim_end_matches(';')
        .trim();
    let (name, after_name) = leading_ident(tail);
    let inside = after_name.trim_start().strip_prefix('(')?;
    let (params_raw, return_type) = parameter_and_return(inside);
    let owner = declaration.owner.clone().unwrap_or_default();
    Some(InheritedMethod {
        name: if name.is_empty() {
            declaration.name.clone()
        } else {
            name.to_string()
        },
        owner: owner.clone(),
        visibility: declaration.visibility,
        is_static: declaration.is_static,
        parameters: split_arguments(params_raw),
        return_type: return_type.trim_end_matches(';').trim().to_string(),
        documentation: owner_label(index, &owner),
    })
}

fn callable_tail(signature: &str) -> Option<&str> {
    let lower = signature.to_ascii_lowercase();
    let mut from = 0;
    while let Some(relative) = lower[from..].find("function") {
        let index = from + relative;
        let end = index + "function".len();
        let before = index == 0 || !ident_byte(lower.as_bytes().get(index - 1).copied());
        let after = !ident_byte(lower.as_bytes().get(end).copied());
        if before && after {
            return signature.get(end..).map(str::trim);
        }
        from = index + 1;
    }
    None
}

fn ident_byte(byte: Option<u8>) -> bool {
    byte.is_some_and(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn split_arguments(list: &str) -> Vec<String> {
    let mut arguments = Vec::new();
    let mut depth = 0_i32;
    let mut start = 0;
    for (index, character) in list.char_indices() {
        match character {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            ',' if depth == 0 => {
                push_argument(&mut arguments, &list[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    push_argument(&mut arguments, list.get(start..).unwrap_or(""));
    arguments
}

fn push_argument(arguments: &mut Vec<String>, text: &str) {
    let text = text.trim().trim_end_matches(';').trim();
    if !text.is_empty() {
        arguments.push(text.to_string());
    }
}

fn from_stub(entity: &PhpSymbol, method: &PhpSymbol) -> InheritedMethod {
    let return_type = if method.return_type.is_empty() || method.return_type == "mixed" {
        String::new()
    } else {
        method.return_type.clone()
    };
    InheritedMethod {
        name: method.name.clone(),
        owner: format!("\\{}", entity.name.to_lowercase()),
        visibility: Visibility::Public,
        is_static: method.is_static,
        parameters: method.parameters.iter().map(stub_parameter).collect(),
        return_type,
        documentation: entity.name.clone(),
    }
}

fn stub_parameter(parameter: &PhpParameter) -> String {
    let mut text = String::new();
    if parameter.variadic {
        text.push_str("...");
    }
    if !parameter.r#type.is_empty() {
        text.push_str(&parameter.r#type);
        text.push(' ');
    }
    if parameter.by_reference {
        text.push('&');
    }
    text.push('$');
    text.push_str(&parameter.name);
    if let Some(default) = &parameter.default {
        text.push_str(" = ");
        text.push_str(default);
    }
    text
}

fn global_stub<'a>(stubs: &'a PhpVersionStubs, fqn: &str) -> Option<&'a PhpSymbol> {
    let name = fqn.trim_matches('\\');
    if name.is_empty() || name.contains('\\') {
        return None;
    }
    stubs.entities.iter().find(|entity| {
        matches!(entity.kind, PhpSymbolKind::Class | PhpSymbolKind::Interface)
            && entity.name.eq_ignore_ascii_case(name)
    })
}

fn owner_label(index: &SymbolIndex, owner: &str) -> String {
    if let Some(declaration) = index.declaration(owner) {
        return declaration.name.clone();
    }
    owner
        .rsplit('\\')
        .next()
        .filter(|part| !part.is_empty())
        .unwrap_or(owner)
        .to_string()
}

fn method_items(methods: &[InheritedMethod], prefix: &str) -> Vec<CompletionItem> {
    methods
        .iter()
        .filter(|method| {
            method
                .name
                .to_lowercase()
                .starts_with(&prefix.to_lowercase())
        })
        .take(100)
        .map(method_item)
        .collect()
}

fn method_item(method: &InheritedMethod) -> CompletionItem {
    let params = method.parameters.join(", ");
    let return_suffix = if method.return_type.is_empty() {
        String::new()
    } else {
        format!(": {}", method.return_type)
    };
    let insert = format!("{}({params}){return_suffix}", method.name);
    let mut description = visibility_word(method.visibility).to_string();
    if method.is_static {
        description.push_str(" static");
    }
    description.push(' ');
    description.push_str(&method.documentation);
    CompletionItem {
        label: method.name.clone(),
        kind: Some(CompletionItemKind::METHOD),
        detail: Some(insert.clone()),
        label_details: Some(CompletionItemLabelDetails {
            detail: Some(format!("({params}){return_suffix}")),
            description: Some(description),
        }),
        documentation: Some(Documentation::String(insert.clone())),
        insert_text: Some(insert),
        filter_text: Some(method.name.clone()),
        sort_text: Some(format!("0{}", method.name.to_lowercase())),
        data: Some(serde_json::json!({"nozhik_override": "method"})),
        ..CompletionItem::default()
    }
}

fn parameter_items(
    methods: &[InheritedMethod],
    method_name: &str,
    inside: &str,
    chunk: &str,
) -> Vec<CompletionItem> {
    let Some(method) = methods
        .iter()
        .find(|method| method.name.eq_ignore_ascii_case(method_name))
    else {
        return Vec::new();
    };
    let last = last_argument(inside);
    let split = inside.len().saturating_sub(last.len());
    let previous = inside.get(..split).unwrap_or("");
    method
        .parameters
        .iter()
        .filter(|parameter| !parameter_used(previous, parameter))
        .filter(|parameter| parameter_matches(parameter, chunk))
        .take(100)
        .map(|parameter| parameter_item(parameter, &method.name))
        .collect()
}

fn parameter_used(previous: &str, parameter: &str) -> bool {
    let Some(variable) = variable_name(parameter) else {
        return false;
    };
    previous
        .split(|character: char| {
            !(character.is_ascii_alphanumeric() || character == '_' || character == '$')
        })
        .any(|word| word == variable)
}

fn parameter_matches(parameter: &str, chunk: &str) -> bool {
    if chunk.is_empty() {
        return true;
    }
    let parameter_lower = parameter.to_lowercase();
    let chunk_lower = chunk.to_lowercase();
    if parameter_lower.starts_with(&chunk_lower) {
        return true;
    }
    variable_name(parameter).is_some_and(|name| name.to_lowercase().starts_with(&chunk_lower))
        || type_name(parameter).is_some_and(|name| {
            name.trim_start_matches(['?', '\\'])
                .to_lowercase()
                .starts_with(chunk_lower.trim_start_matches(['?', '\\', '&']))
        })
}

fn variable_name(parameter: &str) -> Option<&str> {
    let start = parameter.find('$')?;
    let rest = parameter.get(start..)?;
    let end = rest
        .find(|character: char| {
            !(character.is_ascii_alphanumeric() || character == '_' || character == '$')
        })
        .unwrap_or(rest.len());
    rest.get(..end).filter(|name| name.len() > 1)
}

fn type_name(parameter: &str) -> Option<&str> {
    let variable = variable_name(parameter)?;
    let start = parameter.find(variable)?;
    let text = parameter.get(..start)?.trim();
    (!text.is_empty()).then_some(text)
}

fn parameter_item(parameter: &str, method: &str) -> CompletionItem {
    CompletionItem {
        label: parameter.to_string(),
        kind: Some(CompletionItemKind::VARIABLE),
        detail: Some(parameter.to_string()),
        label_details: Some(CompletionItemLabelDetails {
            detail: None,
            description: Some(method.to_string()),
        }),
        documentation: Some(Documentation::String(format!("{method}({parameter})"))),
        insert_text: Some(parameter.to_string()),
        filter_text: Some(parameter.to_string()),
        sort_text: Some(format!("0{parameter}")),
        data: Some(serde_json::json!({"nozhik_override": "parameter"})),
        ..CompletionItem::default()
    }
}

fn return_items(
    methods: &[InheritedMethod],
    method_name: &str,
    prefix: &str,
) -> Vec<CompletionItem> {
    let Some(method) = methods
        .iter()
        .find(|method| method.name.eq_ignore_ascii_case(method_name))
    else {
        return Vec::new();
    };
    if method.return_type.is_empty() {
        return Vec::new();
    }
    let typed = prefix.trim_start_matches(['?', '\\']);
    if !typed.is_empty()
        && !method
            .return_type
            .trim_start_matches(['?', '\\'])
            .to_lowercase()
            .starts_with(&typed.to_lowercase())
        && !method
            .return_type
            .to_lowercase()
            .starts_with(&prefix.to_lowercase())
    {
        return Vec::new();
    }
    vec![CompletionItem {
        label: method.return_type.clone(),
        kind: Some(
            if method
                .return_type
                .chars()
                .next()
                .is_some_and(|character| character == '\\' || character.is_uppercase())
            {
                CompletionItemKind::CLASS
            } else {
                CompletionItemKind::KEYWORD
            },
        ),
        detail: Some(method.return_type.clone()),
        label_details: Some(CompletionItemLabelDetails {
            detail: None,
            description: Some(method.name.clone()),
        }),
        documentation: Some(Documentation::String(format!(
            "{}: {}",
            method.name, method.return_type
        ))),
        insert_text: Some(method.return_type.clone()),
        filter_text: Some(method.return_type.clone()),
        sort_text: Some(format!("0{}", method.return_type.to_lowercase())),
        data: Some(serde_json::json!({"nozhik_override": "return"})),
        ..CompletionItem::default()
    }]
}

fn merge(mut primary: Vec<CompletionItem>, ordinary: Vec<CompletionItem>) -> Vec<CompletionItem> {
    for item in ordinary {
        if !primary
            .iter()
            .any(|existing| existing.label.eq_ignore_ascii_case(&item.label))
        {
            primary.push(item);
        }
    }
    primary.truncate(100);
    primary
}

const fn visibility_word(visibility: Visibility) -> &'static str {
    match visibility {
        Visibility::Public => "public",
        Visibility::Protected => "protected",
        Visibility::Private => "private",
    }
}

/// Widens a plain identifier replacement to the parameter or return type being edited.
#[must_use]
pub fn replacement_start(source: &str, caret: usize, kind: &str) -> Option<usize> {
    match kind {
        "parameter" => parameter_start(source, caret),
        "return" => return_start(source, caret),
        _ => None,
    }
}

fn parameter_start(source: &str, caret: usize) -> Option<usize> {
    let head = source.get(..caret)?;
    let open = open_paren(head)?;
    let inside = source.get(open + 1..caret)?;
    let mut depth = 0_i32;
    let mut start = 0;
    for (index, character) in inside.char_indices() {
        match character {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            ',' if depth == 0 => start = index + 1,
            _ => {}
        }
    }
    let relative = inside.get(start..).unwrap_or("").len()
        - inside.get(start..).unwrap_or("").trim_start().len();
    Some(open + 1 + start + relative)
}

fn open_paren(head: &str) -> Option<usize> {
    let mut depth = 0_i32;
    for (index, character) in head.char_indices().rev() {
        match character {
            ')' => depth += 1,
            '(' if depth == 0 => return Some(index),
            '(' => depth -= 1,
            _ => {}
        }
    }
    None
}

fn return_start(source: &str, caret: usize) -> Option<usize> {
    let head = source.get(..caret)?;
    let colon = head.rfind(':')?;
    if head.as_bytes().get(colon.wrapping_sub(1)) == Some(&b':')
        || head.as_bytes().get(colon + 1) == Some(&b':')
    {
        return None;
    }
    let after = colon + 1;
    let skip = source
        .get(after..caret)?
        .find(|character: char| !character.is_whitespace())
        .unwrap_or(caret - after);
    Some(after + skip)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::symbol_index::LanguageId;

    fn indexed(files: &[(&str, &str)]) -> crate::Result<SymbolIndex> {
        let mut index = SymbolIndex::new(LanguageId::Php);
        for (name, source) in files {
            let path = std::path::PathBuf::from(name);
            index.replace(path.clone(), crate::php_symbols::parse(&path, source)?);
        }
        Ok(index)
    }

    fn caret_at(source: &str, marker: &str) -> crate::Result<usize> {
        Ok(source
            .rfind(marker)
            .ok_or_else(|| crate::Error::Protocol(format!("missing {marker}")))?
            + marker.len())
    }

    #[test]
    fn suggests_public_and_protected_parent_and_interface_methods()
    -> Result<(), Box<dyn std::error::Error>> {
        let parent = "<?php namespace App;\ninterface Contract { public function test1(User $user): void; public function other(): int; }\nclass ParentType implements Contract { public function test1(User $user): void {} public function test2(int $n = 1): string {} protected function test3(): void {} private function testHidden(): void {} final public function testFinal(): void {} public function alpha(): void {} }\n";
        let child = "<?php namespace App;\nclass Child extends ParentType {\n    public function kept(): void { function t }\n    public function t\n}\n";
        let index = indexed(&[("parent.php", parent), ("child.php", child)])?;
        let path = std::path::PathBuf::from("child.php");
        let caret = caret_at(child, "public function t")?;
        let batch = complete(&index, &path, child, caret, caret, None, None, Vec::new());
        assert!(batch.method_name);
        let labels: Vec<_> = batch.items.iter().map(|item| item.label.as_str()).collect();
        assert_eq!(labels, ["test1", "test2", "test3"]);
        assert_eq!(
            batch
                .items
                .first()
                .and_then(|item| item.insert_text.as_deref()),
            Some("test1(User $user): void")
        );
        assert_eq!(
            batch
                .items
                .get(1)
                .and_then(|item| item.insert_text.as_deref()),
            Some("test2(int $n = 1): string")
        );
        assert!(batch.items.get(2).is_some_and(|item| {
            item.label_details.as_ref().is_some_and(|details| {
                details
                    .description
                    .as_deref()
                    .is_some_and(|text| text.contains("protected") && text.contains("ParentType"))
            })
        }));
        let body = caret_at(child, "{ function t")?;
        let hidden = complete(
            &index,
            &path,
            child,
            body,
            body,
            None,
            None,
            vec![class_item("Throwable")],
        );
        assert!(!hidden.method_name);
        assert!(hidden.items.iter().any(|item| item.label == "Throwable"));
        let params_source = "<?php namespace App;\nclass Child extends ParentType {\n    public function test1(\n}\n";
        let params_index = indexed(&[("parent.php", parent), ("params.php", params_source)])?;
        let params_path = std::path::PathBuf::from("params.php");
        let params_caret = caret_at(params_source, "test1(")?;
        let params = complete(
            &params_index,
            &params_path,
            params_source,
            params_caret,
            params_caret,
            None,
            None,
            Vec::new(),
        );
        assert!(!params.method_name);
        assert!(params.items.iter().any(|item| item.label == "User $user"));
        let typed = "<?php namespace App;\nclass Child extends ParentType {\n    public function test2(int $n = 1): \n}\n";
        let return_index = indexed(&[("parent.php", parent), ("return.php", typed)])?;
        let return_path = std::path::PathBuf::from("return.php");
        let return_caret = caret_at(typed, ": ")?;
        let returned = complete(
            &return_index,
            &return_path,
            typed,
            return_caret,
            return_caret,
            None,
            None,
            Vec::new(),
        );
        assert!(
            returned
                .items
                .iter()
                .any(|item| item.insert_text.as_deref() == Some("string"))
        );
        Ok(())
    }

    #[test]
    fn suggests_stdlib_parent_methods_and_replaces_the_typed_parameter()
    -> Result<(), Box<dyn std::error::Error>> {
        let child =
            "<?php namespace App;\nclass Mine extends \\Exception {\n    public function getM\n}\n";
        let index = indexed(&[("mine.php", child)])?;
        let path = std::path::PathBuf::from("mine.php");
        let caret = caret_at(child, "getM")?;
        let stubs = PhpVersionStubs::builtin()?;
        let batch = complete(
            &index,
            &path,
            child,
            caret,
            caret,
            Some(&stubs),
            None,
            Vec::new(),
        );
        let message = batch
            .items
            .iter()
            .find(|item| item.label == "getMessage")
            .ok_or_else(|| crate::Error::Protocol("missing getMessage".into()))?;
        assert_eq!(message.insert_text.as_deref(), Some("getMessage(): string"));
        let parent = "<?php namespace App;\nclass ParentType { public function test2(int $n = 1): string {} }\n";
        let edited = "public function test2(int $n";
        let full =
            format!("<?php namespace App;\nclass Child extends ParentType {{\n    {edited}\n}}\n");
        let index = indexed(&[("parent.php", parent), ("child.php", &full)])?;
        let path = std::path::PathBuf::from("child.php");
        let caret = caret_at(&full, edited)?;
        let batch = complete(&index, &path, &full, caret, caret, None, None, Vec::new());
        let item = batch
            .items
            .iter()
            .find(|item| item.label == "int $n = 1")
            .ok_or_else(|| crate::Error::Protocol("missing parameter".into()))?;
        let replaced = crate::completion::edits(item, &full, caret)
            .ok_or_else(|| crate::Error::Protocol("missing edits".into()))?;
        let (range, text) = replaced
            .first()
            .ok_or_else(|| crate::Error::Protocol("missing edit".into()))?;
        assert_eq!(text, "int $n = 1");
        assert_eq!(&full[range.clone()], "int $n");
        Ok(())
    }

    fn class_item(label: &str) -> CompletionItem {
        CompletionItem {
            label: label.to_string(),
            kind: Some(CompletionItemKind::CLASS),
            ..CompletionItem::default()
        }
    }
}
