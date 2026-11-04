//! JSON lexical highlighting and lossless whitespace formatting, without indexing.
use crate::highlighter::HighlightSpan;
use std::ops::Range;

/// Recognizes JSON through the shared language registry.
#[must_use]
pub fn is_json(uri: &crate::lsp_types::Url) -> bool {
    uri.to_file_path().is_ok_and(|path| {
        nozhik_config::languages::language_for_path(&path)
            == Some(nozhik_config::languages::LanguageId::Json)
    })
}

/// Lexes even incomplete JSON. Ranges are UTF-8 byte offsets, including punctuation.
#[must_use]
pub fn highlight(source: &str) -> Vec<HighlightSpan> {
    let bytes = source.as_bytes();
    let mut spans = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        let token = match bytes[i] {
            b'"' => {
                i += 1;
                while i < bytes.len() {
                    match bytes[i] {
                        b'\\' => i = (i + 2).min(bytes.len()),
                        b'"' => {
                            i += 1;
                            break;
                        }
                        _ => i += 1,
                    }
                }
                if bytes[i..].iter().find(|b| !b.is_ascii_whitespace()) == Some(&b':') {
                    "property"
                } else {
                    "string"
                }
            }
            b'{' | b'}' | b'[' | b']' | b',' | b':' => {
                i += 1;
                "punctuation"
            }
            _ => {
                i += 1;
                while i < bytes.len()
                    && !bytes[i].is_ascii_whitespace()
                    && !b"{}[],:\"".contains(&bytes[i])
                {
                    i += 1;
                }
                if matches!(bytes[start], b'-' | b'0'..=b'9') {
                    "number"
                } else {
                    "constant.builtin"
                }
            }
        };
        spans.push(HighlightSpan {
            range: start..i,
            token: token.into(),
        });
    }
    spans
}

/// Validates JSON and rewrites whitespace only; duplicate keys and number lexemes survive.
/// # Errors
/// Returns syntax errors without modifying the input.
pub fn render(source: &str, unit: Option<&str>) -> crate::Result<String> {
    let mut de = serde_json::Deserializer::from_str(source);
    <serde::de::IgnoredAny as serde::Deserialize>::deserialize(&mut de)
        .and_then(|_| de.end())
        .map_err(|e| crate::Error::Protocol(e.to_string()))?;
    let tokens = highlight(source);
    let mut output = String::with_capacity(source.len());
    let mut depth = 0usize;
    for (index, span) in tokens.iter().enumerate() {
        let text = &source[span.range.clone()];
        let previous = index
            .checked_sub(1)
            .and_then(|i| tokens.get(i))
            .map(|s| &source[s.range.clone()]);
        if matches!(text, "}" | "]") {
            depth = depth.saturating_sub(1);
        }
        if let Some(unit) = unit {
            let empty = matches!((previous, text), (Some("{"), "}") | (Some("["), "]"));
            if !empty && (matches!(previous, Some("{" | "[" | ",")) || matches!(text, "}" | "]")) {
                output.push('\n');
                output.push_str(&unit.repeat(depth));
            } else if previous == Some(":") {
                output.push(' ');
            }
        }
        output.push_str(text);
        if matches!(text, "{" | "[") {
            depth += 1;
        }
    }
    Ok(output)
}

/// Formats the document or an independently valid selected JSON value.
/// # Errors
/// Rejects malformed JSON, invalid ranges and incompatible styles.
pub fn format(
    source: &str,
    style: &nozhik_config::code_style::CodeStyle,
    range: Option<&Range<usize>>,
) -> crate::Result<Vec<(Range<usize>, String)>> {
    if style.language != "json" {
        return Err(crate::Error::Protocol("JSON profile required".into()));
    }
    let range = range.cloned().unwrap_or(0..source.len());
    let selected = source
        .get(range.clone())
        .ok_or_else(|| crate::Error::Protocol("Invalid JSON selection".into()))?;
    // Validate the whole document as well as the independently selected value.
    render(source, None)?;
    let mut formatted = render(selected, Some(&style.unit()))?;
    let line_start = source[..range.start].rfind('\n').map_or(0, |i| i + 1);
    let base: String = source[line_start..range.start]
        .chars()
        .take_while(|c| matches!(c, ' ' | '\t'))
        .collect();
    if !base.is_empty() {
        formatted = formatted.replace('\n', &format!("\n{base}"));
    }
    if selected.ends_with('\n') {
        formatted.push('\n');
    }
    if source.contains("\r\n") {
        formatted = formatted.replace('\n', "\r\n");
    }
    Ok(vec![(range, formatted)])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn profiles_ranges_and_crlf_preserve_surroundings() -> Result<(), Box<dyn std::error::Error>> {
        let styles = nozhik_config::code_style::CodeStyles::builtin()?;
        let mut style = styles
            .selected("json", &std::collections::BTreeMap::new())
            .ok_or("JSON style")?
            .clone();
        style.indent.size = 4;
        let source = "{\r\n  \"a\": {\"x\":1}\r\n}\r\n";
        let start = source.find("{\"x\"").ok_or("start")?;
        let end = start + "{\"x\":1}".len();
        let edits = format(source, &style, Some(&(start..end)))?;
        assert_eq!(edits[0].0, start..end);
        assert_eq!(edits[0].1, "{\r\n      \"x\": 1\r\n  }");
        let full = format(source, &style, None)?;
        assert!(full[0].1.ends_with("\r\n"));
        assert_eq!(render(&full[0].1, None)?, render(source, None)?);
        assert!(format(source, &style, Some(&(start..end - 1))).is_err());
        style.indent.style = nozhik_config::code_style::IndentStyle::Tabs;
        assert!(
            format("{\"x\":1}", &style, None)?[0]
                .1
                .contains("\n\t\"x\"")
        );
        Ok(())
    }
    #[test]
    fn lossless_format_and_invalid_input() -> Result<(), Box<dyn std::error::Error>> {
        let source = r#"{"ключ":1e+999,"ключ":-0,"s":"a \\\" b","a":[true,null,{}]}"#;
        let pretty = render(source, Some("  "))?;
        assert!(pretty.contains("\n  \"ключ\": 1e+999"));
        assert_eq!(render(&pretty, None)?, source);
        assert_eq!(render(&pretty, Some("  "))?, pretty);
        assert!(render("{\"a\":", Some("  ")).is_err());
        assert!(render("{} {}", None).is_err());
        assert!(highlight(source).iter().any(|s| s.token == "property"));
        Ok(())
    }
}
