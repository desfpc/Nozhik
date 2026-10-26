//! Font-independent project icons and path-based test/tool styling.
use crate::theme;
use egui::{Color32, FontId, Pos2, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2};
use nozhik_fs_model::tree::ProjectTree;
use std::path::Path;

fn quality_name(name: &str) -> bool {
    let words: Vec<_> = name.split(['.', '-', '_']).collect();
    words.iter().any(|word| {
        matches!(
            *word,
            "test"
                | "tests"
                | "spec"
                | "specs"
                | "__tests__"
                | "lint"
                | "linters"
                | "eslint"
                | "eslintignore"
                | "eslintrc"
                | "prettier"
                | "prettierrc"
                | "prettierignore"
                | "phpunit"
                | "phpstan"
                | "psalm"
                | "pint"
                | "clippy"
                | "rustfmt"
                | "ruff"
                | "flake8"
                | "pylint"
                | "stylelint"
                | "stylelintrc"
                | "biome"
        )
    }) || name.contains("cs-fixer")
        || name.ends_with("test.php")
        || name.ends_with("tests.php")
        || name.starts_with("test_")
}

fn quality_path(tree: &ProjectTree, path: &Path) -> bool {
    path.strip_prefix(&tree.root_path).is_ok_and(|p| {
        p.components()
            .any(|c| quality_name(&c.as_os_str().to_string_lossy().to_ascii_lowercase()))
    })
}

fn badge(path: &Path, directory: bool) -> (&'static str, Color32) {
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    if directory {
        return match name.as_str() {
            "src" | "source" | "app" | "application" => ("", theme::INFO),
            "docs" | "doc" => ("D", theme::INFO),
            "assets" | "images" | "public" => ("•", theme::WARNING),
            "config" | ".github" | ".cargo" => ("*", theme::MUTED),
            "vendor" | "node_modules" => ("P", theme::WARNING),
            _ => ("", theme::TEXT),
        };
    }
    if name == "dockerfile" || name.starts_with("docker-compose") || name.starts_with("compose.") {
        return ("DK", theme::INFO);
    }
    if matches!(
        name.as_str(),
        ".gitignore" | ".dockerignore" | ".ignore" | ".gitattributes"
    ) {
        return ("/", theme::MUTED);
    }
    if name.starts_with(".env") || matches!(name.as_str(), ".editorconfig" | "makefile") {
        return ("*", theme::WARNING);
    }
    match path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "rs" => ("Rs", theme::WARNING),
        "php" => ("PHP", theme::INFO),
        "js" | "jsx" | "mjs" | "cjs" => ("JS", theme::WARNING),
        "ts" | "tsx" => ("TS", theme::INFO),
        "py" => ("Py", theme::WARNING),
        "go" => ("Go", theme::INFO),
        "c" | "h" | "cpp" | "hpp" | "java" | "kt" | "swift" => ("<>", theme::INFO),
        "json" | "lock" => ("{}", Color32::from_rgb(185, 145, 230)),
        "yaml" | "yml" | "toml" | "ini" | "neon" | "conf" => ("*", theme::WARNING),
        "md" | "markdown" => ("M↓", theme::INFO),
        "html" | "xml" | "vue" | "svelte" => ("<>", theme::WARNING),
        "css" | "scss" | "sass" | "less" => ("#", Color32::from_rgb(185, 145, 230)),
        "sh" | "bash" | "zsh" | "ps1" | "bat" | "cmd" => (">_", theme::SUCCESS),
        "sql" | "sqlite" | "db" => ("DB", theme::INFO),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "ico" => ("•", theme::SUCCESS),
        "zip" | "gz" | "tar" | "7z" => ("Z", theme::WARNING),
        _ => ("", theme::MUTED),
    }
}

pub fn row(
    ui: &mut Ui,
    tree: &ProjectTree,
    path: &Path,
    name: &str,
    directory: Option<bool>,
    selected: bool,
) -> Response {
    let excluded = tree.is_excluded(path);
    let quality = quality_path(tree, path);
    let (tag, icon_color) = badge(path, directory.is_some());
    let color = if excluded {
        theme::ERROR
    } else if quality {
        theme::SUCCESS
    } else {
        theme::TEXT
    };
    let font = FontId::proportional(14.0);
    let text = ui.painter().layout_no_wrap(name.into(), font, color);
    let width = (text.size().x + 58.0).max(ui.available_width());
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 26.0), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, name)
    });
    let fill = if selected {
        theme::ACCENT
    } else if response.hovered() {
        theme::HOVER
    } else if excluded {
        theme::DIFF_DELETE
    } else if quality {
        theme::DIFF_ADD
    } else {
        Color32::TRANSPARENT
    };
    let painter = ui.painter();
    painter.rect_filled(rect, 3.0, fill);
    if response.has_focus() {
        painter.rect_stroke(
            rect,
            3.0,
            Stroke::new(1.0_f32, theme::INFO),
            StrokeKind::Inside,
        );
    }
    let origin = rect.left_center();
    let stroke = Stroke::new(
        1.3_f32,
        if excluded || quality {
            color
        } else {
            icon_color
        },
    );
    paint_icon(painter, origin, directory, tag, stroke, fill);
    painter.galley(
        Pos2::new(origin.x + 49.0, origin.y - text.size().y / 2.0),
        text,
        color,
    );
    response.on_hover_text(if excluded {
        format!(
            "{}\nExcluded by ignore rules / dependency or build directory",
            path.display()
        )
    } else {
        path.display().to_string()
    })
}

fn paint_icon(
    painter: &egui::Painter,
    origin: Pos2,
    directory: Option<bool>,
    tag: &str,
    stroke: Stroke,
    fill: Color32,
) {
    let x = origin.x + 23.0;
    let y = origin.y - 7.0;
    if let Some(is_open) = directory {
        let center = Pos2::new(origin.x + 10.0, origin.y);
        let points = if is_open {
            vec![
                center + Vec2::new(-4.0, -2.0),
                center + Vec2::new(0.0, 2.0),
                center + Vec2::new(4.0, -2.0),
            ]
        } else {
            vec![
                center + Vec2::new(-2.0, -4.0),
                center + Vec2::new(2.0, 0.0),
                center + Vec2::new(-2.0, 4.0),
            ]
        };
        painter.add(egui::Shape::line(points, Stroke::new(1.4_f32, theme::TEXT)));
        painter.add(egui::Shape::closed_line(
            vec![
                Pos2::new(x, y),
                Pos2::new(x + 6.0, y),
                Pos2::new(x + 9.0, y + 3.0),
                Pos2::new(x + 18.0, y + 3.0),
                Pos2::new(x + 18.0, y + 14.0),
                Pos2::new(x, y + 14.0),
            ],
            stroke,
        ));
        if is_open {
            painter.line_segment(
                [Pos2::new(x + 2.0, y + 7.0), Pos2::new(x + 16.0, y + 7.0)],
                stroke,
            );
        }
    } else {
        painter.add(egui::Shape::closed_line(
            vec![
                Pos2::new(x + 2.0, y),
                Pos2::new(x + 12.0, y),
                Pos2::new(x + 16.0, y + 4.0),
                Pos2::new(x + 16.0, y + 15.0),
                Pos2::new(x + 2.0, y + 15.0),
            ],
            stroke,
        ));
        if tag.is_empty() {
            for offset in [6.0, 9.0, 12.0] {
                painter.line_segment(
                    [
                        Pos2::new(x + 5.0, y + offset),
                        Pos2::new(x + 12.0, y + offset),
                    ],
                    stroke,
                );
            }
        }
    }
    if !tag.is_empty() {
        painter.rect_filled(
            Rect::from_center_size(Pos2::new(x + 10.0, y + 10.0), Vec2::new(21.0, 11.0)),
            1.0,
            if fill == Color32::TRANSPARENT {
                theme::BACKGROUND
            } else {
                fill
            },
        );
        painter.text(
            Pos2::new(x + 10.0, y + 10.0),
            egui::Align2::CENTER_CENTER,
            tag,
            FontId::monospace(9.0),
            stroke.color,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn common_types_and_quality_names() {
        for name in [
            "tests",
            "tests-helpers",
            "phpstan.neon",
            ".php-cs-fixer.dist.php",
            "ThingTest.php",
            "main.test.ts",
            ".eslintrc",
            "clippy.toml",
        ] {
            assert!(quality_name(&name.to_ascii_lowercase()), "{name}");
        }
        for name in ["contest.rs", "latest", "main.rs"] {
            assert!(!quality_name(name), "{name}");
        }
        assert_eq!(badge(Path::new("unknown.xyz"), false), ("", theme::MUTED));
        assert_eq!(badge(Path::new("Dockerfile"), false).0, "DK");
        assert_eq!(badge(Path::new("main.ts"), false).0, "TS");
    }
}
