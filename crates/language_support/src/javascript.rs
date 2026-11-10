//! JavaScript highlighting helpers, hover and completion for `.js`, `.jsx`, `.mjs` and `.cjs`.
//! TypeScript files stay unrecognized. There is no formatter, linter or test runner.

use crate::{
    doc_provider::LanguageDocProvider, js_completion, js_stdlib, symbol_index::SymbolIndex,
};
use std::{path::Path, sync::Arc};

/// Recognizes JavaScript through the shared language registry.
#[must_use]
pub fn is_javascript(uri: &crate::lsp_types::Url) -> bool {
    nozhik_config::languages::language_for_path(Path::new(uri.path()))
        == Some(nozhik_config::languages::LanguageId::JavaScript)
}

/// Project or built-in hover at a UTF-8 byte. Member names are resolved from the current line.
#[must_use]
pub fn hover(
    index: Option<&SymbolIndex>,
    path: &Path,
    source: &str,
    byte: usize,
) -> Option<crate::lsp_types::Hover> {
    if let Some(index) = index
        && let Some(symbol) = index.at(path, byte)
    {
        return Some(js_stdlib::hover_markup(
            &symbol.signature,
            &symbol.documentation,
        ));
    }
    let start = source
        .get(..byte.min(source.len()))?
        .rfind('\n')
        .map_or(0, |index| index + 1);
    let end = source
        .get(byte.min(source.len())..)?
        .find('\n')
        .map_or(source.len(), |index| byte.min(source.len()) + index);
    let line = source.get(start..end)?;
    let local = byte.saturating_sub(start);
    if let Some(hover) = js_completion::member_hover(index, path, line, local, byte) {
        return Some(hover);
    }
    let (word_start, word_end) = word_in(line, local)?;
    js_stdlib::global_info(line.get(word_start..word_end)?)
        .map(|info| js_stdlib::hover_markup(&info.signature, &info.description))
}

/// Completion for a JavaScript caret. `source` may be the edited line or the whole file.
#[must_use]
pub fn complete(
    index: Option<&SymbolIndex>,
    path: &Path,
    source: &str,
    byte: usize,
    absolute_byte: usize,
) -> Vec<crate::lsp_types::CompletionItem> {
    js_completion::complete(index, path, source, byte, absolute_byte)
}

/// Exact global and keyword documentation. Bare method names are not linked.
#[must_use]
pub fn documentation() -> Arc<dyn LanguageDocProvider + Send + Sync> {
    Arc::new(js_stdlib::JsDocs)
}

fn word_in(line: &str, byte: usize) -> Option<(usize, usize)> {
    if !line.is_char_boundary(byte.min(line.len())) {
        return None;
    }
    let mut index = byte.min(line.len());
    let on_word = line[index..].chars().next().is_some_and(|character| {
        character.is_alphanumeric() || character == '_' || character == '$'
    });
    if !on_word {
        let (previous, character) = line[..index].char_indices().next_back()?;
        if !(character.is_alphanumeric() || character == '_' || character == '$') {
            return None;
        }
        index = previous;
    }
    let mut start = index;
    while let Some((previous, character)) = line[..start].char_indices().next_back() {
        if !(character.is_alphanumeric() || character == '_' || character == '$') {
            break;
        }
        start = previous;
    }
    let mut end = index;
    for (offset, character) in line[index..].char_indices() {
        if !(character.is_alphanumeric() || character == '_' || character == '$') {
            break;
        }
        end = index + offset + character.len_utf8();
    }
    (start < end).then_some((start, end))
}

#[cfg(test)]
mod tests {
    use crate::highlighter::Highlighter;

    #[test]
    fn highlights_keywords_and_reports_syntax_errors() -> Result<(), Box<dyn std::error::Error>> {
        let mut highlighter = Highlighter::javascript()?;
        let spans = highlighter.highlight("const value = 1; // note\nfunction run() {}\n")?;
        assert!(
            spans.iter().any(|span| span.token.contains("keyword")),
            "{:?}",
            spans
                .iter()
                .map(|span| span.token.as_str())
                .collect::<Vec<_>>()
        );
        assert_eq!(highlighter.diagnostics(), Vec::new());
        let _broken = highlighter.highlight("function ( {")?;
        assert_ne!(highlighter.diagnostics(), Vec::new());
        Ok(())
    }
}
