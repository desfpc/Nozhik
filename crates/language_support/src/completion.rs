//! Completion filtering and transactional replacement planning.
use lsp_types::{CompletionItem, CompletionTextEdit, InsertTextFormat};
use std::ops::Range;

/// Prefix before a UTF-8 caret, including PHP variable/namespace markers.
#[must_use]
pub fn prefix(source: &str, caret: usize) -> Range<usize> {
    let mut start = caret;
    for (i, c) in source[..caret].char_indices().rev() {
        if !(c.is_alphanumeric() || matches!(c, '_' | '$' | '\\')) {
            break;
        }
        start = i;
    }
    start..caret
}
/// Filters and orders suggestions; snippet-only items are excluded because the
/// editor advertises no snippet support.
#[must_use]
pub fn filter(items: Vec<CompletionItem>, prefix: &str) -> Vec<CompletionItem> {
    let mut items: Vec<_> = items
        .into_iter()
        .filter(|item| {
            item.insert_text_format != Some(InsertTextFormat::SNIPPET)
                && item
                    .filter_text
                    .as_ref()
                    .unwrap_or(&item.label)
                    .to_lowercase()
                    .starts_with(&prefix.to_lowercase())
        })
        .collect();
    items.sort_by(|a, b| {
        a.sort_text
            .as_ref()
            .unwrap_or(&a.label)
            .cmp(b.sort_text.as_ref().unwrap_or(&b.label))
    });
    items.truncate(100);
    items
}
/// Plans all completion edits against one original document. The caller applies
/// them atomically, so additional imports cannot shift the main edit coordinates.
#[must_use]
pub fn edits(
    item: &CompletionItem,
    source: &str,
    caret: usize,
) -> Option<Vec<(Range<usize>, String)>> {
    let (range, text) = match &item.text_edit {
        Some(CompletionTextEdit::Edit(edit)) => (
            crate::byte_range(source, edit.range)?,
            edit.new_text.clone(),
        ),
        Some(CompletionTextEdit::InsertAndReplace(edit)) => (
            crate::byte_range(source, edit.replace)?,
            edit.new_text.clone(),
        ),
        None => (
            prefix(source, caret),
            item.insert_text
                .clone()
                .unwrap_or_else(|| item.label.clone()),
        ),
    };
    let mut edits = vec![(range, text)];
    for edit in item.additional_text_edits.iter().flatten() {
        edits.push((
            crate::byte_range(source, edit.range)?,
            edit.new_text.clone(),
        ));
    }
    Some(edits)
}
