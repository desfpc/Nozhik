use crate::{BufferError, CursorSet, Result, TextBuffer, TextEdit};
use ropey::Rope;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct State {
    pub text: Rope,
    pub cursors: CursorSet,
    pub identity: u64,
    pub owner: Arc<()>,
}

#[derive(Debug)]
struct Entry {
    before: State,
    after: State,
    forward: Vec<TextEdit>,
    inverse: Vec<Vec<TextEdit>>,
    group: Option<u64>,
}

/// Transactional undo/redo with copy-on-write rope snapshots and cursor restoration.
/// Grouping is explicit, with no wall-clock dependency. A new branch discards redo.
#[derive(Debug, Default)]
pub struct History {
    undo: Vec<Entry>,
    redo: Vec<Entry>,
    group_open: bool,
}

impl History {
    /// Ends the active typing group.
    pub const fn break_group(&mut self) {
        self.group_open = false;
    }

    pub(super) fn record(
        &mut self,
        before: State,
        after: State,
        forward: Vec<TextEdit>,
        inverse: Vec<TextEdit>,
        group: Option<u64>,
    ) {
        self.redo.clear();
        if self.group_open
            && group.is_some()
            && let Some(previous) = self.undo.last_mut()
            && previous.group == group
            && previous.after.identity == before.identity
        {
            previous.after = after;
            previous.forward.extend(forward);
            previous.inverse.push(inverse);
            return;
        }
        self.undo.push(Entry {
            before,
            after,
            forward,
            inverse: vec![inverse],
            group,
        });
        self.group_open = group.is_some();
    }

    /// Restores a complete group and its original cursors, publishing one commit.
    /// # Errors
    /// Rejects empty history, a different buffer/state and revision exhaustion.
    pub fn undo(&mut self, buffer: &mut TextBuffer) -> Result<()> {
        let entry = self.undo.last().ok_or(BufferError::EmptyHistory)?;
        let inverse = entry.inverse.iter().rev().flatten().cloned().collect();
        Self::restore(buffer, &entry.after, &entry.before, inverse)?;
        if let Some(entry) = self.undo.pop() {
            self.redo.push(entry);
        }
        self.break_group();
        Ok(())
    }

    /// Reapplies a complete group and its resulting cursors.
    /// # Errors
    /// Rejects empty history, a different buffer/state and revision exhaustion.
    pub fn redo(&mut self, buffer: &mut TextBuffer) -> Result<()> {
        let entry = self.redo.last().ok_or(BufferError::EmptyHistory)?;
        Self::restore(buffer, &entry.before, &entry.after, entry.forward.clone())?;
        if let Some(entry) = self.redo.pop() {
            self.undo.push(entry);
        }
        self.break_group();
        Ok(())
    }

    fn restore(
        buffer: &mut TextBuffer,
        expected: &State,
        next: &State,
        edits: Vec<TextEdit>,
    ) -> Result<()> {
        if !Arc::ptr_eq(&buffer.owner, &expected.owner) || buffer.state != expected.identity {
            return Err(BufferError::HistoryMismatch);
        }
        let revision = buffer.next_revision()?;
        buffer.text = next.text.clone();
        buffer.cursors = next.cursors.clone();
        buffer.state = next.identity;
        buffer.revision = revision;
        buffer.publish_modified(edits);
        Ok(())
    }
}
