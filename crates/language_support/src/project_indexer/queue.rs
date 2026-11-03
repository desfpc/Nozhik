//! Coalescing mailboxes: retained work scales with files, never with keystrokes or frames.
use super::{IndexCommand, IndexEvent};
use std::sync::{Condvar, Mutex, PoisonError};

#[derive(Default)]
struct Pending {
    commands: Vec<IndexCommand>,
    closed: bool,
}
#[derive(Default)]
pub(super) struct Commands {
    pending: Mutex<Pending>,
    ready: Condvar,
}
impl Commands {
    pub(super) fn send(&self, command: IndexCommand) -> bool {
        let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        if pending.closed {
            return false;
        }
        match &command {
            IndexCommand::Changed(path) => {
                if pending.commands.iter().any(|c| {
                    matches!(c, IndexCommand::Reconcile | IndexCommand::Invalidate)
                        || matches!(c, IndexCommand::Changed(p) if p == path)
                }) {
                    return true;
                }
                if pending
                    .commands
                    .iter()
                    .filter(|c| matches!(c, IndexCommand::Changed(_)))
                    .count()
                    >= 256
                {
                    pending
                        .commands
                        .retain(|c| !matches!(c, IndexCommand::Changed(_)));
                    pending.commands.push(IndexCommand::Reconcile);
                } else {
                    pending.commands.push(command);
                }
            }
            IndexCommand::Overlay { path, .. } | IndexCommand::Close(path) => {
                pending.commands.retain(|c| {
                    !matches!(c,
                    IndexCommand::Overlay { path: p, .. } | IndexCommand::Close(p) if p == path)
                });
                pending.commands.push(command);
            }
            IndexCommand::Reconcile | IndexCommand::Invalidate => {
                pending
                    .commands
                    .retain(|c| !matches!(c, IndexCommand::Changed(_) | IndexCommand::Reconcile));
                if !pending
                    .commands
                    .iter()
                    .any(|c| matches!(c, IndexCommand::Invalidate))
                {
                    pending.commands.push(command);
                }
            }
        }
        drop(pending);
        self.ready.notify_one();
        true
    }
    pub(super) fn receive(&self) -> Vec<IndexCommand> {
        let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        while pending.commands.is_empty() && !pending.closed {
            pending = self
                .ready
                .wait(pending)
                .unwrap_or_else(PoisonError::into_inner);
        }
        std::mem::take(&mut pending.commands)
    }
    pub(super) fn close(&self) {
        let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        pending.closed = true;
        pending.commands.clear();
        drop(pending);
        self.ready.notify_one();
    }
}
#[derive(Default)]
pub(super) struct Events(Mutex<Vec<IndexEvent>>);
impl Events {
    pub(super) fn send(&self, event: IndexEvent) {
        let mut events = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        events.retain(|old| {
            std::mem::discriminant(old) != std::mem::discriminant(&event)
                && !(matches!(old, IndexEvent::Progress(..) | IndexEvent::Finished)
                    && matches!(event, IndexEvent::Progress(..) | IndexEvent::Finished))
        });
        events.push(event);
    }
    pub(super) fn drain(&self) -> std::vec::IntoIter<IndexEvent> {
        std::mem::take(&mut *self.0.lock().unwrap_or_else(PoisonError::into_inner)).into_iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn latest_overlay_close_and_disk_reconciliation_are_retained() {
        let queue = Commands::default();
        let path = PathBuf::from("a.php");
        for revision in 0..1000 {
            assert!(queue.send(IndexCommand::Overlay {
                path: path.clone(),
                revision,
                text: ropey::Rope::from_str("<?php"),
            }));
            assert!(queue.send(IndexCommand::Changed(path.clone())));
        }
        let batch = queue.receive();
        assert_eq!(batch.len(), 2);
        assert!(
            batch
                .iter()
                .any(|c| matches!(c, IndexCommand::Overlay { revision: 999, .. }))
        );
        for i in 0..1000 {
            assert!(queue.send(IndexCommand::Changed(PathBuf::from(format!("{i}.php")))));
        }
        let batch = queue.receive();
        assert_eq!(batch.len(), 1);
        assert!(matches!(batch.first(), Some(IndexCommand::Reconcile)));
        assert!(queue.send(IndexCommand::Overlay {
            path: path.clone(),
            revision: 1000,
            text: ropey::Rope::new()
        }));
        assert!(queue.send(IndexCommand::Close(path)));
        assert!(matches!(
            queue.receive().as_slice(),
            [IndexCommand::Close(_)]
        ));
        queue.close();
        assert!(queue.receive().is_empty());
        assert!(!queue.send(IndexCommand::Reconcile));
    }
    #[test]
    fn snapshots_and_progress_do_not_accumulate() {
        let queue = Events::default();
        for i in 0..1000 {
            queue.send(IndexEvent::Progress(i, 1000));
            queue.send(IndexEvent::Error(i.to_string()));
        }
        queue.send(IndexEvent::Finished);
        let events: Vec<_> = queue.drain().collect();
        assert_eq!(events.len(), 2);
        assert!(matches!(events.last(), Some(IndexEvent::Finished)));
        assert!(matches!(events.first(), Some(IndexEvent::Error(e)) if e == "999"));
        let index = std::sync::Arc::new(crate::symbol_index::SymbolIndex::new(
            super::super::LanguageId::Php,
        ));
        let old = std::sync::Arc::downgrade(&index);
        queue.send(IndexEvent::Snapshot(
            index,
            std::collections::HashMap::new(),
        ));
        for _ in 0..1000 {
            queue.send(IndexEvent::Snapshot(
                std::sync::Arc::new(crate::symbol_index::SymbolIndex::new(
                    super::super::LanguageId::Php,
                )),
                std::collections::HashMap::new(),
            ));
        }
        assert!(
            old.upgrade().is_none(),
            "obsolete snapshot must be freed before the UI drains"
        );
        assert_eq!(queue.drain().count(), 1);
    }
}
