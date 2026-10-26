//! Multi-session ownership and lifecycle events without UI dependencies.
use crate::{
    Result, TerminalError, TerminalEvent, TerminalSession, TerminalSessionId, Wakeup,
    pty::SessionOptions,
};
use std::{
    collections::BTreeMap,
    sync::mpsc,
    time::{Duration, Instant},
};

/// Owns parallel sessions; removing a handle starts background cleanup.
pub struct TerminalManager {
    sessions: BTreeMap<TerminalSessionId, TerminalSession>,
    next: u64,
    wake: Wakeup,
    events: mpsc::Receiver<TerminalEvent>,
    sender: mpsc::Sender<TerminalEvent>,
}
impl TerminalManager {
    /// Creates an empty manager. No shell is spawned until create is called.
    #[must_use]
    pub fn new(wake: Wakeup) -> Self {
        let (sender, events) = mpsc::channel();
        Self {
            sessions: BTreeMap::new(),
            next: 1,
            wake,
            events,
            sender,
        }
    }
    /// Allocates a fresh ID and schedules shell creation (up to 16 parallel tabs).
    /// # Errors
    /// Returns session limit or exhausted ID errors; spawn failures arrive as events.
    pub fn create(&mut self, options: SessionOptions) -> Result<TerminalSessionId> {
        if self.sessions.len() >= 16 {
            return Err(TerminalError::Invalid(
                "at most 16 terminal tabs may be open".into(),
            ));
        }
        let next = self
            .next
            .checked_add(1)
            .ok_or_else(|| TerminalError::Invalid("terminal IDs exhausted".into()))?;
        let id = TerminalSessionId(self.next);
        self.next = next;
        self.sessions.insert(
            id,
            TerminalSession::start(id, options, self.wake.clone(), self.sender.clone()),
        );
        Ok(id)
    }
    /// Session lookup.
    #[must_use]
    pub fn session(&self, id: TerminalSessionId) -> Option<&TerminalSession> {
        self.sessions.get(&id)
    }
    /// Current sessions in creation order.
    pub fn sessions(&self) -> impl Iterator<Item = &TerminalSession> {
        self.sessions.values()
    }
    /// Removes and signals a tab. Returned handle can be awaited during tests/exit.
    #[must_use]
    pub fn remove(&mut self, id: TerminalSessionId) -> Option<TerminalSession> {
        let session = self.sessions.remove(&id)?;
        session.close();
        Some(session)
    }
    /// Polls up to 128 lifecycle events per frame. The UI decides when to remove tabs.
    pub fn drain(&self) -> impl Iterator<Item = TerminalEvent> + '_ {
        self.events.try_iter().take(128)
    }
    /// Closes all sessions and waits at most three seconds in total, for IDE exit.
    pub fn shutdown(&mut self) {
        for session in self.sessions.values() {
            session.close();
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        for session in std::mem::take(&mut self.sessions).into_values() {
            let _closed = session.wait_closed(deadline.saturating_duration_since(Instant::now()));
        }
    }
}
