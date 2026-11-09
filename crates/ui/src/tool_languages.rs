//! Language switch shared by linter and test settings.
//!
//! The choices are the languages enabled for this project. Each page then shows
//! only the runners implemented for that language.

use nozhik_config::{Config, languages::LanguageId};

/// Languages selected in Settings → Languages, in catalog order.
#[must_use]
pub fn enabled(config: &Config) -> Vec<LanguageId> {
    nozhik_config::languages::LANGUAGES
        .iter()
        .filter(|spec| config.enabled_languages.contains(&spec.id))
        .map(|spec| spec.id)
        .collect()
}

/// Draws the language switch. Returns false when the project has none enabled.
pub fn switch(ui: &mut egui::Ui, config: &Config, selected: &mut LanguageId) -> bool {
    let enabled = enabled(config);
    let Some(fallback) = enabled.first().copied() else {
        return false;
    };
    if !enabled.contains(selected) {
        *selected = fallback;
    }
    ui.horizontal_wrapped(|ui| {
        for id in enabled {
            ui.selectable_value(selected, id, label(id));
        }
    });
    ui.separator();
    true
}

fn label(id: LanguageId) -> &'static str {
    nozhik_config::languages::LANGUAGES
        .iter()
        .find(|spec| spec.id == id)
        .map_or("Language", |spec| spec.label)
}

#[cfg(test)]
mod tests {
    use super::enabled;
    use nozhik_config::{Config, languages::LanguageId};

    #[test]
    fn the_switch_follows_languages_enabled_for_the_project() {
        let config = Config::default();
        let ids = enabled(&config);
        assert!(ids.contains(&LanguageId::Php));
        assert!(ids.contains(&LanguageId::Css));
        assert!(!ids.contains(&LanguageId::Go));
        let php_only = Config {
            enabled_languages: std::iter::once(LanguageId::Php).collect(),
            ..Config::default()
        };
        assert_eq!(enabled(&php_only), vec![LanguageId::Php]);
    }
}
