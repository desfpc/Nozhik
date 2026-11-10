//! Markdown documents open as source, with a session Text/GUI switch.
mod view;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Mode {
    #[default]
    Text,
    Gui,
}

/// Preview size. Small is the default; medium and large scale fonts and spacing.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Scale {
    #[default]
    Small,
    Medium,
    Large,
}

impl Scale {
    const fn factor(self) -> f32 {
        match self {
            Self::Small => 1.0,
            Self::Medium => 1.35,
            Self::Large => 1.7,
        }
    }

    /// Glyph size of the A button itself. This is not the document scale.
    const fn button_size(self) -> f32 {
        match self {
            Self::Small => 11.0,
            Self::Medium => 16.0,
            Self::Large => 22.0,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
struct Preview {
    mode: Mode,
    scale: Scale,
}

/// Per-path preview mode and font size. The buffer stays the source in both modes.
#[derive(Default)]
pub struct MarkdownUi {
    previews: HashMap<PathBuf, Preview>,
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
        let gui = self
            .markdown
            .previews
            .get(&path)
            .is_some_and(|preview| preview.mode == Mode::Gui);
        ui.horizontal(|ui| {
            let preview = self.markdown.previews.entry(path.clone()).or_default();
            ui.selectable_value(&mut preview.mode, Mode::Text, "Text");
            ui.selectable_value(&mut preview.mode, Mode::Gui, "GUI");
            if gui {
                ui.add_space(8.0);
                for step in [Scale::Small, Scale::Medium, Scale::Large] {
                    let label = egui::RichText::new("A").size(step.button_size());
                    ui.selectable_value(&mut preview.scale, step, label);
                }
            }
        });
        if !gui {
            return false;
        }
        let scale = self
            .markdown
            .previews
            .get(&path)
            .map_or(1.0, |preview| preview.scale.factor());
        let text = self.documents[index].buffer.rope().to_string();
        let mut opened = None;
        view::show(ui, &path, &text, enabled, scale, &mut opened);
        if let Some(url) = opened {
            self.follow_markdown_link(&path, &url);
        }
        true
    }

    fn follow_markdown_link(&mut self, document: &Path, url: &str) {
        match view::link_target(document, url) {
            Some(view::LinkTarget::Browser(url)) => crate::editor_view::open_documentation(url),
            Some(view::LinkTarget::File(path)) => self.open_file(path),
            None => {}
        }
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

    const PORTS: &str = "\
### Полезные порты (`docker-compose.yml`)

| Сервис | Порт на хосте | Примечание |
| --- | --- | --- |
| nginx (web) | `8181` | контейнер `web-nozhovecom` |
| PHP-FPM | — | контейнер `php-fpm-nozhovecom` (CLI/тесты — через него) |
| MariaDB | `33066` | user `root`, password `localRoot`, DB `ice2` |
| Redis | `6382` | |
| Meilisearch | `7700` | master key `local_master_key` |
| message (WS) | `8080` | |
| mailer (SMTP) | `2525` | |
";

    #[test]
    fn gui_table_keeps_code_intact_and_shares_row_height() -> Result<(), String> {
        let rendered = gui(PORTS, vec2(520.0, 980.0))?;
        for token in [
            "Примечание",
            "web-nozhovecom",
            "php-fpm-nozhovecom",
            "localRoot",
            "local_master_key",
            "8181",
        ] {
            let glyph = glyph(&rendered, token)?;
            assert_eq!(glyph.rows, 1, "{token} wraps inside one word");
            assert!(
                glyph.size.x > 24.0,
                "{token} is only {} px wide",
                glyph.size.x
            );
            assert!(glyph.size.y < 28.0, "{token} is {} px tall", glyph.size.y);
        }
        let nginx = glyph(&rendered, "nginx")?;
        let _web = glyph(&rendered, "(web)")?;
        let port = glyph(&rendered, "8181")?;
        let php = glyph(&rendered, "PHP-FPM")?;
        let chip = glyph(&rendered, "web-nozhovecom")?;
        assert!(
            (nginx.pos.y - port.pos.y).abs() < 8.0,
            "port is not on the nginx row"
        );
        let row_span = php.pos.y - nginx.pos.y;
        assert!(
            (24.0..160.0).contains(&row_span),
            "nginx row is {row_span} px"
        );
        assert!(
            chip.pos.y >= nginx.pos.y - 2.0 && chip.pos.y < php.pos.y,
            "code chip left the nginx row"
        );
        let rows = grid_rows(&rendered);
        let service = glyph(&rendered, "Сервис")?;
        let header_at = rows
            .iter()
            .position(|row| {
                row.iter()
                    .any(|cell| cell.contains(service.pos + vec2(2.0, 2.0)))
            })
            .ok_or("header row is missing")?;
        let left = rows[header_at][0].left();
        let table: Vec<_> = rows
            .iter()
            .skip(header_at)
            .take_while(|row| (row[0].left() - left).abs() < 1.5)
            .collect();
        assert!(
            table.len() >= 8,
            "expected header and seven services, found {}",
            table.len()
        );
        for row in table.iter().take(8) {
            let height = row[0].height();
            assert!(
                row.iter().all(|cell| (cell.height() - height).abs() < 0.75),
                "cells in a row differ in height"
            );
            assert!(
                (18.0..160.0).contains(&height),
                "row height {height} is not a text row"
            );
            assert!(
                row[2].width() > row[1].width(),
                "notes column is not wider than the port column"
            );
            let note = glyph(&rendered, "Примечание")?;
            if row[2].contains(note.pos + vec2(2.0, 2.0)) {
                assert!(
                    note.pos.x + note.size.x <= row[2].right() - 2.0,
                    "header overflows the notes cell"
                );
            }
        }
        let chip_cell = table
            .iter()
            .flat_map(|row| row.iter())
            .find(|cell| cell.contains(chip.pos + vec2(2.0, 2.0)))
            .ok_or("code chip is outside every cell")?;
        assert!(chip.pos.x + chip.size.x <= chip_cell.right() - 2.0);
        Ok(())
    }

    #[test]
    fn gui_inline_code_stays_in_the_sentence_and_markers_center() -> Result<(), String> {
        let source = "\
## Лицензия и безопасность

- Лицензия — файл `LICENSE` в корне.
- Секреты — в `settings/*.php` и `go-services/pkg/common/config/` (private repo, by design). Не публиковать пароли/токены; security-вопросы — owner’у. Шаблона `SECURITY.md` в репозитории нет.

1. Длинный пункт нумерованного списка переносится на следующую строку, чтобы номер остался на первой.
";
        let rendered = gui(source, vec2(640.0, 760.0))?;
        let license = row_of(&rendered, "Лицензия")?;
        let code = row_of(&rendered, "LICENSE")?;
        let root = row_of(&rendered, "корне")?;
        assert!(
            license.galley.contains(code.row.center())
                && license.galley.contains(root.row.center()),
            "inline code left the sentence: license {:?} code {:?} root {:?}",
            license.galley,
            code.row,
            root.row
        );
        assert!(
            (license.row.top() - code.row.top()).abs() < 1.0
                && (code.row.top() - root.row.top()).abs() < 1.0,
            "LICENSE is not on the same line as the words around it"
        );
        assert!(
            license.galley.height() < license.row.height() * 1.5,
            "the license item wrapped unexpectedly"
        );
        let secrets = row_of(&rendered, "Секреты")?;
        let settings = row_of(&rendered, "settings/*.php")?;
        let security = row_of(&rendered, "SECURITY.md")?;
        assert!(
            secrets.galley.contains(settings.row.center())
                && secrets.galley.contains(security.row.center()),
            "code in the second item is a separate block"
        );
        assert!(
            (secrets.row.top() - settings.row.top()).abs() < 1.0,
            "settings/*.php left the first line"
        );
        assert!(
            secrets.galley.height() > secrets.row.height() * 1.5,
            "the secrets item should wrap so the marker can be checked against the first line"
        );
        code_run(&rendered, "LICENSE")?;
        code_run(&rendered, "go-services/pkg/common/config/")?;
        code_run(&rendered, "SECURITY.md")?;
        let bullet = disc_on(&rendered, &license.row)?;
        assert!(
            bullet.1 >= license.row.height() * 0.2,
            "bullet radius {} is small next to line {}",
            bullet.1,
            license.row.height()
        );
        assert!(
            (bullet.0.y - license.row.center().y).abs() < 2.0,
            "bullet is not centered on the first line"
        );
        let second = disc_on(&rendered, &secrets.row)?;
        let first_line = (second.0.y - secrets.row.center().y).abs();
        let whole_item = (second.0.y - secrets.galley.center().y).abs();
        assert!(
            first_line < 2.0 && first_line + 1.0 < whole_item,
            "second bullet follows the item block instead of its first line"
        );
        let number = glyph(&rendered, "1.")?;
        let numbered = row_of(&rendered, "Длинный")?;
        let number_mid = number.size.y.mul_add(0.5, number.pos.y);
        assert!(
            (number_mid - numbered.row.center().y).abs() < 2.0,
            "ordered marker is not centered on the first line"
        );
        assert!(numbered.galley.height() > numbered.row.height() * 1.4);
        Ok(())
    }

    #[test]
    fn gui_table_aligns_columns() -> Result<(), String> {
        let source = "\
| LEFT | CENTER | RIGHT |
| :--- | :---: | ---: |
| aa | bb | cc |
";
        let rendered = gui(source, vec2(900.0, 400.0))?;
        let rows = grid_rows(&rendered);
        let bb = glyph(&rendered, "bb")?;
        let cc = glyph(&rendered, "cc")?;
        let aa = glyph(&rendered, "aa")?;
        let row = rows
            .iter()
            .find(|row| {
                row.iter()
                    .any(|cell| cell.contains(bb.pos + vec2(1.0, 1.0)))
            })
            .ok_or("aligned row is missing")?;
        let left = row
            .iter()
            .find(|cell| cell.contains(aa.pos + vec2(1.0, 1.0)))
            .ok_or("left cell")?;
        let center = row
            .iter()
            .find(|cell| cell.contains(bb.pos + vec2(1.0, 1.0)))
            .ok_or("center cell")?;
        let right = row
            .iter()
            .find(|cell| cell.contains(cc.pos + vec2(1.0, 1.0)))
            .ok_or("right cell")?;
        let left_gap = aa.pos.x - left.left();
        let center_left = bb.pos.x - center.left();
        let center_right = center.right() - (bb.pos.x + bb.size.x);
        let right_gap = right.right() - (cc.pos.x + cc.size.x);
        assert!(left_gap < 20.0, "left text gap is {left_gap}");
        assert!(
            (center_left - center_right).abs() < 4.0,
            "center gaps {center_left} and {center_right}"
        );
        assert!(right_gap < 20.0, "right text gap is {right_gap}");
        assert!((left.height() - center.height()).abs() < 0.75);
        assert!((center.height() - right.height()).abs() < 0.75);
        Ok(())
    }

    const LINKS: &str = "\
- [documentation/howto/prod-deploy.md](./documentation/howto/prod-deploy.md) — setup prod с нуля
- [documentation/howto/mobile-setup.md](./documentation/howto/mobile-setup.md) — Flutter / Android / Xcode

| Guide |
| --- |
| [table-mobile](./documentation/howto/mobile-setup.md) |

[web](https://example.com/docs)
";

    #[test]
    fn gui_opens_file_links_in_the_editor() -> Result<(), String> {
        let dir = tempfile::tempdir().map_err(|error| error.to_string())?;
        write_file(
            dir.path(),
            "documentation/howto/prod-deploy.md",
            "prod-deploy-marker\n",
        )?;
        write_file(
            dir.path(),
            "documentation/howto/mobile-setup.md",
            "mobile-setup-marker\n",
        )?;
        let readme = dir.path().join("README.md");
        let mut session = GuiSession::open(&readme, LINKS)?;
        let size = vec2(1100.0, 900.0);
        let rendered = session.show_gui(size)?;
        assert!(contains(&rendered, "web"), "web label is missing");
        let prod = link_rect(&rendered, "documentation/howto/prod-deploy.md")?;
        let _ = session.frame(click(prod.center()), size);
        wait_for(&mut session.shell, |shell| {
            shell.documents.iter().any(|document| {
                document.path.ends_with("prod-deploy.md")
                    && document
                        .buffer
                        .rope()
                        .to_string()
                        .contains("prod-deploy-marker")
            })
        })?;
        let readme_index = session
            .shell
            .documents
            .iter()
            .position(|document| document.path == readme)
            .ok_or("readme closed")?;
        session.shell.active = Some(readme_index);
        let rendered = session.frame(Vec::new(), size);
        let mobile = link_rect(&rendered, "table-mobile")?;
        let _ = session.frame(click(mobile.center()), size);
        wait_for(&mut session.shell, |shell| {
            shell.active.is_some_and(|index| {
                shell.documents.get(index).is_some_and(|document| {
                    document.path.ends_with("mobile-setup.md")
                        && document
                            .buffer
                            .rope()
                            .to_string()
                            .contains("mobile-setup-marker")
                })
            })
        })?;
        Ok(())
    }

    #[test]
    fn gui_scale_buttons_grow_text_and_spacing() -> Result<(), String> {
        let source = "Alpha\n\nBeta\n\n- item\n";
        let dir = tempfile::tempdir().map_err(|error| error.to_string())?;
        let path = dir.path().join("scale.md");
        let mut session = GuiSession::open(&path, source)?;
        let size = vec2(900.0, 700.0);
        let small = session.show_gui(size)?;
        let buttons = a_buttons(&small)?;
        assert_eq!(buttons.len(), 3, "expected three A buttons");
        assert!(
            buttons[0].font < buttons[1].font && buttons[1].font < buttons[2].font,
            "A buttons are not ordered small to large"
        );
        let small_text = glyph(&small, "Alpha")?.size.y;
        let small_gap = glyph(&small, "Beta")?.pos.y - glyph(&small, "Alpha")?.pos.y;
        let small_disc = disc_on(&small, &row_of(&small, "item")?.row)?.1;
        let medium = session.frame(click(buttons[1].rect.center()), size);
        let medium_buttons = a_buttons(&medium)?;
        let large = session.frame(click(medium_buttons[2].rect.center()), size);
        let medium_text = glyph(&medium, "Alpha")?.size.y;
        let large_text = glyph(&large, "Alpha")?.size.y;
        let large_gap = glyph(&large, "Beta")?.pos.y - glyph(&large, "Alpha")?.pos.y;
        let large_disc = disc_on(&large, &row_of(&large, "item")?.row)?.1;
        near(medium_text / small_text, 1.35)?;
        near(large_text / small_text, 1.7)?;
        near(large_gap / small_gap, 1.7)?;
        near(large_disc / small_disc, 1.7)?;
        Ok(())
    }

    struct GuiSession {
        shell: NozhikUi,
        ctx: egui::Context,
    }

    impl GuiSession {
        fn open(path: &std::path::Path, source: &str) -> Result<Self, String> {
            let bus = nozhik_event_bus::EventBus::new(32).map_err(|error| error.to_string())?;
            let settings = nozhik_config::ConfigManager::from_path(
                path.with_file_name("settings.toml"),
                bus.clone(),
            )
            .map_err(|error| error.to_string())?;
            let mut shell = NozhikUi::new(bus, settings, nozhik_config::Config::default());
            let name = path.file_name().map_or_else(
                || "note.md".to_owned(),
                |name| name.to_string_lossy().into_owned(),
            );
            shell.documents.push(Document {
                path: path.to_path_buf(),
                binary: false,
                name,
                open: true,
                buffer: nozhik_buffer::TextBuffer::load_from_str(nozhik_core::BufferId(12), source),
            });
            shell.active = Some(0);
            let ctx = egui::Context::default();
            crate::theme::apply(&ctx);
            Ok(Self { shell, ctx })
        }

        fn show_gui(&mut self, size: egui::Vec2) -> Result<egui::FullOutput, String> {
            let source = self.frame(Vec::new(), size);
            let gui = galley(&source, "GUI").ok_or_else(|| "GUI switch is missing".to_string())?;
            let _ = self.frame(click(gui.center()), size);
            Ok(self.frame(Vec::new(), size))
        }

        fn frame(&mut self, events: Vec<Event>, size: egui::Vec2) -> egui::FullOutput {
            frame_at(&mut self.shell, &self.ctx, events, size)
        }
    }

    fn write_file(dir: &std::path::Path, relative: &str, text: &str) -> Result<(), String> {
        let path = dir.join(relative);
        let Some(parent) = path.parent() else {
            return Err(format!("no parent for {relative}"));
        };
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        std::fs::write(path, text).map_err(|error| error.to_string())
    }

    fn wait_for(shell: &mut NozhikUi, done: impl Fn(&NozhikUi) -> bool) -> Result<(), String> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !done(shell) {
            shell.receive_files();
            if std::time::Instant::now() > deadline {
                return Err(format!("timed out: {}", shell.status));
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        Ok(())
    }

    struct AButton {
        rect: Rect,
        font: f32,
    }

    fn a_buttons(output: &egui::FullOutput) -> Result<Vec<AButton>, String> {
        let mut buttons = Vec::new();
        for shape in &output.shapes {
            walk_a(&shape.shape, &mut buttons);
        }
        buttons.sort_by(|left, right| left.rect.left().total_cmp(&right.rect.left()));
        if buttons.len() == 3 {
            Ok(buttons)
        } else {
            Err(format!("found {} A buttons", buttons.len()))
        }
    }

    fn walk_a(shape: &egui::Shape, buttons: &mut Vec<AButton>) {
        match shape {
            egui::Shape::Vec(children) => {
                for child in children {
                    walk_a(child, buttons);
                }
            }
            egui::Shape::Text(text) if text.galley.text() == "A" => {
                let font = text
                    .galley
                    .job
                    .sections
                    .first()
                    .map_or(0.0, |section| section.format.font_id.size);
                buttons.push(AButton {
                    rect: Rect::from_min_size(text.pos, text.galley.size()),
                    font,
                });
            }
            _ => {}
        }
    }

    fn link_rect(output: &egui::FullOutput, label: &str) -> Result<Rect, String> {
        let mut found = None;
        for shape in &output.shapes {
            walk_link(&shape.shape, label, &mut found);
        }
        found.ok_or_else(|| format!("missing link {label}"))
    }

    fn walk_link(shape: &egui::Shape, label: &str, found: &mut Option<Rect>) {
        match shape {
            egui::Shape::Vec(children) => {
                for child in children {
                    walk_link(child, label, found);
                }
            }
            egui::Shape::Text(text) => {
                let Some(byte) = text.galley.text().find(label) else {
                    return;
                };
                let start = text.galley.text()[..byte].chars().count();
                let end = start + label.chars().count();
                let start_rect = text.galley.pos_from_cursor(egui::text::CCursor::new(start));
                let end_rect = text.galley.pos_from_cursor(egui::text::CCursor::new(end));
                let min = text.pos + start_rect.min.to_vec2();
                let max = text.pos + end_rect.max.to_vec2();
                *found = Some(Rect::from_min_max(min, max));
            }
            _ => {}
        }
    }

    fn near(actual: f32, expected: f32) -> Result<(), String> {
        let delta = (actual - expected).abs();
        if delta <= expected.mul_add(0.08, 0.02) {
            Ok(())
        } else {
            Err(format!("{actual} is not near {expected}"))
        }
    }

    fn gui(source: &str, size: egui::Vec2) -> Result<egui::FullOutput, String> {
        let dir = tempfile::tempdir().map_err(|error| error.to_string())?;
        let path = dir.path().join("ports.md");
        let bus = nozhik_event_bus::EventBus::new(32).map_err(|error| error.to_string())?;
        let settings =
            nozhik_config::ConfigManager::from_path(dir.path().join("settings.toml"), bus.clone())
                .map_err(|error| error.to_string())?;
        let mut shell = NozhikUi::new(bus, settings, nozhik_config::Config::default());
        shell.documents.push(Document {
            path,
            binary: false,
            name: "ports.md".into(),
            open: true,
            buffer: nozhik_buffer::TextBuffer::load_from_str(nozhik_core::BufferId(11), source),
        });
        shell.active = Some(0);
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let source_frame = frame_at(&mut shell, &ctx, Vec::new(), size);
        let gui =
            galley(&source_frame, "GUI").ok_or_else(|| "GUI switch is missing".to_string())?;
        let _ = frame_at(&mut shell, &ctx, click(gui.center()), size);
        Ok(frame_at(&mut shell, &ctx, Vec::new(), size))
    }

    fn frame(shell: &mut NozhikUi, ctx: &egui::Context, events: Vec<Event>) -> egui::FullOutput {
        frame_at(shell, ctx, events, vec2(1100.0, 800.0))
    }

    fn frame_at(
        shell: &mut NozhikUi,
        ctx: &egui::Context,
        events: Vec<Event>,
        size: egui::Vec2,
    ) -> egui::FullOutput {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
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

    struct RowHit {
        row: Rect,
        galley: Rect,
    }

    fn row_of(output: &egui::FullOutput, needle: &str) -> Result<RowHit, String> {
        let mut found = None;
        for shape in &output.shapes {
            walk_row(&shape.shape, needle, &mut found);
        }
        found.ok_or_else(|| format!("missing {needle}"))
    }

    fn walk_row(shape: &egui::Shape, needle: &str, found: &mut Option<RowHit>) {
        match shape {
            egui::Shape::Vec(children) => {
                for child in children {
                    walk_row(child, needle, found);
                }
            }
            egui::Shape::Text(text) => {
                let Some(byte) = text.galley.text().find(needle) else {
                    return;
                };
                let index = text.galley.text()[..byte].chars().count();
                let local = text.galley.pos_from_cursor(egui::text::CCursor::new(index));
                *found = Some(RowHit {
                    row: local.translate(text.pos.to_vec2()),
                    galley: Rect::from_min_size(text.pos, text.galley.size()),
                });
            }
            _ => {}
        }
    }

    fn code_run(output: &egui::FullOutput, token: &str) -> Result<(), String> {
        let mut found = false;
        for shape in &output.shapes {
            found |= code_run_in(&shape.shape, token);
        }
        if found {
            Ok(())
        } else {
            Err(format!("{token} is not an inline code run"))
        }
    }

    fn code_run_in(shape: &egui::Shape, token: &str) -> bool {
        match shape {
            egui::Shape::Vec(children) => children.iter().any(|child| code_run_in(child, token)),
            egui::Shape::Text(text) => text.galley.job.sections.iter().any(|section| {
                text.galley.text().get(section.byte_range.clone()) == Some(token)
                    && section.format.font_id.family == egui::FontFamily::Monospace
                    && section.format.background == crate::theme::chip()
            }),
            _ => false,
        }
    }

    fn disc_on(output: &egui::FullOutput, row: &Rect) -> Result<(Pos2, f32), String> {
        let mut discs = Vec::new();
        for shape in &output.shapes {
            walk_discs(&shape.shape, row, &mut discs);
        }
        if discs.len() == 1 {
            Ok(discs[0])
        } else {
            Err(format!(
                "expected one bullet on the line, found {}",
                discs.len()
            ))
        }
    }

    fn walk_discs(shape: &egui::Shape, row: &Rect, discs: &mut Vec<(Pos2, f32)>) {
        match shape {
            egui::Shape::Vec(children) => {
                for child in children {
                    walk_discs(child, row, discs);
                }
            }
            egui::Shape::Circle(circle)
                if circle.fill == crate::theme::text()
                    && (3.0..24.0).contains(&circle.radius)
                    && circle.center.y >= row.top() - 2.0
                    && circle.center.y <= row.bottom() + 2.0 =>
            {
                discs.push((circle.center, circle.radius));
            }
            _ => {}
        }
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

    struct Glyph {
        pos: Pos2,
        size: egui::Vec2,
        rows: usize,
    }

    fn glyph(output: &egui::FullOutput, label: &str) -> Result<Glyph, String> {
        let mut found = None;
        for shape in &output.shapes {
            walk_glyph(&shape.shape, label, &mut found);
        }
        found.ok_or_else(|| format!("missing {label}"))
    }

    fn walk_glyph(shape: &egui::Shape, label: &str, found: &mut Option<Glyph>) {
        match shape {
            egui::Shape::Vec(children) => {
                for child in children {
                    walk_glyph(child, label, found);
                }
            }
            egui::Shape::Text(text) if text.galley.text() == label => {
                *found = Some(Glyph {
                    pos: text.pos,
                    size: text.galley.size(),
                    rows: text.galley.rows.len(),
                });
            }
            _ => {}
        }
    }

    fn grid_rows(output: &egui::FullOutput) -> Vec<Vec<Rect>> {
        let mut rects = Vec::new();
        for shape in &output.shapes {
            walk_cells(&shape.shape, &mut rects);
        }
        rects.sort_by(|left, right| {
            left.min
                .y
                .total_cmp(&right.min.y)
                .then(left.min.x.total_cmp(&right.min.x))
        });
        let mut groups: Vec<Vec<Rect>> = Vec::new();
        for rect in rects {
            if let Some(group) = groups.last_mut()
                && (group[0].top() - rect.top()).abs() < 0.75
            {
                group.push(rect);
                continue;
            }
            groups.push(vec![rect]);
        }
        groups.retain(|group| group.len() == 3 && row_aligned(group));
        groups
    }

    fn walk_cells(shape: &egui::Shape, rects: &mut Vec<Rect>) {
        match shape {
            egui::Shape::Vec(children) => {
                for child in children {
                    walk_cells(child, rects);
                }
            }
            egui::Shape::Rect(rect)
                if rect.corner_radius == egui::CornerRadius::ZERO
                    && rect.stroke.width < 0.1
                    && (rect.fill == crate::theme::background()
                        || rect.fill == crate::theme::surface())
                    && rect.rect.width() > 24.0
                    && rect.rect.width() < 700.0
                    && rect.rect.height() > 16.0
                    && rect.rect.height() < 220.0 =>
            {
                rects.push(rect.rect);
            }
            _ => {}
        }
    }

    fn row_aligned(group: &[Rect]) -> bool {
        let height = group[0].height();
        group.windows(2).all(|pair| {
            (pair[0].height() - height).abs() < 0.75
                && (pair[1].height() - height).abs() < 0.75
                && (pair[0].right() - pair[1].left()).abs() < 1.5
        })
    }
}
