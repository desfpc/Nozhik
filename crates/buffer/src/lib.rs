//! UI-independent UTF-8 text engine. Public edit/search offsets count bytes;
//! navigation char indices count Unicode scalars, never graphemes.
//!
//! ```
//! use nozhik_buffer::{BufferId, SearchEngine, TextBuffer};
//! let mut buffer = TextBuffer::load_from_str(BufferId(1), "Привет\n");
//! buffer.insert(12, " 🦀")?;
//! assert_eq!(SearchEngine::literal("🦀")?.find_all(&buffer), vec![13..17]);
//! buffer.undo()?;
//! assert!(!buffer.is_dirty());
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod cursor;
mod history;
mod search;

pub use cursor::{Cursor, CursorSet};
pub use history::History;
pub use nozhik_core::{BufferId, Position, Selection, TextEdit};
pub use search::{SearchEngine, SearchError};

use nozhik_event_bus::{BufferEvent, Event, EventBus};
use ropey::{Rope, RopeSlice};
use std::{ops::Range, sync::Arc};
use thiserror::Error;

/// Invalid input leaves text, cursors, history and events unchanged.
#[derive(Debug, Error)]
pub enum BufferError {
    /// Offset, line, column or range is outside the buffer or inside UTF-8.
    #[error("invalid text coordinate or range")]
    InvalidPosition,
    /// Transaction ranges overlap, including ambiguous coincident insertions.
    #[error("overlapping transaction edits")]
    OverlappingEdits,
    /// Undo/redo has no entry in this direction.
    #[error("history is empty")]
    EmptyHistory,
    /// A detached history belongs to another buffer or a different state.
    #[error("history does not match the current buffer state")]
    HistoryMismatch,
    /// The monotonic revision counter is exhausted.
    #[error("buffer revision counter exhausted")]
    RevisionOverflow,
}

/// Text engine result.
pub type Result<T> = std::result::Result<T, BufferError>;

/// Atomic replacements, all measured in the original buffer's UTF-8 bytes.
#[derive(Debug, Clone, Default)]
pub struct EditTransaction {
    /// Nonoverlapping replacements; input order does not matter.
    pub edits: Vec<TextEdit>,
    /// Explicit caller-defined typing group. Consecutive equal IDs coalesce.
    /// `None`, cursor movement, save and undo/redo close a group.
    pub group: Option<u64>,
}

/// Cheap copy-on-write snapshot, suitable for workers and resync after bus lag.
#[derive(Debug, Clone)]
pub struct BufferSnapshot {
    /// Identity allocated by the caller; IDs must not be reused by a document store.
    pub buffer_id: BufferId,
    /// Revision increases on text edits, undo and redo.
    pub revision: u64,
    /// Immutable by ownership; editing a clone uses Rope copy-on-write.
    pub text: Rope,
    /// Canonical selections at snapshot time.
    pub cursors: CursorSet,
}

/// A rope, directed cursors and transactional history; no filesystem or UI I/O.
#[derive(Debug)]
pub struct TextBuffer {
    id: BufferId,
    text: Rope,
    cursors: CursorSet,
    history: History,
    bus: Option<EventBus>,
    revision: u64,
    state: u64,
    saved_state: u64,
    owner: Arc<()>,
}

impl TextBuffer {
    /// Loads text as a clean buffer with one caret at byte zero. LF and CRLF
    /// are line breaks; lone CR is text. Line endings are preserved verbatim.
    #[must_use]
    pub fn load_from_str(id: BufferId, text: &str) -> Self {
        Self {
            id,
            text: Rope::from_str(text),
            cursors: CursorSet::default(),
            history: History::default(),
            bus: None,
            revision: 0,
            state: 0,
            saved_state: 0,
            owner: Arc::new(()),
        }
    }

    /// Connects notifications; existing state is not replayed.
    pub fn set_event_bus(&mut self, bus: EventBus) {
        self.bus = Some(bus);
    }

    /// Authoritative snapshot, with shared rope storage rather than a file copy.
    #[must_use]
    pub fn snapshot(&self) -> BufferSnapshot {
        BufferSnapshot {
            buffer_id: self.id,
            revision: self.revision,
            text: self.text.clone(),
            cursors: self.cursors.clone(),
        }
    }

    /// Read-only rope access for rendering/search, without flattening.
    #[must_use]
    pub const fn rope(&self) -> &Rope {
        &self.text
    }

    /// Current sorted, nonoverlapping cursors.
    #[must_use]
    pub const fn cursors(&self) -> &CursorSet {
        &self.cursors
    }

    /// Whether current history state differs from the last explicit save point.
    #[must_use]
    pub const fn is_dirty(&self) -> bool {
        self.state != self.saved_state
    }

    /// Marks this state saved after the caller successfully persists it.
    /// Ends the typing group so undo can return exactly to this state.
    pub const fn mark_saved(&mut self) {
        self.saved_state = self.state;
        self.history.break_group();
    }

    /// Converts a byte boundary into all three consistent position coordinates.
    /// # Errors
    /// Rejects out-of-bounds offsets, partial UTF-8 scalars or u32 overflow.
    pub fn position_at(&self, offset: usize) -> Result<Position> {
        let char_idx = self.byte_to_char(offset)?;
        let line = self.text.char_to_line(char_idx);
        Ok(Position {
            byte_offset: offset,
            line: u32::try_from(line).map_err(|_| BufferError::InvalidPosition)?,
            column: u32::try_from(offset - self.text.line_to_byte(line))
                .map_err(|_| BufferError::InvalidPosition)?,
        })
    }

    fn byte_to_char(&self, offset: usize) -> Result<usize> {
        let index = self
            .text
            .try_byte_to_char(offset)
            .map_err(|_| BufferError::InvalidPosition)?;
        if self.text.char_to_byte(index) != offset {
            return Err(BufferError::InvalidPosition);
        }
        Ok(index)
    }

    /// Converts a Unicode scalar index to a position (byte column).
    /// # Errors
    /// Rejects indices beyond EOF or positions exceeding u32 line/column limits.
    pub fn char_to_position(&self, index: usize) -> Result<Position> {
        let offset = self
            .text
            .try_char_to_byte(index)
            .map_err(|_| BufferError::InvalidPosition)?;
        self.position_at(offset)
    }

    /// Converts a zero-based line and byte column to a Unicode scalar index.
    /// Newline bytes belong to the preceding line; EOF is a valid position.
    /// # Errors
    /// Rejects nonexistent lines, columns spilling into another line and partial UTF-8.
    pub fn line_col_to_char(&self, line: usize, column: usize) -> Result<usize> {
        let start = self
            .text
            .try_line_to_byte(line)
            .map_err(|_| BufferError::InvalidPosition)?;
        if line >= self.text.len_lines() {
            return Err(BufferError::InvalidPosition);
        }
        let offset = start
            .checked_add(column)
            .ok_or(BufferError::InvalidPosition)?;
        let position = self.position_at(offset)?;
        if usize::try_from(position.line).ok() != Some(line) {
            return Err(BufferError::InvalidPosition);
        }
        self.byte_to_char(offset)
    }

    /// Borrowed line slice including its original line ending; no text allocation.
    /// # Errors
    /// Rejects a nonexistent line.
    pub fn line(&self, line: usize) -> Result<RopeSlice<'_>> {
        self.text.get_line(line).ok_or(BufferError::InvalidPosition)
    }

    /// Borrowed viewport of whole lines `[start, end)`; end may equal line count.
    /// # Errors
    /// Rejects reversed or out-of-bounds line ranges.
    pub fn lines(&self, range: Range<usize>) -> Result<RopeSlice<'_>> {
        if range.start > range.end || range.end > self.text.len_lines() {
            return Err(BufferError::InvalidPosition);
        }
        Ok(self
            .text
            .slice(self.text.line_to_char(range.start)..self.text.line_to_char(range.end)))
    }

    /// Replaces selections, validating redundant position coordinates and merging collisions.
    /// # Errors
    /// Rejects empty cursor sets and stale or invalid positions.
    pub fn set_cursors(&mut self, cursors: Vec<Cursor>) -> Result<()> {
        let next = CursorSet::new(self, cursors)?;
        self.history.break_group();
        if next != self.cursors {
            self.cursors = next;
            self.publish_cursors();
        }
        Ok(())
    }

    /// Inserts text at a UTF-8 byte boundary as one undo step.
    /// # Errors
    /// Rejects an invalid offset or revision exhaustion.
    pub fn insert(&mut self, offset: usize, text: &str) -> Result<()> {
        self.apply_transaction(EditTransaction {
            edits: vec![TextEdit {
                range: offset..offset,
                text: text.into(),
            }],
            group: None,
        })
    }

    /// Deletes a half-open UTF-8 byte range as one undo step.
    /// # Errors
    /// Rejects invalid ranges or revision exhaustion.
    pub fn delete(&mut self, range: Range<usize>) -> Result<()> {
        self.apply_transaction(EditTransaction {
            edits: vec![TextEdit {
                range,
                text: String::new(),
            }],
            group: None,
        })
    }

    /// Validates everything before commit. Applies edits from right to left,
    /// remaps cursors with right affinity and merges resulting collisions.
    /// Empty/no-op transactions create neither history nor events.
    /// # Errors
    /// Rejects invalid ranges, overlaps and revision exhaustion atomically.
    pub fn apply_transaction(&mut self, mut tx: EditTransaction) -> Result<()> {
        tx.edits
            .sort_by_key(|edit| (edit.range.start, edit.range.end));
        for edit in &tx.edits {
            if edit.range.start > edit.range.end {
                return Err(BufferError::InvalidPosition);
            }
            self.byte_to_char(edit.range.start)?;
            self.byte_to_char(edit.range.end)?;
        }
        for pair in tx.edits.windows(2) {
            if let [left, right] = pair {
                let overlaps = left.range.end > right.range.start;
                let same_start = left.range.start == right.range.start;
                if overlaps || same_start {
                    return Err(BufferError::OverlappingEdits);
                }
            }
        }
        tx.edits
            .retain(|edit| self.text.byte_slice(edit.range.clone()) != edit.text.as_str());
        if tx.edits.is_empty() {
            return Ok(());
        }
        let revision = self.next_revision()?;
        let before = self.history_state();
        let mut staged = Self::load_from_str(self.id, "");
        staged.text = self.text.clone();
        let mut inverse = Vec::with_capacity(tx.edits.len());
        let mut offsets: Vec<_> = self
            .cursors
            .as_slice()
            .iter()
            .map(|cursor| (cursor.anchor.byte_offset, cursor.head.byte_offset))
            .collect();
        tx.edits.reverse();
        for edit in &tx.edits {
            let start = staged.byte_to_char(edit.range.start)?;
            let end = staged.byte_to_char(edit.range.end)?;
            inverse.push(TextEdit {
                range: edit.range.start..edit.range.start + edit.text.len(),
                text: staged.text.slice(start..end).to_string(),
            });
            staged.text.remove(start..end);
            staged.text.insert(start, &edit.text);
            for (anchor, head) in &mut offsets {
                *anchor = cursor::remap(*anchor, edit);
                *head = cursor::remap(*head, edit);
            }
        }
        inverse.reverse();
        let cursors = offsets
            .into_iter()
            .map(|(anchor, head)| {
                Ok(Cursor {
                    anchor: staged.position_at(anchor)?,
                    head: staged.position_at(head)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let cursors = CursorSet::new(&staged, cursors)?;
        self.text = staged.text;
        self.cursors = cursors;
        self.revision = revision;
        self.state = revision;
        let after = self.history_state();
        self.history
            .record(before, after, tx.edits.clone(), inverse, tx.group);
        self.publish_modified(tx.edits);
        Ok(())
    }

    /// Undoes a whole transaction or explicit typing group.
    /// # Errors
    /// Returns an error for empty history or revision exhaustion.
    pub fn undo(&mut self) -> Result<()> {
        let mut history = std::mem::take(&mut self.history);
        let result = history.undo(self);
        self.history = history;
        result
    }

    /// Reapplies the last undone group.
    /// # Errors
    /// Returns an error for empty history or revision exhaustion.
    pub fn redo(&mut self) -> Result<()> {
        let mut history = std::mem::take(&mut self.history);
        let result = history.redo(self);
        self.history = history;
        result
    }

    /// Detaches history for explicit `History::undo/redo(&mut buffer)` ownership.
    /// Editing afterward starts a new history and invalidates detached entries.
    #[must_use]
    pub fn take_history(&mut self) -> History {
        std::mem::take(&mut self.history)
    }

    fn next_revision(&self) -> Result<u64> {
        self.revision
            .checked_add(1)
            .ok_or(BufferError::RevisionOverflow)
    }

    fn history_state(&self) -> history::State {
        history::State {
            text: self.text.clone(),
            cursors: self.cursors.clone(),
            identity: self.state,
            owner: self.owner.clone(),
        }
    }

    fn publish_modified(&self, edits: Vec<TextEdit>) {
        if let Some(bus) = &self.bus {
            bus.publish(Event::Buffer(BufferEvent::Modified {
                buffer_id: self.id,
                revision: self.revision,
                edits,
            }));
        }
        self.publish_cursors();
    }

    fn publish_cursors(&self) {
        if let Some(bus) = &self.bus {
            bus.publish(Event::Buffer(BufferEvent::CursorChanged {
                buffer_id: self.id,
                revision: self.revision,
                cursors: self.cursors.as_slice().to_vec(),
            }));
        }
    }
}

#[cfg(test)]
mod tests;
