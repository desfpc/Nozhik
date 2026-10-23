use crate::TextBuffer;
use regex_cursor::{Input, engines::meta::Regex};
use std::ops::Range;

/// Regex compilation failure with the original compiler error as its source.
/// The upstream compiler's return type is private, so it is boxed here.
#[derive(Debug, thiserror::Error)]
#[error("invalid search pattern")]
pub struct SearchError {
    #[source]
    source: Box<dyn std::error::Error + Send + Sync>,
}

/// Compiled search over rope chunks, including cross-chunk and multiline matches.
///
/// Only the pattern, regex machinery and result ranges allocate, never a flattened
/// copy of the buffer. Matches are nonoverlapping UTF-8 byte ranges.
#[derive(Debug)]
pub struct SearchEngine(Regex);

impl SearchEngine {
    /// Compiles a regex using Unicode-aware Rust regex syntax.
    /// # Errors
    /// Returns the regex compiler's syntax or size-limit error.
    pub fn regex(pattern: &str) -> std::result::Result<Self, SearchError> {
        Regex::new(pattern).map(Self).map_err(|source| SearchError {
            source: Box::new(source),
        })
    }

    /// Compiles a literal query; an empty query matches every scalar boundary.
    /// # Errors
    /// Returns an error if the escaped pattern exceeds regex compiler limits.
    pub fn literal(text: &str) -> std::result::Result<Self, SearchError> {
        Self::regex(&regex::escape(text))
    }

    /// Finds all matches without copying buffer text. Empty matches respect UTF-8.
    #[must_use]
    pub fn find_all(&self, buffer: &TextBuffer) -> Vec<Range<usize>> {
        self.0
            .find_iter(Input::new(buffer.rope().slice(..)))
            .map(|matched| matched.range())
            .collect()
    }
}
