use crate::{BufferError, Result, Selection, TextBuffer, TextEdit};

/// A directed selection, including an empty caret.
pub type Cursor = Selection;

/// Nonempty, sorted cursor set. Touching selections/carets merge;
/// the leftmost selection's direction is retained (ties use input order).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorSet(Vec<Cursor>);

impl Default for CursorSet {
    fn default() -> Self {
        Self(vec![Cursor::default()])
    }
}

impl CursorSet {
    /// Validates all coordinates in the given revision, then merges collisions.
    /// # Errors
    /// Rejects empty sets and positions whose line/column/offset disagree.
    pub fn new(buffer: &TextBuffer, mut cursors: Vec<Cursor>) -> Result<Self> {
        if cursors.is_empty() {
            return Err(BufferError::InvalidPosition);
        }
        for cursor in &cursors {
            for position in [cursor.anchor, cursor.head] {
                if buffer.position_at(position.byte_offset)? != position {
                    return Err(BufferError::InvalidPosition);
                }
            }
        }
        cursors.sort_by_key(|cursor| cursor.anchor.byte_offset.min(cursor.head.byte_offset));
        let mut merged: Vec<Cursor> = Vec::with_capacity(cursors.len());
        for cursor in cursors {
            let start = cursor.anchor.byte_offset.min(cursor.head.byte_offset);
            let end = cursor.anchor.byte_offset.max(cursor.head.byte_offset);
            if let Some(previous) = merged.last_mut() {
                let left = previous.anchor.byte_offset.min(previous.head.byte_offset);
                let right = previous.anchor.byte_offset.max(previous.head.byte_offset);
                if start <= right {
                    let forward = previous.anchor.byte_offset <= previous.head.byte_offset;
                    let low = buffer.position_at(left)?;
                    let high = buffer.position_at(right.max(end))?;
                    *previous = if forward {
                        Cursor {
                            anchor: low,
                            head: high,
                        }
                    } else {
                        Cursor {
                            anchor: high,
                            head: low,
                        }
                    };
                    continue;
                }
            }
            merged.push(cursor);
        }
        Ok(Self(merged))
    }

    /// Canonical selections; mutation goes through `TextBuffer::set_cursors`.
    #[must_use]
    pub fn as_slice(&self) -> &[Cursor] {
        &self.0
    }
}

pub const fn remap(offset: usize, edit: &TextEdit) -> usize {
    if offset < edit.range.start {
        offset
    } else if offset <= edit.range.end {
        edit.range.start + edit.text.len()
    } else {
        offset - (edit.range.end - edit.range.start) + edit.text.len()
    }
}
