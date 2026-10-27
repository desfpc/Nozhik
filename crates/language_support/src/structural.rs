//! AST error and missing delimiter diagnostics, excluding strings and comments.
use crate::lsp_types::{Diagnostic, DiagnosticSeverity, Range};
use tree_sitter::Node;
fn diagnostic(source: &str, start: usize, end: usize, message: String) -> Diagnostic {
    Diagnostic {
        range: Range::new(crate::position(source, start), crate::position(source, end)),
        severity: Some(DiagnosticSeverity::ERROR),
        source: Some("nozhik-syntax".into()),
        message,
        ..Diagnostic::default()
    }
}
fn errors(node: Node<'_>, source: &str, output: &mut Vec<Diagnostic>) {
    if node.is_missing() || node.is_error() {
        output.push(diagnostic(
            source,
            node.start_byte(),
            node.end_byte(),
            if node.is_missing() {
                format!("Missing '{}'", node.kind())
            } else {
                "Syntax error".into()
            },
        ));
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        errors(child, source, output);
    }
}
/// Finds parser errors and unmatched structural tokens in a PHP AST. Both the
/// opening delimiter and the expected closing position receive error markers.
#[must_use]
pub fn diagnostics(tree: &tree_sitter::Tree, source: &str) -> Vec<Diagnostic> {
    if !tree.root_node().has_error() {
        return Vec::new();
    }
    let mut output = Vec::new();
    errors(tree.root_node(), source, &mut output);
    let mut leaves = Vec::new();
    crate::formatting::tokens(tree.root_node(), &mut leaves);
    let mut stack = Vec::new();
    for node in leaves {
        match node.kind() {
            "{" => stack.push((node.start_byte(), '}')),
            "(" => stack.push((node.start_byte(), ')')),
            "[" => stack.push((node.start_byte(), ']')),
            "}" | ")" | "]" => {
                if let Some(index) = stack
                    .iter()
                    .rposition(|(_, close)| node.kind().starts_with(*close))
                {
                    stack.remove(index);
                }
            }
            _ => {}
        }
    }
    for (byte, close) in stack {
        let message = format!("Unclosed block / missing '{close}'");
        output.push(diagnostic(source, byte, byte + 1, message.clone()));
        let missing = format!("Missing '{close}'");
        let expected = output
            .iter()
            .filter(|d| d.message == missing)
            .filter_map(|d| crate::byte_offset(source, d.range.start))
            .find(|offset| *offset >= byte)
            .unwrap_or(source.len());
        output.push(diagnostic(source, expected, expected, message));
    }
    output
}
