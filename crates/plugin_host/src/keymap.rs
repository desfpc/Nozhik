//! Atomically replaceable keymap profiles with deterministic conflict handling.
use crate::{
    Error, Result,
    registry::{ActionContext, ActionId, ActionRegistry, When},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};

/// Portable logical key chord. Key names follow egui (`S`, `F1`, `Enter`, …).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "Keyboard modifiers are independent flags in the serialized contract"
)]
pub struct KeyChord {
    /// Logical key name.
    pub key: String,
    /// Portable Ctrl on Windows/Linux, Cmd on macOS.
    pub primary: bool,
    /// Explicit Control modifier, independent of platform.
    pub ctrl: bool,
    /// Explicit Command modifier, independent of platform.
    pub cmd: bool,
    /// Alt/Option.
    pub alt: bool,
    /// Shift.
    pub shift: bool,
}
impl Default for KeyChord {
    fn default() -> Self {
        Self {
            key: "S".into(),
            primary: false,
            ctrl: false,
            cmd: false,
            alt: false,
            shift: false,
        }
    }
}
impl KeyChord {
    fn normalized(&self) -> Self {
        let mut chord = self.clone();
        if chord.primary {
            if cfg!(target_os = "macos") {
                chord.cmd = true;
            } else {
                chord.ctrl = true;
            }
            chord.primary = false;
        }
        chord
    }

    /// Label using portable modifier names.
    #[must_use]
    pub fn label(&self) -> String {
        let mut parts = Vec::new();
        for (enabled, name) in [
            (self.primary, "Ctrl/Cmd"),
            (self.ctrl, "Ctrl"),
            (self.cmd, "Cmd"),
            (self.alt, "Alt"),
            (self.shift, "Shift"),
        ] {
            if enabled {
                parts.push(name);
            }
        }
        parts.push(&self.key);
        parts.join("+")
    }
}

/// Keymap source profile.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeymapProfile {
    /// JetBrains-compatible action search and navigation.
    #[default]
    JetBrains,
    /// VS Code command palette.
    #[serde(rename = "VSCode")]
    VsCode,
    /// Only explicitly configured bindings.
    Custom,
}

/// One conditional key binding.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyBinding {
    /// Keyboard chord.
    pub chord: KeyChord,
    /// Registered target action.
    pub action: ActionId,
    /// Additional availability; evaluated alongside the action predicate.
    #[serde(default)]
    pub when: When,
}

/// Hot-reloaded JSON configuration; custom bindings override profile chords.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct KeymapConfig {
    /// Built-in baseline.
    pub profile: KeymapProfile,
    /// Ordered overrides; last binding of an identical chord wins.
    pub bindings: Vec<KeyBinding>,
}

/// Shared keymap snapshot; reload validates before replacing all bindings.
#[derive(Clone, Default)]
pub struct KeymapRegistry(Arc<RwLock<BTreeMap<KeyChord, KeyBinding>>>);
impl KeymapRegistry {
    /// Replaces the current profile atomically. Returns overridden chord labels.
    /// # Errors
    /// Returns invalid binding/predicate or poisoned-lock errors; keeps old bindings.
    pub fn replace(&self, config: KeymapConfig) -> Result<Vec<String>> {
        let mut bindings = BTreeMap::new();
        let mut conflicts = Vec::new();
        for binding in defaults(config.profile).into_iter().chain(config.bindings) {
            binding.when.validate()?;
            if !valid_key(&binding.chord.key)
                || !crate::manifest::identifier(&binding.action)
                || (binding.chord.primary && (binding.chord.ctrl || binding.chord.cmd))
            {
                return Err(Error::Invalid("invalid chord or action".into()));
            }
            if bindings
                .insert(binding.chord.normalized(), binding.clone())
                .is_some()
            {
                conflicts.push(binding.chord.label());
            }
        }
        *self.0.write().map_err(|_| Error::Poisoned)? = bindings;
        Ok(conflicts)
    }

    /// Consistent copy for UI matching and shortcut labels.
    /// # Errors
    /// Returns a poisoned-lock error.
    pub fn bindings(&self) -> Result<Vec<KeyBinding>> {
        Ok(self
            .0
            .read()
            .map_err(|_| Error::Poisoned)?
            .values()
            .cloned()
            .collect())
    }

    /// Resolves and dispatches an emulated keypress through the action registry.
    /// # Errors
    /// Returns unavailable action, queue or lock errors. Unbound chords return false.
    pub fn dispatch(
        &self,
        chord: &KeyChord,
        context: ActionContext,
        actions: &ActionRegistry,
    ) -> Result<bool> {
        let binding = self
            .0
            .read()
            .map_err(|_| Error::Poisoned)?
            .get(&chord.normalized())
            .cloned();
        if let Some(binding) = binding.filter(|b| b.when.matches(context)) {
            actions.execute(&binding.action, context)?;
            return Ok(true);
        }
        Ok(false)
    }
}

fn valid_key(key: &str) -> bool {
    (key.len() == 1
        && key
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit()))
        || matches!(
            key,
            "Enter"
                | "Comma"
                | "Escape"
                | "Space"
                | "Tab"
                | "Backspace"
                | "Delete"
                | "Insert"
                | "Home"
                | "End"
                | "PageUp"
                | "PageDown"
                | "ArrowUp"
                | "ArrowDown"
                | "ArrowLeft"
                | "ArrowRight"
        )
        || key
            .strip_prefix('F')
            .and_then(|v| v.parse::<u8>().ok())
            .is_some_and(|v| (1..=35).contains(&v))
}

fn defaults(profile: KeymapProfile) -> Vec<KeyBinding> {
    if profile == KeymapProfile::Custom {
        return Vec::new();
    }
    let mut entries = vec![
        ("S", false, "ui.save_file", "editorOpen"),
        ("O", false, "ui.open", ""),
        ("Q", false, "ui.exit", ""),
        ("C", false, "ui.copy", "editorFocus"),
        ("X", false, "ui.cut", "editorFocus"),
        ("V", false, "ui.paste", "editorFocus"),
        ("N", false, "ui.new_file", ""),
        ("S", true, "ui.save_all", ""),
        ("W", false, "ui.close_tab", "editorOpen"),
        ("Comma", false, "ui.settings", ""),
        ("Z", false, "ui.undo", "editorFocus"),
        ("Z", true, "ui.redo", "editorFocus"),
        ("F", false, "ui.find", "editorOpen"),
        ("R", false, "ui.replace", "editorOpen"),
        ("K", false, "ui.commit", "projectOpen"),
        ("O", true, "ui.go_file", ""),
        ("E", false, "ui.recent_files", ""),
        ("G", true, "ui.git", ""),
    ];
    if profile == KeymapProfile::JetBrains {
        entries.extend([("A", true, "ui.search_everywhere", "")]);
    } else {
        entries.push(("P", true, "ui.search_everywhere", ""));
    }
    let mut bindings: Vec<_> = entries
        .into_iter()
        .map(|(key, shift, action, when)| KeyBinding {
            chord: KeyChord {
                key: key.into(),
                primary: true,
                shift,
                ..KeyChord::default()
            },
            action: action.into(),
            when: When(when.into()),
        })
        .collect();
    bindings.push(KeyBinding {
        chord: KeyChord {
            key: "F12".into(),
            alt: true,
            ..KeyChord::default()
        },
        action: "ui.new_terminal".into(),
        when: When::default(),
    });
    bindings
}
