//! Virtualized unified diff with two gutters; only visible rows are laid out.
use crate::theme;
use egui::{Color32, FontId};
use nozhik_vcs::model::{DiffLineKind, FileDiff};

enum Row {
    Hunk(usize),
    Line(usize, usize),
}
#[derive(Default)]
pub struct DiffView {
    file: Option<FileDiff>,
    rows: Vec<Row>,
}
impl DiffView {
    pub(crate) fn clear(&mut self) {
        self.file = None;
        self.rows.clear();
    }
    pub(crate) fn set(&mut self, file: FileDiff) {
        self.rows.clear();
        for (h, hunk) in file.hunks.iter().enumerate() {
            self.rows.push(Row::Hunk(h));
            for l in 0..hunk.lines.len() {
                self.rows.push(Row::Line(h, l));
            }
        }
        self.file = Some(file);
    }
    pub(crate) fn show(&self, ui: &mut egui::Ui, font_size: f32) {
        let Some(file) = &self.file else {
            ui.weak("Select a changed file to view its diff");
            return;
        };
        ui.horizontal(|ui| {
            ui.strong(file.path.to_string_lossy());
            ui.colored_label(Color32::from_rgb(90, 200, 120), format!("+{}", file.added));
            ui.colored_label(
                Color32::from_rgb(240, 110, 110),
                format!("−{}", file.deleted),
            );
        });
        if file.binary {
            ui.weak("Binary or oversized file — text diff is unavailable");
            return;
        }
        if file.truncated {
            ui.weak("Diff display limit reached; showing a partial patch");
        }
        if self.rows.is_empty() {
            ui.weak("No textual changes (contents unchanged or file metadata only)");
            return;
        }
        let font = FontId::monospace(font_size);
        let height = font_size + 8.0;
        egui::ScrollArea::both()
            .id_salt(("git_diff", &file.path))
            .auto_shrink([false, false])
            .show_rows(ui, height, self.rows.len(), |ui, visible| {
                for index in visible {
                    let row = &self.rows[index];
                    let (text, old, new, marker, background) = match *row {
                        Row::Hunk(h) => (
                            file.hunks[h].header.as_str(),
                            None,
                            None,
                            ' ',
                            Color32::from_rgb(43, 54, 72),
                        ),
                        Row::Line(h, l) => {
                            let line = &file.hunks[h].lines[l];
                            let (marker, bg) = match line.kind {
                                DiffLineKind::Context => (' ', theme::BACKGROUND),
                                DiffLineKind::Addition => ('+', Color32::from_rgb(27, 59, 40)),
                                DiffLineKind::Deletion => ('−', Color32::from_rgb(68, 34, 38)),
                            };
                            (
                                line.text.trim_end_matches(['\n', '\r']),
                                line.old_line,
                                line.new_line,
                                marker,
                                bg,
                            )
                        }
                    };
                    let galley =
                        ui.painter()
                            .layout_no_wrap(text.into(), font.clone(), theme::TEXT);
                    let width = ui.available_width().max(galley.size().x + 130.0);
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
                    ui.painter().rect_filled(rect, 0.0, background);
                    let y = rect.top() + 4.0;
                    for (number, x) in [(old, 44.0), (new, 92.0)] {
                        if let Some(number) = number {
                            ui.painter().text(
                                egui::pos2(rect.left() + x, y),
                                egui::Align2::RIGHT_TOP,
                                number.to_string(),
                                font.clone(),
                                theme::MUTED,
                            );
                        }
                    }
                    ui.painter().text(
                        egui::pos2(rect.left() + 108.0, y),
                        egui::Align2::LEFT_TOP,
                        marker,
                        font.clone(),
                        theme::TEXT,
                    );
                    ui.painter()
                        .galley(egui::pos2(rect.left() + 126.0, y), galley, theme::TEXT);
                }
            });
    }
}
