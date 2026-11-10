//! Tree-sitter extraction of Go declarations and reference candidates.
//! One file is parsed here. Other files are never reparsed. `go.mod` is read
//! only to name the module of this file.

use crate::{
    go_stdlib,
    symbol_index::{
        FileSymbols, PhpTypeSource, ReferenceLocation, SymbolDeclaration, SymbolKind, Visibility,
        reference_preview,
    },
};
use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    path::{Component, Path},
};
use tree_sitter::Node;

struct Facts<'a> {
    path: &'a Path,
    source: &'a str,
    prefix: String,
    package_name: String,
    imports: HashMap<String, String>,
    dot_imports: Vec<String>,
    declared: HashSet<usize>,
    symbols: FileSymbols,
}

/// Extracts declarations and references from one Go source.
///
/// # Errors
/// Returns a parser failure. A tree that contains error nodes still yields partial facts.
pub fn parse(path: &Path, source: &str) -> crate::Result<FileSymbols> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_go::LANGUAGE.into())
        .map_err(|error| crate::Error::Protocol(error.to_string()))?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| crate::Error::Protocol("Go index parse cancelled".into()))?;
    let package_name = package_name(tree.root_node(), source);
    let mut facts = Facts {
        path,
        source,
        prefix: package_prefix(path, &package_name),
        package_name,
        imports: HashMap::new(),
        dot_imports: Vec::new(),
        declared: HashSet::new(),
        symbols: FileSymbols::default(),
    };
    if !facts.package_name.is_empty()
        && let Some(node) = named_kind(tree.root_node(), "package_clause")
            .and_then(|clause| named_kind(clause, "package_identifier"))
    {
        let name = facts.package_name.clone();
        let fqn = format!("package::{}", facts.prefix);
        let signature = format!("package {name}");
        facts.declare(declared(
            node,
            name,
            fqn,
            SymbolKind::Constant,
            signature,
            0..source.len(),
        ));
    }
    prescan(tree.root_node(), &mut facts);
    walk_file(tree.root_node(), &mut facts);
    Ok(facts.symbols)
}

fn package_name(root: Node<'_>, source: &str) -> String {
    named_kind(root, "package_clause")
        .and_then(|node| named_kind(node, "package_identifier"))
        .map(|node| text(node, source).to_string())
        .unwrap_or_default()
}

fn package_prefix(path: &Path, package_name: &str) -> String {
    let dir = path.parent().unwrap_or(path);
    let import_path = module_import(dir);
    if package_name.ends_with("_test") && package_name != "test" {
        format!("{import_path}#{package_name}")
    } else {
        import_path
    }
}

fn module_import(dir: &Path) -> String {
    let mut current = dir;
    loop {
        let manifest = current.join("go.mod");
        if let Ok(source) = std::fs::read_to_string(&manifest)
            && let Some(module) = module_name(&source)
        {
            let mut package = module;
            if let Ok(relative) = dir.strip_prefix(current) {
                for component in relative.components() {
                    if let Component::Normal(part) = component {
                        package.push('/');
                        package.push_str(&part.to_string_lossy());
                    }
                }
            }
            return package;
        }
        let Some(parent) = current.parent() else {
            break;
        };
        if parent == current {
            break;
        }
        current = parent;
    }
    dir.display().to_string()
}

fn module_name(source: &str) -> Option<String> {
    for line in source.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("module ") else {
            continue;
        };
        let name = rest.split_whitespace().next().unwrap_or("");
        if !name.is_empty() && !name.starts_with("//") {
            return Some(name.to_string());
        }
    }
    None
}

struct Declared<'a> {
    node: Node<'a>,
    name: String,
    fqn: String,
    kind: SymbolKind,
    owner: Option<String>,
    type_name: Option<String>,
    type_source: Option<PhpTypeSource>,
    signature: String,
    scope: Range<usize>,
}

const fn declared(
    node: Node<'_>,
    name: String,
    fqn: String,
    kind: SymbolKind,
    signature: String,
    scope: Range<usize>,
) -> Declared<'_> {
    Declared {
        node,
        name,
        fqn,
        kind,
        owner: None,
        type_name: None,
        type_source: None,
        signature,
        scope,
    }
}

impl Facts<'_> {
    fn declare(&mut self, declared: Declared<'_>) {
        let Declared {
            node,
            name,
            fqn,
            kind,
            owner,
            type_name,
            type_source,
            signature,
            scope,
        } = declared;
        if name.is_empty() || name == "_" {
            return;
        }
        let access = visibility(&name);
        self.declared.insert(node.start_byte());
        let (line, column) = point(node);
        let documentation = documentation_of(node, self.source);
        self.symbols.declarations.push(SymbolDeclaration {
            fqn,
            name,
            kind,
            path: self.path.into(),
            range: node.byte_range(),
            line,
            column,
            owner,
            base_types: Vec::new(),
            trait_types: Vec::new(),
            scope,
            visibility: access,
            is_static: false,
            type_name,
            type_source,
            signature,
            documentation,
            usages_count: 0,
        });
    }

    fn qualify(&self, name: &str) -> String {
        if go_stdlib::PREDECLARED.contains(&name) {
            format!("builtin::{name}")
        } else {
            format!("{}::{name}", self.prefix)
        }
    }

    fn type_fqn(&self, node: Node<'_>) -> Option<String> {
        match node.kind() {
            "parenthesized_type" | "pointer_type" | "negated_type" => {
                node.named_child(0).and_then(|child| self.type_fqn(child))
            }
            "type_identifier" => Some(self.qualify(text(node, self.source))),
            "qualified_type" => {
                let package = field(node, "package")?;
                let name = field(node, "name")?;
                let package_name = text(package, self.source);
                let path = self
                    .imports
                    .get(package_name)
                    .cloned()
                    .or_else(|| {
                        go_stdlib::package(package_name).map(|package| package.path.to_string())
                    })
                    .unwrap_or_else(|| package_name.to_string());
                Some(format!("{path}::{}", text(name, self.source)))
            }
            "generic_type" => field(node, "type").and_then(|child| self.type_fqn(child)),
            "parameter_list" => node
                .named_children(&mut node.walk())
                .find_map(|child| field(child, "type").and_then(|ty| self.type_fqn(ty))),
            _ => None,
        }
    }

    fn result_type(&self, node: Node<'_>) -> Option<String> {
        field(node, "result").and_then(|result| self.type_fqn(result))
    }

    fn reference(&mut self, node: Node<'_>, names: Vec<String>) {
        let name = text(node, self.source);
        if name.is_empty()
            || name == "_"
            || self.declared.contains(&node.start_byte())
            || go_stdlib::KEYWORDS.contains(&name)
        {
            return;
        }
        let mut names = names;
        if self.package_name.ends_with("_test")
            && name.chars().next().is_some_and(char::is_uppercase)
            && let Some(import_path) = self.prefix.split('#').next()
        {
            names.push(format!("{import_path}::{name}"));
        }
        for path in &self.dot_imports {
            names.push(format!("{path}::{name}"));
        }
        self.symbols
            .references
            .push((names, location(node, self.source, self.path)));
    }

    fn member(&mut self, receiver: PhpTypeSource, name: &str, node: Node<'_>) {
        if name.is_empty() || name == "_" {
            return;
        }
        self.symbols.member_references.push((
            receiver,
            name.to_string(),
            location(node, self.source, self.path),
        ));
    }
}

fn prescan(root: Node<'_>, facts: &mut Facts<'_>) {
    for child in named(root) {
        match child.kind() {
            "import_declaration" => scan_imports(child, facts),
            "function_declaration" => scan_function(child, facts),
            "method_declaration" => scan_method(child, facts),
            "type_declaration" => scan_types(child, facts),
            "const_declaration" | "var_declaration" => scan_values(child, facts, None),
            _ => {}
        }
    }
}

fn scan_imports(node: Node<'_>, facts: &mut Facts<'_>) {
    for spec in named(node).into_iter().flat_map(|child| {
        if child.kind() == "import_spec" {
            vec![child]
        } else {
            named(child)
                .into_iter()
                .filter(|spec| spec.kind() == "import_spec")
                .collect()
        }
    }) {
        let Some(path_node) = field(spec, "path") else {
            continue;
        };
        let path = import_path(text(path_node, facts.source));
        if path.is_empty() {
            continue;
        }
        let alias_node = field(spec, "name");
        let alias = alias_node.map(|node| text(node, facts.source).to_string());
        match alias.as_deref() {
            Some("_") => {}
            Some(".") => {
                facts.dot_imports.push(path.clone());
                if let Some(node) = alias_node {
                    declare_import(facts, node, ".", &path);
                }
            }
            Some(name) => {
                facts.imports.insert(name.to_string(), path.clone());
                if let Some(node) = alias_node {
                    declare_import(facts, node, name, &path);
                }
            }
            None => {
                let name = path.rsplit('/').next().unwrap_or(&path);
                facts.imports.insert(name.to_string(), path.clone());
                declare_import(facts, path_node, name, &path);
            }
        }
    }
}

fn declare_import(facts: &mut Facts<'_>, node: Node<'_>, alias: &str, path: &str) {
    let file = facts.path.display().to_string();
    let end = facts.source.len();
    let fqn = format!("import::{file}::{alias}");
    facts.declare(Declared {
        type_source: Some(PhpTypeSource::Class(format!("package:{path}"))),
        ..declared(
            node,
            alias.to_string(),
            fqn,
            SymbolKind::Variable,
            format!("import \"{path}\""),
            0..end,
        )
    });
}

fn scan_function(node: Node<'_>, facts: &mut Facts<'_>) {
    let Some(name_node) = field(node, "name") else {
        return;
    };
    let name = text(name_node, facts.source).to_string();
    let fqn = format!("{}::{name}", facts.prefix);
    let type_name = facts.result_type(node);
    facts.declare(Declared {
        type_name,
        ..declared(
            name_node,
            name,
            fqn,
            SymbolKind::Function,
            signature_of(node, facts.source),
            node.byte_range(),
        )
    });
}

fn scan_method(node: Node<'_>, facts: &mut Facts<'_>) {
    let Some(name_node) = field(node, "name") else {
        return;
    };
    let name = text(name_node, facts.source).to_string();
    let owner = field(node, "receiver").and_then(|receiver| facts.type_fqn(receiver));
    let fqn = owner.as_ref().map_or_else(
        || format!("{}::{name}", facts.prefix),
        |owner| format!("{owner}::{name}"),
    );
    let type_name = facts.result_type(node);
    facts.declare(Declared {
        owner,
        type_name,
        ..declared(
            name_node,
            name,
            fqn,
            SymbolKind::Method,
            signature_of(node, facts.source),
            node.byte_range(),
        )
    });
}

fn scan_types(node: Node<'_>, facts: &mut Facts<'_>) {
    for spec in named(node) {
        if !matches!(spec.kind(), "type_spec" | "type_alias") {
            for nested in named(spec) {
                if matches!(nested.kind(), "type_spec" | "type_alias") {
                    scan_type(nested, facts);
                }
            }
            continue;
        }
        scan_type(spec, facts);
    }
}

fn scan_type(node: Node<'_>, facts: &mut Facts<'_>) {
    let Some(name_node) = field(node, "name") else {
        return;
    };
    let name = text(name_node, facts.source).to_string();
    let fqn = format!("{}::{name}", facts.prefix);
    facts.declare(declared(
        name_node,
        name,
        fqn.clone(),
        SymbolKind::Class,
        signature_of(node, facts.source),
        0..facts.source.len(),
    ));
    if let Some(body) = field(node, "type") {
        scan_type_body(&fqn, body, facts);
    }
}

fn scan_type_body(owner: &str, node: Node<'_>, facts: &mut Facts<'_>) {
    match node.kind() {
        "struct_type" => {
            for field_node in named(node)
                .into_iter()
                .filter(|child| child.kind() == "field_declaration_list")
                .flat_map(|list| named(list))
            {
                scan_field(owner, field_node, facts);
            }
        }
        "interface_type" => {
            for child in named(node) {
                if child.kind() == "method_elem" {
                    scan_interface_method(owner, child, facts);
                } else if let Some(embedded) = facts.type_fqn(child) {
                    push_base(facts, owner, embedded);
                }
            }
        }
        _ => {}
    }
}

fn scan_field(owner: &str, node: Node<'_>, facts: &mut Facts<'_>) {
    let type_name = field(node, "type").and_then(|ty| facts.type_fqn(ty));
    let names = fields(node, "name");
    if names.is_empty() {
        if let Some(embedded) = type_name {
            push_base(facts, owner, embedded);
        }
        return;
    }
    for name_node in names {
        let name = text(name_node, facts.source).to_string();
        let fqn = format!("{owner}::{name}");
        facts.declare(Declared {
            owner: Some(owner.to_string()),
            type_name: type_name.clone(),
            ..declared(
                name_node,
                name,
                fqn,
                SymbolKind::Property,
                signature_of(node, facts.source),
                0..facts.source.len(),
            )
        });
    }
}

fn scan_interface_method(owner: &str, node: Node<'_>, facts: &mut Facts<'_>) {
    let Some(name_node) = field(node, "name") else {
        return;
    };
    let name = text(name_node, facts.source).to_string();
    let fqn = format!("{owner}::{name}");
    facts.declare(Declared {
        owner: Some(owner.to_string()),
        type_name: facts.result_type(node),
        ..declared(
            name_node,
            name,
            fqn,
            SymbolKind::Method,
            signature_of(node, facts.source),
            0..facts.source.len(),
        )
    });
}

fn scan_values(node: Node<'_>, facts: &mut Facts<'_>, owner: Option<&str>) {
    for spec in named(node) {
        if matches!(spec.kind(), "const_spec" | "var_spec") {
            let type_name = field(spec, "type").and_then(|ty| facts.type_fqn(ty));
            let kind = if node.kind() == "const_declaration" || spec.kind() == "const_spec" {
                SymbolKind::Constant
            } else {
                SymbolKind::Variable
            };
            for name_node in fields(spec, "name") {
                let name = text(name_node, facts.source).to_string();
                let fqn = owner.map_or_else(
                    || format!("{}::{name}", facts.prefix),
                    |owner| format!("{owner}::{name}"),
                );
                facts.declare(Declared {
                    owner: owner.map(ToString::to_string),
                    type_name: type_name.clone(),
                    ..declared(
                        name_node,
                        name,
                        fqn,
                        kind,
                        signature_of(spec, facts.source),
                        0..facts.source.len(),
                    )
                });
            }
        } else {
            scan_values(spec, facts, owner);
        }
    }
}

fn push_base(facts: &mut Facts<'_>, owner: &str, base: String) {
    if let Some(declaration) = facts
        .symbols
        .declarations
        .iter_mut()
        .rev()
        .find(|declaration| declaration.fqn == owner)
    {
        declaration.base_types.push(base);
    }
}

fn walk_file(root: Node<'_>, facts: &mut Facts<'_>) {
    for child in named(root) {
        match child.kind() {
            "function_declaration" | "method_declaration" => walk_function(child, facts),
            "const_declaration" | "var_declaration" => walk_value_refs(child, facts),
            "type_declaration" => walk_type_refs(child, facts),
            "import_declaration" | "package_clause" => {}
            _ => walk_stmt(child, facts, 0..facts.source.len()),
        }
    }
}

fn walk_function(node: Node<'_>, facts: &mut Facts<'_>) {
    let scope = node.byte_range();
    if let Some(receiver) = field(node, "receiver") {
        declare_params(receiver, facts, scope.clone());
        walk_type_node(receiver, facts);
    }
    if let Some(parameters) = field(node, "parameters") {
        declare_params(parameters, facts, scope.clone());
        walk_type_node(parameters, facts);
    }
    if let Some(result) = field(node, "result") {
        walk_type_node(result, facts);
    }
    if let Some(body) = field(node, "body") {
        walk_stmt(body, facts, scope);
    }
}

fn declare_params(list: Node<'_>, facts: &mut Facts<'_>, scope: Range<usize>) {
    for parameter in named(list) {
        let type_name = field(parameter, "type").and_then(|ty| facts.type_fqn(ty));
        for name_node in fields(parameter, "name") {
            let name = text(name_node, facts.source).to_string();
            let fqn = format!(
                "{}:{scope_start}:{name}",
                facts.path.display(),
                scope_start = scope.start
            );
            facts.declare(Declared {
                type_name: type_name.clone(),
                ..declared(
                    name_node,
                    name,
                    fqn,
                    SymbolKind::Variable,
                    signature_of(parameter, facts.source),
                    scope.clone(),
                )
            });
        }
    }
}

fn walk_value_refs(node: Node<'_>, facts: &mut Facts<'_>) {
    for spec in named(node) {
        if let Some(value) = field(spec, "value") {
            walk_expr(value, facts, 0..facts.source.len());
        } else {
            walk_value_refs(spec, facts);
        }
        if let Some(ty) = field(spec, "type") {
            walk_type_node(ty, facts);
        }
    }
}

fn walk_type_refs(node: Node<'_>, facts: &mut Facts<'_>) {
    for child in named(node) {
        if let Some(ty) = field(child, "type") {
            walk_type_node(ty, facts);
        } else {
            walk_type_refs(child, facts);
        }
    }
}

fn walk_type_node(node: Node<'_>, facts: &mut Facts<'_>) {
    match node.kind() {
        "type_identifier" => {
            let name = text(node, facts.source);
            let qualified = facts.qualify(name);
            facts.reference(node, vec![qualified]);
        }
        "qualified_type" => {
            if let Some(name_node) = field(node, "name")
                && let Some(package) = field(node, "package")
            {
                let package_name = text(package, facts.source);
                let path = facts
                    .imports
                    .get(package_name)
                    .cloned()
                    .unwrap_or_else(|| package_name.to_string());
                let type_name = text(name_node, facts.source).to_string();
                facts.reference(name_node, vec![format!("{path}::{type_name}")]);
            }
        }
        "comment" | "interpreted_string_literal" | "raw_string_literal" => {}
        _ => {
            for child in named(node) {
                walk_type_node(child, facts);
            }
        }
    }
}

fn walk_stmt(node: Node<'_>, facts: &mut Facts<'_>, scope: Range<usize>) {
    match node.kind() {
        "short_var_declaration" => walk_short_var(node, facts, scope),
        "if_statement"
        | "for_statement"
        | "expression_switch_statement"
        | "type_switch_statement"
        | "select_statement" => {
            let inner = node.byte_range();
            for child in named(node) {
                walk_stmt(child, facts, inner.clone());
            }
        }
        "range_clause" => walk_range(node, facts, scope),
        "const_declaration" | "var_declaration" => {
            scan_values(node, facts, None);
            walk_value_refs(node, facts);
        }
        "function_declaration" | "method_declaration" => walk_function(node, facts),
        "comment" | "interpreted_string_literal" | "raw_string_literal" => {}
        _ => {
            for child in named(node) {
                if matches!(
                    child.kind(),
                    "identifier"
                        | "type_identifier"
                        | "field_identifier"
                        | "call_expression"
                        | "selector_expression"
                        | "composite_literal"
                        | "func_literal"
                        | "unary_expression"
                        | "binary_expression"
                        | "parenthesized_expression"
                        | "index_expression"
                        | "slice_expression"
                        | "type_assertion_expression"
                        | "type_conversion_expression"
                ) {
                    walk_expr(child, facts, scope.clone());
                } else {
                    walk_stmt(child, facts, scope.clone());
                }
            }
        }
    }
}

fn walk_short_var(node: Node<'_>, facts: &mut Facts<'_>, scope: Range<usize>) {
    let right = field(node, "right");
    if let Some(right) = right {
        walk_expr(right, facts, scope.clone());
    }
    let results = right
        .and_then(|right| call_results(right, facts))
        .unwrap_or_default();
    let left = field(node, "left")
        .map(|left| named(left))
        .unwrap_or_default();
    for (index, name_node) in left.iter().enumerate() {
        if name_node.kind() != "identifier" {
            continue;
        }
        let name = text(*name_node, facts.source).to_string();
        let type_name = results.get(index).cloned().or_else(|| {
            (name == "err" && index + 1 == left.len()).then(|| "builtin::error".to_string())
        });
        let file = facts.path.display().to_string();
        let fqn = format!("{file}:{}:{name}", name_node.start_byte());
        facts.declare(Declared {
            type_name,
            ..declared(
                *name_node,
                name,
                fqn,
                SymbolKind::Variable,
                signature_of(node, facts.source),
                scope.clone(),
            )
        });
    }
}

fn walk_range(node: Node<'_>, facts: &mut Facts<'_>, scope: Range<usize>) {
    if let Some(right) = field(node, "right") {
        walk_expr(right, facts, scope.clone());
    }
    if let Some(left) = field(node, "left") {
        for name_node in named(left) {
            if name_node.kind() != "identifier" || facts.declared.contains(&name_node.start_byte())
            {
                walk_expr(name_node, facts, scope.clone());
                continue;
            }
            let name = text(name_node, facts.source).to_string();
            let file = facts.path.display().to_string();
            let fqn = format!("{file}:{}:{name}", name_node.start_byte());
            facts.declare(declared(
                name_node,
                name,
                fqn,
                SymbolKind::Variable,
                signature_of(node, facts.source),
                scope.clone(),
            ));
        }
    }
}

fn walk_expr(node: Node<'_>, facts: &mut Facts<'_>, scope: Range<usize>) {
    match node.kind() {
        "call_expression" => {
            if let Some(function) = field(node, "function") {
                walk_callee(function, facts, scope.clone());
            }
            if let Some(arguments) = field(node, "arguments") {
                for child in named(arguments) {
                    walk_expr(child, facts, scope.clone());
                }
            }
        }
        "selector_expression" => walk_selector(node, facts, scope),
        "composite_literal" => {
            if let Some(ty) = field(node, "type") {
                walk_type_node(ty, facts);
            }
            if let Some(body) = field(node, "body") {
                let owner = field(node, "type").and_then(|ty| facts.type_fqn(ty));
                for child in named(body) {
                    if child.kind() == "keyed_element" {
                        let keyed = field(child, "key").and_then(|key| {
                            (key.kind() == "identifier")
                                .then(|| (key, text(key, facts.source).to_string(), owner.clone()))
                        });
                        if let Some((key, key_name, Some(owner))) = keyed {
                            facts.member(PhpTypeSource::Class(owner), &key_name, key);
                        }
                        if let Some(value) = field(child, "value") {
                            walk_expr(value, facts, scope.clone());
                        }
                    } else {
                        walk_expr(child, facts, scope.clone());
                    }
                }
            }
        }
        "func_literal" => {
            let inner = node.byte_range();
            if let Some(parameters) = field(node, "parameters") {
                declare_params(parameters, facts, inner.clone());
            }
            if let Some(body) = field(node, "body") {
                walk_stmt(body, facts, inner);
            }
        }
        "identifier" | "type_identifier" | "field_identifier" => {
            let name = text(node, facts.source);
            if let Some(found) = binding(facts, name, node.start_byte()) {
                let fqn = found.fqn.clone();
                facts.reference(node, vec![fqn]);
            } else {
                let qualified = facts.qualify(name);
                let local = local_candidate(facts, name);
                facts.reference(node, vec![qualified, local]);
            }
        }
        "comment"
        | "interpreted_string_literal"
        | "raw_string_literal"
        | "int_literal"
        | "float_literal"
        | "imaginary_literal"
        | "rune_literal"
        | "nil"
        | "true"
        | "false"
        | "iota" => {}
        _ => {
            for child in named(node) {
                walk_expr(child, facts, scope.clone());
            }
        }
    }
}

fn walk_callee(node: Node<'_>, facts: &mut Facts<'_>, scope: Range<usize>) {
    if node.kind() == "selector_expression" {
        walk_selector(node, facts, scope);
        return;
    }
    if node.kind() == "identifier" {
        let name = text(node, facts.source);
        if binding(facts, name, node.start_byte()).is_none() {
            let qualified = facts.qualify(name);
            facts.reference(node, vec![qualified]);
        }
        return;
    }
    walk_expr(node, facts, scope);
}

fn walk_selector(node: Node<'_>, facts: &mut Facts<'_>, scope: Range<usize>) {
    let Some(field_node) = field(node, "field") else {
        return;
    };
    let field_name = text(field_node, facts.source).to_string();
    let Some(operand) = field(node, "operand") else {
        return;
    };
    if operand.kind() == "identifier" {
        let alias = text(operand, facts.source);
        if let Some(path) = package_path(facts, alias, operand.start_byte()) {
            let alias_fqn = package_alias_fqn(facts, alias);
            facts.reference(operand, alias_fqn);
            facts.reference(field_node, vec![format!("{path}::{field_name}")]);
            return;
        }
    }
    walk_expr(operand, facts, scope);
    if let Some(receiver) = expr_type(operand, facts) {
        facts.member(receiver, &field_name, field_node);
    }
}

fn package_alias_fqn(facts: &Facts<'_>, alias: &str) -> Vec<String> {
    facts
        .symbols
        .declarations
        .iter()
        .rev()
        .find(|declaration| {
            declaration.name == alias && declaration.signature.starts_with("import ")
        })
        .map(|declaration| vec![declaration.fqn.clone()])
        .unwrap_or_default()
}

fn package_path(facts: &Facts<'_>, name: &str, byte: usize) -> Option<String> {
    if let Some(binding) = binding(facts, name, byte)
        && let Some(PhpTypeSource::Class(path)) = &binding.type_source
        && let Some(path) = path.strip_prefix("package:")
    {
        return Some(path.to_string());
    }
    if binding(facts, name, byte).is_some() {
        return None;
    }
    facts
        .imports
        .get(name)
        .cloned()
        .or_else(|| go_stdlib::package(name).map(|package| package.path.to_string()))
}

fn call_results(node: Node<'_>, facts: &Facts<'_>) -> Option<Vec<String>> {
    let call = if node.kind() == "call_expression" {
        node
    } else {
        named(node)
            .into_iter()
            .find(|child| child.kind() == "call_expression")?
    };
    let function = field(call, "function")?;
    if function.kind() == "identifier" {
        let name = text(function, facts.source);
        if let Some(found) = find_fqn(facts, &facts.qualify(name)) {
            return found.type_name.clone().map(|type_name| vec![type_name]);
        }
        return go_stdlib::builtin_return(name).map(|type_name| vec![type_name.to_string()]);
    }
    if function.kind() == "selector_expression" {
        let operand = field(function, "operand")?;
        let field_node = field(function, "field")?;
        let field_name = text(field_node, facts.source);
        if operand.kind() == "identifier"
            && let Some(path) =
                package_path(facts, text(operand, facts.source), operand.start_byte())
        {
            if let Some(found) = find_fqn(facts, &format!("{path}::{field_name}")) {
                return found.type_name.clone().map(|type_name| vec![type_name]);
            }
            return go_stdlib::func_return(&path, field_name)
                .map(|type_name| vec![type_name.to_string()]);
        }
    }
    None
}

fn expr_type(node: Node<'_>, facts: &Facts<'_>) -> Option<PhpTypeSource> {
    match node.kind() {
        "identifier" => {
            let name = text(node, facts.source);
            if let Some(found) = binding(facts, name, node.start_byte()) {
                if found
                    .type_source
                    .as_ref()
                    .is_some_and(|source| matches!(source, PhpTypeSource::Class(path) if path.starts_with("package:")))
                {
                    return None;
                }
                if let Some(type_name) = &found.type_name {
                    return Some(PhpTypeSource::Class(type_name.clone()));
                }
                return found.type_source.clone();
            }
            Some(PhpTypeSource::Symbol(vec![facts.qualify(name)]))
        }
        "call_expression" => {
            let results = call_results(node, facts)?;
            results
                .first()
                .map(|type_name| PhpTypeSource::Class(type_name.clone()))
        }
        "selector_expression" => {
            let operand = field(node, "operand")?;
            let field_name = text(field(node, "field")?, facts.source);
            if operand.kind() == "identifier"
                && package_path(facts, text(operand, facts.source), operand.start_byte()).is_some()
            {
                return None;
            }
            Some(PhpTypeSource::Member(
                Box::new(expr_type(operand, facts)?),
                field_name.to_string(),
            ))
        }
        "unary_expression" | "parenthesized_expression" => node
            .named_child(0)
            .and_then(|child| expr_type(child, facts)),
        "composite_literal" => field(node, "type")
            .and_then(|ty| facts.type_fqn(ty))
            .map(PhpTypeSource::Class),
        _ => None,
    }
}

fn binding<'a>(facts: &'a Facts<'_>, name: &str, byte: usize) -> Option<&'a SymbolDeclaration> {
    facts
        .symbols
        .declarations
        .iter()
        .filter(|declaration| {
            declaration.name == name
                && declaration.scope.contains(&byte)
                && declaration.kind == SymbolKind::Variable
        })
        .min_by_key(|declaration| {
            declaration
                .scope
                .end
                .saturating_sub(declaration.scope.start)
        })
}

fn find_fqn<'a>(facts: &'a Facts<'_>, fqn: &str) -> Option<&'a SymbolDeclaration> {
    facts
        .symbols
        .declarations
        .iter()
        .find(|declaration| declaration.fqn == fqn)
}

fn local_candidate(facts: &Facts<'_>, name: &str) -> String {
    format!("{}::{name}", facts.prefix)
}

fn import_path(token: &str) -> String {
    token
        .trim()
        .trim_matches(|character| character == '"' || character == '`')
        .to_string()
}

fn visibility(name: &str) -> Visibility {
    if name.chars().next().is_some_and(char::is_uppercase) {
        Visibility::Public
    } else {
        Visibility::Private
    }
}

fn text<'a>(node: Node<'_>, source: &'a str) -> &'a str {
    source.get(node.byte_range()).unwrap_or("")
}

fn field<'a>(node: Node<'a>, name: &str) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    if !cursor.goto_first_child() {
        return None;
    }
    loop {
        if cursor.field_name() == Some(name) {
            return Some(cursor.node());
        }
        if !cursor.goto_next_sibling() {
            return None;
        }
    }
}

fn fields<'a>(node: Node<'a>, name: &str) -> Vec<Node<'a>> {
    let mut cursor = node.walk();
    let mut output = Vec::new();
    if !cursor.goto_first_child() {
        return output;
    }
    loop {
        if cursor.field_name() == Some(name) {
            output.push(cursor.node());
        }
        if !cursor.goto_next_sibling() {
            break;
        }
    }
    output
}

fn named(node: Node<'_>) -> Vec<Node<'_>> {
    node.named_children(&mut node.walk()).collect()
}

fn named_kind<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    named(node).into_iter().find(|child| child.kind() == kind)
}

fn point(node: Node<'_>) -> (u32, u32) {
    let start = node.start_position();
    (
        u32::try_from(start.row).unwrap_or(u32::MAX),
        u32::try_from(start.column).unwrap_or(u32::MAX),
    )
}

fn location(node: Node<'_>, source: &str, path: &Path) -> ReferenceLocation {
    let byte = node.start_byte();
    let (preview, preview_offset) = reference_preview(source, byte);
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
    let start = node.start_byte();
    let line_start = source
        .get(..start)
        .and_then(|prefix| prefix.rfind('\n'))
        .map_or(0, |index| index + 1);
    let line_end = source
        .get(start..)
        .and_then(|tail| tail.find('\n'))
        .map_or(source.len(), |index| start + index);
    let mut line = source
        .get(line_start..line_end)
        .unwrap_or("")
        .trim()
        .to_string();
    if line.len() > 180 {
        let mut limit = 180;
        while !line.is_char_boundary(limit) {
            limit -= 1;
        }
        line.truncate(limit);
        line.push('…');
    }
    line
}

fn documentation_of(node: Node<'_>, source: &str) -> String {
    let mut current = node;
    for _ in 0..3 {
        let Some(previous) = current.prev_named_sibling() else {
            if let Some(parent) = current.parent() {
                current = parent;
                continue;
            }
            break;
        };
        if previous.kind() == "comment" {
            let comment = text(previous, source).trim();
            if comment.len() <= 500 {
                return comment.to_string();
            }
        }
        break;
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::parse;
    use crate::symbol_index::{SymbolIndex, SymbolKind};
    use std::path::Path;

    const SOURCE: &str = r#"package demo

import (
	"fmt"
	"strings"
)

// Server serves one name.
type Server struct {
	Name string
}

func (s *Server) Listen(addr string) error {
	if addr == "" {
		return fmt.Errorf("missing")
	}
	fmt.Println(strings.TrimSpace(s.Name))
	return nil
}

func New(name string) *Server {
	return &Server{Name: name}
}
"#;

    #[test]
    fn declarations_methods_and_calls_are_indexed() -> Result<(), Box<dyn std::error::Error>> {
        let path = Path::new("/work/demo.go");
        let facts = parse(path, SOURCE)?;
        let server = facts
            .declarations
            .iter()
            .find(|declaration| {
                declaration.name == "Server" && declaration.kind == SymbolKind::Class
            })
            .ok_or("Server")?;
        let listen = facts
            .declarations
            .iter()
            .find(|declaration| declaration.name == "Listen")
            .ok_or("Listen")?;
        assert_eq!(listen.owner.as_deref(), Some(server.fqn.as_str()));
        assert!(
            listen
                .type_name
                .as_deref()
                .is_some_and(|ty| ty.ends_with("error"))
        );
        assert!(
            facts
                .declarations
                .iter()
                .any(|declaration| declaration.name == "New")
        );
        assert!(
            facts
                .declarations
                .iter()
                .any(|declaration| declaration.name == "Name")
        );
        assert!(facts.declarations.iter().any(|declaration| {
            declaration.name == "fmt" && declaration.signature.contains("import \"fmt\"")
        }));
        let mut index = SymbolIndex::new(crate::symbol_index::LanguageId::Go);
        index.replace(path.into(), facts);
        let println = SOURCE.find("Println").ok_or("Println")?;
        assert!(index.at(path, println).is_none());
        let name = SOURCE.find("Name string").ok_or("field")?;
        assert_eq!(
            index
                .at(path, name)
                .map(|declaration| declaration.name.as_str()),
            Some("Name")
        );
        let call = SOURCE.find("s.Name").ok_or("use")?;
        assert_eq!(
            index
                .at(path, call + 2)
                .map(|declaration| declaration.name.as_str()),
            Some("Name")
        );
        Ok(())
    }
}
