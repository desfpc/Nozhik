//! Shared value types. This crate has no runtime, filesystem, or UI dependencies.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Opaque buffer identity, allocated by the future buffer store, never reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BufferId(pub u64);

/// Zero-based position. `column` counts UTF-8 bytes, not characters or UTF-16 units.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Position {
    /// Zero-based line index.
    pub line: u32,
    /// Zero-based UTF-8 byte offset within the line.
    pub column: u32,
}

/// Half-open range `[start, end)` in a particular buffer revision.
///
/// The buffer store must additionally check bounds and UTF-8 boundaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "SpanRepr", into = "SpanRepr")]
pub struct Span {
    start: Position,
    end: Position,
}

/// A range ends before it starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("span end precedes its start")]
pub struct InvalidSpan;

impl Span {
    /// Constructs an ordered range; equal endpoints represent an insertion.
    ///
    /// # Errors
    /// Returns [`InvalidSpan`] if `end < start`.
    pub fn new(start: Position, end: Position) -> Result<Self, InvalidSpan> {
        if end < start {
            return Err(InvalidSpan);
        }
        Ok(Self { start, end })
    }

    /// Inclusive start of the range.
    #[must_use]
    pub const fn start(self) -> Position {
        self.start
    }

    /// Exclusive end of the range.
    #[must_use]
    pub const fn end(self) -> Position {
        self.end
    }
}

#[derive(Serialize, Deserialize)]
struct SpanRepr {
    start: Position,
    end: Position,
}

impl TryFrom<SpanRepr> for Span {
    type Error = InvalidSpan;

    fn try_from(value: SpanRepr) -> Result<Self, Self::Error> {
        Self::new(value.start, value.end)
    }
}

impl From<Span> for SpanRepr {
    fn from(value: Span) -> Self {
        Self {
            start: value.start,
            end: value.end,
        }
    }
}

/// Typed invalidation keys; subscribers reread the current settings snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ConfigKey {
    /// All settings may have changed.
    All,
    /// Editor indentation width.
    TabWidth,
    /// Editor font size.
    FontSize,
    /// Color theme identifier.
    Theme,
}

#[cfg(test)]
mod tests {
    use super::{Position, Span};

    #[test]
    fn rejects_reversed_span() {
        let start = Position { line: 1, column: 0 };
        assert!(Span::new(start, Position::default()).is_err());
        assert!(Span::new(start, start).is_ok());
    }
}
