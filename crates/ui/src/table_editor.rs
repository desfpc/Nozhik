//! Table data editing with local staging and explicit submission.
use nozhik_database::{
    DbDriverType, DbValue, QueryResult, changes::TableChanges, introspection::TableInfo,
};
use std::collections::{BTreeSet, HashMap};

#[derive(Clone, Copy)]
pub enum TableAction {
    Reload,
    Submit,
    Ddl,
    Cancel,
}
pub struct TableEditor {
    pub driver: DbDriverType,
    pub info: TableInfo,
    pub changes: Option<TableChanges>,
    pub action: Option<TableAction>,
    selected: BTreeSet<usize>,
    preview: bool,
    inputs: HashMap<(usize, usize), String>,
    reveal_row: Option<usize>,
}
impl TableEditor {
    pub fn new(driver: DbDriverType, info: TableInfo) -> Self {
        Self {
            driver,
            info,
            changes: None,
            action: None,
            selected: BTreeSet::new(),
            preview: false,
            inputs: HashMap::new(),
            reveal_row: None,
        }
    }
    pub fn loaded(&mut self, schema: &str, result: QueryResult) {
        let keys = result
            .columns
            .iter()
            .enumerate()
            .filter(|(_, c)| {
                self.info
                    .columns
                    .iter()
                    .any(|m| m.name == c.name && m.is_primary_key)
            })
            .map(|(i, _)| i)
            .collect();
        self.changes = Some(TableChanges::new(
            self.driver,
            schema.into(),
            self.info.name.clone(),
            result,
            keys,
        ));
        self.selected.clear();
        self.inputs.clear();
    }
    pub const fn modal_open(&self) -> bool {
        self.preview
    }
    pub fn dirty(&self) -> bool {
        self.changes.as_ref().is_some_and(TableChanges::is_dirty)
    }
    #[allow(
        clippy::too_many_lines,
        reason = "One table toolbar controls the staged grid and SQL preview"
    )]
    pub fn show(&mut self, ui: &mut egui::Ui, busy: bool, enabled: bool) {
        let dirty = self.dirty();
        let editable = !self.info.is_view && self.changes.is_some();
        ui.add_enabled_ui(enabled && !self.preview, |ui| {
            egui::ScrollArea::horizontal()
                .id_salt("table_toolbar")
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(!busy && !dirty, egui::Button::new("↻"))
                            .on_hover_text("Reload Page")
                            .clicked()
                        {
                            self.action = Some(TableAction::Reload);
                        }
                        if ui
                            .add_enabled(busy, egui::Button::new("■"))
                            .on_hover_text("Cancel Running Statements")
                            .clicked()
                        {
                            self.action = Some(TableAction::Cancel);
                        }
                        ui.separator();
                        if ui
                            .add_enabled(!busy && editable, egui::Button::new("+"))
                            .on_hover_text("Add Row")
                            .clicked()
                            && let Some(changes) = &mut self.changes
                        {
                            changes.add_row();
                            ui.ctx().request_repaint();
                            self.selected.clear();
                            self.selected.insert(changes.rows.len() - 1);
                            self.reveal_row = changes.rows.len().checked_sub(1);
                        }
                        if ui
                            .add_enabled(
                                !busy && editable && !self.selected.is_empty(),
                                egui::Button::new("−"),
                            )
                            .on_hover_text("Delete Rows")
                            .clicked()
                            && let Some(changes) = &mut self.changes
                        {
                            for &i in self.selected.iter().rev() {
                                if changes.rows.get(i).is_some_and(|r| r.original.is_none()) {
                                    changes.rows.remove(i);
                                } else if let Some(row) = changes.rows.get_mut(i) {
                                    row.deleted = true;
                                }
                            }
                            self.inputs.clear();
                            self.selected.clear();
                            ui.ctx().request_repaint();
                        }
                        if ui
                            .add_enabled(!busy && dirty, egui::Button::new("Revert"))
                            .on_hover_text("Revert")
                            .clicked()
                            && let Some(changes) = &mut self.changes
                        {
                            changes.revert();
                            ui.ctx().request_repaint();
                            self.inputs.clear();
                            self.selected.clear();
                            ui.ctx().request_repaint();
                        }
                        if ui
                            .add_enabled(
                                !busy && dirty,
                                egui::Button::new("Preview Pending Changes"),
                            )
                            .clicked()
                        {
                            self.preview = true;
                            ui.ctx().request_repaint();
                        }
                        if ui
                            .add_enabled(
                                !busy && dirty,
                                egui::Button::new(
                                    egui::RichText::new("Submit").color(crate::theme::SUCCESS),
                                ),
                            )
                            .clicked()
                        {
                            self.action = Some(TableAction::Submit);
                        }
                        ui.separator();
                        if ui
                            .add_enabled(
                                !busy && !self.info.objects.ddl.is_empty(),
                                egui::Button::new("DDL"),
                            )
                            .on_hover_text(
                                self.info.objects.error.as_deref().unwrap_or("Go to DDL"),
                            )
                            .clicked()
                        {
                            self.action = Some(TableAction::Ddl);
                        }
                        if busy {
                            ui.spinner();
                        }
                    });
                });
        });
        if let Some(changes) = &mut self.changes {
            ui.add_enabled_ui(enabled && !busy && !self.preview, |ui| {
                self::grid(
                    ui,
                    changes,
                    &mut self.selected,
                    &mut self.inputs,
                    self.reveal_row.take(),
                    !self.info.is_view,
                );
            });
        }
        if self.dirty() != dirty {
            ui.ctx().request_repaint();
        }
        if self.preview {
            let mut close = false;
            egui::Modal::new(ui.id().with("dml_preview")).show(ui.ctx(), |ui| {
                ui.set_width(680.0);
                ui.heading("DML Preview");
                if let Some(changes) = &self.changes {
                    match changes.statements() {
                        Ok(statements) => {
                            let mut sql = format!("BEGIN;\n{}\nCOMMIT;", statements.join("\n"));
                            egui::ScrollArea::both().max_height(360.0).show(ui, |ui| {
                                ui.add(
                                    egui::TextEdit::multiline(&mut sql)
                                        .font(egui::TextStyle::Monospace)
                                        .desired_width(f32::INFINITY)
                                        .interactive(false),
                                );
                            });
                            ui.horizontal(|ui| {
                                if ui.button("Cancel").clicked() {
                                    close = true;
                                }
                                if ui
                                    .add_enabled(
                                        enabled && !busy && dirty,
                                        egui::Button::new("Submit"),
                                    )
                                    .clicked()
                                {
                                    self.action = Some(TableAction::Submit);
                                    close = true;
                                }
                            });
                        }
                        Err(e) => {
                            ui.colored_label(crate::theme::ERROR, e.to_string());
                            if ui.button("Close").clicked() {
                                close = true;
                            }
                        }
                    }
                }
            });
            if close {
                self.preview = false;
                ui.ctx().request_repaint();
            }
        }
    }
}
#[allow(
    clippy::too_many_lines,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "Retained row and column counts are bounded by database result limits"
)]
fn grid(
    ui: &mut egui::Ui,
    changes: &mut TableChanges,
    selected: &mut BTreeSet<usize>,
    inputs: &mut HashMap<(usize, usize), String>,
    reveal_row: Option<usize>,
    editable: bool,
) {
    let row_height = 28.0;
    let width = 180.0;
    let header = 36.0;
    let gutter = 56.0;
    egui::ScrollArea::both()
        .id_salt("editable_grid")
        .auto_shrink([false, false])
        .max_height((ui.available_height() - 25.0).max(50.0))
        .show_viewport(ui, |ui, viewport| {
            let (rect, _) = ui.allocate_exact_size(
                egui::vec2(
                    (changes.baseline.columns.len() as f32)
                        .mul_add(width, gutter)
                        .max(ui.available_width()),
                    (changes.rows.len() as f32).mul_add(row_height, header),
                ),
                egui::Sense::hover(),
            );
            if let Some(row) = reveal_row {
                let target = egui::Rect::from_min_size(
                    egui::pos2(
                        rect.left(),
                        (row as f32).mul_add(row_height, rect.top() + header),
                    ),
                    egui::vec2(gutter, row_height),
                );
                ui.scroll_to_rect(target, Some(egui::Align::BOTTOM));
            }
            let start = ((viewport.min.y - header).max(0.0) / row_height).floor() as usize;
            let end = ((viewport.max.y / row_height).ceil() as usize + 1).min(changes.rows.len());
            let columns = ((viewport.min.x - gutter).max(0.0) / width).floor() as usize
                ..(((viewport.max.x - gutter).max(0.0) / width).ceil() as usize)
                    .min(changes.baseline.columns.len());
            for i in start..end {
                let Some(row) = changes.rows.get_mut(i) else {
                    continue;
                };
                let y = (i as f32).mul_add(row_height, rect.top() + header);
                let number = egui::Rect::from_min_size(
                    egui::pos2(rect.left(), y),
                    egui::vec2(gutter, row_height),
                );
                let response = ui.put(
                    number,
                    egui::Button::selectable(
                        selected.contains(&i),
                        format!(
                            "{}{}",
                            i + 1,
                            if row.deleted {
                                " −"
                            } else if row.original.is_none() {
                                " +"
                            } else {
                                ""
                            }
                        ),
                    ),
                );
                if response.clicked() {
                    if ui.input(|input| input.modifiers.shift) {
                        let anchor = selected.last().copied().unwrap_or(i);
                        selected.extend(anchor.min(i)..=anchor.max(i));
                    } else if ui.input(|input| input.modifiers.command) {
                        if !selected.remove(&i) {
                            selected.insert(i);
                        }
                    } else {
                        selected.clear();
                        selected.insert(i);
                    }
                }
                for c in columns.clone() {
                    let Some(value) = row.values.get_mut(c) else {
                        continue;
                    };
                    let cell = egui::Rect::from_min_size(
                        egui::pos2((c as f32).mul_add(width, rect.left() + gutter), y),
                        egui::vec2(width, row_height),
                    );
                    let fill = if selected.contains(&i) {
                        crate::theme::HOVER
                    } else if i % 2 == 0 {
                        crate::theme::PANEL
                    } else {
                        crate::theme::BACKGROUND
                    };
                    ui.painter().rect_filled(cell, 0.0, fill);
                    ui.painter().line_segment(
                        [cell.right_top(), cell.right_bottom()],
                        egui::Stroke::new(1.0_f32, crate::theme::BORDER),
                    );
                    ui.painter().line_segment(
                        [cell.left_bottom(), cell.right_bottom()],
                        egui::Stroke::new(1.0_f32, crate::theme::BORDER),
                    );
                    let text = inputs.entry((i, c)).or_insert_with(|| {
                        value
                            .as_ref()
                            .map_or_else(|| "DEFAULT".into(), ToString::to_string)
                    });
                    let modified = row
                        .original
                        .as_ref()
                        .is_none_or(|original| original.get(c) != value.as_ref());
                    let can_edit = editable && !row.deleted;
                    let response = ui.put(
                        cell,
                        egui::TextEdit::singleline(text)
                            .id(ui.id().with(("cell", i, c)))
                            .font(egui::TextStyle::Monospace)
                            .frame(false)
                            .interactive(can_edit)
                            .text_color(if row.deleted {
                                crate::theme::ERROR
                            } else if modified {
                                crate::theme::SUCCESS
                            } else {
                                crate::theme::TEXT
                            }),
                    );
                    if response.clicked() && !selected.contains(&i) {
                        selected.clear();
                        selected.insert(i);
                    }
                    if response.changed() {
                        ui.ctx().request_repaint();
                        *value = Some(parse_input(
                            text,
                            &changes.baseline.columns[c].data_type,
                            value.as_ref(),
                        ));
                        if let Some(original) = row.original.as_ref().and_then(|r| r.get(c))
                            && !matches!(original, DbValue::Null)
                            && text == &original.to_string()
                        {
                            *value = Some(original.clone());
                        }
                    }
                    response.context_menu(|ui| {
                        if ui
                            .add_enabled(can_edit, egui::Button::new("Set NULL"))
                            .clicked()
                        {
                            *value = Some(DbValue::Null);
                            *text = "NULL".into();
                            ui.close();
                        }
                        if ui
                            .add_enabled(
                                can_edit && row.original.is_none(),
                                egui::Button::new("Use DEFAULT"),
                            )
                            .clicked()
                        {
                            *value = None;
                            *text = "DEFAULT".into();
                            ui.close();
                        }
                    });
                }
            }
            for c in columns {
                let column = &changes.baseline.columns[c];
                let cell = egui::Rect::from_min_size(
                    rect.min + egui::vec2((c as f32).mul_add(width, gutter), viewport.min.y),
                    egui::vec2(width, header),
                );
                ui.painter().rect_filled(cell, 0.0, crate::theme::PANEL);
                ui.painter().line_segment(
                    [cell.right_top(), cell.right_bottom()],
                    egui::Stroke::new(1.0_f32, crate::theme::BORDER),
                );
                ui.painter().line_segment(
                    [cell.left_bottom(), cell.right_bottom()],
                    egui::Stroke::new(1.0_f32, crate::theme::BORDER),
                );
                ui.painter()
                    .with_clip_rect(cell.intersect(ui.clip_rect()))
                    .text(
                        cell.min + egui::vec2(5.0, 2.0),
                        egui::Align2::LEFT_TOP,
                        format!(
                            "{}{}\n{}",
                            if changes.keys.contains(&c) { "PK " } else { "" },
                            column.name,
                            column.data_type
                        ),
                        egui::FontId::monospace(12.0),
                        crate::theme::TEXT,
                    );
            }
        });
    ui.label(format!(
        "{} rows · {} selected{}{}",
        changes.rows.len(),
        selected.len(),
        if changes.is_dirty() {
            " · pending changes"
        } else {
            ""
        },
        if changes.baseline.truncated {
            " · TRUNCATED"
        } else {
            ""
        }
    ));
}
fn parse_input(text: &str, kind: &str, previous: Option<&DbValue>) -> DbValue {
    let kind = kind.to_ascii_uppercase();
    if kind.contains("INT") {
        if let Ok(v) = text.parse() {
            return DbValue::Int(v);
        }
    } else if ["REAL", "FLOAT", "DOUBLE"].iter().any(|s| kind.contains(s)) {
        if let Ok(v) = text.parse() {
            return DbValue::Float(v);
        }
    } else if kind.contains("BOOL") {
        if let Ok(v) = text.parse() {
            return DbValue::Bool(v);
        }
    } else if (matches!(previous, Some(DbValue::Bytes(_)))
        || ["BLOB", "BINARY", "BYTEA"].iter().any(|s| kind.contains(s)))
        && let Some(hex) = text.strip_prefix("0x")
        && hex.len().is_multiple_of(2)
    {
        let bytes = (0..hex.len())
            .step_by(2)
            .map(|i| {
                hex.get(i..i + 2)
                    .and_then(|s| u8::from_str_radix(s, 16).ok())
            })
            .collect::<Option<Vec<_>>>();
        if let Some(bytes) = bytes {
            return DbValue::Bytes(bytes);
        }
    }
    DbValue::Text(text.into())
}
