//! CSS parsing, class declarations, color references and conservative formatting.
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    path::Path,
};
use tree_sitter::{Node, Parser, Tree};

/// One class selector, preserving case and exact source coordinates.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClassDeclaration {
    /// CSS identifier decoded from escapes, without the leading dot.
    pub name: String,
    /// Original UTF-8 name bytes.
    pub range: Range<usize>,
}
/// A color painted beside a declaration. References edit their resolved definition.
#[derive(Clone, Debug)]
pub struct ColorOccurrence {
    /// Location of the visible value (literal or var call).
    pub range: Range<usize>,
    /// Literal bytes to replace in this document.
    pub edit_range: Range<usize>,
    /// Unmultiplied sRGBA components.
    pub rgba: [u8; 4],
}
/// Recognizes CSS through the shared language registry.
#[must_use]
pub fn is_css(uri: &crate::lsp_types::Url) -> bool {
    nozhik_config::languages::language_for_path(Path::new(uri.path()))
        == Some(nozhik_config::languages::LanguageId::Css)
}
fn parse(source: &str) -> crate::Result<Tree> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_css::LANGUAGE.into())
        .map_err(|e| crate::Error::Protocol(e.to_string()))?;
    parser
        .parse(source, None)
        .ok_or_else(|| crate::Error::Protocol("CSS parse cancelled".into()))
}
fn walk<'a>(node: Node<'a>, visit: &mut impl FnMut(Node<'a>)) {
    visit(node);
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        walk(child, visit);
    }
}
/// Extracts only actual class selectors, excluding strings, comments and properties.
/// # Errors
/// Returns grammar/parser errors.
pub fn classes(source: &str) -> crate::Result<Vec<ClassDeclaration>> {
    let tree = parse(source)?;
    let mut result = Vec::new();
    walk(tree.root_node(), &mut |node| {
        if node.kind() == "class_name"
            && node.parent().is_some_and(|p| p.kind() == "class_selector")
        {
            result.push(ClassDeclaration {
                name: decode_identifier(&source[node.byte_range()]),
                range: node.byte_range(),
            });
        }
    });
    Ok(result)
}
fn decode_identifier(raw: &str) -> String {
    let mut chars = raw.chars().peekable();
    let mut result = String::new();
    while let Some(c) = chars.next() {
        if c != '\\' {
            result.push(c);
            continue;
        }
        let mut hex = String::new();
        while hex.len() < 6 && chars.peek().is_some_and(char::is_ascii_hexdigit) {
            if let Some(c) = chars.next() {
                hex.push(c);
            }
        }
        if hex.is_empty() {
            if let Some(c) = chars.next() {
                result.push(c);
            }
        } else {
            result.push(
                u32::from_str_radix(&hex, 16)
                    .ok()
                    .and_then(char::from_u32)
                    .filter(|c| *c != '\0')
                    .unwrap_or('\u{fffd}'),
            );
            if chars.peek().is_some_and(char::is_ascii_whitespace) {
                chars.next();
            }
        }
    }
    result
}
fn resolve_variable(
    value: &str,
    source: &str,
    definitions: &HashMap<String, Range<usize>>,
    seen: &mut HashSet<String>,
) -> Option<(Range<usize>, [u8; 4])> {
    let name = value.strip_prefix("var(")?.strip_suffix(')')?.trim();
    if seen.len() >= 64 || !seen.insert(name.into()) {
        return None;
    }
    let resolved = (|| {
        let range = definitions.get(name)?;
        let text = source[range.clone()].trim().trim_end_matches(';').trim();
        if let Ok(color) = text.parse::<csscolorparser::Color>() {
            let offset = source[range.clone()].find(text)?;
            Some((
                range.start + offset..range.start + offset + text.len(),
                color.to_rgba8(),
            ))
        } else {
            resolve_variable(text, source, definitions, seen)
        }
    })();
    seen.remove(name);
    resolved
}
fn value_colors(
    node: Node<'_>,
    source: &str,
    definitions: &HashMap<String, Range<usize>>,
    seen: &mut HashSet<String>,
    output: &mut Vec<ColorOccurrence>,
) {
    let raw = &source[node.byte_range()];
    if matches!(
        node.kind(),
        "comment" | "string_value" | "property_name" | "important"
    ) {
        return;
    }
    if matches!(
        node.kind(),
        "color_value" | "plain_value" | "call_expression"
    ) {
        if let Ok(color) = raw.parse::<csscolorparser::Color>() {
            output.push(ColorOccurrence {
                range: node.byte_range(),
                edit_range: node.byte_range(),
                rgba: color.to_rgba8(),
            });
            return;
        }
        if node.kind() == "call_expression" && raw.starts_with("var(") {
            let name = raw[4..].split([',', ')']).next().unwrap_or("").trim();
            if seen.insert(name.into()) {
                if let Some(range) = definitions.get(name) {
                    let value = source[range.clone()].trim().trim_end_matches(';').trim();
                    if let Ok(color) = value.parse::<csscolorparser::Color>() {
                        let offset = source[range.clone()].find(value).unwrap_or(0);
                        output.push(ColorOccurrence {
                            range: node.byte_range(),
                            edit_range: range.start + offset..range.start + offset + value.len(),
                            rgba: color.to_rgba8(),
                        });
                    } else if let Some((range, rgba)) =
                        resolve_variable(value, source, definitions, seen)
                    {
                        output.push(ColorOccurrence {
                            range: node.byte_range(),
                            edit_range: range,
                            rgba,
                        });
                    }
                }
                seen.remove(name);
            }
            return;
        }
        // url() contents are not color values.
        if raw.starts_with("url(") {
            return;
        }
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        value_colors(child, source, definitions, seen, output);
    }
}
/// Finds literal colors and document-local custom property references in declarations.
/// # Errors
/// Returns grammar/parser errors.
pub fn colors(source: &str) -> crate::Result<Vec<ColorOccurrence>> {
    let tree = parse(source)?;
    let mut definitions = HashMap::new();
    let mut ambiguous = HashSet::new();
    walk(tree.root_node(), &mut |node| {
        if node.kind() == "declaration"
            && let Some(property) = node.named_child(0)
        {
            let name = &source[property.byte_range()];
            if name.starts_with("--")
                && let Some(colon) = source[property.end_byte()..node.end_byte()].find(':')
                && definitions
                    .insert(
                        name.into(),
                        property.end_byte() + colon + 1..node.end_byte(),
                    )
                    .is_some()
            {
                ambiguous.insert(name.to_owned());
            }
        }
    });
    definitions.retain(|name, _| !ambiguous.contains(name));
    let mut result = Vec::new();
    walk(tree.root_node(), &mut |node| {
        if node.kind() == "declaration" {
            value_colors(node, source, &definitions, &mut HashSet::new(), &mut result);
        }
    });
    result.sort_by_key(|c| c.range.start);
    Ok(result)
}
fn leaves<'a>(node: Node<'a>, result: &mut Vec<Node<'a>>) {
    if node.child_count() == 0
        || matches!(
            node.kind(),
            "comment" | "string_value" | "plain_value" | "color_value"
        )
    {
        result.push(node);
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        leaves(child, result);
    }
}
/// Formats whitespace between CSS tokens, verifying the syntax tree and token text.
/// Selection edits are restricted to the original UTF-8 range.
/// # Errors
/// Rejects malformed CSS, invalid ranges and any change to syntax or tokens.
pub fn format(
    source: &str,
    style: &nozhik_config::code_style::CodeStyle,
    selection: Option<&Range<usize>>,
) -> crate::Result<Vec<(Range<usize>, String)>> {
    if style.language != "css" {
        return Err(crate::Error::Protocol(
            "Select a CSS formatting profile".into(),
        ));
    }
    let tree = parse(source)?;
    if tree.root_node().has_error() {
        return Err(crate::Error::Protocol(
            "Fix CSS syntax errors before formatting".into(),
        ));
    }
    if selection.is_some_and(|r| {
        r.start > r.end || !source.is_char_boundary(r.start) || !source.is_char_boundary(r.end)
    }) {
        return Err(crate::Error::Protocol("Invalid CSS selection".into()));
    }
    let mut tokens = Vec::new();
    leaves(tree.root_node(), &mut tokens);
    let ending = if source.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut depth = 0usize;
    let mut edits = Vec::new();
    for pair in tokens.windows(2) {
        let left = pair[0];
        let right = pair[1];
        if left.kind() == "{" {
            depth += 1;
        }
        if right.kind() == "}" {
            depth = depth.saturating_sub(1);
        }
        let range = left.end_byte()..right.start_byte();
        if selection.is_some_and(|r| range.start < r.start || range.end > r.end) {
            continue;
        }
        let original = &source[range.clone()];
        if !original.chars().all(char::is_whitespace) {
            continue;
        }
        let structural = |n: Node<'_>| n.parent().is_some_and(|p| p.kind() == "block");
        let newline = (left.kind() == "{" && structural(left))
            || (left.kind() == ";" && left.parent().is_some_and(|p| p.kind() == "declaration"))
            || (left.kind() == "}" && right.kind() != ";")
            || (right.kind() == "}" && structural(right));
        let replacement = if newline {
            format!("{ending}{}", style.unit().repeat(depth))
        } else if right.kind() == "{"
            || (left.kind() == ":" && left.parent().is_some_and(|p| p.kind() == "declaration"))
            || (left.kind() == "," && style.spacing.after_comma)
        {
            " ".into()
        } else if matches!(right.kind(), ";" | ",") {
            String::new()
        } else {
            continue;
        };
        if replacement != original {
            edits.push((range, replacement));
        }
    }
    let mut formatted = source.to_owned();
    for (range, text) in edits.iter().rev() {
        formatted.replace_range(range.clone(), text);
    }
    let after = parse(&formatted)?;
    let mut after_tokens = Vec::new();
    leaves(after.root_node(), &mut after_tokens);
    if after.root_node().has_error()
        || tree.root_node().to_sexp() != after.root_node().to_sexp()
        || tokens
            .iter()
            .map(|n| &source[n.byte_range()])
            .ne(after_tokens.iter().map(|n| &formatted[n.byte_range()]))
    {
        return Err(crate::Error::Protocol(
            "CSS formatting would change syntax".into(),
        ));
    }
    Ok(edits)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn css_selectors_colors_and_formatting() -> Result<(), Box<dyn std::error::Error>> {
        let source = r#"/* .fake #bad */ :root{--white:#fff;--alias:var(--white)} .Card:hover,.card,.sm\:wide{color:var(--alias);background:linear-gradient(red,rgba(0, 0, 255, .5));content:".fake #abc";width:calc(100% - 2px);background-image:url(red)}"#;
        let names: Vec<_> = classes(source)?.into_iter().map(|c| c.name).collect();
        assert_eq!(names, ["Card", "card", "sm:wide"]);
        let found = colors(source)?;
        assert_eq!(
            found.len(),
            5,
            "literal, two resolved aliases and gradient stops: {found:?}"
        );
        assert_eq!(found[0].rgba, [255, 255, 255, 255]);
        assert_eq!(found[2].edit_range, found[0].edit_range);
        assert_eq!(&source[found[3].range.clone()], "red");
        assert_eq!(found[4].rgba, [0, 0, 255, 128]);
        let styles = nozhik_config::code_style::CodeStyles::builtin()?;
        let style = styles
            .selected("css", &std::collections::BTreeMap::default())
            .ok_or("missing CSS style")?;
        let edits = format(source, style, None)?;
        assert_ne!(edits.len(), 0);
        let mut formatted = source.to_string();
        for (range, text) in edits.into_iter().rev() {
            formatted.replace_range(range, &text);
        }
        assert!(
            formatted.contains("    color: var(--alias);\n"),
            "{formatted}"
        );
        assert!(formatted.contains("calc(100% - 2px)"));
        assert!(format(&formatted, style, None)?.is_empty(), "idempotent");
        let range = source.find("color:").ok_or("missing declaration")?
            ..source
                .find(";background:")
                .ok_or("missing declaration end")?;
        assert!(
            format(source, style, Some(&range))?
                .iter()
                .all(|(r, _)| r.start >= range.start && r.end <= range.end)
        );
        assert!(format("a { color:", style, None).is_err());
        assert!(colors(":root {--a:var(--b);--b:var(--a)} a{color:var(--a)}")?.is_empty());
        let chained = ":root{--base:#fff;--a:var(--base);--b:var(--base)} a{background:linear-gradient(var(--a),var(--b))}";
        assert_eq!(
            colors(chained)?.len(),
            5,
            "shared variable dependencies resolve independently"
        );
        assert_eq!(
            colors(":root{--a:red} .dark{--a:blue} a{color:var(--a)}")?.len(),
            2,
            "ambiguous variables are not guessed"
        );
        let mut highlighter = crate::highlighter::Highlighter::css()?;
        let spans = highlighter.highlight(source)?;
        assert!(spans.iter().any(|s| s.token == "css.color"));
        assert_eq!(highlighter.diagnostics().len(), 0);
        highlighter.highlight("a { color: #123456; }")?;
        assert_eq!(highlighter.diagnostics().len(), 0);
        Ok(())
    }
}
