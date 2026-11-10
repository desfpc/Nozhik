//! Go highlighting, hover and completion for `.go` files.
//! Formatting is local. Tests and `golangci-lint` run in Docker.

use crate::{
    doc_provider::LanguageDocProvider, go_completion, go_stdlib, symbol_index::SymbolIndex,
};
use std::{path::Path, sync::Arc};

/// Recognizes Go through the shared language registry.
#[must_use]
pub fn is_go(uri: &crate::lsp_types::Url) -> bool {
    nozhik_config::languages::language_for_path(Path::new(uri.path()))
        == Some(nozhik_config::languages::LanguageId::Go)
}

pub use crate::go_format::format;

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
        return Some(go_stdlib::hover_markup(
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
    if let Some(hover) = go_completion::member_hover(index, path, line, local, byte) {
        return Some(hover);
    }
    let (word_start, word_end) = word_in(line, local)?;
    let word = line.get(word_start..word_end)?;
    if let Some(item) = go_stdlib::BUILTINS
        .iter()
        .chain(go_stdlib::CONSTANTS)
        .find(|item| item.name == word)
    {
        return Some(go_stdlib::hover_markup(item.signature, item.documentation));
    }
    go_stdlib::package(word).map(|package| {
        go_stdlib::hover_markup(
            &format!("import \"{}\"", package.path),
            package.documentation,
        )
    })
}

/// Completion for a Go caret. `source` may be the edited line or the whole file.
#[must_use]
pub fn complete(
    index: Option<&SymbolIndex>,
    path: &Path,
    source: &str,
    byte: usize,
    absolute_byte: usize,
) -> Vec<crate::lsp_types::CompletionItem> {
    go_completion::complete(index, path, source, byte, absolute_byte)
}

/// Exact built-in and standard-library documentation.
#[must_use]
pub fn documentation() -> Arc<dyn LanguageDocProvider + Send + Sync> {
    Arc::new(go_stdlib::GoDocs)
}

fn word_in(line: &str, byte: usize) -> Option<(usize, usize)> {
    if !line.is_char_boundary(byte.min(line.len())) {
        return None;
    }
    let mut index = byte.min(line.len());
    let on_word = line[index..]
        .chars()
        .next()
        .is_some_and(|character| character.is_alphanumeric() || character == '_');
    if !on_word {
        let (previous, character) = line[..index].char_indices().next_back()?;
        if !(character.is_alphanumeric() || character == '_') {
            return None;
        }
        index = previous;
    }
    let mut start = index;
    while let Some((previous, character)) = line[..start].char_indices().next_back() {
        if !(character.is_alphanumeric() || character == '_') {
            break;
        }
        start = previous;
    }
    let mut end = index;
    for (offset, character) in line[index..].char_indices() {
        if !(character.is_alphanumeric() || character == '_') {
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
        let mut highlighter = Highlighter::go()?;
        let spans = highlighter.highlight("package demo\nfunc Run() {}\n")?;
        assert!(
            spans.iter().any(|span| span.token.contains("keyword")),
            "{:?}",
            spans
                .iter()
                .map(|span| span.token.as_str())
                .collect::<Vec<_>>()
        );
        assert_eq!(highlighter.diagnostics(), Vec::new());
        let _broken = highlighter.highlight("func ( {\n")?;
        assert_ne!(highlighter.diagnostics(), Vec::new());
        Ok(())
    }

    #[test]
    fn hover_reads_the_standard_library() -> Result<(), Box<dyn std::error::Error>> {
        let source = "fmt.Println";
        let byte = source.find("Println").ok_or("Println")? + 1;
        let hover =
            super::hover(None, std::path::Path::new("demo.go"), source, byte).ok_or("hover")?;
        let crate::lsp_types::HoverContents::Markup(contents) = hover.contents else {
            return Err("markup".into());
        };
        assert!(contents.value.contains("Println"), "{}", contents.value);
        Ok(())
    }
}
