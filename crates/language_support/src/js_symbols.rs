//! Tree-sitter extraction of JavaScript declarations and reference candidates.
//! One file is parsed here. Other files are never reparsed; imports keep candidate names.

use crate::{
    js_stdlib,
    symbol_index::{
        FileSymbols, PhpTypeSource, ReferenceLocation, SymbolDeclaration, SymbolKind, Visibility,
    },
};
use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    path::{Component, Path, PathBuf},
};
use tree_sitter::Node;

struct Value {
    name: Option<String>,
    source: Option<PhpTypeSource>,
    constructor: bool,
}

impl Value {
    const fn unknown() -> Self {
        Self {
            name: None,
            source: None,
            constructor: false,
        }
    }

    fn typed(name: &str, constructor: bool) -> Self {
        Self {
            name: Some(name.to_string()),
            source: Some(PhpTypeSource::Class(name.to_string())),
            constructor,
        }
    }

    const fn candidates(names: Vec<String>, constructor: bool) -> Self {
        Self {
            name: None,
            source: Some(PhpTypeSource::Symbol(names)),
            constructor,
        }
    }
}

#[derive(Clone)]
enum ThisMode {
    Inherit,
    Instance(String),
    Static(String),
    Shadow,
}

#[derive(Clone)]
struct Frame {
    scope: Range<usize>,
    function_scope: Range<usize>,
    owner: Option<String>,
    this_mode: ThisMode,
    top_level: bool,
}

struct Facts<'a> {
    path: &'a Path,
    source: &'a str,
    classes: HashMap<String, String>,
    functions: HashMap<String, String>,
    imports: HashMap<String, Vec<String>>,
    reserved: HashSet<String>,
    symbols: FileSymbols,
}

/// Extracts declarations and references from one JavaScript source.
///
/// # Errors
/// Returns a parser failure. A tree that contains error nodes still yields partial facts.
pub fn parse(path: &Path, source: &str) -> crate::Result<FileSymbols> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_javascript::LANGUAGE.into())
        .map_err(|error| crate::Error::Protocol(error.to_string()))?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| crate::Error::Protocol("JavaScript index parse cancelled".into()))?;
    let root = tree.root_node();
    let mut facts = Facts {
        path,
        source,
        classes: HashMap::new(),
        functions: HashMap::new(),
        imports: HashMap::new(),
        reserved: HashSet::new(),
        symbols: FileSymbols::default(),
    };
    prescan(root, &mut facts);
    declare_imports(root, &mut facts);
    let frame = Frame {
        scope: root.byte_range(),
        function_scope: root.byte_range(),
        owner: None,
        this_mode: ThisMode::Shadow,
        top_level: true,
    };
    walk(root, &mut facts, &frame);
    Ok(facts.symbols)
}

fn text<'a>(node: Node<'_>, source: &'a str) -> &'a str {
    source.get(node.byte_range()).unwrap_or("")
}

fn child_kind<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|child| child.kind() == kind)
}

fn has_kind(node: Node<'_>, kind: &str) -> bool {
    child_kind(node, kind).is_some()
}

fn named(node: Node<'_>) -> Vec<Node<'_>> {
    node.named_children(&mut node.walk()).collect()
}

fn path_key(path: &Path) -> String {
    path.display().to_string()
}

fn qualified(path: &Path, name: &str) -> String {
    format!("{}::{name}", path_key(path))
}

fn local_name(path: &Path, scope_start: usize, name: &str) -> String {
    format!("{}:{scope_start}:{name}", path_key(path))
}

fn point(node: Node<'_>) -> (u32, u32) {
    let start = node.start_position();
    (
        u32::try_from(start.row).unwrap_or(u32::MAX),
        u32::try_from(start.column).unwrap_or(u32::MAX),
    )
}

fn reference_at(node: Node<'_>, source: &str, path: &Path) -> ReferenceLocation {
    let byte = node.start_byte();
    let (preview, preview_offset) = crate::symbol_index::reference_preview(source, byte);
    let (line, column) = point(node);
    ReferenceLocation {
        path: path.into(),
        byte,
        length: node.byte_range().len(),
        line,
        column,
        preview,
        preview_offset,
    }
}

fn signature_of(node: Node<'_>, source: &str) -> String {
    let raw = text(node, source);
    let end = raw.find('{').unwrap_or(raw.len());
    let mut collapsed = String::new();
    for word in raw[..end].split_whitespace() {
        if !collapsed.is_empty() {
            collapsed.push(' ');
        }
        collapsed.push_str(word);
        if collapsed.len() > 180 {
            break;
        }
    }
    if collapsed.len() > 180 {
        let mut limit = 180;
        while !collapsed.is_char_boundary(limit) {
            limit -= 1;
        }
        collapsed.truncate(limit);
        collapsed.push('…');
    }
    collapsed
}

fn documentation_of(node: Node<'_>, source: &str) -> String {
    let mut current = Some(node);
    for _ in 0..4 {
        let Some(cursor) = current else {
            break;
        };
        if let Some(previous) = cursor.prev_named_sibling()
            && previous.kind() == "comment"
        {
            let comment = text(previous, source);
            if comment.len() <= 2000 {
                return comment.to_string();
            }
            let mut limit = 2000;
            while !comment.is_char_boundary(limit) {
                limit -= 1;
            }
            return comment[..limit].to_string();
        }
        current = cursor.parent();
    }
    String::new()
}

fn jsdoc_name(comment: &str) -> Option<String> {
    for tag in ["@returns", "@return", "@type"] {
        let Some(index) = comment.find(tag) else {
            continue;
        };
        let mut rest = comment[index + tag.len()..].trim_start();
        if let Some(stripped) = rest.strip_prefix('{') {
            rest = stripped.trim_start();
        }
        let end = rest
            .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$'))
            .unwrap_or(rest.len());
        let name = &rest[..end];
        if name.is_empty() || matches!(name, "void" | "null" | "undefined") {
            return None;
        }
        return Some(name.to_string());
    }
    None
}

fn unquote(raw: &str) -> Option<&str> {
    let bytes = raw.as_bytes();
    if bytes.len() >= 2
        && (bytes[0] == b'"' || bytes[0] == b'\'')
        && bytes[0] == bytes[bytes.len() - 1]
    {
        raw.get(1..raw.len() - 1)
    } else {
        None
    }
}

fn property_name<'a>(node: Node<'_>, source: &'a str) -> Option<&'a str> {
    match node.kind() {
        "string" => unquote(text(node, source)).filter(|name| !name.is_empty()),
        "property_identifier" | "private_property_identifier" | "identifier" | "number" => {
            let name = text(node, source);
            (!name.is_empty()).then_some(name)
        }
        _ => None,
    }
}

fn module_file(from: &Path, specifier: &str) -> Option<PathBuf> {
    if !(specifier.starts_with("./") || specifier.starts_with("../")) {
        return None;
    }
    let mut path = from.parent()?.to_path_buf();
    for component in Path::new(specifier).components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !path.pop() {
                    return None;
                }
            }
            Component::Normal(part) => path.push(part),
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    if path.extension().is_some() {
        return path.is_file().then_some(path);
    }
    for extension in ["js", "mjs", "cjs", "jsx"] {
        let candidate = path.with_extension(extension);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    let index = path.join("index.js");
    index.is_file().then_some(index)
}

fn class_identity(path: &Path, node: Node<'_>, source: &str) -> Option<(String, String, bool)> {
    if let Some(name_node) = node.child_by_field_name("name") {
        let name = text(name_node, source);
        if name.is_empty() {
            return None;
        }
        return Some((name.to_string(), qualified(path, name), false));
    }
    if node
        .parent()
        .is_some_and(|parent| parent.kind() == "export_statement" && has_kind(parent, "default"))
    {
        return Some(("default".into(), qualified(path, "default"), true));
    }
    let name = format!("anon{}", node.start_byte());
    Some((name.clone(), qualified(path, &name), true))
}

fn prescan(node: Node<'_>, facts: &mut Facts<'_>) {
    match node.kind() {
        "class_declaration" | "class" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                let name = text(name_node, facts.source);
                if !name.is_empty() {
                    let fqn = qualified(facts.path, name);
                    facts
                        .classes
                        .entry(name.to_string())
                        .or_insert_with(|| fqn.clone());
                    facts.reserved.insert(fqn);
                }
            }
        }
        "function_declaration" | "generator_function_declaration"
            if node.parent().is_some_and(|parent| {
                parent.kind() == "program" || parent.kind() == "export_statement"
            }) =>
        {
            if let Some(name_node) = node.child_by_field_name("name") {
                let name = text(name_node, facts.source);
                if !name.is_empty() {
                    let fqn = qualified(facts.path, name);
                    facts
                        .functions
                        .entry(name.to_string())
                        .or_insert_with(|| fqn.clone());
                    facts.reserved.insert(fqn);
                }
            }
        }
        _ => {}
    }
    for child in named(node) {
        prescan(child, facts);
    }
}

fn declare_imports(node: Node<'_>, facts: &mut Facts<'_>) {
    if node.kind() == "import_statement" {
        import_statement(node, facts);
    }
    for child in named(node) {
        declare_imports(child, facts);
    }
}

fn import_statement(node: Node<'_>, facts: &mut Facts<'_>) {
    let Some(source_node) = node.child_by_field_name("source") else {
        return;
    };
    let Some(specifier) = unquote(text(source_node, facts.source)) else {
        return;
    };
    let module = module_file(facts.path, specifier);
    let Some(clause) = child_kind(node, "import_clause") else {
        return;
    };
    for child in named(clause) {
        match child.kind() {
            "identifier" => {
                let local = text(child, facts.source);
                if local.is_empty() {
                    continue;
                }
                let candidates = module.as_ref().map_or_else(Vec::new, |file| {
                    vec![qualified(file, local), qualified(file, "default")]
                });
                if !candidates.is_empty() {
                    facts.imports.insert(local.to_string(), candidates.clone());
                    push_reference(facts, candidates.clone(), child);
                }
                let frame = top_frame(facts);
                let scope = frame.scope.clone();
                declare_binding(
                    facts,
                    child,
                    local,
                    Value::candidates(candidates, true),
                    &frame,
                    scope,
                    "",
                );
            }
            "named_imports" => {
                for specifier_node in named(child) {
                    named_import(specifier_node, module.as_deref(), facts);
                }
            }
            "namespace_import" => {
                if let Some(local_node) = named(child).into_iter().next() {
                    let local = text(local_node, facts.source);
                    if !local.is_empty() {
                        let frame = top_frame(facts);
                        let scope = frame.scope.clone();
                        declare_binding(
                            facts,
                            local_node,
                            local,
                            Value::unknown(),
                            &frame,
                            scope,
                            "",
                        );
                    }
                }
            }
            _ => {}
        }
    }
}

fn named_import(node: Node<'_>, module: Option<&Path>, facts: &mut Facts<'_>) {
    if node.kind() != "import_specifier" {
        return;
    }
    let Some(exported_node) = node.child_by_field_name("name") else {
        return;
    };
    let exported = if exported_node.kind() == "identifier" {
        text(exported_node, facts.source)
    } else {
        ""
    };
    if exported.is_empty() {
        return;
    }
    let local_node = node.child_by_field_name("alias").unwrap_or(exported_node);
    let local = text(local_node, facts.source);
    if local.is_empty() {
        return;
    }
    let candidates = module.map_or_else(Vec::new, |file| vec![qualified(file, exported)]);
    if !candidates.is_empty() {
        facts.imports.insert(local.to_string(), candidates.clone());
        push_reference(facts, candidates.clone(), exported_node);
    }
    let frame = top_frame(facts);
    let scope = frame.scope.clone();
    declare_binding(
        facts,
        local_node,
        local,
        Value::candidates(candidates, true),
        &frame,
        scope,
        "",
    );
}

const fn top_frame(facts: &Facts<'_>) -> Frame {
    let end = facts.source.len();
    Frame {
        scope: 0..end,
        function_scope: 0..end,
        owner: None,
        this_mode: ThisMode::Shadow,
        top_level: true,
    }
}

fn push_reference(facts: &mut Facts<'_>, names: Vec<String>, node: Node<'_>) {
    if names.is_empty() {
        return;
    }
    let location = reference_at(node, facts.source, facts.path);
    facts.symbols.references.push((names, location));
}

fn type_from_doc(comment: &str, facts: &Facts<'_>) -> Option<Value> {
    let name = jsdoc_name(comment)?;
    if let Some(fqn) = facts.classes.get(&name) {
        return Some(Value::typed(fqn, false));
    }
    if let Some(candidates) = facts.imports.get(&name) {
        return Some(Value::candidates(candidates.clone(), false));
    }
    Some(Value::typed(&name, false))
}

fn apply_doc(value: Value, comment: &str, facts: &Facts<'_>) -> Value {
    type_from_doc(comment, facts).unwrap_or(value)
}

#[allow(
    clippy::too_many_arguments,
    reason = "A declaration carries identity, span, type and documentation together"
)]
fn push_symbol(
    facts: &mut Facts<'_>,
    fqn: String,
    name: &str,
    kind: SymbolKind,
    node: Node<'_>,
    owner: Option<String>,
    base_types: Vec<String>,
    scope: Range<usize>,
    visibility: Visibility,
    is_static: bool,
    value: &Value,
    signature: String,
    documentation: String,
) {
    let (line, column) = point(node);
    facts.symbols.declarations.push(SymbolDeclaration {
        fqn,
        name: name.to_string(),
        kind,
        path: facts.path.to_path_buf(),
        range: node.byte_range(),
        line,
        column,
        owner,
        base_types,
        trait_types: Vec::new(),
        scope,
        visibility,
        is_static,
        type_name: value.name.clone(),
        type_source: value.source.clone(),
        signature,
        documentation,
        usages_count: 0,
    });
}

fn declare_binding(
    facts: &mut Facts<'_>,
    node: Node<'_>,
    name: &str,
    value: Value,
    frame: &Frame,
    scope: Range<usize>,
    documentation: &str,
) {
    if name.is_empty() {
        return;
    }
    let qualified_name = qualified(facts.path, name);
    let fqn = if frame.top_level
        && scope.start == frame.scope.start
        && !facts.reserved.contains(&qualified_name)
        && !facts
            .symbols
            .declarations
            .iter()
            .any(|declaration| declaration.fqn == qualified_name)
    {
        qualified_name
    } else {
        local_name(facts.path, scope.start, name)
    };
    let documented = apply_doc(value, documentation, facts);
    push_symbol(
        facts,
        fqn,
        name,
        SymbolKind::Variable,
        node,
        None,
        Vec::new(),
        scope,
        Visibility::Public,
        documented.constructor,
        &documented,
        name.to_string(),
        documentation.to_string(),
    );
}

fn declare_this(facts: &mut Facts<'_>, frame: &Frame, at: usize) {
    let (value, is_static) = match &frame.this_mode {
        ThisMode::Instance(fqn) => (Value::typed(fqn, false), false),
        ThisMode::Static(fqn) => (Value::typed(fqn, true), true),
        ThisMode::Shadow => (Value::unknown(), false),
        ThisMode::Inherit => return,
    };
    let fqn = local_name(facts.path, frame.function_scope.start, "this");
    if facts
        .symbols
        .declarations
        .iter()
        .any(|declaration| declaration.fqn == fqn)
    {
        return;
    }
    facts.symbols.declarations.push(SymbolDeclaration {
        fqn,
        name: "this".into(),
        kind: SymbolKind::Variable,
        path: facts.path.to_path_buf(),
        range: at..at,
        line: 0,
        column: 0,
        owner: frame.owner.clone(),
        base_types: Vec::new(),
        trait_types: Vec::new(),
        scope: frame.function_scope.clone(),
        visibility: Visibility::Public,
        is_static,
        type_name: value.name,
        type_source: value.source,
        signature: "this".into(),
        documentation: String::new(),
        usages_count: 0,
    });
}

fn find_var<'a>(facts: &'a Facts<'_>, name: &str, byte: usize) -> Option<&'a SymbolDeclaration> {
    facts.symbols.declarations.iter().rev().find(|declaration| {
        declaration.kind == SymbolKind::Variable
            && declaration.name == name
            && declaration.scope.contains(&byte)
            && declaration.range.start < byte
    })
}

fn value_of(declaration: &SymbolDeclaration) -> Value {
    Value {
        name: declaration.type_name.clone(),
        source: declaration.type_source.clone(),
        constructor: declaration.is_static,
    }
}

fn eval_name(facts: &Facts<'_>, name: &str, byte: usize, frame: &Frame) -> Value {
    if name == "this" {
        return match &frame.this_mode {
            ThisMode::Instance(fqn) => Value::typed(fqn, false),
            ThisMode::Static(fqn) => Value::typed(fqn, true),
            ThisMode::Inherit | ThisMode::Shadow => {
                find_var(facts, "this", byte).map_or_else(Value::unknown, value_of)
            }
        };
    }
    if let Some(declaration) = find_var(facts, name, byte) {
        return value_of(declaration);
    }
    if let Some(candidates) = facts.imports.get(name) {
        return Value::candidates(candidates.clone(), true);
    }
    if let Some(fqn) = facts.classes.get(name) {
        return Value::typed(fqn, true);
    }
    if facts.functions.contains_key(name) {
        return Value::typed("Function", false);
    }
    if let Some(binding) = js_stdlib::global_binding(name) {
        return Value::typed(binding.type_name, false);
    }
    if js_stdlib::type_named(name).is_some() {
        return Value::typed(name, true);
    }
    if js_stdlib::global_fn(name).is_some() {
        return Value::typed("Function", false);
    }
    Value::unknown()
}

fn return_of(declaration: &SymbolDeclaration) -> Value {
    if declaration.type_name.is_none() && declaration.type_source.is_none() {
        return Value::unknown();
    }
    Value {
        name: declaration.type_name.clone(),
        source: declaration.type_source.clone(),
        constructor: false,
    }
}

fn object_shape(path: &Path, byte: usize) -> String {
    format!("{}::object:{byte}", path_key(path))
}

/// Whether `name` is the synthetic type of one object literal.
///
/// The tail is `object:` plus the literal's start byte. Completion lists that
/// literal's own keys and does not treat the name as a project class.
#[must_use]
pub fn is_object_shape(name: &str) -> bool {
    name.rsplit_once("::").is_some_and(|(_, tail)| {
        let Some(digits) = tail.strip_prefix("object:") else {
            return false;
        };
        !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
    })
}

fn dot_key(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first == '_' || first == '$' || first.is_alphabetic() => chars
            .all(|character| character == '_' || character == '$' || character.is_alphanumeric()),
        _ => false,
    }
}

fn dot_property<'a>(node: Node<'_>, source: &'a str) -> Option<&'a str> {
    property_name(node, source).filter(|name| dot_key(name))
}

fn member_name_node(node: Node<'_>) -> Option<Node<'_>> {
    match node.kind() {
        "pair" => node.child_by_field_name("key"),
        "method_definition" => node.child_by_field_name("name"),
        "shorthand_property_identifier" => Some(node),
        _ => None,
    }
}

fn object_member_name<'a>(node: Node<'_>, source: &'a str) -> Option<&'a str> {
    let key = member_name_node(node)?;
    if node.kind() == "shorthand_property_identifier" {
        let name = text(key, source);
        return dot_key(name).then_some(name);
    }
    dot_property(key, source)
}

fn object_type(facts: &Facts<'_>, node: Node<'_>) -> Value {
    if named(node)
        .iter()
        .any(|child| object_member_name(*child, facts.source).is_some())
    {
        Value::typed(&object_shape(facts.path, node.start_byte()), false)
    } else {
        Value::typed("Object", false)
    }
}

fn declare_object_shape(facts: &mut Facts<'_>, node: Node<'_>, frame: &Frame) {
    let shape = object_shape(facts.path, node.start_byte());
    let scope = node.byte_range();
    let mut methods = Vec::new();
    for child in named(node) {
        if child.kind() == "method_definition" {
            declare_object_member(facts, child, frame, &shape, scope.clone(), None);
            methods.push(child);
        } else {
            let value = child.child_by_field_name("value");
            declare_object_member(facts, child, frame, &shape, scope.clone(), value);
            walk(child, facts, frame);
        }
    }
    for method in methods {
        walk_function_body(
            facts,
            method,
            frame,
            ThisMode::Instance(shape.clone()),
            true,
            None,
        );
    }
}

fn declare_object_member(
    facts: &mut Facts<'_>,
    node: Node<'_>,
    frame: &Frame,
    shape: &str,
    scope: Range<usize>,
    value_node: Option<Node<'_>>,
) {
    let Some(name_node) = member_name_node(node) else {
        return;
    };
    let Some(name) = object_member_name(node, facts.source) else {
        return;
    };
    let method = node.kind() == "method_definition";
    let accessor = method && (has_kind(node, "get") || has_kind(node, "set"));
    let kind = if method && !accessor {
        SymbolKind::Method
    } else {
        SymbolKind::Property
    };
    let value = if method && !accessor {
        Value::typed("Function", false)
    } else if node.kind() == "shorthand_property_identifier" {
        eval_name(facts, name, name_node.start_byte(), frame)
    } else {
        value_node.map_or_else(Value::unknown, |value| eval(facts, value, frame, 0))
    };
    push_symbol(
        facts,
        format!("{shape}::{name}"),
        name,
        kind,
        name_node,
        Some(shape.to_string()),
        Vec::new(),
        scope,
        Visibility::Public,
        false,
        &value,
        signature_of(node, facts.source),
        documentation_of(node, facts.source),
    );
}

fn member_return(facts: &Facts<'_>, object: &Value, name: &str) -> Value {
    let Some(type_name) = object.name.as_deref() else {
        return Value::unknown();
    };
    if let Some(member) = js_stdlib::member(type_name, name, object.constructor) {
        return if member.returns.is_empty() {
            Value::unknown()
        } else {
            Value::typed(member.returns, false)
        };
    }
    facts
        .symbols
        .declarations
        .iter()
        .rev()
        .find(|declaration| {
            declaration.owner.as_deref() == Some(type_name) && declaration.name == name
        })
        .map_or_else(Value::unknown, return_of)
}

fn eval(facts: &Facts<'_>, node: Node<'_>, frame: &Frame, depth: u8) -> Value {
    if depth > 8 {
        return Value::unknown();
    }
    match node.kind() {
        "parenthesized_expression" => named(node)
            .into_iter()
            .next()
            .map_or_else(Value::unknown, |child| eval(facts, child, frame, depth + 1)),
        "identifier" => eval_name(facts, text(node, facts.source), node.start_byte(), frame),
        "new_expression" => {
            node.child_by_field_name("constructor")
                .map_or_else(Value::unknown, |constructor| {
                    let mut value = eval(facts, constructor, frame, depth + 1);
                    value.constructor = false;
                    value
                })
        }
        "call_expression" => node
            .child_by_field_name("function")
            .map_or_else(Value::unknown, |callee| {
                eval_call(facts, callee, frame, depth + 1)
            }),
        "member_expression" => {
            let Some(object) = node.child_by_field_name("object") else {
                return Value::unknown();
            };
            let Some(property) = node.child_by_field_name("property") else {
                return Value::unknown();
            };
            let Some(name) = property_name(property, facts.source) else {
                return Value::unknown();
            };
            let object = eval(facts, object, frame, depth + 1);
            member_return(facts, &object, name)
        }
        "array" => Value::typed("Array", false),
        "string" | "template_string" => Value::typed("String", false),
        "number" => Value::typed("Number", false),
        "regex" => Value::typed("RegExp", false),
        "object" => object_type(facts, node),
        "true" | "false" => Value::typed("Boolean", false),
        "await_expression" => awaited_value(facts, node, frame, depth),
        "arrow_function" | "function_expression" | "generator_function" => {
            Value::typed("Function", false)
        }
        "class" | "class_declaration" => class_identity(facts.path, node, facts.source)
            .map_or_else(Value::unknown, |(_, fqn, _)| Value::typed(&fqn, true)),
        _ => Value::unknown(),
    }
}

fn eval_call(facts: &Facts<'_>, callee: Node<'_>, frame: &Frame, depth: u8) -> Value {
    match callee.kind() {
        "member_expression" => eval(facts, callee, frame, depth),
        "identifier" => {
            let name = text(callee, facts.source);
            let byte = callee.start_byte();
            if let Some(declaration) = find_var(facts, name, byte)
                && declaration.type_name.as_deref() == Some("Function")
            {
                return Value::unknown();
            }
            if let Some(fqn) = facts.functions.get(name)
                && let Some(declaration) = facts
                    .symbols
                    .declarations
                    .iter()
                    .find(|declaration| &declaration.fqn == fqn)
            {
                return return_of(declaration);
            }
            if let Some(fun) = js_stdlib::global_fn(name) {
                return if fun.returns.is_empty() {
                    Value::unknown()
                } else {
                    Value::typed(fun.returns, false)
                };
            }
            Value::unknown()
        }
        _ => Value::unknown(),
    }
}

fn bind_pattern(
    facts: &mut Facts<'_>,
    node: Node<'_>,
    value: &Value,
    frame: &Frame,
    scope: Range<usize>,
    documentation: &str,
) {
    match node.kind() {
        "identifier" | "shorthand_property_identifier_pattern" => {
            declare_binding(
                facts,
                node,
                text(node, facts.source),
                Value {
                    name: value.name.clone(),
                    source: value.source.clone(),
                    constructor: value.constructor,
                },
                frame,
                scope,
                documentation,
            );
        }
        "rest_pattern" | "array_pattern" => {
            for child in named(node) {
                bind_pattern(
                    facts,
                    child,
                    &Value::unknown(),
                    frame,
                    scope.clone(),
                    documentation,
                );
            }
        }
        "assignment_pattern" | "object_assignment_pattern" => {
            let right = node.child_by_field_name("right");
            let bound = right.map_or_else(
                || Value {
                    name: value.name.clone(),
                    source: value.source.clone(),
                    constructor: value.constructor,
                },
                |right| eval(facts, right, frame, 0),
            );
            if let Some(left) = node.child_by_field_name("left") {
                bind_pattern(facts, left, &bound, frame, scope, documentation);
            }
            if let Some(right) = right {
                walk(right, facts, frame);
            }
        }
        "object_pattern" => {
            for child in named(node) {
                match child.kind() {
                    "pair_pattern" => {
                        if let Some(bound) = child.child_by_field_name("value") {
                            bind_pattern(
                                facts,
                                bound,
                                &Value::unknown(),
                                frame,
                                scope.clone(),
                                documentation,
                            );
                        }
                    }
                    _ => bind_pattern(
                        facts,
                        child,
                        &Value::unknown(),
                        frame,
                        scope.clone(),
                        documentation,
                    ),
                }
            }
        }
        _ => {}
    }
}

fn class_bases(node: Node<'_>, facts: &Facts<'_>) -> Vec<String> {
    let Some(heritage) = child_kind(node, "class_heritage") else {
        return Vec::new();
    };
    let Some(ident) = named(heritage)
        .into_iter()
        .find(|child| child.kind() == "identifier")
    else {
        return Vec::new();
    };
    let name = text(ident, facts.source);
    if let Some(fqn) = facts.classes.get(name) {
        return vec![fqn.clone()];
    }
    if let Some(candidates) = facts.imports.get(name) {
        return candidates.clone();
    }
    vec![name.to_string()]
}

fn declare_class(facts: &mut Facts<'_>, node: Node<'_>, frame: &Frame) {
    let Some((name, fqn, anonymous)) = class_identity(facts.path, node, facts.source) else {
        return;
    };
    let Some(name_node) = node
        .child_by_field_name("name")
        .or_else(|| child_kind(node, "class_body").map(|_| node))
    else {
        return;
    };
    let bases = class_bases(node, facts);
    let documentation = documentation_of(node, facts.source);
    let own = Value::typed(&fqn, false);
    push_symbol(
        facts,
        fqn.clone(),
        &name,
        SymbolKind::Class,
        name_node,
        None,
        bases,
        node.byte_range(),
        Visibility::Public,
        false,
        &own,
        signature_of(node, facts.source),
        documentation,
    );
    if !anonymous
        && node.parent().is_some_and(|parent| {
            parent.kind() == "export_statement" && has_kind(parent, "default")
        })
    {
        let alias = qualified(facts.path, "default");
        if alias != fqn {
            push_symbol(
                facts,
                alias,
                "default",
                SymbolKind::Class,
                name_node,
                None,
                vec![fqn.clone()],
                node.byte_range(),
                Visibility::Public,
                false,
                &Value::typed(&fqn, false),
                signature_of(node, facts.source),
                String::new(),
            );
        }
    }
    let class_frame = Frame {
        scope: node.byte_range(),
        function_scope: node.byte_range(),
        owner: Some(fqn.clone()),
        this_mode: ThisMode::Instance(fqn),
        top_level: false,
    };
    if let Some(heritage) = child_kind(node, "class_heritage") {
        walk(heritage, facts, frame);
    }
    if let Some(body) = node.child_by_field_name("body") {
        walk(body, facts, &class_frame);
    }
}

fn member_flags(node: Node<'_>, name: &str) -> (SymbolKind, bool, Visibility) {
    let accessor = has_kind(node, "get") || has_kind(node, "set");
    let kind = if accessor || node.kind() == "field_definition" {
        SymbolKind::Property
    } else {
        SymbolKind::Method
    };
    let is_static = has_kind(node, "static") && name != "constructor";
    (kind, is_static, visibility_of(name))
}

fn visibility_of(name: &str) -> Visibility {
    if name.starts_with('#') {
        Visibility::Private
    } else {
        Visibility::Public
    }
}

fn declare_method(facts: &mut Facts<'_>, node: Node<'_>, frame: &Frame) {
    let Some(owner) = frame.owner.clone().filter(|owner| !owner.is_empty()) else {
        walk_function_body(facts, node, frame, ThisMode::Shadow, true, None);
        return;
    };
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let Some(name) = property_name(name_node, facts.source) else {
        walk_function_body(facts, node, frame, ThisMode::Shadow, true, None);
        return;
    };
    let (kind, is_static, visibility) = member_flags(node, name);
    let documentation = documentation_of(node, facts.source);
    let value = type_from_doc(&documentation, facts).unwrap_or_else(Value::unknown);
    let fqn = format!("{owner}::{name}");
    push_symbol(
        facts,
        fqn,
        name,
        kind,
        name_node,
        Some(owner.clone()),
        Vec::new(),
        node.byte_range(),
        visibility,
        is_static,
        &value,
        signature_of(node, facts.source),
        documentation,
    );
    let mode = if is_static {
        ThisMode::Static(owner)
    } else {
        ThisMode::Instance(owner)
    };
    walk_function_body(facts, node, frame, mode, true, None);
}

fn declare_field(facts: &mut Facts<'_>, node: Node<'_>, frame: &Frame) {
    let Some(owner) = frame.owner.clone().filter(|owner| !owner.is_empty()) else {
        if let Some(value) = node.child_by_field_name("value") {
            walk(value, facts, frame);
        }
        return;
    };
    let Some(name_node) = node.child_by_field_name("property") else {
        return;
    };
    let Some(name) = property_name(name_node, facts.source) else {
        return;
    };
    let documentation = documentation_of(node, facts.source);
    let value_node = node.child_by_field_name("value");
    let evaluated = value_node.map_or_else(Value::unknown, |value| eval(facts, value, frame, 0));
    let value = apply_doc(evaluated, &documentation, facts);
    let is_static = has_kind(node, "static");
    push_symbol(
        facts,
        format!("{owner}::{name}"),
        name,
        SymbolKind::Property,
        name_node,
        Some(owner),
        Vec::new(),
        node.byte_range(),
        visibility_of(name),
        is_static,
        &value,
        signature_of(node, facts.source),
        documentation,
    );
    if let Some(value) = value_node {
        walk(value, facts, frame);
    }
}

fn declare_function(facts: &mut Facts<'_>, node: Node<'_>, frame: &Frame, first: Option<&Value>) {
    let Some(name_node) = node.child_by_field_name("name") else {
        walk_function_body(facts, node, frame, ThisMode::Shadow, true, first);
        return;
    };
    let name = text(name_node, facts.source);
    if name.is_empty() {
        return;
    }
    let top = frame.top_level
        && node.parent().is_some_and(|parent| {
            parent.kind() == "program" || parent.kind() == "export_statement"
        });
    let fqn = if top {
        qualified(facts.path, name)
    } else {
        local_name(facts.path, frame.scope.start, name)
    };
    let documentation = documentation_of(node, facts.source);
    let value = type_from_doc(&documentation, facts).unwrap_or_else(Value::unknown);
    push_symbol(
        facts,
        fqn,
        name,
        SymbolKind::Function,
        name_node,
        None,
        Vec::new(),
        node.byte_range(),
        Visibility::Public,
        false,
        &value,
        signature_of(node, facts.source),
        documentation,
    );
    walk_function_body(facts, node, frame, ThisMode::Shadow, true, first);
}

fn callback_value(facts: &Facts<'_>, call: Node<'_>, frame: &Frame) -> Value {
    let Some(callee) = call.child_by_field_name("function") else {
        return Value::unknown();
    };
    if callee.kind() != "member_expression" {
        return Value::unknown();
    }
    let Some(receiver) = callee.child_by_field_name("object") else {
        return Value::unknown();
    };
    let Some(property) = callee.child_by_field_name("property") else {
        return Value::unknown();
    };
    let Some(name) = property_name(property, facts.source) else {
        return Value::unknown();
    };
    if matches!(name, "addEventListener" | "removeEventListener") {
        return Value::typed(listener_event(facts, call), false);
    }
    if name == "then" {
        return promise_value(facts, receiver, frame);
    }
    let object = eval(facts, receiver, frame, 0);
    let Some(type_name) = object.name.as_deref() else {
        return Value::unknown();
    };
    let item = js_stdlib::callback_item(type_name, name);
    if item.is_empty() {
        Value::unknown()
    } else {
        Value::typed(item, false)
    }
}

fn listener_event(facts: &Facts<'_>, call: Node<'_>) -> &'static str {
    let Some(arguments) = call.child_by_field_name("arguments") else {
        return "Event";
    };
    let Some(first) = named(arguments).into_iter().next() else {
        return "Event";
    };
    if first.kind() == "string" {
        unquote(text(first, facts.source)).map_or("Event", js_stdlib::event_type)
    } else {
        "Event"
    }
}

fn awaited_value(facts: &Facts<'_>, node: Node<'_>, frame: &Frame, depth: u8) -> Value {
    let Some(child) = named(node).into_iter().next() else {
        return Value::unknown();
    };
    let resolved = promise_value(facts, child, frame);
    if resolved.name.is_some() {
        return resolved;
    }
    let inner = eval(facts, child, frame, depth + 1);
    if inner.name.as_deref() == Some("Promise") {
        Value::unknown()
    } else {
        inner
    }
}

fn promise_value(facts: &Facts<'_>, node: Node<'_>, frame: &Frame) -> Value {
    if node.kind() != "call_expression" {
        return Value::unknown();
    }
    let Some(callee) = node.child_by_field_name("function") else {
        return Value::unknown();
    };
    let resolved = match callee.kind() {
        "identifier" => js_stdlib::global_promise(text(callee, facts.source)),
        "member_expression" => {
            let Some(object) = callee.child_by_field_name("object") else {
                return Value::unknown();
            };
            let Some(property) = callee.child_by_field_name("property") else {
                return Value::unknown();
            };
            let Some(method) = property_name(property, facts.source) else {
                return Value::unknown();
            };
            let object = eval(facts, object, frame, 0);
            let Some(type_name) = object.name.as_deref() else {
                return Value::unknown();
            };
            js_stdlib::promise_result(type_name, method)
        }
        _ => "",
    };
    if resolved.is_empty() {
        Value::unknown()
    } else {
        Value::typed(resolved, false)
    }
}

fn collection_element(facts: &Facts<'_>, node: Node<'_>, frame: &Frame) -> Value {
    let Some(type_name) = eval(facts, node, frame, 0).name else {
        return Value::unknown();
    };
    let element = js_stdlib::element_of(&type_name);
    if element.is_empty() {
        Value::unknown()
    } else {
        Value::typed(element, false)
    }
}

fn walk_callback(facts: &mut Facts<'_>, node: Node<'_>, frame: &Frame, value: &Value) {
    if node.kind() == "arrow_function" {
        walk_function_body(facts, node, frame, ThisMode::Inherit, false, Some(value));
    } else {
        declare_function(facts, node, frame, Some(value));
    }
}

fn walk_call(facts: &mut Facts<'_>, node: Node<'_>, frame: &Frame) {
    if let Some(callee) = node.child_by_field_name("function") {
        walk(callee, facts, frame);
    }
    let item = callback_value(facts, node, frame);
    let Some(arguments) = node.child_by_field_name("arguments") else {
        return;
    };
    for child in named(arguments) {
        if item.name.is_some()
            && matches!(
                child.kind(),
                "arrow_function" | "function_expression" | "generator_function"
            )
        {
            walk_callback(facts, child, frame, &item);
        } else {
            walk(child, facts, frame);
        }
    }
}

fn walk_function_body(
    facts: &mut Facts<'_>,
    node: Node<'_>,
    frame: &Frame,
    mode: ThisMode,
    bind_this: bool,
    first: Option<&Value>,
) {
    let function_frame = Frame {
        scope: node.byte_range(),
        function_scope: node.byte_range(),
        owner: frame.owner.clone(),
        this_mode: mode,
        top_level: false,
    };
    if bind_this {
        declare_this(facts, &function_frame, node.start_byte());
    }
    let unknown = Value::unknown();
    let mut supplied = first;
    if let Some(parameter) = node.child_by_field_name("parameter") {
        let value = supplied.take().unwrap_or(&unknown);
        bind_pattern(
            facts,
            parameter,
            value,
            &function_frame,
            function_frame.scope.clone(),
            "",
        );
    }
    if let Some(parameters) = node.child_by_field_name("parameters") {
        for child in named(parameters) {
            let value = supplied.take().unwrap_or(&unknown);
            bind_pattern(
                facts,
                child,
                value,
                &function_frame,
                function_frame.scope.clone(),
                "",
            );
        }
    }
    if let Some(body) = node.child_by_field_name("body") {
        walk(body, facts, &function_frame);
    }
}

fn declare_variables(facts: &mut Facts<'_>, node: Node<'_>, frame: &Frame) {
    let scope = if node.kind() == "variable_declaration" {
        frame.function_scope.clone()
    } else {
        frame.scope.clone()
    };
    let documentation = documentation_of(node, facts.source);
    for child in named(node) {
        if child.kind() != "variable_declarator" {
            continue;
        }
        let value_node = child.child_by_field_name("value");
        let evaluated =
            value_node.map_or_else(Value::unknown, |value| eval(facts, value, frame, 0));
        if let Some(name) = child.child_by_field_name("name") {
            bind_pattern(
                facts,
                name,
                &evaluated,
                frame,
                scope.clone(),
                &documentation,
            );
        }
        if let Some(value) = value_node {
            walk(value, facts, frame);
        }
    }
}

fn binding_name(node: Node<'_>) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    let selected = parent.child_by_field_name("name") == Some(node)
        || parent.child_by_field_name("property") == Some(node)
        || parent.child_by_field_name("parameter") == Some(node);
    selected
        && matches!(
            parent.kind(),
            "class_declaration"
                | "class"
                | "function_declaration"
                | "generator_function_declaration"
                | "function_expression"
                | "generator_function"
                | "method_definition"
                | "field_definition"
                | "variable_declarator"
                | "import_specifier"
                | "arrow_function"
                | "catch_clause"
                | "pair"
                | "pair_pattern"
        )
}

fn record_identifier(facts: &mut Facts<'_>, node: Node<'_>, _frame: &Frame) {
    if binding_name(node) {
        return;
    }
    if node
        .parent()
        .is_some_and(|parent| parent.kind() == "pair" || parent.kind() == "pair_pattern")
        && node
            .parent()
            .and_then(|parent| parent.child_by_field_name("key"))
            == Some(node)
    {
        return;
    }
    let name = text(node, facts.source);
    if name.is_empty() {
        return;
    }
    let mut targets = Vec::new();
    if let Some(declaration) = find_var(facts, name, node.start_byte()) {
        targets.push(declaration.fqn.clone());
        if declaration.is_static
            && let Some(source) = &declaration.type_source
        {
            match source {
                PhpTypeSource::Class(fqn) => targets.push(fqn.clone()),
                PhpTypeSource::Symbol(names) => targets.extend(names.clone()),
                PhpTypeSource::Member(_, _) => {}
            }
        }
    } else if let Some(candidates) = facts.imports.get(name) {
        targets.extend(candidates.clone());
    } else if let Some(fqn) = facts.classes.get(name) {
        targets.push(fqn.clone());
    } else if let Some(fqn) = facts.functions.get(name) {
        targets.push(fqn.clone());
    }
    push_reference(facts, targets, node);
}

fn record_member(facts: &mut Facts<'_>, node: Node<'_>, frame: &Frame) {
    let Some(object) = node.child_by_field_name("object") else {
        return;
    };
    let Some(property) = node.child_by_field_name("property") else {
        walk(object, facts, frame);
        return;
    };
    let object_value = eval(facts, object, frame, 0);
    if let Some(name) = property_name(property, facts.source) {
        let project = match &object_value.source {
            Some(PhpTypeSource::Class(type_name)) => type_name.contains("::"),
            Some(PhpTypeSource::Symbol(_)) => true,
            Some(PhpTypeSource::Member(_, _)) | None => false,
        };
        if project && let Some(source) = object_value.source {
            let location = reference_at(property, facts.source, facts.path);
            facts
                .symbols
                .member_references
                .push((source, name.to_string(), location));
        }
    }
    walk(object, facts, frame);
}

fn narrow(frame: &Frame, scope: Range<usize>, mode: ThisMode) -> Frame {
    Frame {
        scope,
        function_scope: frame.function_scope.clone(),
        owner: frame.owner.clone(),
        this_mode: mode,
        top_level: false,
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "Each JavaScript node kind owns its declarations and the nodes it still walks"
)]
fn walk(node: Node<'_>, facts: &mut Facts<'_>, frame: &Frame) {
    match node.kind() {
        "import_statement" | "private_property_identifier" | "comment" => return,
        "class_declaration" | "class" => {
            declare_class(facts, node, frame);
            return;
        }
        "method_definition" => {
            declare_method(facts, node, frame);
            return;
        }
        "field_definition" => {
            declare_field(facts, node, frame);
            return;
        }
        "function_declaration"
        | "generator_function_declaration"
        | "function_expression"
        | "generator_function" => {
            declare_function(facts, node, frame, None);
            return;
        }
        "arrow_function" => {
            walk_function_body(facts, node, frame, ThisMode::Inherit, false, None);
            return;
        }
        "call_expression" => {
            walk_call(facts, node, frame);
            return;
        }
        "class_static_block" => {
            let mode = frame
                .owner
                .clone()
                .filter(|owner| !owner.is_empty())
                .map_or(ThisMode::Shadow, ThisMode::Static);
            let block = narrow(frame, node.byte_range(), mode);
            let mut block = block;
            block.function_scope = node.byte_range();
            declare_this(facts, &block, node.start_byte());
            if let Some(body) = node.child_by_field_name("body") {
                walk(body, facts, &block);
            } else {
                for child in named(node) {
                    walk(child, facts, &block);
                }
            }
            return;
        }
        "statement_block" | "for_statement" => {
            let inner = narrow(frame, node.byte_range(), frame.this_mode.clone());
            for child in named(node) {
                walk(child, facts, &inner);
            }
            return;
        }
        "for_in_statement" => {
            let inner = narrow(frame, node.byte_range(), frame.this_mode.clone());
            let scope = if has_kind(node, "var") {
                frame.function_scope.clone()
            } else {
                node.byte_range()
            };
            let element = if has_kind(node, "of") {
                node.child_by_field_name("right")
                    .map_or_else(Value::unknown, |right| {
                        collection_element(facts, right, &inner)
                    })
            } else {
                Value::unknown()
            };
            if let Some(left) = node.child_by_field_name("left") {
                if matches!(
                    left.kind(),
                    "member_expression" | "subscript_expression" | "parenthesized_expression"
                ) {
                    walk(left, facts, &inner);
                } else {
                    bind_pattern(facts, left, &element, &inner, scope, "");
                }
            }
            if let Some(right) = node.child_by_field_name("right") {
                walk(right, facts, &inner);
            }
            if let Some(body) = node.child_by_field_name("body") {
                walk(body, facts, &inner);
            }
            return;
        }
        "catch_clause" => {
            let inner = narrow(frame, node.byte_range(), frame.this_mode.clone());
            if let Some(parameter) = node.child_by_field_name("parameter") {
                bind_pattern(
                    facts,
                    parameter,
                    &Value::unknown(),
                    &inner,
                    node.byte_range(),
                    "",
                );
            }
            if let Some(body) = node.child_by_field_name("body") {
                walk(body, facts, &inner);
            }
            return;
        }
        "object" => {
            declare_object_shape(facts, node, frame);
            return;
        }
        "lexical_declaration" | "variable_declaration" => {
            declare_variables(facts, node, frame);
            return;
        }
        "member_expression" => {
            record_member(facts, node, frame);
            return;
        }
        "identifier" | "property_identifier" | "shorthand_property_identifier" => {
            record_identifier(facts, node, frame);
            return;
        }
        _ => {}
    }
    for child in named(node) {
        walk(child, facts, frame);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::symbol_index::{LanguageId, SymbolIndex};
    use std::path::PathBuf;

    fn index_source(source: &str) -> Result<(SymbolIndex, PathBuf), Box<dyn std::error::Error>> {
        let path = PathBuf::from("sample.js");
        let mut index = SymbolIndex::new(LanguageId::JavaScript);
        index.replace(path.clone(), parse(&path, source)?);
        Ok((index, path))
    }

    #[test]
    fn query_selector_all_types_node_list_and_element() -> Result<(), Box<dyn std::error::Error>> {
        let source = "const links = document.querySelectorAll('.js-contact-show');\nlinks.forEach(function (link) {\n    link.addEventListener('click', handleClick);\n});\nlinks.forEach((item) => item.classList.add('on'));\nfor (const node of links) { node.remove(); }\n";
        let (index, _) = index_source(source)?;
        let typed = |name: &str| {
            index
                .declarations()
                .find(|declaration| declaration.name == name)
                .and_then(|declaration| declaration.type_name.clone())
        };
        assert_eq!(typed("links").as_deref(), Some("NodeList"));
        assert_eq!(typed("link").as_deref(), Some("HTMLElement"));
        assert_eq!(typed("item").as_deref(), Some("HTMLElement"));
        assert_eq!(typed("node").as_deref(), Some("HTMLElement"));
        Ok(())
    }

    #[test]
    fn events_fetch_and_query_results_carry_host_types() -> Result<(), Box<dyn std::error::Error>> {
        let source = "const element = document.querySelector('a');\nelement.addEventListener('click', function (event) {\n    event.preventDefault();\n});\nelement.addEventListener(type, function (plain) { plain.preventDefault(); });\nconst response = await fetch('/api');\nconst body = await response.text();\nresponse.json().then(function (payload) { return payload; });\nconst file = element.files.item(0);\n";
        let (index, _) = index_source(source)?;
        let typed = |name: &str| {
            index
                .declarations()
                .find(|declaration| declaration.name == name)
                .and_then(|declaration| declaration.type_name.clone())
        };
        assert_eq!(typed("element").as_deref(), Some("HTMLElement"));
        assert_eq!(typed("event").as_deref(), Some("MouseEvent"));
        assert_eq!(typed("plain").as_deref(), Some("Event"));
        assert_eq!(typed("response").as_deref(), Some("Response"));
        assert_eq!(typed("body").as_deref(), Some("String"));
        assert_eq!(typed("payload"), None);
        assert_eq!(typed("file").as_deref(), Some("File"));
        Ok(())
    }

    #[test]
    fn class_members_new_and_private_usages() -> Result<(), Box<dyn std::error::Error>> {
        let source = "class Car { drive() {} static build() {} #hidden() {} }\nconst car = new Car();\ncar.drive();\n";
        let (index, path) = index_source(source)?;
        let class_fqn = index
            .declarations()
            .find(|declaration| declaration.kind == SymbolKind::Class && declaration.name == "Car")
            .map(|declaration| declaration.fqn.clone())
            .ok_or("class")?;
        let drive = index
            .declaration(&format!("{class_fqn}::drive"))
            .ok_or("drive")?;
        assert_eq!(drive.usages_count, 1);
        assert!(!drive.is_static);
        let hidden = index
            .declaration(&format!("{class_fqn}::#hidden"))
            .ok_or("hidden")?;
        assert!(hidden.is_unused());
        assert_eq!(hidden.visibility, Visibility::Private);
        let build = index
            .declaration(&format!("{class_fqn}::build"))
            .ok_or("build")?;
        assert!(build.is_static);
        assert!(build.is_unused());
        let car = index
            .declarations()
            .find(|declaration| declaration.name == "car")
            .ok_or("car")?;
        assert!(!car.is_static);
        assert_eq!(car.type_name.as_deref(), Some(class_fqn.as_str()));
        assert!(
            index
                .at(&path, source.find("drive()").ok_or("call")?)
                .is_some()
        );
        Ok(())
    }

    #[test]
    fn extends_links_inherited_methods() -> Result<(), Box<dyn std::error::Error>> {
        let source = "class Vehicle { go() {} }\nclass Car extends Vehicle { drive() {} }\n";
        let (index, _) = index_source(source)?;
        let car = index
            .declarations()
            .find(|declaration| declaration.name == "Car")
            .ok_or("car")?;
        let vehicle = index
            .declarations()
            .find(|declaration| declaration.name == "Vehicle")
            .ok_or("vehicle")?;
        assert_eq!(car.base_types, vec![vehicle.fqn.clone()]);
        let inherited = index
            .resolve(&format!("{}::go", car.fqn))
            .ok_or("inherited")?;
        assert_eq!(inherited.name, "go");
        Ok(())
    }

    #[test]
    fn relative_named_import_links_without_reparsing() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let car = dir.path().join("car.js");
        let main = dir.path().join("main.js");
        std::fs::write(&car, "export class Car { drive() {} }\n")?;
        std::fs::write(
            &main,
            "import { Car } from './car.js';\nconst car = new Car();\ncar.drive();\n",
        )?;
        let mut index = SymbolIndex::new(LanguageId::JavaScript);
        index.replace(car.clone(), parse(&car, &std::fs::read_to_string(&car)?)?);
        index.replace(
            main.clone(),
            parse(&main, &std::fs::read_to_string(&main)?)?,
        );
        let class_fqn = qualified(&car, "Car");
        let declaration = index.declaration(&class_fqn).ok_or("class")?;
        assert!(declaration.usages_count >= 1);
        let method = index
            .declaration(&format!("{class_fqn}::drive"))
            .ok_or("drive")?;
        assert_eq!(method.usages_count, 1);
        Ok(())
    }

    #[test]
    fn default_export_import_uses_alias_when_names_differ() -> Result<(), Box<dyn std::error::Error>>
    {
        let dir = tempfile::tempdir()?;
        let car = dir.path().join("car.js");
        let main = dir.path().join("main.js");
        std::fs::write(&car, "export default class Car { drive() {} }\n")?;
        std::fs::write(
            &main,
            "import Vehicle from './car.js';\nconst car = new Vehicle();\ncar.drive();\n",
        )?;
        let mut index = SymbolIndex::new(LanguageId::JavaScript);
        index.replace(car.clone(), parse(&car, &std::fs::read_to_string(&car)?)?);
        index.replace(
            main.clone(),
            parse(&main, &std::fs::read_to_string(&main)?)?,
        );
        let class_fqn = qualified(&car, "Car");
        let method = index
            .resolve(&format!("{}::drive", qualified(&car, "default")))
            .ok_or("default drive")?;
        assert_eq!(method.fqn, format!("{class_fqn}::drive"));
        assert_eq!(method.usages_count, 1);
        Ok(())
    }

    #[test]
    fn object_literal_records_own_keys() -> Result<(), Box<dyn std::error::Error>> {
        let source = "const payload = {\n    owner_id: parseInt(link.dataset.owner, 10),\n    contact_type: link.dataset.type,\n    source: link.dataset.source,\n    meta: { kind: \"page\" },\n    note: true,\n    save() { return this.source; },\n};\nconst copy = payload;\npayload.owner_id;\nconst blank = {};\nconst extra = { ...payload, [key]: 1 };\n";
        let (index, _) = index_source(source)?;
        let typed = |name: &str| {
            index
                .declarations()
                .find(|declaration| declaration.name == name && declaration.owner.is_none())
                .and_then(|declaration| declaration.type_name.clone())
        };
        let payload = typed("payload").ok_or("payload type")?;
        assert!(is_object_shape(&payload), "{payload}");
        assert_eq!(typed("copy").as_deref(), Some(payload.as_str()));
        assert_eq!(typed("blank").as_deref(), Some("Object"));
        assert_eq!(typed("extra").as_deref(), Some("Object"));
        let member = |owner: &str, name: &str| {
            index
                .declarations()
                .find(|declaration| {
                    declaration.owner.as_deref() == Some(owner) && declaration.name == name
                })
                .cloned()
        };
        let owner_id = member(&payload, "owner_id").ok_or("owner_id")?;
        assert_eq!(owner_id.kind, SymbolKind::Property);
        assert_eq!(owner_id.type_name.as_deref(), Some("Number"));
        assert!(owner_id.usages_count >= 1);
        assert!(!owner_id.is_unused());
        let note = member(&payload, "note").ok_or("note")?;
        assert_eq!(note.usages_count, 0);
        assert!(!note.is_unused());
        let meta = member(&payload, "meta").ok_or("meta")?;
        let meta_type = meta.type_name.ok_or("meta type")?;
        assert!(is_object_shape(&meta_type), "{meta_type}");
        let kind = member(&meta_type, "kind").ok_or("kind")?;
        assert_eq!(kind.type_name.as_deref(), Some("String"));
        let save = member(&payload, "save").ok_or("save")?;
        assert_eq!(save.kind, SymbolKind::Method);
        assert_eq!(save.type_name.as_deref(), Some("Function"));
        assert!(!save.is_unused());
        let this_type = index
            .declarations()
            .find(|declaration| declaration.name == "this")
            .and_then(|declaration| declaration.type_name.clone())
            .ok_or("this")?;
        assert_eq!(this_type, payload);
        assert!(member(&payload, "contact_type").is_some());
        assert!(member(&payload, "source").is_some());
        Ok(())
    }
}
