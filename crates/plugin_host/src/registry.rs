//! Thread-safe action registry and context predicates, independent of egui.
use crate::{Error, Request, Result};
use nozhik_event_bus::{Event, EventBus};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};
use tokio::sync::mpsc;

/// Stable global action identity.
pub type ActionId = String;

/// Context snapshot supplied by the UI at dispatch time.
#[derive(Debug, Clone, Copy, Default)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "Context keys are independent facts supplied by the UI"
)]
pub struct ActionContext {
    /// An editable document is open.
    pub editor_open: bool,
    /// Keyboard focus belongs to the editor.
    pub editor_focus: bool,
    /// A project tree file is selected.
    pub selected_file: bool,
    /// A project is loaded.
    pub project_open: bool,
}

/// Empty string means always; supported keys are joined with `&&` and optional `!`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct When(pub String);

impl When {
    /// Rejects unsupported syntax instead of silently enabling an action.
    /// # Errors
    /// Returns an invalid predicate error.
    pub fn validate(&self) -> Result<()> {
        if self.0.trim().is_empty() {
            return Ok(());
        }
        for term in self.0.split("&&") {
            if !matches!(
                term.trim().trim_start_matches('!'),
                "editorOpen" | "editorFocus" | "selectedFile" | "projectOpen"
            ) || term.trim().starts_with("!!")
            {
                return Err(Error::Invalid(format!(
                    "unsupported when expression: {}",
                    self.0
                )));
            }
        }
        Ok(())
    }

    /// Evaluates the validated predicate; malformed predicates fail closed.
    #[must_use]
    pub fn matches(&self, context: ActionContext) -> bool {
        if self.validate().is_err() {
            return false;
        }
        self.0.trim().is_empty()
            || self.0.split("&&").all(|term| {
                let term = term.trim();
                let value = match term.trim_start_matches('!') {
                    "editorOpen" => context.editor_open,
                    "editorFocus" => context.editor_focus,
                    "selectedFile" => context.selected_file,
                    "projectOpen" => context.project_open,
                    _ => false,
                };
                value != term.starts_with('!')
            })
    }
}

/// Searchable action descriptor; handlers are bus events or queued plugin calls.
#[derive(Debug, Clone)]
pub struct Action {
    /// Unique ID.
    pub id: ActionId,
    /// Search/display label.
    pub title: String,
    /// Availability predicate.
    pub when: When,
    /// Owning extension; `None` identifies a built-in action.
    pub owner: Option<String>,
}

/// Shared registry; snapshots release locks before sending events or plugin requests.
#[derive(Clone)]
pub struct ActionRegistry {
    actions: Arc<RwLock<BTreeMap<ActionId, Action>>>,
    bus: EventBus,
    plugin_requests: mpsc::Sender<Request>,
}

impl ActionRegistry {
    pub(crate) fn new(bus: EventBus, plugin_requests: mpsc::Sender<Request>) -> Self {
        Self {
            actions: Arc::default(),
            bus,
            plugin_requests,
        }
    }

    /// Registers one action, rejecting duplicates and malformed predicates.
    /// # Errors
    /// Returns a duplicate, invalid predicate or poisoned-lock error.
    pub fn register(&self, action: Action) -> Result<()> {
        self.register_batch(vec![action])
    }

    pub(crate) fn register_batch(&self, actions: Vec<Action>) -> Result<()> {
        let mut current = self.actions.write().map_err(|_| Error::Poisoned)?;
        let mut ids = std::collections::HashSet::new();
        for action in &actions {
            action.when.validate()?;
            if !crate::manifest::identifier(&action.id) || action.title.trim().is_empty() {
                return Err(Error::Invalid("invalid action identity or title".into()));
            }
            if current.contains_key(&action.id) || !ids.insert(&action.id) {
                return Err(Error::Invalid(format!("duplicate action: {}", action.id)));
            }
        }
        current.extend(
            actions
                .into_iter()
                .map(|action| (action.id.clone(), action)),
        );
        drop(current);
        Ok(())
    }

    /// Removes every action belonging to one extension.
    /// # Errors
    /// Returns a poisoned-lock error.
    pub fn unregister_owner(&self, owner: &str) -> Result<()> {
        self.actions
            .write()
            .map_err(|_| Error::Poisoned)?
            .retain(|_, a| a.owner.as_deref() != Some(owner));
        Ok(())
    }

    /// Returns actions in stable ID order, filtered by context.
    /// # Errors
    /// Returns a poisoned-lock error.
    pub fn available(&self, context: ActionContext) -> Result<Vec<Action>> {
        Ok(self
            .actions
            .read()
            .map_err(|_| Error::Poisoned)?
            .values()
            .filter(|a| a.when.matches(context))
            .cloned()
            .collect())
    }

    /// Queues a plugin command or emits a built-in action notification without waiting.
    /// # Errors
    /// Returns unknown/unavailable action, full/stopped worker, or lock errors.
    pub fn execute(&self, id: &str, context: ActionContext) -> Result<()> {
        let action = self
            .actions
            .read()
            .map_err(|_| Error::Poisoned)?
            .get(id)
            .cloned()
            .ok_or_else(|| Error::Invalid(format!("unknown action: {id}")))?;
        if !action.when.matches(context) {
            return Err(Error::Invalid("action unavailable in this context".into()));
        }
        if let Some(owner) = action.owner {
            self.plugin_requests
                .try_send(Request::Execute {
                    owner,
                    command: id.into(),
                })
                .map_err(|_| Error::Queue)?;
        }
        self.bus.publish(Event::ActionTriggered(id.into()));
        Ok(())
    }
}
