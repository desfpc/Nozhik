//! Whitespace-only Go formatting. Tokens, comments and literals stay byte-for-byte.
//! A partial selection is refused: gofmt rewrites the whole file, and a range edit
//! must not change text outside the selection.

use nozhik_config::code_style::CodeStyle;
use std::ops::Range;
use tree_sitter::{Node, Parser, Tree};

/// Formats one Go snapshot.
///
/// # Errors
/// Returns when the profile is not Go, the source does not parse, a selection is
/// partial, or the rewrite would change a token.
pub fn format(
    source: &str,
    style: &CodeStyle,
    range: Option<&Range<usize>>,
) -> crate::Result<Vec<(Range<usize>, String)>> {
    if style.language != "go" {
        return Err(crate::Error::Protocol("Go profile required".into()));
    }
    if let Some(range) = range
        && (range.start != 0 || range.end != source.len())
    {
        return Err(crate::Error::Protocol(
            "Go formatting changes the whole file. Clear the selection and try again.".into(),
        ));
    }
    let tree = parse(source)?;
    if tree.root_node().has_error() {
        return Err(crate::Error::Protocol(
            "Cannot format Go with syntax errors".into(),
        ));
    }
    let original = leaves(&tree, source);
    let mut formatted = render(&original, source, style);
    if source.contains("\r\n") {
        formatted = formatted.replace('\n', "\r\n");
    }
    let check = parse(&formatted)?;
    if check.root_node().has_error() {
        return Err(crate::Error::Protocol("Formatted Go did not parse".into()));
    }
    let rewritten = leaves(&check, &formatted);
    if token_text(&original, source) != token_text(&rewritten, &formatted) {
        return Err(crate::Error::Protocol(
            "Go formatter refused to change tokens".into(),
        ));
    }
    if formatted == source {
        return Ok(Vec::new());
    }
    Ok(vec![(0..source.len(), formatted)])
}

fn parse(source: &str) -> crate::Result<Tree> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_go::LANGUAGE.into())
        .map_err(|error| crate::Error::Protocol(error.to_string()))?;
    parser
        .parse(source, None)
        .ok_or_else(|| crate::Error::Protocol("Go parse cancelled".into()))
}

fn leaves<'a>(tree: &'a Tree, source: &str) -> Vec<Node<'a>> {
    let mut output = Vec::new();
    collect(tree.root_node(), source, &mut output);
    output
}

fn collect<'a>(node: Node<'a>, source: &str, output: &mut Vec<Node<'a>>) {
    if node.child_count() == 0 {
        let text = source.get(node.byte_range()).unwrap_or("");
        if !text.is_empty() && !text.chars().all(char::is_whitespace) {
            output.push(node);
        }
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect(child, source, output);
    }
}

fn token_text(nodes: &[Node<'_>], source: &str) -> Vec<String> {
    nodes
        .iter()
        .map(|node| source.get(node.byte_range()).unwrap_or("").to_string())
        .collect()
}

fn render(tokens: &[Node<'_>], source: &str, style: &CodeStyle) -> String {
    let mut out = String::new();
    let mut braces = 0_i32;
    let mut parens = 0_i32;
    let mut brackets = 0_i32;
    let mut after_colon = false;
    for (index, token) in tokens.iter().enumerate() {
        let text = source.get(token.byte_range()).unwrap_or("");
        if index > 0 {
            let previous = tokens[index - 1];
            let left = source.get(previous.byte_range()).unwrap_or("");
            let gap = source
                .get(previous.end_byte()..token.start_byte())
                .unwrap_or("");
            let newline = gap.contains('\n');
            let blank = gap.bytes().filter(|byte| *byte == b'\n').count() >= 2;
            if newline {
                if left == ":" {
                    after_colon = true;
                }
                if matches!(text, "case" | "default" | "}") {
                    after_colon = false;
                }
            }
            let before = if index >= 2 {
                source.get(tokens[index - 2].byte_range()).unwrap_or("")
            } else {
                ""
            };
            let separator = if newline {
                let indent = indent_for(text, braces, parens, brackets, after_colon);
                format!(
                    "{}{}",
                    if blank { "\n\n" } else { "\n" },
                    style.unit().repeat(indent)
                )
            } else {
                inline_sep(before, left, text, style).to_string()
            };
            out.push_str(&separator);
        }
        out.push_str(text);
        match text {
            "{" => braces += 1,
            "}" => braces -= 1,
            "(" => parens += 1,
            ")" => parens -= 1,
            "[" => brackets += 1,
            "]" => brackets -= 1,
            _ => {}
        }
    }
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn indent_for(
    token: &str,
    mut braces: i32,
    mut parens: i32,
    mut brackets: i32,
    after_colon: bool,
) -> usize {
    if token == "}" {
        braces -= 1;
    }
    if token == ")" {
        parens -= 1;
    }
    if token == "]" {
        brackets -= 1;
    }
    let mut level = usize::try_from(braces.max(0) + parens.max(0) + brackets.max(0)).unwrap_or(0);
    if after_colon && !matches!(token, "case" | "default" | "}") {
        level += 1;
    }
    level
}

fn inline_sep(before: &str, left: &str, right: &str, style: &CodeStyle) -> &'static str {
    if right.starts_with("//") || right.starts_with("/*") {
        return " ";
    }
    if matches!(right, "," | ";" | ")" | "]" | "}")
        || matches!(left, "." | "(" | "[")
        || right == "."
    {
        return "";
    }
    if right == "(" && left != ")" && !keyword_space_before_paren(left) {
        return "";
    }
    if right == "{" || left == ":" {
        return " ";
    }
    if left == "," {
        return if style.spacing.after_comma { " " } else { "" };
    }
    if is_operator(left) {
        if matches!(left, "++" | "--") {
            return "";
        }
        if is_operand_end(before) {
            return if style.spacing.around_operators {
                " "
            } else {
                ""
            };
        }
        return "";
    }
    if is_operator(right) {
        if matches!(right, "++" | "--") {
            return "";
        }
        if is_operand_end(left) || is_keyword(left) {
            return if style.spacing.around_operators {
                " "
            } else {
                ""
            };
        }
        return "";
    }
    if is_keyword(left) && !matches!(right, ")" | "]" | "}" | "[" | "(") {
        return " ";
    }
    if is_word(left)
        && (is_word(right)
            || right.starts_with('"')
            || right.starts_with('`')
            || right.starts_with('\''))
    {
        return " ";
    }
    if (left == "}" || left == ")") && (is_word(right) || right == "(") {
        return " ";
    }
    ""
}

fn keyword_space_before_paren(word: &str) -> bool {
    matches!(word, "if" | "for" | "switch" | "select")
}

fn is_operand_end(token: &str) -> bool {
    // Keywords such as `return` and `if` take a unary operator (`return -1`, `if !ok`).
    (is_word(token) && !crate::go_stdlib::KEYWORDS.contains(&token))
        || matches!(token, ")" | "]" | "++" | "--")
}

fn is_operator(token: &str) -> bool {
    matches!(
        token,
        ":=" | "="
            | "=="
            | "!="
            | "<"
            | ">"
            | "<="
            | ">="
            | "+"
            | "-"
            | "*"
            | "/"
            | "%"
            | "&&"
            | "||"
            | "<-"
            | "+="
            | "-="
            | "*="
            | "/="
            | "%="
            | "&="
            | "|="
            | "^="
            | "<<="
            | ">>="
            | "&^="
            | "&"
            | "|"
            | "^"
            | "<<"
            | ">>"
            | "&^"
            | "!"
            | "++"
            | "--"
    )
}

fn is_keyword(token: &str) -> bool {
    crate::go_stdlib::KEYWORDS.contains(&token)
        || matches!(token, "true" | "false" | "nil" | "iota")
}

fn is_word(token: &str) -> bool {
    token
        .chars()
        .next()
        .is_some_and(|character| character == '_' || character.is_alphabetic())
}

#[cfg(test)]
mod tests {
    use super::format;

    #[test]
    fn spacing_indent_and_tokens_survive() -> Result<(), Box<dyn std::error::Error>> {
        let styles = nozhik_config::code_style::CodeStyles::builtin()?;
        let style = styles
            .selected("go", &std::collections::BTreeMap::new())
            .ok_or("go style")?;
        let source = "package demo\n\nfunc Sum(a int,b int) int {\nreturn a+b\n}\n";
        let edits = format(source, style, None)?;
        let (_, formatted) = edits.first().ok_or("edit")?;
        assert_eq!(
            formatted,
            "package demo\n\nfunc Sum(a int, b int) int {\n\treturn a + b\n}\n"
        );
        assert!(format(source, style, Some(&(0..4))).is_err());
        assert!(format("package demo\nfunc (\n", style, None).is_err());
        Ok(())
    }
}
