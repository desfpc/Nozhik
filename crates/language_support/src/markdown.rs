//! Markdown lexical highlighting. Ranges are UTF-8 bytes and do not overlap.
//!
//! The lexer covers `CommonMark` blocks used in project notes: ATX and setext
//! headings, fenced code, lists, quotes, thematic breaks and `GitHub` tables,
//! plus inline code, emphasis, strong, strikethrough, links and autolinks.
//! The GUI renderer parses the same constructs again; this pass only paints
//! the source.
use crate::highlighter::HighlightSpan;

/// Recognizes Markdown through the shared language registry.
#[must_use]
pub fn is_markdown(uri: &crate::lsp_types::Url) -> bool {
    uri.to_file_path().is_ok_and(|path| {
        nozhik_config::languages::language_for_path(&path)
            == Some(nozhik_config::languages::LanguageId::Markdown)
    })
}

/// Lexes Markdown, including unfinished constructs. Markers stay in the span
/// so the source view can color `**bold**` and `` `code` `` together with their text.
#[must_use]
pub fn highlight(source: &str) -> Vec<HighlightSpan> {
    let lines = line_ranges(source);
    let mut spans = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let (start, end) = lines[index];
        let line = &source[start..end];
        if let Some((marker, length)) = opening_fence(line) {
            let mut close = lines.len() - 1;
            let mut found = false;
            for next in index + 1..lines.len() {
                if closing_fence(&source[lines[next].0..lines[next].1], marker, length) {
                    close = next;
                    found = true;
                    break;
                }
            }
            push(&mut spans, start..lines[close].1, "markup.raw");
            index = if found { close + 1 } else { lines.len() };
            continue;
        }
        if let Some(table_end) = table_block(&lines, index, source) {
            for (row, &(row_start, row_end)) in lines.iter().enumerate().take(table_end).skip(index)
            {
                let text = &source[row_start..row_end];
                if row == index + 1 {
                    push(&mut spans, row_start..row_end, "markup.rule");
                } else {
                    scan_inline(text, row_start, None, true, &mut spans);
                }
            }
            index = table_end;
            continue;
        }
        if index + 1 < lines.len()
            && setext_title(line)
            && setext_marker(&source[lines[index + 1].0..lines[index + 1].1])
        {
            scan_inline(line, start, Some("markup.heading"), false, &mut spans);
            let (mark_start, mark_end) = lines[index + 1];
            push(&mut spans, mark_start..mark_end, "markup.heading");
            index += 2;
            continue;
        }
        if thematic(line) {
            push(&mut spans, start..end, "markup.rule");
            index += 1;
            continue;
        }
        scan_line(line, start, &mut spans);
        index += 1;
    }
    spans
}

fn line_ranges(source: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut start = 0;
    for (offset, _) in source.match_indices('\n') {
        let mut end = offset;
        if end > start && source.as_bytes()[end - 1] == b'\r' {
            end -= 1;
        }
        ranges.push((start, end));
        start = offset + 1;
    }
    if start < source.len() {
        let mut end = source.len();
        if source.as_bytes()[end - 1] == b'\r' {
            end -= 1;
        }
        ranges.push((start, end));
    }
    ranges
}

fn push(spans: &mut Vec<HighlightSpan>, range: std::ops::Range<usize>, token: &str) {
    if range.start >= range.end {
        return;
    }
    if let Some(last) = spans.last_mut()
        && last.range.end == range.start
        && last.token == token
    {
        last.range.end = range.end;
        return;
    }
    spans.push(HighlightSpan {
        range,
        token: token.to_owned(),
    });
}

fn opening_fence(line: &str) -> Option<(u8, usize)> {
    let bytes = line.as_bytes();
    let mut index = 0;
    while index < bytes.len() && bytes[index] == b' ' && index < 3 {
        index += 1;
    }
    let marker = *bytes.get(index)?;
    if marker != b'`' && marker != b'~' {
        return None;
    }
    let mut length = 0;
    while bytes.get(index + length) == Some(&marker) {
        length += 1;
    }
    if length < 3 {
        return None;
    }
    let info = &line[index + length..];
    if marker == b'`' && info.contains('`') {
        return None;
    }
    Some((marker, length))
}

fn closing_fence(line: &str, marker: u8, length: usize) -> bool {
    let bytes = line.as_bytes();
    let mut index = 0;
    while index < bytes.len() && bytes[index] == b' ' && index < 3 {
        index += 1;
    }
    let Some(rest) = bytes.get(index..) else {
        return false;
    };
    rest.len() >= length
        && rest.iter().take(length).all(|byte| *byte == marker)
        && rest
            .iter()
            .skip(length)
            .all(|byte| *byte == b' ' || *byte == b'\t')
}

fn count_byte(bytes: &[u8], marker: u8) -> usize {
    let mut marks = 0;
    for byte in bytes {
        if *byte == marker {
            marks += 1;
        }
    }
    marks
}

fn thematic(line: &str) -> bool {
    let trimmed = line.trim();
    let bytes = trimmed.as_bytes();
    let Some(marker) = bytes.first().copied() else {
        return false;
    };
    if !matches!(marker, b'-' | b'*' | b'_') {
        return false;
    }
    let marks = count_byte(bytes, marker);
    marks >= 3
        && bytes
            .iter()
            .all(|byte| *byte == marker || byte.is_ascii_whitespace())
}

fn setext_marker(line: &str) -> bool {
    let trimmed = line.trim();
    let bytes = trimmed.as_bytes();
    let Some(marker) = bytes.first().copied() else {
        return false;
    };
    (marker == b'=' || marker == b'-') && bytes.iter().all(|byte| *byte == marker)
}

fn setext_title(line: &str) -> bool {
    let trimmed = line.trim();
    !trimmed.is_empty()
        && heading_marker(line).is_none()
        && list_marker(line).is_none()
        && opening_fence(line).is_none()
        && !table_row(line)
        && !trimmed.starts_with('>')
}

fn table_row(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.contains('|') && heading_marker(line).is_none() && opening_fence(line).is_none()
}

fn table_separator(line: &str) -> bool {
    let trimmed = line.trim();
    if !trimmed.contains('|') || count_byte(trimmed.as_bytes(), b'-') < 3 {
        return false;
    }
    let mut cells = 0;
    for cell in trimmed.trim_matches('|').split('|') {
        let cell = cell.trim();
        if cell.is_empty() {
            return false;
        }
        let core = cell.trim_matches(':');
        if core.is_empty() || !core.bytes().all(|byte| byte == b'-') {
            return false;
        }
        cells += 1;
    }
    cells >= 1
}

fn table_block(lines: &[(usize, usize)], index: usize, source: &str) -> Option<usize> {
    let next = index.checked_add(1)?;
    let separator = lines.get(next)?;
    let row = &source[lines[index].0..lines[index].1];
    let separator = &source[separator.0..separator.1];
    if !table_row(row) || !table_separator(separator) {
        return None;
    }
    let mut end = next + 1;
    while let Some(line) = lines.get(end) {
        let text = &source[line.0..line.1];
        if text.trim().is_empty() || !table_row(text) || table_separator(text) {
            break;
        }
        end += 1;
    }
    Some(end)
}

fn heading_marker(line: &str) -> Option<(usize, usize)> {
    let bytes = line.as_bytes();
    let mut index = 0;
    while index < bytes.len() && bytes[index] == b' ' && index < 3 {
        index += 1;
    }
    let hash_start = index;
    let mut hashes = 0;
    while bytes.get(index) == Some(&b'#') && hashes < 7 {
        index += 1;
        hashes += 1;
    }
    if !(1..=6).contains(&hashes) {
        return None;
    }
    if index < bytes.len() && bytes[index] != b' ' && bytes[index] != b'\t' {
        return None;
    }
    if bytes
        .get(index)
        .is_some_and(|byte| *byte == b' ' || *byte == b'\t')
    {
        index += 1;
    }
    Some((hash_start, index))
}

fn list_marker(line: &str) -> Option<(usize, usize)> {
    let bytes = line.as_bytes();
    let mut index = 0;
    while index < bytes.len() && bytes[index] == b' ' && index < 24 {
        index += 1;
    }
    if matches!(bytes.get(index), Some(b'-' | b'*' | b'+')) && bytes.get(index + 1) == Some(&b' ') {
        return Some((index, index + 2));
    }
    let start = index;
    let mut digits = 0;
    while bytes.get(index).is_some_and(u8::is_ascii_digit) && digits < 9 {
        index += 1;
        digits += 1;
    }
    if digits > 0
        && matches!(bytes.get(index), Some(b'.' | b')'))
        && bytes.get(index + 1) == Some(&b' ')
    {
        return Some((start, index + 2));
    }
    None
}

fn scan_line(line: &str, base: usize, spans: &mut Vec<HighlightSpan>) {
    let mut cursor = 0;
    loop {
        let bytes = line.as_bytes().get(cursor..).unwrap_or_default();
        let mut indent = 0;
        while indent < bytes.len() && bytes[indent] == b' ' && indent < 3 {
            indent += 1;
        }
        if bytes.get(indent) == Some(&b'>') {
            let mut end = indent + 1;
            if bytes.get(end) == Some(&b' ') {
                end += 1;
            }
            push(
                spans,
                (base + cursor + indent)..(base + cursor + end),
                "markup.quote",
            );
            cursor += end;
            continue;
        }
        break;
    }
    let rest = line.get(cursor..).unwrap_or_default();
    if let Some((hash_start, content)) = heading_marker(rest) {
        push(
            spans,
            (base + cursor + hash_start)..(base + cursor + content),
            "markup.heading",
        );
        scan_inline(
            rest.get(content..).unwrap_or_default(),
            base + cursor + content,
            Some("markup.heading"),
            false,
            spans,
        );
        return;
    }
    if let Some((mark_start, mark_end)) = list_marker(rest) {
        push(
            spans,
            (base + cursor + mark_start)..(base + cursor + mark_end),
            "markup.list",
        );
        scan_inline(
            rest.get(mark_end..).unwrap_or_default(),
            base + cursor + mark_end,
            None,
            false,
            spans,
        );
        return;
    }
    scan_inline(rest, base + cursor, None, false, spans);
}

enum Paint {
    Atom {
        end: usize,
        token: &'static str,
    },
    Wrap {
        end: usize,
        token: &'static str,
        inner_start: usize,
        inner_end: usize,
    },
}

fn scan_inline(
    text: &str,
    base: usize,
    default: Option<&str>,
    pipes: bool,
    spans: &mut Vec<HighlightSpan>,
) {
    let mut index = 0;
    while index < text.len() {
        if let Some(length) = escaped_len(text, index) {
            emit_gap(spans, base, index..index + length, default);
            index += length;
            continue;
        }
        let special = next_special(text, index, pipes);
        if special > index {
            emit_gap(spans, base, index..special, default);
            index = special;
            continue;
        }
        if pipes && text.as_bytes().get(index) == Some(&b'|') {
            push(spans, (base + index)..(base + index + 1), "markup.list");
            index += 1;
            continue;
        }
        if let Some(paint) = try_paint(text, index) {
            match paint {
                Paint::Atom { end, token } if end > index => {
                    push(spans, (base + index)..(base + end), token);
                    index = end;
                }
                Paint::Wrap {
                    end,
                    token,
                    inner_start,
                    inner_end,
                } if end > index && inner_start >= index && inner_end <= end => {
                    push(spans, (base + index)..(base + inner_start), token);
                    scan_inline(
                        text.get(inner_start..inner_end).unwrap_or_default(),
                        base + inner_start,
                        Some(token),
                        pipes,
                        spans,
                    );
                    push(spans, (base + inner_end)..(base + end), token);
                    index = end;
                }
                _ => {
                    let length = text[index..].chars().next().map_or(1, char::len_utf8);
                    emit_gap(spans, base, index..index + length, default);
                    index += length;
                }
            }
            continue;
        }
        let length = text[index..].chars().next().map_or(1, char::len_utf8);
        emit_gap(spans, base, index..index + length, default);
        index += length;
    }
}

fn emit_gap(
    spans: &mut Vec<HighlightSpan>,
    base: usize,
    range: std::ops::Range<usize>,
    default: Option<&str>,
) {
    if let Some(token) = default {
        push(spans, (base + range.start)..(base + range.end), token);
    }
}

fn next_special(text: &str, index: usize, pipes: bool) -> usize {
    text.get(index..)
        .and_then(|tail| {
            tail.char_indices().find(|(_, character)| {
                matches!(character, '\\' | '`' | '*' | '_' | '~' | '[' | '!' | '<')
                    || (pipes && *character == '|')
            })
        })
        .map_or(text.len(), |(offset, _)| index + offset)
}

fn try_paint(text: &str, index: usize) -> Option<Paint> {
    code_span(text, index)
        .or_else(|| angle(text, index))
        .or_else(|| strike(text, index))
        .or_else(|| link(text, index))
        .or_else(|| emphasis(text, index))
}

fn code_span(text: &str, index: usize) -> Option<Paint> {
    if text.as_bytes().get(index) != Some(&b'`') {
        return None;
    }
    let length = text[index..]
        .bytes()
        .take_while(|byte| *byte == b'`')
        .count();
    if length == 0 {
        return None;
    }
    let mut cursor = index + length;
    let bytes = text.as_bytes();
    while cursor < bytes.len() {
        if bytes[cursor] != b'`' {
            cursor += 1;
            continue;
        }
        let run = bytes[cursor..]
            .iter()
            .take_while(|byte| **byte == b'`')
            .count();
        if run == length {
            return Some(Paint::Atom {
                end: cursor + run,
                token: "markup.raw",
            });
        }
        cursor += run;
    }
    None
}

fn angle(text: &str, index: usize) -> Option<Paint> {
    if text.as_bytes().get(index) != Some(&b'<') {
        return None;
    }
    let relative = text.get(index + 1..)?.find('>')?;
    let end = index + 1 + relative;
    let inner = text.get(index + 1..end)?;
    if inner.is_empty() || inner.chars().any(char::is_whitespace) {
        return None;
    }
    let token = if looks_like_url(inner) {
        "markup.link"
    } else if looks_like_tag(inner) {
        "markup.raw"
    } else {
        return None;
    };
    Some(Paint::Atom {
        end: end + 1,
        token,
    })
}

fn looks_like_url(inner: &str) -> bool {
    let lower = inner.to_ascii_lowercase();
    lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("mailto:")
        || inner.contains('@')
}

fn looks_like_tag(inner: &str) -> bool {
    let rest = inner.strip_prefix('/').unwrap_or(inner);
    if rest.starts_with('!') {
        return true;
    }
    rest.starts_with(|character: char| character.is_ascii_alphabetic())
}

fn strike(text: &str, index: usize) -> Option<Paint> {
    if !text.get(index..)?.starts_with("~~") {
        return None;
    }
    if text[index + 2..]
        .chars()
        .next()
        .is_none_or(char::is_whitespace)
    {
        return None;
    }
    let close = find_closer(text, index + 2, '~', 2)?;
    (close > index + 2).then_some(Paint::Wrap {
        end: close + 2,
        token: "markup.strike",
        inner_start: index + 2,
        inner_end: close,
    })
}

fn link(text: &str, index: usize) -> Option<Paint> {
    let label_at = if text.get(index..)?.starts_with("![") {
        index + 2
    } else if text.as_bytes().get(index) == Some(&b'[') {
        index + 1
    } else {
        return None;
    };
    let label_end = find_unescaped(text, label_at, b']')?;
    let paren = label_end + 1;
    if text.as_bytes().get(paren) != Some(&b'(') {
        return None;
    }
    let url_end = find_unescaped(text, paren + 1, b')')?;
    Some(Paint::Wrap {
        end: url_end + 1,
        token: "markup.link",
        inner_start: label_at,
        inner_end: label_end,
    })
}

fn emphasis(text: &str, index: usize) -> Option<Paint> {
    let marker = text[index..].chars().next()?;
    if marker != '*' && marker != '_' {
        return None;
    }
    let run = text[index..]
        .chars()
        .take_while(|character| *character == marker)
        .count();
    let attempts: &[(usize, &str)] = if run >= 3 {
        &[
            (3, "markup.strong_emphasis"),
            (2, "markup.strong"),
            (1, "markup.emphasis"),
        ]
    } else if run >= 2 {
        &[(2, "markup.strong"), (1, "markup.emphasis")]
    } else if run == 1 {
        &[(1, "markup.emphasis")]
    } else {
        return None;
    };
    for &(need, token) in attempts {
        if marker == '_' && (word_before(text, index) || word_at(text, index + need)) {
            continue;
        }
        if text[index + need..]
            .chars()
            .next()
            .is_none_or(char::is_whitespace)
        {
            continue;
        }
        let Some(close) = find_closer(text, index + need, marker, need) else {
            continue;
        };
        if close <= index + need {
            continue;
        }
        return Some(Paint::Wrap {
            end: close + need,
            token,
            inner_start: index + need,
            inner_end: close,
        });
    }
    None
}

fn find_closer(text: &str, from: usize, marker: char, need: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut index = from;
    while index < bytes.len() {
        if let Some(length) = escaped_len(text, index) {
            index += length;
            continue;
        }
        if bytes[index] == b'`'
            && let Some(Paint::Atom { end, .. }) = code_span(text, index)
        {
            index = end;
            continue;
        }
        let character = text[index..].chars().next()?;
        if character != marker {
            index += character.len_utf8();
            continue;
        }
        let run = text[index..]
            .chars()
            .take_while(|item| *item == marker)
            .count();
        let flanked = text[..index]
            .chars()
            .next_back()
            .is_some_and(|item| !item.is_whitespace());
        let underscore = marker == '_' && word_at(text, index + need);
        if run >= need && flanked && !underscore {
            return Some(index);
        }
        index += run;
    }
    None
}

fn find_unescaped(text: &str, from: usize, target: u8) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut index = from;
    while index < bytes.len() {
        if let Some(length) = escaped_len(text, index) {
            index += length;
            continue;
        }
        if bytes[index] == target {
            return Some(index);
        }
        index += 1;
    }
    None
}

fn escaped_len(text: &str, index: usize) -> Option<usize> {
    if text.as_bytes().get(index) != Some(&b'\\') {
        return None;
    }
    let character = text.get(index + 1..)?.chars().next()?;
    is_ascii_punctuation(character).then_some(1 + character.len_utf8())
}

const fn is_ascii_punctuation(character: char) -> bool {
    matches!(character, '!'..='/' | ':'..='@' | '['..='`' | '{'..='~')
}

fn word_before(text: &str, index: usize) -> bool {
    text.get(..index)
        .and_then(|prefix| prefix.chars().next_back())
        .is_some_and(|character| character.is_alphanumeric() || character == '_')
}

fn word_at(text: &str, index: usize) -> bool {
    text.get(index..)
        .and_then(|tail| tail.chars().next())
        .is_some_and(|character| character.is_alphanumeric() || character == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token_at(source: &str, needle: &str) -> Result<String, String> {
        let start = source
            .find(needle)
            .ok_or_else(|| format!("missing {needle}"))?;
        let middle = start + needle.len() / 2;
        let spans = highlight(source);
        assert_flat(source, &spans);
        spans
            .iter()
            .find(|span| span.range.contains(&middle))
            .map(|span| span.token.clone())
            .ok_or_else(|| format!("{needle} has no token"))
    }

    fn assert_flat(source: &str, spans: &[HighlightSpan]) {
        let mut end = 0;
        for span in spans {
            assert!(span.range.start >= end, "overlapping {:?}", span.range);
            assert!(span.range.end <= source.len());
            assert!(source.is_char_boundary(span.range.start));
            assert!(source.is_char_boundary(span.range.end));
            assert!(!span.range.is_empty());
            end = span.range.end;
        }
    }

    #[test]
    fn source_formatting_is_nonoverlapping() -> Result<(), String> {
        let source = "# Схема `кода`\r\n\r\n**ключевые** поля и *имя* и ~~черновик~~.\n\n[сайт](https://example.com) и <https://example.com>\n\n- пункт\n> цитата\n\n| A | B |\n| --- | --- |\n| **да** | `нет` |\n\n```\n**не код**\n```\n\nsnake_case и \\*не курсив*\n";
        assert_eq!(token_at(source, "# Схема")?, "markup.heading");
        assert_eq!(token_at(source, "`кода`")?, "markup.raw");
        assert_eq!(token_at(source, "**ключевые**")?, "markup.strong");
        assert_eq!(token_at(source, "*имя*")?, "markup.emphasis");
        assert_eq!(token_at(source, "~~черновик~~")?, "markup.strike");
        assert_eq!(
            token_at(source, "[сайт](https://example.com)")?,
            "markup.link"
        );
        assert_eq!(token_at(source, "- ")?, "markup.list");
        let quote = source
            .find("\n> цитата")
            .ok_or_else(|| "quote line".to_owned())?;
        let spans = highlight(source);
        assert_eq!(
            spans
                .iter()
                .find(|span| span.range.contains(&(quote + 1)))
                .map(|span| span.token.as_str()),
            Some("markup.quote")
        );
        assert_eq!(token_at(source, "| --- | --- |")?, "markup.rule");
        assert_eq!(token_at(source, "**да**")?, "markup.strong");
        assert_eq!(token_at(source, "`нет`")?, "markup.raw");
        let spans = highlight(source);
        assert!(
            !spans.iter().any(|span| span.token == "markup.strong"
                && source[span.range.clone()].contains("не код"))
        );
        assert!(spans.iter().any(|span| {
            span.token == "markup.raw" && source[span.range.clone()].contains("**не код**")
        }));
        assert!(token_at(source, "snake_case").is_err());
        assert!(token_at(source, "\\*не курсив*").is_err());
        assert!(!source[highlight(source)[0].range.clone()].contains('\r'));
        Ok(())
    }

    #[test]
    fn strong_emphasis_and_setext() -> Result<(), String> {
        let source = "***оба***\n\nЗаголовок\n==========\n";
        assert_eq!(token_at(source, "***оба***")?, "markup.strong_emphasis");
        assert_eq!(token_at(source, "Заголовок")?, "markup.heading");
        assert_eq!(token_at(source, "==========")?, "markup.heading");
        Ok(())
    }
}
