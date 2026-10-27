//! Tree-sitter extraction. Parsing is confined to the project worker.
use crate::symbol_index::{
    FileSymbols, ReferenceLocation, SymbolDeclaration, SymbolKind, Visibility,
};
use std::{collections::HashMap, ops::Range, path::Path};
use tree_sitter::Node;

#[derive(Clone, Default)]
struct Context {
    namespace: String,
    aliases: HashMap<String, String>,
    owner: Option<String>,
    parent: Option<String>,
    scope: Range<usize>,
}
impl Context {
    fn qualify(&self, name: &str) -> String {
        if matches!(name.to_ascii_lowercase().as_str(), "self" | "static")
            && let Some(owner) = &self.owner
        {
            return owner.clone();
        }
        if name.eq_ignore_ascii_case("parent")
            && let Some(parent) = &self.parent
        {
            return parent.clone();
        }
        if let Some(relative) = name.strip_prefix("namespace\\") {
            return crate::type_inference::qualify(relative, &self.namespace);
        }
        let (first, rest) = name.split_once('\\').unwrap_or((name, ""));
        self.aliases.get(&first.to_lowercase()).map_or_else(
            || crate::type_inference::qualify(name, &self.namespace),
            |alias| {
                if rest.is_empty() {
                    alias.clone()
                } else {
                    format!("{alias}\\{rest}")
                }
            },
        )
    }
}
fn text<'a>(node: Node<'_>, source: &'a str) -> &'a str {
    source.get(node.byte_range()).unwrap_or("")
}
fn children(node: Node<'_>) -> Vec<Node<'_>> {
    node.named_children(&mut node.walk()).collect()
}
fn class_kind(kind: &str) -> bool {
    matches!(
        kind,
        "class_declaration" | "interface_declaration" | "trait_declaration" | "enum_declaration"
    )
}
fn callable(kind: &str) -> bool {
    matches!(
        kind,
        "method_declaration" | "function_definition" | "anonymous_function" | "arrow_function"
    )
}
fn enter(node: Node<'_>, source: &str, context: &Context) -> Context {
    let mut context = context.clone();
    if node.kind() == "namespace_definition" {
        context.namespace = node
            .child_by_field_name("name")
            .map_or("", |n| text(n, source))
            .trim_matches('\\')
            .into();
        context.aliases.clear();
        if node.child_by_field_name("body").is_some() {
            context.scope = node.byte_range();
        }
    }
    if class_kind(node.kind())
        && let Some(name) = node.child_by_field_name("name")
    {
        context.parent = children(node)
            .into_iter()
            .find(|n| n.kind() == "base_clause")
            .and_then(|n| n.named_child(0))
            .map(|n| context.qualify(text(n, source)).to_lowercase());
        context.owner = Some(context.qualify(text(name, source)).to_lowercase());
        context.scope = node.byte_range();
    }
    if callable(node.kind()) {
        context.scope = node.byte_range();
    }
    context
}
fn import_target<'tree, 'source>(
    clause: Node<'tree>,
    source: &'source str,
) -> Option<(Node<'tree>, String, &'source str)> {
    let parent = clause.parent()?;
    if clause.child_by_field_name("type").is_some()
        || parent.child_by_field_name("type").is_some()
        || parent.parent().is_some_and(|p| {
            parent.kind() == "namespace_use_group" && p.child_by_field_name("type").is_some()
        })
    {
        return None;
    }
    let name = clause.named_child(0)?;
    let prefix = if parent.kind() == "namespace_use_group" {
        parent
            .parent()
            .and_then(|p| p.named_child(0))
            .map_or("", |n| text(n, source))
            .trim_end_matches('\\')
    } else {
        ""
    };
    let full = if prefix.is_empty() {
        text(name, source).to_string()
    } else {
        format!("{prefix}\\{}", text(name, source))
    };
    let alias = clause.child_by_field_name("alias").map_or_else(
        || text(name, source).rsplit('\\').next().unwrap_or(""),
        |n| text(n, source),
    );
    Some((
        name,
        crate::type_inference::qualify(&full, "").to_lowercase(),
        alias,
    ))
}
fn import(node: Node<'_>, source: &str, context: &mut Context) {
    if node.kind() != "namespace_use_declaration" {
        return;
    }
    let clauses = node
        .child_by_field_name("body")
        .map_or_else(|| children(node), children);
    for clause in clauses
        .into_iter()
        .filter(|n| n.kind() == "namespace_use_clause")
    {
        if let Some((_, full, alias)) = import_target(clause, source) {
            context.aliases.insert(alias.to_lowercase(), full);
        }
    }
}
fn visibility(raw: &str) -> Visibility {
    if raw.split_whitespace().any(|s| s == "private") {
        Visibility::Private
    } else if raw.split_whitespace().any(|s| s == "protected") {
        Visibility::Protected
    } else {
        Visibility::Public
    }
}
#[allow(
    clippy::too_many_lines,
    reason = "One AST declaration maps source coordinates, modifiers and metadata together"
)]
fn declaration(
    node: Node<'_>,
    source: &str,
    path: &Path,
    context: &Context,
) -> Option<SymbolDeclaration> {
    let kind = match node.kind() {
        k if class_kind(k) => SymbolKind::Class,
        "function_definition" => SymbolKind::Function,
        "method_declaration" => SymbolKind::Method,
        "property_element" => SymbolKind::Property,
        "const_element" => SymbolKind::Constant,
        "simple_parameter" | "variadic_parameter" | "assignment_expression" => SymbolKind::Variable,
        _ => return None,
    };
    let name_node = node
        .child_by_field_name(if node.kind() == "assignment_expression" {
            "left"
        } else {
            "name"
        })
        .or_else(|| {
            (kind == SymbolKind::Constant)
                .then(|| node.named_child(0))
                .flatten()
        })?;
    if kind == SymbolKind::Variable && name_node.kind() != "variable_name" {
        return None;
    }
    let name = text(name_node, source).to_string();
    let parent = node.parent().unwrap_or(node);
    let header = if matches!(kind, SymbolKind::Property | SymbolKind::Constant) {
        parent
    } else {
        node
    };
    let raw = text(header, source)
        .split('{')
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    let owner = (!matches!(
        kind,
        SymbolKind::Class | SymbolKind::Function | SymbolKind::Variable
    ))
    .then(|| context.owner.clone())
    .flatten();
    let fqn = if kind == SymbolKind::Variable {
        format!("{}:{}:{name}", path.display(), context.scope.start)
    } else if let Some(owner) = &owner {
        format!(
            "{owner}::{}",
            if kind == SymbolKind::Method {
                name.to_lowercase()
            } else {
                name.trim_start_matches('$').into()
            }
        )
    } else {
        let fqn = context.qualify(&name);
        if kind == SymbolKind::Constant {
            fqn
        } else {
            fqn.to_lowercase()
        }
    };
    let type_node = node
        .child_by_field_name("type")
        .or_else(|| parent.child_by_field_name("type"))
        .or_else(|| node.child_by_field_name("return_type"));
    let type_name = type_node
        .map(|n| {
            context
                .qualify(text(n, source).trim_start_matches('?'))
                .to_lowercase()
        })
        .or_else(|| {
            let right = node.child_by_field_name("right")?;
            (right.kind() == "object_creation_expression")
                .then(|| {
                    right
                        .named_child(0)
                        .map(|n| context.qualify(text(n, source)).to_lowercase())
                })
                .flatten()
        });
    let previous = header.prev_named_sibling();
    Some(SymbolDeclaration {
        fqn,
        name,
        kind,
        path: path.into(),
        range: name_node.byte_range(),
        line: u32::try_from(name_node.start_position().row).unwrap_or(u32::MAX),
        column: u32::try_from(name_node.start_position().column).unwrap_or(u32::MAX),
        owner,
        base_types: if kind == SymbolKind::Class {
            children(node)
                .into_iter()
                .filter(|n| matches!(n.kind(), "base_clause" | "class_interface_clause"))
                .flat_map(children)
                .map(|n| context.qualify(text(n, source)).to_lowercase())
                .collect()
        } else {
            Vec::new()
        },
        scope: if class_kind(node.kind()) {
            node.byte_range()
        } else {
            context.scope.clone()
        },
        visibility: visibility(&raw),
        is_static: raw.split_whitespace().any(|s| s == "static"),
        type_name,
        signature: raw,
        documentation: previous
            .filter(|n| n.kind() == "comment")
            .map_or("", |n| text(n, source))
            .into(),
        usages_count: 0,
    })
}
fn collect(node: Node<'_>, source: &str, path: &Path, context: &Context, facts: &mut FileSymbols) {
    // Class declarations must use their enclosing namespace, before entering their own scope.
    if let Some(d) = declaration(node, source, path, context) {
        facts.declarations.push(d);
    }
    let mut context = enter(node, source, context);
    if matches!(
        node.kind(),
        "program" | "namespace_definition" | "namespace_use_declaration"
    ) {
        import(node, source, &mut context);
        facts.name_scopes.push(crate::symbol_index::NameScope {
            range: node.start_byte()..context.scope.end,
            namespace: context.namespace.clone(),
            aliases: context.aliases.clone(),
        });
    }
    for child in children(node) {
        if child.kind() == "namespace_definition" && child.child_by_field_name("body").is_none() {
            context = enter(child, source, &context);
        }
        import(child, source, &mut context);
        collect(child, source, path, &context, facts);
    }
}
fn receiver(raw: &str, byte: usize, context: &Context, facts: &FileSymbols) -> Option<String> {
    if let Some((left, member)) = raw.rsplit_once("->") {
        let owner = receiver(left.trim_end(), byte, context, facts)?;
        return facts
            .declarations
            .iter()
            .find(|d| {
                d.owner.as_deref() == Some(&owner)
                    && d.name.trim_start_matches('$') == member.trim()
            })
            .and_then(|d| d.type_name.clone());
    }
    if raw.eq_ignore_ascii_case("parent") {
        return context.parent.clone();
    }
    if matches!(raw, "$this" | "self" | "static") {
        return context.owner.clone();
    }
    if raw.starts_with('$') {
        return facts
            .declarations
            .iter()
            .filter(|d| {
                d.kind == SymbolKind::Variable
                    && d.name == raw
                    && d.scope.contains(&byte)
                    && d.range.start < byte
            })
            .max_by_key(|d| d.range.start)
            .and_then(|d| d.type_name.clone());
    }
    Some(context.qualify(raw).to_lowercase())
}
fn push_reference(
    facts: &mut FileSymbols,
    names: Vec<String>,
    name: Node<'_>,
    source: &str,
    path: &Path,
) {
    if names.is_empty() {
        return;
    }
    let line_start = name
        .start_byte()
        .saturating_sub(name.start_position().column);
    facts.references.push((
        names,
        ReferenceLocation {
            path: path.into(),
            byte: name.start_byte(),
            length: name.byte_range().len(),
            line: u32::try_from(name.start_position().row).unwrap_or(u32::MAX),
            column: u32::try_from(name.start_position().column).unwrap_or(u32::MAX),
            preview: source
                .get(line_start..)
                .and_then(|s| s.lines().next())
                .unwrap_or("")
                .into(),
        },
    ));
}
fn type_references<'a>(
    node: Node<'a>,
    source: &str,
    context: &Context,
    facts: &FileSymbols,
) -> Vec<(Node<'a>, String)> {
    if node.kind() == "namespace_use_clause" {
        return import_target(node, source)
            .map(|(name, full, _)| vec![(name, full)])
            .unwrap_or_default();
    }
    let names = match node.kind() {
        "base_clause" | "class_interface_clause" | "type_list" => children(node),
        "attribute" | "class_constant_access_expression" => {
            node.named_child(0).into_iter().collect()
        }
        "scoped_call_expression" | "scoped_property_access_expression" => {
            node.child_by_field_name("scope").into_iter().collect()
        }
        "binary_expression"
            if node
                .child_by_field_name("operator")
                .is_some_and(|n| text(n, source).eq_ignore_ascii_case("instanceof")) =>
        {
            node.child_by_field_name("right").into_iter().collect()
        }
        _ => Vec::new(),
    };
    names
        .into_iter()
        .filter(|n| {
            matches!(
                n.kind(),
                "name" | "qualified_name" | "relative_name" | "relative_scope"
            )
        })
        .filter_map(|name| {
            receiver(text(name, source), name.start_byte(), context, facts)
                .map(|target| (name, target))
        })
        .collect()
}
fn references(
    node: Node<'_>,
    source: &str,
    path: &Path,
    context: &Context,
    facts: &mut FileSymbols,
) {
    let mut context = enter(node, source, context);
    let member = matches!(
        node.kind(),
        "member_call_expression"
            | "member_access_expression"
            | "nullsafe_member_call_expression"
            | "nullsafe_member_access_expression"
            | "scoped_call_expression"
            | "scoped_property_access_expression"
            | "class_constant_access_expression"
    );
    let target = if member {
        node.child_by_field_name("name")
            .or_else(|| node.named_child(1))
    } else if node.kind() == "function_call_expression" {
        node.child_by_field_name("function")
    } else if matches!(node.kind(), "object_creation_expression" | "named_type") {
        node.named_child(0)
    } else if node.kind() == "variable_name"
        && !facts
            .declarations
            .iter()
            .any(|d| d.range == node.byte_range())
        && !node
            .parent()
            .is_some_and(|p| p.child_by_field_name("name").is_some_and(|n| n == node))
    {
        Some(node)
    } else {
        None
    };
    if let Some(name) = target {
        let raw = text(name, source);
        let names = if member {
            let object = node
                .child_by_field_name("object")
                .or_else(|| node.child_by_field_name("scope"))
                .or_else(|| node.named_child(0));
            object
                .and_then(|n| receiver(text(n, source), node.start_byte(), &context, facts))
                .map(|owner| {
                    let name = if node.kind().contains("call") {
                        raw.to_lowercase()
                    } else {
                        raw.trim_start_matches('$').into()
                    };
                    vec![format!("{owner}::{name}")]
                })
                .unwrap_or_default()
        } else if raw.starts_with('$') {
            facts
                .declarations
                .iter()
                .filter(|d| {
                    d.kind == SymbolKind::Variable
                        && d.name == raw
                        && d.scope.contains(&node.start_byte())
                        && d.range.start < node.start_byte()
                })
                .max_by_key(|d| d.range.start)
                .map(|d| vec![d.fqn.clone()])
                .unwrap_or_default()
        } else {
            let full = context.qualify(raw).to_lowercase();
            if node.kind() == "function_call_expression" {
                vec![full, format!("\\{}", raw.to_lowercase())]
            } else {
                vec![full]
            }
        };
        push_reference(facts, names, name, source, path);
    }
    for (name, target) in type_references(node, source, &context, facts) {
        push_reference(facts, vec![target], name, source, path);
    }

    for child in children(node) {
        if child.kind() == "namespace_definition" && child.child_by_field_name("body").is_none() {
            context = enter(child, source, &context);
        }
        import(child, source, &mut context);
        references(child, source, path, &context, facts);
    }
}
/// Extracts declarations and symbolic reference candidates from exactly one source.
pub fn parse(path: &Path, source: &str) -> crate::Result<FileSymbols> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_php::LANGUAGE_PHP.into())
        .map_err(|e| crate::Error::Protocol(e.to_string()))?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| crate::Error::Protocol("PHP index parse cancelled".into()))?;
    let context = Context {
        scope: 0..source.len(),
        ..Context::default()
    };
    let mut facts = FileSymbols::default();
    collect(tree.root_node(), source, path, &context, &mut facts);
    references(tree.root_node(), source, path, &context, &mut facts);
    Ok(facts)
}
