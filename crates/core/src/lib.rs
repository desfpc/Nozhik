//! Shared value types. This crate has no runtime, filesystem, or UI dependencies.

use serde::{Deserialize, Serialize};
use std::ops::Range;
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
    /// Absolute UTF-8 byte offset, validated against the owning buffer revision.
    pub byte_offset: usize,
}

/// Directed selection in one buffer revision; a caret has equal endpoints.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Selection {
    /// Fixed endpoint.
    pub anchor: Position,
    /// Moving endpoint.
    pub head: Position,
}

/// Replacement of a UTF-8 byte range with text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEdit {
    /// Half-open byte range in the transaction's original revision.
    pub range: Range<usize>,
    /// Replacement text.
    pub text: String,
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
        let start = Position {
            line: 1,
            column: 0,
            byte_offset: 0,
        };
        assert!(Span::new(start, Position::default()).is_err());
        assert!(Span::new(start, start).is_ok());
    }
}

/// Revocable execution authorization. Invoke on a worker immediately before
/// starting external code; implementations may verify filesystem fingerprints.
#[derive(Clone)]
pub struct ExecutionGuard {
    verify: std::sync::Arc<dyn Fn() -> bool + Send + Sync>,
    current: std::sync::Arc<dyn Fn() -> bool + Send + Sync>,
}
impl ExecutionGuard {
    /// Wraps a host-owned authorization callback; never derive it from project settings.
    #[must_use]
    pub fn new(check: impl Fn() -> bool + Send + Sync + 'static) -> Self {
        let check = std::sync::Arc::new(check);
        Self {
            verify: check.clone(),
            current: check,
        }
    }
    /// Separates launch verification from a cheap revocation check for running sessions.
    #[must_use]
    pub fn with_status(
        verify: impl Fn() -> bool + Send + Sync + 'static,
        current: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Self {
        Self {
            verify: std::sync::Arc::new(verify),
            current: std::sync::Arc::new(current),
        }
    }
    /// Whether two clones refer to the same host authorization session.
    #[must_use]
    pub fn same_authorization(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.verify, &other.verify)
    }
    /// Checks revocation without starting a new operation.
    #[must_use]
    pub fn current(&self) -> bool {
        (self.current)()
    }
    /// Explicit authorization for standalone library callers with trusted inputs.
    #[must_use]
    pub fn unrestricted() -> Self {
        Self::new(|| true)
    }
    /// Restricted mode, also used while trust is being checked.
    #[must_use]
    pub fn restricted() -> Self {
        Self::new(|| false)
    }
    /// Revalidates authorization. Call outside the UI thread and before execution.
    #[must_use]
    pub fn check(&self) -> bool {
        (self.verify)()
    }
}
impl std::fmt::Debug for ExecutionGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ExecutionGuard(..)")
    }
}
