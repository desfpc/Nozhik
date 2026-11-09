//! Markdown documents open as source, with a session Text/GUI switch.
mod view;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Text,
    Gui,
}

/// Per-path preview mode. The buffer stays the source in both modes.
#[derive(Default)]
pub struct MarkdownUi {
    modes: HashMap<PathBuf, Mode>,
}

fn is_markdown_path(path: &Path) -> bool {
    nozhik_config::languages::language_for_path(path)
        == Some(nozhik_config::languages::LanguageId::Markdown)
}

impl crate::NozhikUi {
    /// Draws the Text/GUI switch for a Markdown document.
    ///
    /// Returns whether the rendered view replaced the text editor for this frame.
    pub(crate) fn markdown_toolbar(
        &mut self,
        ui: &mut egui::Ui,
        index: usize,
        enabled: bool,
    ) -> bool {
        let path = self.documents[index].path.clone();
        if !is_markdown_path(&path) {
            return false;
        }
        let gui = self.markdown.modes.get(&path) == Some(&Mode::Gui);
        ui.horizontal(|ui| {
            let mode = self
                .markdown
                .modes
                .entry(path.clone())
                .or_insert(Mode::Text);
            ui.selectable_value(mode, Mode::Text, "Text");
            ui.selectable_value(mode, Mode::Gui, "GUI");
        });
        if !gui {
            return false;
        }
        let text = self.documents[index].buffer.rope().to_string();
        view::show(ui, &path, &text, enabled);
        true
    }
}

#[cfg(test)]
mod tests {
    use crate::{Document, NozhikUi};
    use egui::{Event, PointerButton, Pos2, Rect, vec2};

    const FIXTURE: &str = "# Схема\n\nОписание **ключевых** полей, *имён* и `индексов`.\n\n## 0) Источник\n\n| Приоритет | Источник |\n| --- | --- |\n| 1 | migrations |\n\n- назначение таблицы\n\n> только имя как в БД\n";

    #[test]
    fn text_highlights_formatting_and_gui_renders_it() -> Result<(), String> {
        let dir = tempfile::tempdir().map_err(|error| error.to_string())?;
        let path = dir.path().join("DATABASE.md");
        let bus = nozhik_event_bus::EventBus::new(32).map_err(|error| error.to_string())?;
        let settings =
            nozhik_config::ConfigManager::from_path(dir.path().join("settings.toml"), bus.clone())
                .map_err(|error| error.to_string())?;
        let mut shell = NozhikUi::new(bus, settings, nozhik_config::Config::default());
        shell.documents.push(Document {
            path: path.clone(),
            binary: false,
            name: "DATABASE.md".into(),
            open: true,
            buffer: nozhik_buffer::TextBuffer::load_from_str(nozhik_core::BufferId(9), FIXTURE),
        });
        shell.active = Some(0);
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while !shell.language.views.get(&path).is_some_and(|view| {
            view.spans.iter().any(|span| span.token == "markup.strong")
                && view
                    .spans
                    .iter()
                    .any(|span| span.token == "markup.emphasis")
                && view.spans.iter().any(|span| span.token == "markup.raw")
        }) && std::time::Instant::now() < deadline
        {
            shell.language_tick(&ctx);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let view = shell.language.views.get(&path).ok_or("Markdown view")?;
        assert!(view.spans.iter().any(|span| span.token == "markup.heading"));
        assert!(view.stubs.is_none());
        let source = frame(&mut shell, &ctx, Vec::new());
        assert!(galley(&source, "Text").is_some());
        assert!(galley(&source, "GUI").is_some());
        assert!(contains(&source, "**ключевых**"));
        assert!(
            source.shapes.iter().any(|shape| match &shape.shape {
                egui::Shape::Text(text) => text.galley.job.sections.iter().any(|section| {
                    section.format.color == crate::theme::syntax_color("markup.strong")
                }),
                _ => false,
            }),
            "strong source text keeps its highlight"
        );
        assert!(
            source.shapes.iter().any(|shape| match &shape.shape {
                egui::Shape::Text(text) => {
                    text.galley.text().contains("имён")
                        && text
                            .galley
                            .job
                            .sections
                            .iter()
                            .any(|section| section.format.italics)
                }
                _ => false,
            }),
            "emphasis is italic in the source"
        );
        assert!(
            source.shapes.iter().any(|shape| match &shape.shape {
                egui::Shape::Text(text) => text
                    .galley
                    .job
                    .sections
                    .iter()
                    .any(|section| section.format.background == crate::theme::chip()),
                _ => false,
            }),
            "inline code keeps a chip background"
        );
        let gui = galley(&source, "GUI").ok_or_else(|| "GUI switch is missing".to_string())?;
        let _ = frame(&mut shell, &ctx, click(gui.center()));
        let rendered = frame(&mut shell, &ctx, Vec::new());
        assert!(contains(&rendered, "Схема"));
        assert!(contains(&rendered, "ключевых"));
        assert!(contains(&rendered, "индексов"));
        assert!(contains(&rendered, "Приоритет"));
        assert!(contains(&rendered, "migrations"));
        assert!(contains(&rendered, "назначение таблицы"));
        assert!(contains(&rendered, "только имя как в БД"));
        assert!(!contains(&rendered, "**ключевых**"));
        assert!(!contains(&rendered, "| --- |"));
        Ok(())
    }

    fn frame(shell: &mut NozhikUi, ctx: &egui::Context, events: Vec<Event>) -> egui::FullOutput {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1100.0, 800.0))),
                events,
                ..Default::default()
            },
            |ctx| shell.editor_area(ctx),
        )
    }

    fn click(pos: Pos2) -> Vec<Event> {
        vec![
            Event::PointerMoved(pos),
            Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    }

    fn galley(output: &egui::FullOutput, label: &str) -> Option<Rect> {
        output.shapes.iter().find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == label => {
                Some(Rect::from_min_size(text.pos, text.galley.size()))
            }
            _ => None,
        })
    }

    fn contains(output: &egui::FullOutput, label: &str) -> bool {
        output.shapes.iter().any(|shape| match &shape.shape {
            egui::Shape::Text(text) => text.galley.text().contains(label),
            _ => false,
        })
    }
}
