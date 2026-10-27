//! Conservative PHP AST formatting: only whitespace between syntax tokens changes.
use nozhik_config::code_style::{BracePosition, CodeStyle};
use std::ops::Range;
use tree_sitter::{Node, Parser, Tree};

pub(crate) fn parse(source: &str) -> crate::Result<Tree> {
    let mut parser = Parser::new();
    parser
        .set_language(&if source.contains("<?") {
            tree_sitter_php::LANGUAGE_PHP.into()
        } else {
            tree_sitter_php::LANGUAGE_PHP_ONLY.into()
        })
        .map_err(|e| crate::Error::Protocol(e.to_string()))?;
    parser
        .parse(source, None)
        .ok_or_else(|| crate::Error::Protocol("parse cancelled".into()))
}
fn markup(node: Node<'_>) -> bool {
    matches!(
        node.kind(),
        "text" | "inline_text" | "text_interpolation" | "php_end_tag"
    )
}
fn protected_text(tree: &Tree, source: &str) -> Vec<String> {
    let mut leaves = Vec::new();
    tokens(tree.root_node(), &mut leaves);
    leaves
        .into_iter()
        .filter(|n| {
            markup(*n)
                || matches!(
                    n.kind(),
                    "string" | "encapsed_string" | "heredoc" | "nowdoc"
                )
        })
        .map(|n| source[n.byte_range()].to_owned())
        .collect()
}
fn opaque(node: Node<'_>) -> bool {
    matches!(
        node.kind(),
        "string"
            | "encapsed_string"
            | "heredoc"
            | "nowdoc"
            | "comment"
            | "variable_name"
            | "text"
            | "inline_text"
            | "text_interpolation"
    )
}
pub(crate) fn tokens<'a>(node: Node<'a>, output: &mut Vec<Node<'a>>) {
    if node.is_missing() {
        return;
    }
    if node.child_count() == 0 || opaque(node) {
        output.push(node);
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        tokens(child, output);
    }
}
fn block(node: Node<'_>) -> bool {
    node.parent().is_some_and(|p| {
        matches!(
            p.kind(),
            "compound_statement" | "declaration_list" | "switch_block"
        )
    })
}
fn brace_position<'a>(node: Node<'_>, style: &'a CodeStyle) -> &'a BracePosition {
    let owner = node.parent().and_then(|p| p.parent());
    match owner.map(|p| p.kind()) {
        Some(
            "class_declaration"
            | "interface_declaration"
            | "trait_declaration"
            | "enum_declaration",
        ) => &style.braces.classes,
        Some("function_definition" | "method_declaration") => &style.braces.functions,
        _ => &style.braces.control_structures,
    }
}
fn binary(node: Node<'_>) -> bool {
    node.parent().is_some_and(|p| {
        matches!(
            p.kind(),
            "binary_expression"
                | "assignment_expression"
                | "augmented_assignment_expression"
                | "arrow_function"
                | "pair"
        )
    }) && matches!(
        node.kind(),
        "=" | "+"
            | "-"
            | "*"
            | "/"
            | "."
            | "%"
            | "=="
            | "==="
            | "!="
            | "!=="
            | "<"
            | ">"
            | "<="
            | ">="
            | "&&"
            | "||"
            | "??"
            | "=>"
            | "+="
            | "-="
            | ".="
            | "&"
            | "|"
            | "^"
            | "**"
            | "<<"
            | ">>"
    )
}
fn ancestor(node: Node<'_>, kind: &str) -> bool {
    let mut parent = node.parent();
    while let Some(p) = parent {
        if p.kind() == kind {
            return true;
        }
        parent = p.parent();
    }
    false
}
fn selected_edit(
    source: &str,
    range: Range<usize>,
    text: String,
    selection: Option<&Range<usize>>,
) -> Option<(Range<usize>, String)> {
    let (range, text) = if let Some(selection) = selection {
        let selected = range.start.max(selection.start)..range.end.min(selection.end);
        if selected.start > selected.end {
            return None;
        }
        let before = source.get(range.start..selected.start)?;
        let after = source.get(selected.end..range.end)?;
        let text = text.strip_prefix(before)?.strip_suffix(after)?.to_owned();
        (selected, text)
    } else {
        (range, text)
    };
    (source.get(range.clone())? != text).then_some((range, text))
}

/// Computes whitespace replacements in original UTF-8 coordinates.
///
/// A range limits
/// edits strictly to the selection, while indentation uses the complete AST.
/// Literal payloads and non-PHP markup remain byte-for-byte unchanged.
/// # Errors
/// Returns parser or unsupported/malformed source errors without changing text.
#[allow(
    clippy::too_many_lines,
    reason = "Ordered AST token spacing rules share scope depth and semantic validation"
)]
pub fn php_edits(
    source: &str,
    style: &CodeStyle,
    selection: Option<&Range<usize>>,
) -> crate::Result<Vec<(Range<usize>, String)>> {
    if style.language != "php" {
        return Err(crate::Error::Protocol(
            "No local formatter for this language".into(),
        ));
    }
    let tree = parse(source)?;
    if tree.root_node().has_error() {
        return Err(crate::Error::Protocol(
            "Cannot format PHP with syntax errors".into(),
        ));
    }
    let mut leaves = Vec::new();
    tokens(tree.root_node(), &mut leaves);
    let mixed = leaves.iter().any(|n| markup(*n));
    let mut edits = Vec::new();
    if let Some(first) = leaves.first() {
        let prefix = 0..first.start_byte();
        if !mixed
            && prefix.end > 0
            && source[prefix.clone()].trim().is_empty()
            && selection.is_none_or(|s| prefix.start >= s.start && prefix.end <= s.end)
        {
            edits.push((prefix, String::new()));
        }
    }
    let mut depth = 0usize;
    let mut parentheses = 0usize;
    for pair in leaves.windows(2) {
        let a = pair[0];
        let b = pair[1];
        let left = &source[a.byte_range()];
        let right = &source[b.byte_range()];
        if left == "{" && block(a) {
            depth += 1;
        }
        if left == "}" && block(a) {
            depth = depth.saturating_sub(1);
        }
        if left == "(" {
            parentheses += 1;
        }
        if left == ")" {
            parentheses = parentheses.saturating_sub(1);
        }
        if markup(a) || markup(b) || mixed && (a.kind() == "php_tag" || b.kind() == "php_tag") {
            continue;
        }
        let indent = depth.saturating_sub(usize::from(right == "}" && block(b)));
        let newline = |blank: u8| {
            format!(
                "{}{}",
                "\n".repeat(usize::from(blank) + 1),
                style.unit().repeat(indent)
            )
        };
        let gap = if left == "<?php" {
            newline(1)
        } else if right == "{" && block(b) {
            if *brace_position(b, style) == BracePosition::NextLine {
                newline(0)
            } else {
                " ".into()
            }
        } else if left == "{" && block(a) || right == "}" && block(b) {
            newline(0)
        } else if left == ";" && a.parent().is_none_or(|p| p.kind() != "for_statement") {
            newline(if ancestor(a, "namespace_definition") {
                style.empty_lines.after_namespace
            } else {
                0
            })
        } else if left == "}" && block(a) {
            let indent = depth;
            if matches!(right, ";" | "," | ")" | "]") {
                String::new()
            } else if matches!(right, "else" | "elseif" | "catch" | "finally") {
                " ".into()
            } else {
                let blank = if ancestor(a, "method_declaration") && right != "}" {
                    style.empty_lines.between_methods
                } else {
                    0
                };
                format!(
                    "{}{}",
                    "\n".repeat(usize::from(blank) + 1),
                    style.unit().repeat(indent)
                )
            }
        } else if a.kind() == "comment" && (left.starts_with("//") || left.starts_with('#')) {
            newline(0)
        } else if right == ","
            || right == ";"
            || matches!(right, ")" | "]" | "->" | "?->" | "::")
            || matches!(left, "[" | "->" | "?->" | "::" | "!" | "~")
        {
            if right == ")" && left != "(" && style.spacing.inside_parentheses {
                " ".into()
            } else {
                String::new()
            }
        } else if left == "(" {
            if right != ")" && style.spacing.inside_parentheses {
                " ".into()
            } else {
                String::new()
            }
        } else if right == "(" {
            if matches!(
                left,
                "if" | "elseif" | "for" | "foreach" | "while" | "switch" | "catch" | "match"
            ) {
                " ".into()
            } else {
                String::new()
            }
        } else if left == "," {
            if style.spacing.after_comma {
                " ".into()
            } else {
                String::new()
            }
        } else if binary(a) || binary(b) {
            if style.spacing.around_operators {
                " ".into()
            } else {
                String::new()
            }
        } else if matches!(right, ":" | "++" | "--")
            || matches!(left, "++" | "--" | "\\")
            || right == "\\"
        {
            String::new()
        } else {
            " ".into()
        };
        let range = a.end_byte()..b.start_byte();
        if source
            .get(range.clone())
            .is_some_and(|s| s.chars().all(char::is_whitespace))
            && let Some(edit) = selected_edit(source, range, gap, selection)
        {
            edits.push(edit);
        }
    }
    if selection.is_none() && !mixed {
        let end = leaves.last().map_or(0, Node::end_byte);
        if source
            .get(end..)
            .is_some_and(|s| s.trim().is_empty() && s != "\n")
        {
            edits.push((end..source.len(), "\n".into()));
        }
    }
    // Whitespace can affect PHP comments and tokenization. Reject any semantic change.
    let mut formatted = source.to_owned();
    for (range, text) in edits.iter().rev() {
        formatted.replace_range(range.clone(), text);
    }
    let checked = parse(&formatted)?;
    if checked.root_node().has_error()
        || checked.root_node().to_sexp() != tree.root_node().to_sexp()
        || protected_text(&tree, source) != protected_text(&checked, &formatted)
    {
        return Err(crate::Error::Protocol(
            "Local formatting would change PHP syntax".into(),
        ));
    }
    Ok(edits)
}

// Apply server edits first, then repair only existing line-leading indentation.
// LSP token/spacing changes retain priority. Return one original-coordinate edit.
pub(crate) fn normalize_lsp_indentation(
    source: &str,
    style: &CodeStyle,
    selection: Option<&Range<usize>>,
    mut edits: Vec<(Range<usize>, String)>,
) -> crate::Result<Vec<(Range<usize>, String)>> {
    edits.sort_by_key(|(range, _)| range.start);
    if edits
        .windows(2)
        .any(|pair| pair[0].0.end > pair[1].0.start || pair[0].0.start == pair[1].0.start)
    {
        return Err(crate::Error::Protocol("Overlapping formatter edits".into()));
    }
    let mut formatted = source.to_owned();
    for (range, text) in edits.into_iter().rev() {
        formatted.replace_range(range, &text);
    }
    if protected_text(&parse(source)?, source) != protected_text(&parse(&formatted)?, &formatted) {
        return php_edits(source, style, selection);
    }
    let selected = selection.map(|r| r.start..formatted.len() - (source.len() - r.end));
    if let Ok(local) = php_edits(&formatted, style, None) {
        let mut indentation_edits = Vec::new();
        for (range, text) in local {
            if let Some(last_newline) = formatted[..range.end].rfind('\n') {
                let start = last_newline + 1;
                if start >= range.start
                    && text.contains('\n')
                    && let Some(indent) = text.rsplit('\n').next()
                    && let Some(edit) = selected_edit(
                        &formatted,
                        start..range.end,
                        indent.into(),
                        selected.as_ref(),
                    )
                {
                    indentation_edits.push(edit);
                }
            }
        }
        for (range, text) in indentation_edits.into_iter().rev() {
            formatted.replace_range(range, &text);
        }
    }
    let prefix: usize = source
        .chars()
        .zip(formatted.chars())
        .take_while(|(a, b)| a == b)
        .map(|(c, _)| c.len_utf8())
        .sum();
    let suffix: usize = source[prefix..]
        .chars()
        .rev()
        .zip(formatted[prefix..].chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(c, _)| c.len_utf8())
        .sum();
    if prefix == source.len() && prefix == formatted.len() {
        return Ok(Vec::new());
    }
    Ok(vec![(
        prefix..source.len() - suffix,
        formatted[prefix..formatted.len() - suffix].into(),
    )])
}

/// Leading indentation in logical columns, using configured tab stops.
fn indent_width(text: &str, size: u8) -> usize {
    text.chars()
        .take_while(|c| matches!(c, ' ' | '\t'))
        .fold(0, |column, c| {
            if c == '\t' {
                column + usize::from(size) - column % usize::from(size)
            } else {
                column + 1
            }
        })
}
fn indentation(columns: usize, style: &CodeStyle) -> String {
    match style.indent.style {
        nozhik_config::code_style::IndentStyle::Spaces => " ".repeat(columns),
        nozhik_config::code_style::IndentStyle::Tabs => format!(
            "{}{}",
            "\t".repeat(columns / usize::from(style.indent.size)),
            " ".repeat(columns % usize::from(style.indent.size))
        ),
    }
}

// A bounded lexical indentation pass for clipboard fragments. It does not parse
// documents on the UI thread. Quoted delimiters/comments never create nesting;
// multiline literals and heredocs retain their original relative whitespace.
enum FragmentIndent {
    Relative,
    Structural { levels: Vec<usize>, depth: usize },
    Verbatim,
}
fn fragment_indents(text: &str) -> FragmentIndent {
    let mut depth = 0usize;
    let mut quote = None;
    let mut comment = false;
    let mut structural = false;
    let mut levels = Vec::new();
    if text.contains("<<<") {
        return FragmentIndent::Verbatim;
    }
    for line in text.split_inclusive('\n') {
        if quote.is_some() || comment {
            return FragmentIndent::Verbatim;
        }
        let closing = line
            .trim_start()
            .chars()
            .take_while(|c| matches!(c, '}' | ']' | ')'))
            .count();
        levels.push(depth.saturating_sub(closing));
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            if comment {
                if c == '*' && chars.peek() == Some(&'/') {
                    chars.next();
                    comment = false;
                }
            } else if let Some(delimiter) = quote {
                if c == '\\' {
                    chars.next();
                } else if c == delimiter {
                    quote = None;
                }
            } else if c == '/' && chars.peek() == Some(&'*') {
                chars.next();
                comment = true;
            } else if c == '#' || c == '/' && chars.peek() == Some(&'/') {
                break;
            } else if matches!(c, '\'' | '"' | '`') {
                quote = Some(c);
            } else if matches!(c, '{' | '[' | '(') {
                depth += 1;
                structural = true;
            } else if matches!(c, '}' | ']' | ')') {
                depth = depth.saturating_sub(1);
            }
        }
    }
    if quote.is_some() || comment {
        FragmentIndent::Verbatim
    } else if structural {
        FragmentIndent::Structural { levels, depth }
    } else {
        FragmentIndent::Relative
    }
}

/// Computes the text inserted by Enter from the current line prefix only.
///
/// Open brackets add indentation; quoted/commented brackets are ignored. Without
/// a profile, Enter remains a literal newline. `newline` is LF or CRLF.
#[must_use]
pub fn newline_text(before: &str, newline: &str, style: Option<&CodeStyle>) -> String {
    let Some(style) = style else {
        return newline.into();
    };
    let depth = match fragment_indents(before) {
        FragmentIndent::Structural { depth, .. } => depth,
        FragmentIndent::Relative => 0,
        FragmentIndent::Verbatim => return newline.into(),
    };
    let columns = indent_width(before, style.indent.size) + depth * usize::from(style.indent.size);
    format!("{newline}{}", indentation(columns, style))
}

/// Rebase clipboard indentation to the target line, preserving existing prefix
/// whitespace. PHP block nesting also repairs inconsistent relative indentation.
#[must_use]
pub fn smart_paste(text: &str, source: &str, byte: usize, style: Option<&CodeStyle>) -> String {
    let Some(style) = style else {
        return text.into();
    };
    let Some(prefix) = source.get(..byte) else {
        return text.into();
    };
    let line_start = prefix.rfind('\n').map_or(0, |i| i + 1);
    let line = source[line_start..].split('\n').next().unwrap_or("");
    let before = &source[line_start..byte];
    if !before.trim().is_empty() && !text.contains('\n') {
        return text.into();
    }
    let mut base = indent_width(line, style.indent.size);
    if base == 0
        && line.trim().is_empty()
        && let Some(previous) = source[..line_start]
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
    {
        base = indent_width(previous, style.indent.size)
            + if previous.trim_end().ends_with('{') {
                usize::from(style.indent.size)
            } else {
                0
            };
    }
    let consumed = if before.trim().is_empty() {
        indent_width(before, style.indent.size)
    } else {
        base
    };
    let outer = text
        .lines()
        .filter(|s| !s.trim().is_empty())
        .map(|line| indent_width(line, style.indent.size))
        .min()
        .unwrap_or(0);
    let levels = if style.language == "php" {
        match fragment_indents(text) {
            FragmentIndent::Structural { levels, .. } => Some(levels),
            FragmentIndent::Relative => None,
            FragmentIndent::Verbatim => return text.into(),
        }
    } else {
        None
    };
    let mut output = String::new();
    for (i, line) in text.split_inclusive('\n').enumerate() {
        let relative = levels
            .as_ref()
            .and_then(|levels| levels.get(i))
            .map_or_else(
                || indent_width(line, style.indent.size).saturating_sub(outer),
                |level| level * usize::from(style.indent.size),
            );
        let indent = (base + relative).saturating_sub(if i == 0 { consumed } else { 0 });
        output.push_str(&indentation(indent, style));
        output.push_str(line.trim_start_matches([' ', '\t']));
    }
    output
}

/// Returns a paste replacement in original UTF-8 bytes. A caret in leading
/// whitespace replaces that prefix so a single line is indented exactly once.
#[must_use]
pub fn paste_edit(
    source: &str,
    mut range: Range<usize>,
    text: &str,
    style: Option<&CodeStyle>,
) -> (Range<usize>, String) {
    if text.is_empty()
        || style.is_some_and(|s| s.language == "php")
            && matches!(fragment_indents(text), FragmentIndent::Verbatim)
    {
        return (range, text.into());
    }
    if style.is_some()
        && let Some(prefix) = source.get(..range.start)
    {
        let start = prefix.rfind('\n').map_or(0, |i| i + 1);
        let end = start
            + source[start..]
                .chars()
                .take_while(|c| matches!(c, ' ' | '\t'))
                .map(char::len_utf8)
                .sum::<usize>();
        if range.start <= end {
            range.start = start;
            range.end = range.end.max(end);
        }
    }
    let text = smart_paste(text, source, range.start, style);
    (range, text)
}

#[cfg(test)]
mod tests {
    #[test]
    fn partial_server_formatting_still_indents_nested_print()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = "function check($condition, $message) {\n    if (!$condition) {\nprint('test');\n        throw new RuntimeException($message);\n    }\n}";
        let styles = nozhik_config::code_style::CodeStyles::builtin()?;
        let style = styles
            .selected("php", &std::collections::BTreeMap::new())
            .ok_or("style")?;
        let edits = super::normalize_lsp_indentation(
            source,
            style,
            None,
            vec![(0..0, "/* LSP */ ".into())],
        )?;
        let mut output = source.to_owned();
        for (range, text) in edits.into_iter().rev() {
            output.replace_range(range, &text);
        }
        assert!(output.starts_with("/* LSP */ function check"));
        assert!(output.contains(
            "\n        print('test');\n        throw new RuntimeException($message);\n    }\n}"
        ));
        Ok(())
    }
    #[test]
    fn server_rewriting_heredoc_falls_back_to_literal_preserving_formatting()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = "<?php function f(){\nfile_put_contents('x', <<<INI\n[app]\n  key = value\nINI);\nprint('test');\n}";
        let styles = nozhik_config::code_style::CodeStyles::builtin()?;
        let style = styles
            .selected("php", &std::collections::BTreeMap::new())
            .ok_or("style")?;
        let server = source.replace("  key = value", " key = value");
        let edits =
            super::normalize_lsp_indentation(source, style, None, vec![(0..source.len(), server)])?;
        let mut output = source.to_owned();
        for (range, text) in edits.into_iter().rev() {
            output.replace_range(range, &text);
        }
        assert!(output.contains("<<<INI\n[app]\n  key = value\nINI"));
        assert!(output.contains("\n    print('test');\n"));
        assert!(output.contains("function f()\n{"));
        Ok(())
    }
}
