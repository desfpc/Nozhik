//! Incremental PHP trees and query capture ranges (UTF-8 bytes).
use std::ops::Range;
use tree_sitter::{InputEdit, Parser, Point, Query, QueryCursor, StreamingIterator, Tree};

/// A nonoverlapping query capture interval, sorted by byte offset.
#[derive(Clone, Debug)]
pub struct HighlightSpan {
    /// Half-open UTF-8 byte interval.
    pub range: Range<usize>,
    /// Tree-sitter capture name, independent of the UI palette.
    pub token: String,
}

/// Per-document incremental parser, owned by a background worker.
pub struct Highlighter {
    parser: Parser,
    query: Query,
    tree: Option<Tree>,
    source: String,
}
impl Highlighter {
    /// Loads the bundled PHP grammar and highlights.scm.
    /// # Errors
    /// Returns grammar/query compatibility failures.
    pub fn php() -> crate::Result<Self> {
        let language = tree_sitter_php::LANGUAGE_PHP.into();
        let mut parser = Parser::new();
        parser
            .set_language(&language)
            .map_err(|e| crate::Error::Protocol(e.to_string()))?;
        let query = Query::new(&language, tree_sitter_php::HIGHLIGHTS_QUERY)
            .map_err(|e| crate::Error::Protocol(e.to_string()))?;
        Ok(Self {
            parser,
            query,
            tree: None,
            source: String::new(),
        })
    }
    /// Reparses a minimal contiguous replacement using the previous edited tree.
    /// # Errors
    /// Returns cancellation if the parser could not produce a tree.
    pub fn highlight(&mut self, source: &str) -> crate::Result<Vec<HighlightSpan>> {
        if let Some(tree) = &mut self.tree {
            let mut start = self
                .source
                .bytes()
                .zip(source.bytes())
                .take_while(|(a, b)| a == b)
                .count();
            while !source.is_char_boundary(start) {
                start -= 1;
            }
            let mut suffix = self.source[start..]
                .bytes()
                .rev()
                .zip(source[start..].bytes().rev())
                .take_while(|(a, b)| a == b)
                .count();
            while !source.is_char_boundary(source.len() - suffix)
                || !self.source.is_char_boundary(self.source.len() - suffix)
            {
                suffix -= 1;
            }
            let old_end = self.source.len() - suffix;
            let new_end = source.len() - suffix;
            tree.edit(&InputEdit {
                start_byte: start,
                old_end_byte: old_end,
                new_end_byte: new_end,
                start_position: point(&self.source, start),
                old_end_position: point(&self.source, old_end),
                new_end_position: point(source, new_end),
            });
        }
        let tree = self
            .parser
            .parse(source, self.tree.as_ref())
            .ok_or_else(|| crate::Error::Protocol("parse cancelled".into()))?;
        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(&self.query, tree.root_node(), source.as_bytes());
        let mut spans = Vec::new();
        while let Some(m) = matches.next() {
            for capture in m.captures {
                spans.push(HighlightSpan {
                    range: capture.node.byte_range(),
                    token: self.query.capture_names()[capture.index as usize].into(),
                });
            }
        }
        self.tree = Some(tree);
        source.clone_into(&mut self.source);
        Ok(flatten(&spans))
    }
}
fn point(source: &str, byte: usize) -> Point {
    let prefix = &source[..byte];
    Point::new(
        prefix.bytes().filter(|b| *b == b'\n').count(),
        prefix.rfind('\n').map_or(byte, |i| byte - i - 1),
    )
}

// Resolve nesting once in the worker, allowing viewport binary searches in UI.
fn flatten(spans: &[HighlightSpan]) -> Vec<HighlightSpan> {
    let mut boundaries = std::collections::BTreeMap::<usize, Vec<(usize, bool)>>::new();
    for (i, span) in spans.iter().enumerate() {
        boundaries
            .entry(span.range.start)
            .or_default()
            .push((i, true));
        boundaries
            .entry(span.range.end)
            .or_default()
            .push((i, false));
    }
    let mut active = std::collections::BTreeSet::new();
    let mut previous = 0;
    let mut output: Vec<HighlightSpan> = Vec::new();
    for (byte, events) in boundaries {
        if byte > previous
            && let Some(index) = active.last().copied()
        {
            let span: &HighlightSpan = &spans[index];
            if let Some(last) = output.last_mut()
                && last.range.end == previous
                && last.token == span.token
            {
                last.range.end = byte;
            } else {
                output.push(HighlightSpan {
                    range: previous..byte,
                    token: span.token.clone(),
                });
            }
        }
        for (i, start) in events {
            if start {
                active.insert(i);
            } else {
                active.remove(&i);
            }
        }
        previous = byte;
    }
    output
}
