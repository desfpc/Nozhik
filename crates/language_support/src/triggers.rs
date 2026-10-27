//! Declarative language completion activation.
use serde::{Deserialize, Serialize};

/// Language-owned trigger strings, independent of editor input handling.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LanguageTriggerConfig {
    /// Stable language identifier.
    pub language_id: String,
    /// Instance access operators.
    pub member_access_triggers: Vec<String>,
    /// Static access operators.
    pub static_access_triggers: Vec<String>,
    /// Characters activating identifier completion.
    pub auto_popup_triggers: Vec<char>,
}
impl LanguageTriggerConfig {
    /// Built-in PHP preset. A dot is concatenation, never member access.
    #[must_use]
    pub fn php() -> Self {
        Self {
            language_id: "php".into(),
            member_access_triggers: vec!["->".into()],
            static_access_triggers: vec!["::".into()],
            auto_popup_triggers: vec!['$', '\\', '>'],
        }
    }
    /// Whether newly inserted text warrants a completion query.
    #[must_use]
    pub fn activates(&self, before: &str) -> bool {
        self.member_access_triggers
            .iter()
            .chain(&self.static_access_triggers)
            .any(|s| before.ends_with(s))
            || before.chars().last().is_some_and(|c| {
                c.is_alphanumeric() || c == '_' || self.auto_popup_triggers.contains(&c)
            })
    }
    /// Separates the receiver and unfinished member, if an operator precedes it.
    #[must_use]
    pub fn access<'a>(&self, before: &'a str) -> Option<(&'a str, &'a str, bool)> {
        let start = before
            .rfind(|c: char| !c.is_alphanumeric() && c != '_' && c != '$')
            .map_or(0, |i| i + 1);
        let prefix = before.get(start..)?;
        let receiver = before.get(..start)?.trim_end();
        for (operators, static_access) in [
            (&self.member_access_triggers, false),
            (&self.static_access_triggers, true),
        ] {
            for operator in operators {
                if let Some(left) = receiver.strip_suffix(operator) {
                    return Some((left.trim_end(), prefix, static_access));
                }
            }
        }
        None
    }
}

impl Default for LanguageTriggerConfig {
    /// PHP is the currently enabled semantic language; other languages supply their own config.
    fn default() -> Self {
        Self::php()
    }
}
