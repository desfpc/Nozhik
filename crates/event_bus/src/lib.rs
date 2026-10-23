//! Bounded, asynchronous pub/sub. Events are notifications, not durable commands.

use std::{path::PathBuf, sync::Arc};

use nozhik_core::{BufferId, ConfigKey, Selection, Span, TextEdit};
use thiserror::Error;
use tokio::sync::broadcast;

/// System notifications shared by independent subsystems.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Committed text engine notification.
    Buffer(BufferEvent),
    /// A file has been opened successfully.
    FileOpened(PathBuf),
    /// A committed edit: replace `range` in the pre-edit buffer with `text`.
    BufferModified {
        /// Buffer affected by this edit.
        id: BufferId,
        /// Range in the pre-edit buffer.
        range: Span,
        /// Replacement text; empty text means deletion.
        text: String,
    },
    /// Settings invalidation; obtain a fresh snapshot from the manager.
    ConfigChanged(ConfigKey),
    /// An action was triggered; delivery and execution are not guaranteed.
    ActionTriggered(String),
}

/// Buffer notifications. On lag, fetch an authoritative buffer snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BufferEvent {
    /// Atomic commit; edits are replayed sequentially in the listed order.
    Modified {
        /// Affected buffer.
        buffer_id: BufferId,
        /// Monotonically increasing revision after this commit.
        revision: u64,
        /// Replacements in application order, including undo/redo.
        edits: Vec<TextEdit>,
    },
    /// Canonical selections after movement or editing.
    CursorChanged {
        /// Affected buffer.
        buffer_id: BufferId,
        /// Text revision these positions refer to.
        revision: u64,
        /// Sorted, merged selections.
        cursors: Vec<Selection>,
    },
}

/// Errors constructing an event bus.
#[derive(Debug, Error)]
pub enum BusError {
    /// Capacity is zero or exceeds Tokio's supported bound.
    #[error("event capacity must be in 1..=usize::MAX / 2")]
    InvalidCapacity,
}

/// Cheaply cloned producer handle. Every subscription receives its own stream.
#[derive(Debug, Clone)]
pub struct EventBus {
    sender: broadcast::Sender<Arc<Event>>,
}

impl EventBus {
    /// Creates a bounded bus without requiring a running Tokio runtime.
    ///
    /// # Errors
    /// Returns [`BusError::InvalidCapacity`] for an unsupported capacity.
    pub fn new(capacity: usize) -> Result<Self, BusError> {
        if capacity == 0 || capacity > usize::MAX / 2 {
            return Err(BusError::InvalidCapacity);
        }
        let (sender, _) = broadcast::channel(capacity);
        Ok(Self { sender })
    }

    /// Subscribes to future events. There is no historical replay.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<Event>> {
        self.sender.subscribe()
    }

    /// Publishes without waiting; returns the number of current subscribers.
    ///
    /// Zero subscribers is normal. A positive count does not acknowledge receipt.
    #[allow(
        clippy::must_use_candidate,
        reason = "Publishing is the effect; subscriber count is optional telemetry"
    )]
    pub fn publish(&self, event: Event) -> usize {
        self.sender.send(Arc::new(event)).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::{Event, EventBus};
    use std::{error::Error, sync::Arc};
    use tokio::sync::broadcast::error::RecvError;

    #[tokio::test]
    async fn publishes_to_every_subscriber() -> Result<(), Box<dyn Error>> {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<EventBus>();
        assert_send_sync::<Event>();
        let bus = EventBus::new(8)?;
        let mut first = bus.subscribe();
        let mut second = bus.subscribe();
        let producer = bus.clone();
        let delivered = std::thread::spawn(move || {
            producer.publish(Event::ActionTriggered("app.start".into()))
        });
        assert_eq!(delivered.join().map_err(|_| "producer panicked")?, 2);
        let event = first.recv().await?;
        let other = second.recv().await?;
        assert_eq!(*event, Event::ActionTriggered("app.start".into()));
        assert!(Arc::ptr_eq(&event, &other));
        Ok(())
    }

    #[tokio::test]
    async fn reports_lag_and_shutdown() -> Result<(), Box<dyn Error>> {
        let bus = EventBus::new(1)?;
        assert_eq!(bus.publish(Event::ActionTriggered("unobserved".into())), 0);
        let mut receiver = bus.subscribe();
        bus.publish(Event::ActionTriggered("old".into()));
        bus.publish(Event::ActionTriggered("new".into()));
        assert!(matches!(receiver.recv().await, Err(RecvError::Lagged(1))));
        assert_eq!(
            *receiver.recv().await?,
            Event::ActionTriggered("new".into())
        );
        drop(bus);
        assert!(matches!(receiver.recv().await, Err(RecvError::Closed)));
        assert!(EventBus::new(0).is_err());
        assert!(EventBus::new(usize::MAX).is_err());
        Ok(())
    }
}
