//! YAML syntax and conservative whitespace formatting; no project index or LSP.
use nozhik_config::code_style::{CodeStyle, IndentStyle};
use std::{collections::BTreeMap, ops::Range, path::Path};
use tree_sitter::{Node, Parser, Tree};

/// Recognizes both YAML suffixes through the shared registry.
#[must_use]
pub fn is_yaml(uri: &crate::lsp_types::Url) -> bool {
    nozhik_config::languages::language_for_path(Path::new(uri.path()))
        == Some(nozhik_config::languages::LanguageId::Yaml)
}
fn parse(source: &str) -> crate::Result<Tree> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_yaml::LANGUAGE.into())
        .map_err(|e| crate::Error::Protocol(e.to_string()))?;
    parser
        .parse(source, None)
        .ok_or_else(|| crate::Error::Protocol("YAML parse cancelled".into()))
}
fn opaque(node: Node<'_>) -> bool {
    matches!(
        node.kind(),
        "block_scalar" | "plain_scalar" | "single_quote_scalar" | "double_quote_scalar" | "comment"
    )
}
fn tokens<'a>(node: Node<'a>, output: &mut Vec<Node<'a>>) {
    if opaque(node) || node.child_count() == 0 {
        output.push(node);
    } else {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            tokens(child, output);
        }
    }
}
// The grammar omits trailing empty lines even with keep chomping (`|+`).
// Include them explicitly: their spaces can be part of the scalar payload.
fn scalar_end(node: Node<'_>, source: &str) -> usize {
    let mut end = node.end_byte();
    if node.kind() == "block_scalar" {
        for line in source[end..].split_inclusive('\n') {
            if !line
                .bytes()
                .all(|b| matches!(b, b' ' | b'\t' | b'\r' | b'\n'))
            {
                break;
            }
            end += line.len();
        }
    }
    end
}
// Collection columns derive from syntax, including compact `- key:` mappings.
fn columns(
    node: Node<'_>,
    source: &str,
    parent: Option<(usize, usize)>,
    size: usize,
    lines: &mut BTreeMap<usize, (usize, usize)>,
) {
    let context = if matches!(node.kind(), "block_mapping" | "block_sequence") {
        let old = node.start_position().column;
        let new = parent.map_or(0, |(column, target)| {
            if old > column {
                target
                    + if node.start_position().row
                        == node.parent().map_or(usize::MAX, |n| n.start_position().row)
                        && node.parent().is_some_and(|n| n.kind() == "block_node")
                        && node
                            .parent()
                            .and_then(|n| n.parent())
                            .is_some_and(|n| n.kind() == "block_sequence_item")
                    {
                        2
                    } else {
                        size
                    }
            } else {
                target
            }
        });
        Some((old, new))
    } else {
        parent
    };
    if opaque(node) || node.child_count() == 0 {
        if let Some((old, new)) = context {
            let text = &source[node.start_byte()..scalar_end(node, source)];
            let last = node.start_position().row + text.bytes().filter(|b| *b == b'\n').count()
                - usize::from(text.ends_with('\n'));
            for row in node.start_position().row..=last {
                lines.entry(row).or_insert((old, new));
            }
        }
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        columns(child, source, context, size, lines);
    }
}
// Multiline scalars retain every byte except the common line-leading shift.
fn signature(node: Node<'_>, source: &str) -> Vec<(String, String)> {
    let mut leaves = Vec::new();
    tokens(node, &mut leaves);
    leaves
        .into_iter()
        .map(|n| {
            let text = &source[n.start_byte()..scalar_end(n, source)];
            let value = if text.contains('\n') && opaque(n) {
                let mut lines = text.split('\n');
                let first = lines.next().unwrap_or("");
                let rest: Vec<_> = lines.collect();
                let mut ancestor = n.parent();
                let mut indent = 0;
                while let Some(parent) = ancestor {
                    if matches!(parent.kind(), "block_mapping" | "block_sequence") {
                        indent = parent.start_position().column;
                        break;
                    }
                    ancestor = parent.parent();
                }
                let mut normalized = first.to_owned();
                for line in rest {
                    normalized.push('\n');
                    normalized.push_str(
                        &line[line.bytes().take_while(|b| *b == b' ').count().min(indent)..],
                    );
                }
                normalized
            } else {
                text.into()
            };
            (n.kind().into(), value)
        })
        .collect()
}
/// Formats indentation and YAML separators without rewriting scalar values.
/// The entire candidate is reparsed and compared before returning selected edits.
/// # Errors
/// Rejects invalid YAML, tab profiles, invalid selections or structural changes.
pub fn format(
    source: &str,
    style: &CodeStyle,
    selection: Option<&Range<usize>>,
) -> crate::Result<Vec<(Range<usize>, String)>> {
    if style.language != "yaml" || style.indent.style != IndentStyle::Spaces {
        return Err(crate::Error::Protocol(
            "YAML requires a YAML profile with space indentation".into(),
        ));
    }
    if selection.is_some_and(|r| {
        r.start > r.end || !source.is_char_boundary(r.start) || !source.is_char_boundary(r.end)
    }) {
        return Err(crate::Error::Protocol("Invalid YAML selection".into()));
    }
    let tree = parse(source)?;
    if tree.root_node().has_error() {
        return Err(crate::Error::Protocol(
            "Fix YAML syntax errors before formatting; indentation determines structure".into(),
        ));
    }
    let mut edits = Vec::new();
    let mut line_columns = BTreeMap::new();
    columns(
        tree.root_node(),
        source,
        None,
        usize::from(style.indent.size),
        &mut line_columns,
    );
    let mut offset = 0;
    for (row, line) in source.split_inclusive('\n').enumerate() {
        let indent = line.bytes().take_while(|b| *b == b' ').count();
        if let Some(&(old, new)) = line_columns.get(&row)
            && indent >= old
        {
            let replacement = " ".repeat(new + indent - old);
            if replacement != line[..indent] {
                edits.push((offset..offset + indent, replacement));
            }
        }
        offset += line.len();
    }
    let mut leaves = Vec::new();
    tokens(tree.root_node(), &mut leaves);
    for pair in leaves.windows(2) {
        let left = pair[0];
        let right = pair[1];
        let range = left.end_byte()..right.start_byte();
        let gap = &source[range.clone()];
        if gap.contains(['\n', '\r']) || !gap.bytes().all(|b| matches!(b, b' ' | b'\t')) {
            continue;
        }
        let replacement = match (left.kind(), right.kind()) {
            (_, "," | "]" | "}") | ("[" | "{", _) => Some(""),
            (":" | "-" | "?", _) => Some(" "),
            (",", _) => Some(if style.spacing.after_comma { " " } else { "" }),
            _ => None,
        };
        if let Some(text) = replacement
            && text != gap
        {
            edits.push((range, text.into()));
        }
    }
    edits.retain(|(r, _)| selection.is_none_or(|s| r.start >= s.start && r.end <= s.end));
    edits.sort_by_key(|(r, _)| r.start);
    let mut candidate = source.to_owned();
    for (r, text) in edits.iter().rev() {
        candidate.replace_range(r.clone(), text);
    }
    let after = parse(&candidate)?;
    if after.root_node().has_error()
        || tree.root_node().to_sexp() != after.root_node().to_sexp()
        || signature(tree.root_node(), source) != signature(after.root_node(), &candidate)
    {
        return Err(crate::Error::Protocol(
            "YAML formatting would change structure or scalar content; select the complete block"
                .into(),
        ));
    }
    Ok(edits)
}

#[cfg(test)]
mod tests {
    use super::*;
    type TestResult = Result<(), Box<dyn std::error::Error>>;
    fn apply(
        source: &str,
        style: &CodeStyle,
        selection: Option<&Range<usize>>,
    ) -> crate::Result<String> {
        let mut output = source.to_owned();
        for (range, text) in format(source, style, selection)?.into_iter().rev() {
            output.replace_range(range, &text);
        }
        Ok(output)
    }
    #[test]
    fn pipelines_indentation_scalars_and_idempotence() -> TestResult {
        let catalog = nozhik_config::code_style::CodeStyles::builtin()?;
        let style = catalog
            .selected("yml", &BTreeMap::new())
            .ok_or("YAML profile")?;
        for source in [
            "options:\n    docker:   true\ndefinitions:\n    services:\n        docker:\n            memory: 3072\nx-artifacts: &artifacts\n    - bitbucket/artifacts/**\n    - logs/**\npipelines:\n    - parallel:\n        - step:\n            <<: *artifacts\n            runs-on: ['self.hosted',  'linux']\n",
            "---\nroot:\n    shell: |\n        echo hello\n          echo indented\n    folded: >-\n        line one\n        line two\n    explicit: |2+\n        leading spaces\n    quoted: \"hello #world: yes\"\n    plain: hello\n        world\n    flow: {a: 1, b: [true, null]}\n...\n---\nother: !!str 42\n",
            "list:\r\n    - one\r\n    - two\r\n# Комментарий 🦀\r\n",
            "root:\n    - key: value\n      other:\n          - nested: 2\n            next: true\n",
        ] {
            let formatted = apply(source, style, None)?;
            assert_eq!(apply(&formatted, style, None)?, formatted, "{formatted}");
            assert!(!parse(&formatted)?.root_node().has_error());
            assert_eq!(source.contains("\r\n"), formatted.contains("\r\n"));
            if source.starts_with("options") {
                assert!(
                    formatted.contains("options:\n  docker: true\n"),
                    "{formatted}"
                );
                assert!(formatted.contains("  - logs/**"), "{formatted}");
            }
        }
        Ok(())
    }
    #[test]
    fn block_scalar_payload_and_compact_sequences_remain_exact() -> TestResult {
        let catalog = nozhik_config::code_style::CodeStyles::builtin()?;
        let mut style = catalog
            .selected("yaml", &BTreeMap::new())
            .ok_or("style")?
            .clone();
        let source =
            "root:\n    script: |2+\n      echo one\n        echo two\n      \n    next: true\n";
        assert_eq!(
            apply(source, &style, None)?,
            "root:\n  script: |2+\n    echo one\n      echo two\n    \n  next: true\n"
        );
        let body = source.find("      echo").ok_or("body")?;
        let end = source.find("    next").ok_or("next")?;
        assert!(
            format(source, &style, Some(&(body..end))).is_err(),
            "body-only indentation changes scalar content"
        );
        style.indent.size = 4;
        let compact = "steps:\n -    name: build\n      script:\n       - echo hello\n";
        assert_eq!(
            apply(compact, &style, None)?,
            "steps:\n    - name: build\n      script:\n          - echo hello\n"
        );
        Ok(())
    }
    #[test]
    fn selection_errors_tabs_and_editing_preserve_yaml() -> TestResult {
        let catalog = nozhik_config::code_style::CodeStyles::builtin()?;
        let style = catalog.selected("yaml", &BTreeMap::new()).ok_or("style")?;
        let source = "root:\n    first:   1\n    second:   2\n";
        let start = source.find("first").ok_or("first")?;
        let end = source.find("\n    second").ok_or("second")?;
        let formatted = apply(source, style, Some(&(start..end)))?;
        assert_eq!(formatted, "root:\n    first: 1\n    second:   2\n");
        assert!(format("root: [broken", style, None).is_err());
        assert!(format("a: 1", style, Some(&(9..10))).is_err());
        let mut tabs = style.clone();
        tabs.indent.style = IndentStyle::Tabs;
        assert!(format(source, &tabs, None).is_err());
        assert_eq!(
            crate::formatting::newline_text("    command: '{'", "\r\n", Some(style)),
            "\r\n    "
        );
        let pasted = "  script: |\n      echo hi\n        echo exact\n";
        assert_eq!(
            crate::formatting::paste_edit(source, 6..6, pasted, Some(style)),
            (6..6, pasted.into())
        );
        Ok(())
    }
    #[test]
    fn highlighting_incremental_keys_anchors_and_diagnostics() -> TestResult {
        let source = "# note\nx-artifacts: &artifacts\n  - logs/**\nstep:\n  <<: *artifacts\n  name: 'Привет 🦀'\n  enabled: true\n  size: 3072\n";
        let mut parser = crate::highlighter::Highlighter::yaml()?;
        let spans = parser.highlight(source)?;
        for token in [
            "yaml.key",
            "yaml.anchor",
            "comment",
            "string",
            "number",
            "constant.builtin",
        ] {
            assert!(spans.iter().any(|s| s.token == token), "{token}: {spans:?}");
        }
        assert_eq!(parser.diagnostics().len(), 0);
        let changed = source.replace("Привет 🦀", "Мир 🦀🦀");
        let incremental = parser.highlight(&changed)?;
        let fresh = crate::highlighter::Highlighter::yaml()?.highlight(&changed)?;
        assert_eq!(
            incremental
                .iter()
                .map(|s| (&s.range, &s.token))
                .collect::<Vec<_>>(),
            fresh
                .iter()
                .map(|s| (&s.range, &s.token))
                .collect::<Vec<_>>()
        );
        parser.highlight("root: [broken")?;
        assert_ne!(parser.diagnostics().len(), 0);
        Ok(())
    }
}
