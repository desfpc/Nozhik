//! Shared CSS class identities and markup references supplied by language extractors.
use crate::{
    lsp_types::{CompletionItem, CompletionItemKind, CompletionTextEdit, TextEdit},
    symbol_index::{FileSymbols, ReferenceLocation, SymbolIndex},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    ops::Range,
    path::{Path, PathBuf},
};

/// A selector declaration or a static HTML class token, with exact UTF-8 coordinates.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClassOccurrence {
    /// Decoded, case-sensitive class identity.
    pub name: String,
    /// Original source location and preview.
    pub location: ReferenceLocation,
    /// True for CSS selectors; false for markup class attributes.
    pub declaration: bool,
}
/// Immutable cross-language class snapshot. New extractors contribute `FileSymbols` facts.
#[derive(Clone, Debug, Default)]
pub struct ClassIndex {
    by_name: BTreeMap<String, Vec<ClassOccurrence>>,
    by_file: HashMap<PathBuf, Vec<ClassOccurrence>>,
    attributes: HashMap<PathBuf, Vec<Range<usize>>>,
}
impl ClassIndex {
    /// Builds lookup tables from already parsed partitions, without reading source files.
    #[must_use]
    pub fn from_partitions<'a>(partitions: impl Iterator<Item = &'a SymbolIndex>) -> Self {
        let mut result = Self::default();
        for partition in partitions {
            for (path, facts) in partition.file_facts() {
                result
                    .attributes
                    .insert(path.to_path_buf(), facts.class_attributes.clone());
                result
                    .by_file
                    .insert(path.to_path_buf(), facts.class_occurrences.clone());
                for occurrence in &facts.class_occurrences {
                    result
                        .by_name
                        .entry(occurrence.name.clone())
                        .or_default()
                        .push(occurrence.clone());
                }
            }
        }
        for rows in result.by_name.values_mut() {
            rows.sort_by(|a, b| {
                (&a.location.path, a.location.byte).cmp(&(&b.location.path, b.location.byte))
            });
        }
        result
    }
    /// Finds only class tokens; standard CSS properties never resolve here.
    #[must_use]
    pub fn at(&self, path: &Path, byte: usize) -> Option<&ClassOccurrence> {
        self.by_file
            .get(path)?
            .iter()
            .find(|o| (o.location.byte..o.location.byte + o.location.length).contains(&byte))
    }
    /// CSS selector declarations in name order. Markup usages are omitted.
    pub fn declarations(&self) -> impl Iterator<Item = &ClassOccurrence> {
        self.by_name
            .values()
            .flat_map(|rows| rows.iter().filter(|row| row.declaration))
    }

    /// All indexed selectors and markup usages, in deterministic path/byte order.
    #[must_use]
    pub fn occurrences(&self, name: &str) -> &[ClassOccurrence] {
        self.by_name.get(name).map_or(&[], Vec::as_slice)
    }
    /// Completes a class token in an indexed selector or literal class attribute.
    /// None means this position belongs to another language feature.
    #[must_use]
    pub fn complete(&self, path: &Path, source: &str, caret: usize) -> Option<Vec<CompletionItem>> {
        if !source.is_char_boundary(caret) {
            return None;
        }
        let attribute = self
            .attributes
            .get(path)
            .into_iter()
            .flatten()
            .find(|r| r.start <= caret && caret <= r.end);
        let range = if let Some(attribute) = attribute {
            let start = source
                .get(attribute.start..caret)?
                .rfind(|c: char| c.is_ascii_whitespace())
                .map_or(attribute.start, |i| attribute.start + i + 1);
            let end = source
                .get(caret..attribute.end)?
                .find(|c: char| c.is_ascii_whitespace())
                .map_or(attribute.end, |i| caret + i);
            start..end
        } else {
            let occurrence = self.by_file.get(path)?.iter().find(|o| {
                o.declaration
                    && o.location.byte <= caret
                    && caret <= o.location.byte + o.location.length
            })?;
            occurrence.location.byte..occurrence.location.byte + occurrence.location.length
        };
        let prefix = source.get(range.start..caret)?;
        if prefix.contains(['<', '>', '&', '"', '\'', '\\']) {
            return Some(Vec::new());
        }
        let mut items = Vec::new();
        for (name, occurrences) in self.by_name.range(prefix.to_string()..) {
            if !name.starts_with(prefix) {
                break;
            }
            if !occurrences.iter().any(|o| o.declaration) {
                continue;
            }
            let text = if attribute.is_some() {
                name.clone()
            } else {
                escape_identifier(name)
            };
            items.push(CompletionItem {
                label: name.clone(),
                kind: Some(CompletionItemKind::CLASS),
                detail: Some("CSS class".into()),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                    range: crate::lsp_types::Range::new(
                        crate::position(source, range.start),
                        crate::position(source, range.end),
                    ),
                    new_text: text,
                })),
                ..CompletionItem::default()
            });
            if items.len() == 100 {
                break;
            }
        }
        Some(items)
    }
}
fn escape_identifier(name: &str) -> String {
    name.chars()
        .enumerate()
        .map(|(i, c)| {
            if c.is_ascii_digit() && i == 0 {
                format!("\\{:x} ", u32::from(c))
            } else if c.is_alphanumeric() || matches!(c, '_' | '-') {
                c.to_string()
            } else {
                format!("\\{c}")
            }
        })
        .collect()
}
/// Captures exact source coordinates without UTF-16 conversion or file I/O.
#[must_use]
pub fn location(path: &Path, source: &str, range: Range<usize>) -> ReferenceLocation {
    Locations::new(source).at(path, range)
}
struct Locations<'a> {
    source: &'a str,
    lines: Vec<usize>,
}
impl<'a> Locations<'a> {
    fn new(source: &'a str) -> Self {
        let lines = std::iter::once(0)
            .chain(source.match_indices('\n').map(|(i, _)| i + 1))
            .collect();
        Self { source, lines }
    }
    fn at(&self, path: &Path, range: Range<usize>) -> ReferenceLocation {
        let line = self
            .lines
            .partition_point(|start| *start <= range.start)
            .saturating_sub(1);
        let start = self.lines.get(line).copied().unwrap_or(0);
        let (preview, preview_offset) =
            crate::symbol_index::reference_preview(self.source, range.start);
        ReferenceLocation {
            path: path.into(),
            byte: range.start,
            length: range.len(),
            line: u32::try_from(line).unwrap_or(u32::MAX),
            column: u32::try_from(range.start - start).unwrap_or(u32::MAX),
            preview,
            preview_offset,
        }
    }
}
/// Adds CSS selector facts while preserving the existing selector API.
pub fn selectors(path: &Path, source: &str, facts: &mut FileSymbols) {
    let locations = Locations::new(source);
    facts.class_occurrences = facts
        .css_classes
        .iter()
        .map(|c| ClassOccurrence {
            name: c.name.clone(),
            location: locations.at(path, c.range.clone()),
            declaration: true,
        })
        .collect();
}
/// Adds static classes from PHP's actual markup nodes; PHP expressions stay opaque.
pub(crate) fn php_markup(
    path: &Path,
    source: &str,
    tree: &tree_sitter::Tree,
    facts: &mut FileSymbols,
) {
    fn copy(node: tree_sitter::Node<'_>, source: &str, masked: &mut [u8]) {
        if node.kind() == "php_end_tag" {
            let start = node.end_byte();
            let length = source.as_bytes()[start..]
                .iter()
                .take_while(|b| b.is_ascii_whitespace())
                .count();
            masked[start..start + length]
                .copy_from_slice(&source.as_bytes()[start..start + length]);
        }
        if node.kind() == "text" {
            masked[node.byte_range()].copy_from_slice(&source.as_bytes()[node.byte_range()]);
            return;
        }
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            copy(child, source, masked);
        }
    }
    let mut masked = vec![1; source.len()];
    copy(tree.root_node(), source, &mut masked);
    let Ok(markup) = String::from_utf8(masked) else {
        return;
    };
    markup_classes(path, source, &markup, facts);
}
fn markup_classes(path: &Path, source: &str, markup: &str, facts: &mut FileSymbols) {
    let locations = Locations::new(source);
    let bytes = markup.as_bytes();
    let lower = markup.to_ascii_lowercase();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"<!--") {
            i = lower[i + 4..]
                .find("-->")
                .map_or(bytes.len(), |n| i + n + 7);
            continue;
        }
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        i += 1;
        let tag_start = i;
        while bytes
            .get(i)
            .is_some_and(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b':' | b'_'))
        {
            i += 1;
        }
        if i == tag_start {
            continue;
        }
        let tag = &lower[tag_start..i];
        while i < bytes.len() && bytes[i] != b'>' {
            if bytes[i].is_ascii_whitespace() || bytes[i] == b'/' {
                i += 1;
                continue;
            }
            let name_start = i;
            while bytes
                .get(i)
                .is_some_and(|b| !b.is_ascii_whitespace() && !matches!(b, b'=' | b'>' | b'/'))
            {
                i += 1;
            }
            let name = &lower[name_start..i];
            while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
                i += 1;
            }
            if bytes.get(i) != Some(&b'=') {
                continue;
            }
            i += 1;
            while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
                i += 1;
            }
            let quote = bytes.get(i).copied().filter(|b| matches!(b, b'\'' | b'"'));
            if quote.is_some() {
                i += 1;
            }
            let start = i;
            while bytes.get(i).is_some_and(|b| {
                quote.map_or_else(|| !b.is_ascii_whitespace() && *b != b'>', |q| *b != q)
            }) {
                i += 1;
            }
            if name == "class" {
                let value = &markup[start..i];
                // Attribute segments adjacent to dynamic PHP form unknown runtime tokens.
                for (offset, segment) in segments(value) {
                    let range = start + offset..start + offset + segment.len();
                    facts.class_attributes.push(range.clone());
                    let mut at = range.start;
                    for token in segment.split_inclusive(|c: char| c.is_ascii_whitespace()) {
                        let name = token.trim_end_matches(|c: char| c.is_ascii_whitespace());
                        if !name.is_empty() && !name.contains('&') {
                            facts.class_occurrences.push(ClassOccurrence {
                                name: name.into(),
                                location: locations.at(path, at..at + name.len()),
                                declaration: false,
                            });
                        }
                        at += token.len();
                    }
                }
            }
            if quote.is_some() && i < bytes.len() {
                i += 1;
            }
        }
        if matches!(tag, "script" | "style" | "textarea" | "title") {
            let closing = format!("</{tag}");
            i = lower[i..]
                .match_indices(&closing)
                .find_map(|(n, _)| {
                    let end = i + n + closing.len();
                    bytes
                        .get(end)
                        .filter(|b| b.is_ascii_whitespace() || matches!(b, b'>' | b'/'))
                        .map(|_| end)
                })
                .unwrap_or(bytes.len());
        }
    }
}
fn segments(value: &str) -> Vec<(usize, &str)> {
    let mut result = Vec::new();
    let mut offset = 0;
    let dynamic = value.contains('\u{1}');
    for segment in value.split('\u{1}') {
        let mut start = 0;
        let mut end = segment.len();
        if offset > 0 {
            start = segment
                .find(|c: char| c.is_ascii_whitespace())
                .map_or(end, |i| i + 1);
        }
        if offset + end < value.len() {
            end = segment
                .rfind(|c: char| c.is_ascii_whitespace())
                .unwrap_or(0);
        }
        if start <= end && (!segment.is_empty() || !dynamic) {
            result.push((offset + start, &segment[start..end]));
        }
        offset += segment.len() + 1;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn php_markup_cross_language_classes_are_exact_and_case_sensitive()
    -> Result<(), Box<dyn std::error::Error>> {
        let css_path = Path::new("style.css");
        let css_source = ".Card,.card,.left-menu,.sm\\:wide { color:red } .Card:hover {margin:0}";
        let mut css = FileSymbols {
            css_classes: crate::css::classes(css_source)?,
            ..FileSymbols::default()
        };
        selectors(css_path, css_source, &mut css);
        let php_path = Path::new("view.php");
        let source = "Привет <div CLASS=\"Card left-menu sm:wide\" data-class='fake'></div><!-- <i class='fake'> --><script>let s = '<b class=\"fake\">';</script><?php $s='<i class=\"fake\">'; ?><i class=\"Card <?= $dynamic ?> card\"></i><i class=\"Card<?= $suffix ?> tail\"></i>";
        let php = crate::php_symbols::parse(php_path, source)?;
        assert_eq!(
            php.class_occurrences
                .iter()
                .map(|o| o.name.as_str())
                .collect::<Vec<_>>(),
            ["Card", "left-menu", "sm:wide", "Card", "card", "tail"]
        );
        let mut css_index = SymbolIndex::new(crate::symbol_index::LanguageId::Css);
        css_index.replace(css_path.into(), css);
        let mut php_index = SymbolIndex::new(crate::symbol_index::LanguageId::Php);
        php_index.replace(php_path.into(), php);
        let custom = crate::php_symbols::parse(
            php_path,
            "<script-card><b class='Card'></b></script-card><script>let x = '</script-card><b class=\"fake\">';</script>",
        )?;
        assert_eq!(
            custom
                .class_occurrences
                .iter()
                .map(|o| o.name.as_str())
                .collect::<Vec<_>>(),
            ["Card"]
        );
        let index = ClassIndex::from_partitions([&css_index, &php_index].into_iter());
        assert_eq!(index.occurrences("Card").len(), 4);
        assert_eq!(index.occurrences("card").len(), 2);
        assert_eq!(index.occurrences("fake").len(), 0);
        let caret = source.find("left-menu").ok_or("class")? + 5;
        let items = index
            .complete(php_path, source, caret)
            .ok_or("class context")?;
        assert_eq!(items.first().map(|i| i.label.as_str()), Some("left-menu"));
        let edits = crate::completion::edits(&items[0], source, caret).ok_or("completion edit")?;
        assert_eq!(&source[edits[0].0.clone()], "left-menu");
        assert!(
            index
                .at(css_path, css_source.find("color").ok_or("property")?)
                .is_none()
        );
        let usage = index.at(php_path, caret).ok_or("usage")?;
        assert!(!usage.declaration);
        assert_eq!(
            &source[usage.location.byte..usage.location.byte + usage.location.length],
            "left-menu"
        );
        assert!(
            index
                .complete(
                    php_path,
                    source,
                    source.find("$dynamic").ok_or("PHP expression")?
                )
                .is_none()
        );
        Ok(())
    }
}
