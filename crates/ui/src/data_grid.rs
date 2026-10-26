//! Two-dimensional viewport rendering for retained SQL results.
use nozhik_database::{DbValue, QueryResult};
use std::cmp::Ordering;

#[derive(Default)]
pub struct DataGrid {
    pub result: QueryResult,
    order: Vec<usize>,
    sort: Option<(usize, bool)>,
    selected: Option<(usize, usize)>,
}
impl DataGrid {
    pub fn new(result: QueryResult) -> Self {
        let order = (0..result.rows.len()).collect();
        Self {
            result,
            order,
            ..Self::default()
        }
    }
    fn sort_column(&mut self, column: usize) {
        self.sort = match self.sort {
            Some((c, false)) if c == column => Some((column, true)),
            Some((c, true)) if c == column => None,
            _ => Some((column, false)),
        };
        self.order = (0..self.result.rows.len()).collect();
        if let Some((column, desc)) = self.sort {
            self.order.sort_by(|a, b| {
                let ordering = match (
                    self.result.rows.get(*a).and_then(|r| r.get(column)),
                    self.result.rows.get(*b).and_then(|r| r.get(column)),
                ) {
                    (Some(a), Some(b)) => compare(a, b),
                    _ => Ordering::Equal,
                };
                if desc { ordering.reverse() } else { ordering }
            });
        }
    }
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::too_many_lines,
        reason = "Bounded result indices are converted to egui logical viewport coordinates"
    )]
    pub fn show(&mut self, ui: &mut egui::Ui) {
        let grid_id = ui.id().with("data_grid");
        ui.interact(
            ui.available_rect_before_wrap(),
            grid_id,
            egui::Sense::click(),
        );
        let mut sort = None;
        egui::ScrollArea::both()
            .max_height((ui.available_height() - 40.0).max(30.0))
            .id_salt(grid_id)
            .auto_shrink([false, false])
            .show_viewport(ui, |ui, viewport| {
                let width = 180.0;
                let row_height = 25.0;
                let header_height = 40.0;
                let size = egui::vec2(
                    (self.result.columns.len() as f32 * width).max(ui.available_width()),
                    (self.order.len() as f32).mul_add(row_height, header_height),
                );
                let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                let columns = (viewport.min.x / width).floor() as usize
                    ..((viewport.max.x / width).ceil() as usize).min(self.result.columns.len());
                let start =
                    ((viewport.min.y - header_height).max(0.0) / row_height).floor() as usize;
                let end = ((viewport.max.y / row_height).ceil() as usize + 1).min(self.order.len());
                for visible in start..end {
                    let Some(&index) = self.order.get(visible) else {
                        continue;
                    };
                    for col in columns.clone() {
                        let cell = egui::Rect::from_min_size(
                            rect.min
                                + egui::vec2(
                                    col as f32 * width,
                                    (visible as f32).mul_add(row_height, header_height),
                                ),
                            egui::vec2(width, row_height),
                        );
                        let response =
                            ui.interact(cell, grid_id.with((index, col)), egui::Sense::click());
                        if response.clicked() {
                            self.selected = Some((index, col));
                            ui.memory_mut(|m| m.request_focus(grid_id));
                        }
                        let fill = if self.selected == Some((index, col)) {
                            crate::theme::ACCENT
                        } else if visible % 2 == 0 {
                            crate::theme::PANEL
                        } else {
                            crate::theme::BACKGROUND
                        };
                        ui.painter().rect_filled(cell, 0.0, fill);
                        if let Some(value) = self.result.rows.get(index).and_then(|r| r.get(col)) {
                            let painter =
                                ui.painter().with_clip_rect(cell.intersect(ui.clip_rect()));
                            let label: String = match value {
                                DbValue::Text(s) | DbValue::Json(s) | DbValue::DateTime(s) => {
                                    s.chars().take(256).collect()
                                }
                                DbValue::Bytes(b) => {
                                    DbValue::Bytes(b.iter().take(128).copied().collect())
                                        .to_string()
                                }
                                _ => value.to_string(),
                            };
                            painter.text(
                                cell.left_center() + egui::vec2(5.0, 0.0),
                                egui::Align2::LEFT_CENTER,
                                label,
                                egui::FontId::monospace(13.0),
                                crate::theme::TEXT,
                            );
                            response.context_menu(|ui| {
                                if ui.button("Copy value").clicked() {
                                    ui.ctx().copy_text(value.to_string());
                                    ui.close();
                                }
                                for (label, separator) in
                                    [("Copy row as CSV", ','), ("Copy row as TSV", '\t')]
                                {
                                    if ui.button(label).clicked() {
                                        if let Some(row) = self.result.rows.get(index) {
                                            ui.ctx().copy_text(encode_row(row, separator));
                                        }
                                        ui.close();
                                    }
                                }
                            });
                        }
                    }
                }
                // Sticky header overlays only the currently visible columns.
                for col in columns {
                    let Some(column) = self.result.columns.get(col) else {
                        continue;
                    };
                    let cell = egui::Rect::from_min_size(
                        rect.min + egui::vec2(col as f32 * width, viewport.min.y),
                        egui::vec2(width, header_height),
                    );
                    ui.painter().rect_filled(cell, 0.0, crate::theme::PANEL);
                    let marker = self
                        .sort
                        .filter(|(c, _)| *c == col)
                        .map_or("", |(_, desc)| if desc { " ↓" } else { " ↑" });
                    ui.painter()
                        .with_clip_rect(cell.intersect(ui.clip_rect()))
                        .text(
                            cell.min + egui::vec2(5.0, 3.0),
                            egui::Align2::LEFT_TOP,
                            format!("{}{marker}\n{}", column.name, column.data_type),
                            egui::FontId::monospace(12.0),
                            crate::theme::TEXT,
                        );
                    if ui
                        .interact(cell, grid_id.with(("header", col)), egui::Sense::click())
                        .clicked()
                    {
                        sort = Some(col);
                    }
                }
            });
        ui.label(format!(
            "{} rows retrieved in {} ms · {} rows affected{}",
            self.result.rows.len(),
            self.result.execution_time_ms,
            self.result.rows_affected,
            if self.result.truncated {
                " · TRUNCATED (10,000 rows / 16 MiB)"
            } else {
                ""
            }
        ));
        if let Some(column) = sort {
            self.sort_column(column);
        }
        if ui.memory(|m| m.has_focus(grid_id))
            && ui.input_mut(|i| {
                let copy = i.consume_key(egui::Modifiers::COMMAND, egui::Key::C)
                    || i.events.iter().any(|e| matches!(e, egui::Event::Copy));
                if copy {
                    i.events.retain(|e| !matches!(e, egui::Event::Copy));
                }
                copy
            })
            && let Some((row, col)) = self.selected
            && let Some(value) = self.result.rows.get(row).and_then(|r| r.get(col))
        {
            ui.ctx().copy_text(value.to_string());
        }
    }
}
fn compare(a: &DbValue, b: &DbValue) -> Ordering {
    match (a, b) {
        (DbValue::Null, DbValue::Null) => Ordering::Equal,
        (DbValue::Null, _) => Ordering::Less,
        (_, DbValue::Null) => Ordering::Greater,
        (DbValue::Int(a), DbValue::Int(b)) => a.cmp(b),
        (DbValue::Float(a), DbValue::Float(b)) => a.total_cmp(b),
        (DbValue::Bool(a), DbValue::Bool(b)) => a.cmp(b),
        _ => a.to_string().cmp(&b.to_string()),
    }
}
fn encode_row(row: &[DbValue], separator: char) -> String {
    row.iter()
        .map(|v| {
            let s = v.to_string();
            if s.contains([separator, '"', '\n', '\r']) {
                format!("\"{}\"", s.replace('"', "\"\""))
            } else {
                s
            }
        })
        .collect::<Vec<_>>()
        .join(&separator.to_string())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sorting_is_numeric_and_cycles_back_to_original_order() {
        let mut grid = DataGrid::new(QueryResult {
            rows: vec![
                vec![DbValue::Int(10)],
                vec![DbValue::Null],
                vec![DbValue::Int(2)],
            ],
            ..QueryResult::default()
        });
        grid.sort_column(0);
        assert_eq!(grid.order, vec![1, 2, 0]);
        grid.sort_column(0);
        assert_eq!(grid.order, vec![0, 2, 1]);
        grid.sort_column(0);
        assert_eq!(grid.order, vec![0, 1, 2]);
        assert_eq!(
            encode_row(&[DbValue::Text("a,\"b\n".into())], ','),
            "\"a,\"\"b\n\""
        );
    }
}

#[cfg(test)]
mod viewport_tests {
    use super::*;
    #[test]
    fn viewport_bounds_paint_work_and_focused_copy_uses_selected_value() {
        let result = QueryResult {
            columns: (0..100)
                .map(|i| nozhik_database::ColumnInfo {
                    name: format!("column{i}"),
                    data_type: "INT".into(),
                })
                .collect(),
            rows: (0..500)
                .map(|_| (0..100).map(DbValue::Int).collect())
                .collect(),
            ..QueryResult::default()
        };
        let mut grid = DataGrid::new(result);
        grid.selected = Some((0, 2));
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(400.0, 300.0),
            )),
            ..egui::RawInput::default()
        };
        let frame = ctx.run(input.clone(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                grid.show(ui);
                ui.memory_mut(|m| m.request_focus(ui.id().with("data_grid")));
            });
        });
        let painted = frame
            .shapes
            .iter()
            .filter(|s| matches!(s.shape, egui::Shape::Text(_)))
            .count();
        assert!(
            painted < 100,
            "Only viewport cells should be painted, got {painted}"
        );
        let input = egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::C,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::COMMAND,
            }],
            ..input
        };
        let frame = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| grid.show(ui));
        });
        assert!(
            frame
                .platform_output
                .commands
                .iter()
                .any(|c| matches!(c,egui::OutputCommand::CopyText(text) if text=="2"))
        );
    }
}
